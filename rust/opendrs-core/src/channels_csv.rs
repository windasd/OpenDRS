//! Export / import the channel list as CSV, one row per channel, zone by zone (port of
//! `channels_csv.py`).
//!
//! Export: UTF-8 with a BOM and CRLF line ends; every value as the channel table shows it,
//! references (TX contact, RX group, encryption key, scan list, alarm list) as "number name".
//!
//! Import updates the channels the CSV names and appends new ones; nothing is deleted. A row is
//! located by 分區 + 頻道 (position in that zone, as before the import); an empty or too-large
//! position appends a channel copied from the zone's last one. Only columns present are applied,
//! blank cells keep the current value, and a row with any invalid cell is skipped as a whole.
//!
//! Text is read with Python's rules (`str.strip`, `casefold`, `isdecimal`, `Decimal`, see
//! [`crate::pystr`]), so a file gets exactly the same result and messages as in the Python version.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::LazyLock;

use crate::alarm_enc::{alarm_lists, encryption_keys};
use crate::channel_fields::{self as cf, CTCSS, DCS};
use crate::device::frequency_problem_hz;
use crate::f136::Project;
use crate::layout::{LayoutError, add_channel, check_layout, zone_channel_rows};
use crate::pycsv;
use crate::pystr::{
    PyDecimal, casefold, decimal_usize, decimal_value, is_decimal, is_space, strip,
};
use crate::radio_settings::Setting;
use crate::records::{Channel, hz_to_mhz, mhz_to_hz};
use crate::rxgroup::rx_groups;
use crate::scanlist::scan_lists;
use crate::textcodec::{NameCodec, read_legacy_text};

const NONE_WORDS: [&str; 7] = ["", "0", "無", "（無）", "(無)", "none", "off"];

fn is_none_word(text: &str) -> bool {
    NONE_WORDS.contains(&casefold(text).as_str())
}

// ---------------------------------------------------------------- references
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefKind {
    Contact,
    Group,
    Key,
    Scan,
    Alarm,
}

impl RefKind {
    const ALL: [RefKind; 5] = [
        RefKind::Contact,
        RefKind::Group,
        RefKind::Key,
        RefKind::Scan,
        RefKind::Alarm,
    ];
}

fn ref_options(project: &Project, kind: RefKind, codec: NameCodec) -> Vec<(usize, String)> {
    let numbered = |names: Vec<String>| (1usize..).zip(names).collect::<Vec<_>>();
    match kind {
        RefKind::Contact => project
            .contacts
            .iter()
            .map(|c| (usize::from(c.index()), c.name(codec)))
            .collect(),
        RefKind::Group => numbered(
            rx_groups(&project.image)
                .iter()
                .map(|g| g.name(codec))
                .collect(),
        ),
        RefKind::Key => numbered(
            encryption_keys(&project.image)
                .iter()
                .map(|k| k.alias(codec))
                .collect(),
        ),
        RefKind::Scan => numbered(
            scan_lists(&project.image)
                .iter()
                .map(|s| s.name(codec))
                .collect(),
        ),
        RefKind::Alarm => numbered(
            alarm_lists(&project.image)
                .iter()
                .map(|a| a.name(codec))
                .collect(),
        ),
    }
}

/// What the columns read from one project snapshot: its codec and the numbered names every
/// reference column offers, decoded once (a table shows many cells per frame).
pub struct View<'a> {
    pub project: &'a Project,
    pub codec: NameCodec,
    refs: [Vec<(usize, String)>; 5],
}

impl<'a> View<'a> {
    pub fn new(project: &'a Project, codec: NameCodec) -> Self {
        View {
            project,
            codec,
            refs: RefKind::ALL.map(|kind| ref_options(project, kind, codec)),
        }
    }

    fn options(&self, kind: RefKind) -> &[(usize, String)] {
        &self.refs[kind as usize]
    }
}

/// `dict(options).get(number)`: the last option with that number wins, as in a Python dict.
fn ref_name(options: &[(usize, String)], number: usize) -> Option<&str> {
    options
        .iter()
        .rev()
        .find(|(n, _)| *n == number)
        .map(|(_, name)| name.as_str())
}

fn ref_text(view: &View, kind: RefKind, number: u32) -> String {
    if number == 0 {
        return String::new();
    }
    match usize::try_from(number)
        .ok()
        .and_then(|n| ref_name(view.options(kind), n))
    {
        Some(name) => format!("{number} {name}"),
        None => number.to_string(),
    }
}

/// `re.match(r"^#?(\d+)(?:\s+(.*))?$", t)` on stripped text: the number as written, its value
/// (saturating) and the text after the whitespace, if any.
fn ref_number(t: &str) -> Option<(&str, usize, Option<&str>)> {
    let body = t.strip_prefix('#').unwrap_or(t);
    let digits_end = body
        .char_indices()
        .find(|(_, ch)| decimal_value(*ch).is_none())
        .map_or(body.len(), |(at, _)| at);
    let (digits, after) = body.split_at(digits_end);
    let value = decimal_usize(digits)?;
    if after.is_empty() {
        return Some((digits, value, None));
    }
    let rest = after.trim_start_matches(is_space);
    // `\s+` must match something and `.*` cannot cross a line break.
    (rest.len() < after.len() && !rest.contains('\n')).then_some((digits, value, Some(rest)))
}

fn ref_parse(view: &View, kind: RefKind, text: &str) -> Result<u32, String> {
    let t = strip(text);
    if is_none_word(t) {
        return Ok(0);
    }
    let options = view.options(kind);
    let found = |n: usize| u32::try_from(n).map_err(|_| format!("編號 {n} 不存在"));
    let number = ref_number(t);
    if let Some((digits, value, None)) = number {
        return match ref_name(options, value) {
            Some(_) => found(value),
            None => Err(format!("編號 {digits} 不存在")),
        };
    }
    if let Some((_, value, Some(rest))) = number {
        if ref_name(options, value).is_some_and(|name| strip(name) == strip(rest)) {
            return found(value); // "number name", as exported
        }
    }
    let hits: Vec<usize> = options
        .iter()
        .filter(|(_, name)| strip(name) == t)
        .map(|(n, _)| *n)
        .collect();
    match hits.as_slice() {
        [one] => return found(*one),
        [_, _, ..] => return Err(format!("「{t}」不只一個，請改用編號")),
        [] => {}
    }
    match number {
        Some((digits, value, rest)) => match ref_name(options, value) {
            Some(name) => Err(format!(
                "編號 {digits} 是「{}」，不是「{}」",
                strip(name),
                strip(rest.unwrap_or_default())
            )),
            None => Err(format!("編號 {digits} 不存在")),
        },
        None => Err(format!("找不到「{t}」")),
    }
}

// ---------------------------------------------------------------- simple values
fn choice(text: &str, options: &[(String, u32)], extra: &[(&str, u32)]) -> Result<u32, String> {
    let t = casefold(strip(text));
    if let Some((_, value)) = options.iter().find(|(label, _)| casefold(label) == t) {
        return Ok(*value);
    }
    if let Some((_, value)) = extra.iter().find(|(alias, _)| *alias == t) {
        return Ok(*value);
    }
    if let Some(number) = decimal_usize(&t) {
        if let Some((_, value)) = options
            .iter()
            .find(|(_, v)| usize::try_from(*v) == Ok(number))
        {
            return Ok(*value);
        }
    }
    let labels: Vec<&str> = options.iter().map(|(label, _)| label.as_str()).collect();
    Err(format!(
        "無效的選項「{}」（可用：{}）",
        strip(text),
        labels.join("、")
    ))
}

const MODE_ALIASES: [(&str, u32); 6] = [
    ("analog", 0),
    ("fm", 0),
    ("a", 0),
    ("digital", 1),
    ("dmr", 1),
    ("d", 1),
];
const SLOT_ALIASES: [(&str, u32); 6] = [
    ("ts1", 1),
    ("slot1", 1),
    ("1", 1),
    ("ts2", 2),
    ("slot2", 2),
    ("2", 2),
];

/// `re.match(r"^D?(\d{3})([NI]?)$", t)`: the three digits and the polarity letter ("" if none).
fn dcs_input(t: &str) -> Option<(&str, &str)> {
    let body = t.strip_prefix('D').unwrap_or(t);
    let digits_end = body.char_indices().nth(3).map_or(body.len(), |(at, _)| at);
    let (digits, polarity) = body.split_at(digits_end);
    let three_digits =
        digits.chars().count() == 3 && digits.chars().all(|ch| decimal_value(ch).is_some());
    (three_digits && matches!(polarity, "" | "N" | "I")).then_some((digits, polarity))
}

fn tone_parse(text: &str) -> Result<u16, String> {
    let t = strip(text).to_uppercase().replace(' ', "");
    if is_none_word(&t) {
        return Ok(cf::OFF);
    }
    if let Some((code, polarity)) = dcs_input(&t) {
        if t.starts_with('D') || !polarity.is_empty() {
            let index = DCS
                .iter()
                .position(|d| *d == code)
                .ok_or_else(|| format!("沒有 DCS {code}"))?;
            let kind: u16 = if polarity == "I" { 3 } else { 2 };
            return Ok(kind << 8 | index as u16);
        }
    }
    let invalid = || format!("無效的亞音「{}」", strip(text));
    let hz = match PyDecimal::parse(t.strip_suffix("HZ").unwrap_or(&t)) {
        Some(hz @ PyDecimal::Finite { .. }) => hz,
        _ => return Err(invalid()),
    };
    CTCSS
        .iter()
        .position(|c| PyDecimal::parse(c).as_ref() == Some(&hz))
        .map(|i| 1 << 8 | i as u16)
        .ok_or_else(|| format!("沒有 CTCSS {}", strip(text)))
}

/// `re.sub(r"\s*mhz$", "", t, flags=re.IGNORECASE)`.
fn without_mhz(t: &str) -> &str {
    let split = t.char_indices().rev().nth(2).map(|(at, _)| at);
    match split {
        Some(at) if t[at..].eq_ignore_ascii_case("mhz") => t[..at].trim_end_matches(is_space),
        _ => t,
    }
}

fn freq(project: &Project, text: &str) -> Result<u32, String> {
    let hz = mhz_to_hz(without_mhz(strip(text))).map_err(|e| e.to_string())?;
    if hz == 0 {
        return Err("頻率不可為 0".into());
    }
    if let Some(problem) = frequency_problem_hz(hz, project.device().bands().as_deref()) {
        return Err(problem);
    }
    u32::try_from(hz).map_err(|_| "頻率超出 32-bit 範圍".to_string())
}

// ---------------------------------------------------------------- columns
type Getter = Box<dyn Fn(&View, &Channel) -> String + Send + Sync>;
type Putter = Box<dyn Fn(&View, &Channel, &str) -> Result<Channel, String> + Send + Sync>;

/// One channel column: how it is shown and how typed text is applied.
pub struct Field {
    pub key: &'static str,
    pub header: String,
    pub aliases: Vec<&'static str>,
    get: Getter,
    put: Putter,
}

impl Field {
    /// The text the channel table and the CSV show for `channel`.
    pub fn text(&self, view: &View, channel: &Channel) -> String {
        (self.get)(view, channel)
    }

    /// `channel` with `text` applied, or why the text is not acceptable.
    pub fn apply(&self, view: &View, channel: &Channel, text: &str) -> Result<Channel, String> {
        (self.put)(view, channel, text)
    }
}

fn setting_field(
    field: &'static LazyLock<Setting>,
    key: &'static str,
    aliases: &[&'static str],
) -> Field {
    Field {
        key,
        header: field.label.clone(),
        aliases: aliases.to_vec(),
        get: Box::new(move |_, ch| field.display(&ch.0)),
        put: Box::new(move |_, ch, t| {
            let raw = choice(t, &field.choices, &[])?;
            cf::put(ch, field, i64::from(raw)).map_err(|e| e.to_string())
        }),
    }
}

fn ref_field(
    kind: RefKind,
    key: &'static str,
    header: &str,
    get_number: fn(&Channel) -> u32,
    set_number: fn(&Channel, u32) -> Result<Channel, String>,
    aliases: &[&'static str],
) -> Field {
    Field {
        key,
        header: header.into(),
        aliases: aliases.to_vec(),
        get: Box::new(move |v, ch| ref_text(v, kind, get_number(ch))),
        put: Box::new(move |v, ch, t| set_number(ch, ref_parse(v, kind, t)?)),
    }
}

fn tone_out(value: u16) -> String {
    if value == cf::OFF {
        String::new()
    } else {
        cf::tone_label(value)
    }
}

fn small(n: u32) -> Result<u8, String> {
    u8::try_from(n).map_err(|_| "編號超出範圍".to_string())
}

fn tone_field(key: &'static str, header: &str, tx: bool, aliases: &[&'static str]) -> Field {
    Field {
        key,
        header: header.into(),
        aliases: aliases.to_vec(),
        get: Box::new(move |_, ch| tone_out(cf::tone(ch, tx))),
        put: Box::new(move |_, ch, t| {
            cf::with_tone(ch, tx, tone_parse(t)?).map_err(|e| e.to_string())
        }),
    }
}

fn basic_fields() -> Vec<Field> {
    let mode_options: Vec<(String, u32)> = (0u32..)
        .zip(Channel::MODES)
        .map(|(v, label)| (label.to_string(), v))
        .collect();
    let slot_options: Vec<(String, u32)> = (0u32..)
        .zip(cf::SLOT_LABELS)
        .map(|(v, label)| (label.to_string(), v))
        .collect();
    let cc_options: Vec<(String, u32)> = (0..16).map(|i| (i.to_string(), i)).collect();
    vec![
        Field {
            key: "name",
            header: "名稱".into(),
            aliases: vec!["name", "channelname", "頻道名稱"],
            get: Box::new(|v, ch| ch.name(v.codec)),
            put: Box::new(|v, ch, t| ch.with_name(strip(t), v.codec).map_err(|e| e.to_string())),
        },
        Field {
            key: "rx",
            header: "接收MHz".into(),
            aliases: vec!["rx", "rxfreq", "接收", "接收頻率"],
            get: Box::new(|_, ch| hz_to_mhz(ch.rx_hz())),
            put: Box::new(|v, ch, t| Ok(ch.with_rx(freq(v.project, t)?))),
        },
        Field {
            key: "tx",
            header: "發射MHz".into(),
            aliases: vec!["tx", "txfreq", "發射", "發射頻率"],
            get: Box::new(|_, ch| hz_to_mhz(ch.tx_hz())),
            put: Box::new(|v, ch, t| Ok(ch.with_tx(freq(v.project, t)?))),
        },
        Field {
            key: "mode",
            header: "模式".into(),
            aliases: vec!["mode"],
            get: Box::new(|_, ch| ch.mode_label().to_string()),
            put: Box::new(move |_, ch, t| {
                let mode = choice(t, &mode_options, &MODE_ALIASES)?;
                ch.with_mode(small(mode)?).map_err(|e| e.to_string())
            }),
        },
        Field {
            key: "cc",
            header: "色碼".into(),
            aliases: vec!["cc", "colorcode", "色馬"],
            get: Box::new(|_, ch| ch.color_code().to_string()),
            put: Box::new(move |_, ch, t| {
                let cc = choice(t, &cc_options, &[])?;
                cf::with_color_code(ch, small(cc)?).map_err(|e| e.to_string())
            }),
        },
        Field {
            key: "slot",
            header: "時隙".into(),
            aliases: vec!["slot", "timeslot", "ts"],
            get: Box::new(|_, ch| cf::slot_label(ch)),
            put: Box::new(move |_, ch, t| {
                let slot = choice(t, &slot_options, &SLOT_ALIASES)?;
                cf::with_slot(ch, slot as usize).map_err(|e| e.to_string())
            }),
        },
    ]
}

fn build_fields() -> Vec<Field> {
    let mut fields = basic_fields();
    fields.extend([
        setting_field(&cf::POWER, "power", &["power"]),
        setting_field(&cf::CALL_INDICATOR, "call_indicator", &["callindicator"]),
        setting_field(
            &cf::PRIVATE_CALL_TYPE,
            "private_call_type",
            &["privatecalltype"],
        ),
        setting_field(&cf::MESSAGE_TYPE, "message_type", &["messagetype"]),
        setting_field(&cf::TX_POLICY, "tx_policy", &["txpolicy", "admit"]),
        ref_field(
            RefKind::Contact,
            "contact",
            "發話對象",
            |ch| u32::from(ch.tx_contact()),
            |ch, n| {
                u16::try_from(n)
                    .map(|n| ch.with_tx_contact(n))
                    .map_err(|_| "聯絡人編號超出範圍".into())
            },
            &["contact", "txcontact"],
        ),
        ref_field(
            RefKind::Group,
            "group",
            "接收群組",
            |ch| u32::from(ch.rx_group()),
            |ch, n| ch.with_rx_group(small(n)?).map_err(|e| e.to_string()),
            &["rxgroup", "grouplist"],
        ),
        ref_field(
            RefKind::Key,
            "key",
            "加密列表",
            |ch| u32::from(cf::encryption_key(ch)),
            |ch, n| cf::with_encryption_key(ch, small(n)?).map_err(|e| e.to_string()),
            &["encryption", "key"],
        ),
        ref_field(
            RefKind::Scan,
            "scan",
            "掃描列表",
            |ch| cf::get(ch, &cf::SCAN_LIST),
            |ch, n| cf::put(ch, &cf::SCAN_LIST, i64::from(n)).map_err(|e| e.to_string()),
            &["scanlist", "scan"],
        ),
        ref_field(
            RefKind::Alarm,
            "alarm",
            "緊急系統",
            |ch| cf::get(ch, &cf::EMERGENCY),
            |ch, n| cf::put(ch, &cf::EMERGENCY, i64::from(n)).map_err(|e| e.to_string()),
            &["emergency", "alarm"],
        ),
        tone_field(
            "rx_tone",
            "接收亞音",
            false,
            &["rxtone", "rxctcss", "接收亞音頻"],
        ),
        tone_field(
            "tx_tone",
            "發射亞音",
            true,
            &["txtone", "txctcss", "發射亞音頻"],
        ),
    ]);
    fields
}

/// The channel columns, in CSV / table order.
pub static FIELDS: LazyLock<Vec<Field>> = LazyLock::new(build_fields);

/// Columns that locate a row: (key, header, aliases).
pub const LOCATE: [(&str, &str, &[&str]); 3] = [
    ("zone", "分區", &["zone", "zoneno", "分區編號", "区域"]),
    ("zone_name", "分區名稱", &["zonename"]),
    ("ch", "頻道", &["ch", "channel", "no", "頻道編號", "位置"]),
];

pub fn header() -> Vec<String> {
    LOCATE
        .iter()
        .map(|(_, title, _)| title.to_string())
        .chain(FIELDS.iter().map(|f| f.header.clone()))
        .collect()
}

fn norm(header: &str) -> String {
    casefold(strip(header))
        .replace([' ', '_'], "")
        .replace("(mhz)", "mhz")
        .replace("（mhz）", "mhz")
}

fn column_map(header: &[String]) -> HashMap<&'static str, usize> {
    let mut names: HashMap<String, &'static str> = HashMap::new();
    for (key, title, aliases) in LOCATE {
        for alias in std::iter::once(title).chain(aliases.iter().copied()) {
            names.insert(norm(alias), key);
        }
    }
    for f in FIELDS.iter() {
        for alias in [f.header.as_str(), f.key]
            .into_iter()
            .chain(f.aliases.iter().copied())
        {
            names.insert(norm(alias), f.key);
        }
    }
    let mut out = HashMap::new();
    for (i, h) in header.iter().enumerate() {
        let n = norm(h);
        let key = names
            .get(&n)
            .or_else(|| n.strip_suffix("mhz").and_then(|s| names.get(s)));
        if let Some(key) = key {
            out.entry(*key).or_insert(i);
        }
    }
    out
}

// ---------------------------------------------------------------- export
/// The CSV file for `project`; fails if the zones do not tile the channel list.
pub fn export_channels(project: &Project, codec: NameCodec) -> Result<Vec<u8>, LayoutError> {
    check_layout(project)?;
    let view = View::new(project, codec);
    let mut text = pycsv::write_row(&header());
    for (zone_row, zone) in project.zones.iter().enumerate() {
        for (position, row) in zone_channel_rows(project, zone_row).enumerate() {
            let ch = &project.channels[row];
            let mut cells = vec![
                (zone_row + 1).to_string(),
                zone.name(codec),
                (position + 1).to_string(),
            ];
            cells.extend(FIELDS.iter().map(|f| f.text(&view, ch)));
            text.push_str(&pycsv::write_row(&cells));
        }
    }
    Ok([b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat())
}

// ---------------------------------------------------------------- import
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelImport {
    pub project: Project,
    pub updated: usize,
    pub added: usize,
    pub unchanged: usize,
    pub errors: Vec<String>,
    pub encoding: String,
}

impl ChannelImport {
    fn failed(project: &Project, message: String, encoding: &str) -> Self {
        ChannelImport {
            project: project.clone(),
            updated: 0,
            added: 0,
            unchanged: 0,
            errors: vec![message],
            encoding: encoding.into(),
        }
    }

    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("更新 {} 個頻道", self.updated),
            format!("新增 {} 個", self.added),
            format!("未變動 {} 個", self.unchanged),
        ];
        if !self.errors.is_empty() {
            parts.push(format!("{} 列略過", self.errors.len()));
        }
        parts.join("，")
    }
}

type Columns = HashMap<&'static str, usize>;

fn cell<'a>(row: &'a [String], columns: &Columns, key: &str) -> &'a str {
    columns
        .get(key)
        .and_then(|i| row.get(*i))
        .map_or("", |c| strip(c))
}

fn apply_row(
    view: &View,
    channel: &Channel,
    row: &[String],
    columns: &Columns,
    line: usize,
    new: bool,
) -> (Channel, Vec<String>) {
    let mut ch = *channel;
    let mut errors = Vec::new();
    for f in FIELDS.iter() {
        let value = columns
            .get(f.key)
            .and_then(|i| row.get(*i))
            .filter(|c| !strip(c).is_empty());
        let Some(value) = value else {
            if new && f.key == "name" {
                errors.push(format!("第 {line} 列：新頻道需要名稱"));
            }
            continue;
        };
        if !new && strip(value) == strip(&f.text(view, &ch)) {
            continue;
        }
        match f.apply(view, &ch, value) {
            Ok(updated) => ch = updated,
            Err(e) => errors.push(format!("第 {line} 列「{}」：{e}", f.header)),
        }
    }
    (ch, errors)
}

/// CSV rows; a file whose header is one cell holding ';' or a tab uses that delimiter.
fn read_rows(text: &str) -> Result<Vec<Vec<String>>, pycsv::CsvError> {
    let rows: Vec<Vec<String>> = pycsv::read(text).collect::<Result<_, _>>()?;
    if let Some(first) = rows.first().filter(|r| r.len() == 1) {
        for delimiter in [';', '\t'] {
            if first[0].contains(delimiter) {
                return pycsv::read_with(text, delimiter).collect();
            }
        }
    }
    Ok(rows)
}

/// Position (1-based) of a channel in `rows` with the same name and frequencies, or 0.
fn same_channel(
    project: &Project,
    rows: Range<usize>,
    channel: &Channel,
    codec: NameCodec,
) -> usize {
    let key = (channel.name(codec), channel.rx_hz(), channel.tx_hz());
    rows.enumerate()
        .find(|(_, r)| {
            let other = &project.channels[*r];
            (other.name(codec), other.rx_hz(), other.tx_hz()) == key
        })
        .map_or(0, |(i, _)| i + 1)
}

/// Where a row goes: an existing channel (row in the project) or the end of its zone.
enum Target {
    Existing(usize),
    Append,
}

/// Running state of an import; positions refer to the zones as they were before it. The view is
/// of the original project: an import changes channels only, never the names references show.
struct Importer<'a> {
    view: View<'a>,
    columns: Columns,
    original_len: Vec<usize>,
    zone_names: Vec<String>,
    seen: HashMap<(usize, usize), usize>,
    result: ChannelImport,
}

impl Importer<'_> {
    /// The zone row and target of a CSV row, or the reason it is skipped.
    fn locate(&mut self, row: &[String], line: usize) -> Result<(usize, Target), String> {
        let zone_count = self.zone_names.len();
        let zone_text = cell(row, &self.columns, "zone");
        let zone_no = decimal_usize(zone_text)
            .filter(|z| (1..=zone_count).contains(z))
            .ok_or_else(|| format!("第 {line} 列：分區「{zone_text}」不存在（1–{zone_count}）"))?;
        let zone_row = zone_no - 1;
        let zone_name = &self.zone_names[zone_row];
        let given_name = cell(row, &self.columns, "zone_name");
        if !given_name.is_empty() && given_name != zone_name {
            return Err(format!(
                "第 {line} 列：分區名稱「{given_name}」與分區 {zone_no}「{zone_name}」不符（匯入不會改分區名稱；若分區順序變了，請更正分區編號）"
            ));
        }
        let ch_text = cell(row, &self.columns, "ch");
        if !ch_text.is_empty() && !is_decimal(ch_text) {
            return Err(format!("第 {line} 列：頻道「{ch_text}」不是數字"));
        }
        let position = decimal_usize(ch_text).unwrap_or(0);
        if !(1..=self.original_len[zone_row]).contains(&position) {
            return Ok((zone_row, Target::Append));
        }
        if let Some(first) = self.seen.get(&(zone_row, position)) {
            return Err(format!(
                "第 {line} 列：分區 {zone_no} 第 {position} 台已由第 {first} 列修改，同一頻道只能出現一次"
            ));
        }
        self.seen.insert((zone_row, position), line);
        // Appends only go after the original channels, so the position still points the same.
        let rows = zone_channel_rows(&self.result.project, zone_row);
        Ok((zone_row, Target::Existing(rows.start + position - 1)))
    }

    fn update(&mut self, target: usize, row: &[String], line: usize) {
        let current = &self.result.project;
        let original = current.channels[target];
        let (channel, errors) = apply_row(&self.view, &original, row, &self.columns, line, false);
        if !errors.is_empty() {
            self.result.errors.extend(errors);
        } else if channel == original {
            self.result.unchanged += 1;
        } else if let Some(next) = current.with_channel(target, channel) {
            self.result.project = next;
            self.result.updated += 1;
        } else {
            self.result
                .errors
                .push(format!("第 {line} 列：找不到要更新的頻道"));
        }
    }

    fn append(&mut self, zone_row: usize, row: &[String], line: usize) -> Result<(), String> {
        let current = &self.result.project;
        let rows = zone_channel_rows(current, zone_row);
        let zone_no = zone_row + 1;
        let Some(last) = rows.end.checked_sub(1).filter(|_| !rows.is_empty()) else {
            return Err(format!(
                "第 {line} 列：分區 {zone_no} 沒有頻道可當新頻道的範本"
            ));
        };
        let template = current.channels[last];
        let (channel, errors) = apply_row(&self.view, &template, row, &self.columns, line, true);
        if !errors.is_empty() {
            self.result.errors.extend(errors);
            return Ok(());
        }
        let twin = same_channel(current, rows, &channel, self.view.codec);
        if twin != 0 {
            return Err(format!(
                "第 {line} 列：分區 {zone_no} 第 {twin} 台已有相同名稱與頻率的頻道，略過以免重複新增"
            ));
        }
        let (next, new_row) =
            add_channel(current, last).map_err(|e| format!("第 {line} 列：{e}"))?;
        let index = next.channels[new_row].index();
        let next = next
            .with_channel(new_row, channel.with_index(index))
            .ok_or_else(|| format!("第 {line} 列：新增的頻道不見了"))?;
        self.result.project = next;
        self.result.added += 1;
        Ok(())
    }

    fn row(&mut self, row: &[String], line: usize) {
        let outcome = self
            .locate(row, line)
            .and_then(|(zone_row, target)| match target {
                Target::Existing(target) => {
                    self.update(target, row, line);
                    Ok(())
                }
                Target::Append => self.append(zone_row, row, line),
            });
        if let Err(message) = outcome {
            self.result.errors.push(message);
        }
    }
}

/// Apply a channel CSV to `project`; never fails on bad content (problems are listed).
pub fn import_channels(project: &Project, data: &[u8], codec: NameCodec) -> ChannelImport {
    let (text, encoding) = read_legacy_text(data);
    if let Err(e) = check_layout(project) {
        return ChannelImport::failed(
            project,
            format!("專案的分區配置有誤，無法匯入：{e}"),
            encoding,
        );
    }
    let rows = match read_rows(&text) {
        Ok(rows) => rows,
        Err(e) => return ChannelImport::failed(project, format!("無法解析 CSV：{e}"), encoding),
    };
    let Some(header_row) = rows.first() else {
        return ChannelImport::failed(project, "檔案是空的".into(), encoding);
    };
    let columns = column_map(header_row);
    let missing: Vec<&str> = LOCATE
        .iter()
        .filter(|(key, _, _)| matches!(*key, "zone" | "ch") && !columns.contains_key(key))
        .map(|(_, title, _)| *title)
        .collect();
    if !missing.is_empty() {
        return ChannelImport::failed(
            project,
            format!("缺少欄位：{}", missing.join("、")),
            encoding,
        );
    }
    let mut importer = Importer {
        view: View::new(project, codec),
        columns,
        original_len: (0..project.zones.len())
            .map(|z| zone_channel_rows(project, z).len())
            .collect(),
        zone_names: project
            .zones
            .iter()
            .map(|z| strip(&z.name(codec)).to_string())
            .collect(),
        seen: HashMap::new(),
        result: ChannelImport {
            project: project.clone(),
            updated: 0,
            added: 0,
            unchanged: 0,
            errors: Vec::new(),
            encoding: encoding.into(),
        },
    };
    for (i, row) in rows.iter().enumerate().skip(1) {
        if row.iter().all(|c| strip(c).is_empty()) {
            continue;
        }
        importer.row(row, i + 1);
    }
    importer.result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reference_numbers_like_the_python_regex() {
        assert_eq!(ref_number("12"), Some(("12", 12, None)));
        assert_eq!(ref_number("#１２ TG 9"), Some(("１２", 12, Some("TG 9"))));
        assert_eq!(ref_number("12\x1cname"), Some(("12", 12, Some("name"))));
        assert_eq!(ref_number("12 \n name"), Some(("12", 12, Some("name"))));
        assert_eq!(ref_number("12 a\nb"), None);
        assert_eq!(ref_number("12abc"), None);
        assert_eq!(ref_number("TG9"), None);
    }

    #[test]
    fn tones_and_choices() {
        assert_eq!(
            tone_parse(" d023n ").map(cf::tone_label),
            Ok("D023N".into())
        );
        assert_eq!(tone_parse("754I").map(|v| v >> 8), Ok(3));
        assert_eq!(tone_parse("88.50 Hz"), tone_parse("８８.５"));
        assert_eq!(tone_parse("OFF"), Ok(0));
        assert_eq!(tone_parse("D999"), Err("沒有 DCS 999".into()));
        assert_eq!(tone_parse("nan"), Err("無效的亞音「nan」".into()));
        assert_eq!(tone_parse("1E999"), Err("沒有 CTCSS 1E999".into()));
        let options = vec![("低".to_string(), 0), ("高".to_string(), 1)];
        assert_eq!(choice(" １ ", &options, &[]), Ok(1));
        assert!(
            choice("²", &options, &[])
                .unwrap_err()
                .starts_with("無效的選項「²」")
        );
        assert_eq!(without_mhz("439.5 \u{3000}MHz"), "439.5");
        assert_eq!(without_mhz("mhz"), "");
        assert_eq!(without_mhz("中z"), "中z");
    }
}
