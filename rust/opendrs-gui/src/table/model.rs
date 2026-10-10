//! What the channel, zone and contact tables show and how a cell edit is applied (port of
//! `models.py` and the choice lists of `delegates.py`). Pure functions of the project, so the
//! egui table only draws and the behaviour is compared with the Python version cell by cell.

use std::sync::LazyLock;

use opendrs_core::alarm_enc::{alarm_lists, encryption_keys};
use opendrs_core::channel_fields as cf;
use opendrs_core::device::frequency_problem;
use opendrs_core::f136::Project;
use opendrs_core::layout::{move_channel, zone_rows};
use opendrs_core::pystr::{PyInt, casefold, strip};
use opendrs_core::radio_settings::Setting;
use opendrs_core::records::{Channel, hz_to_mhz, mhz_to_hz};
use opendrs_core::rxgroup::rx_groups;
use opendrs_core::scanlist::scan_lists;
use opendrs_core::textcodec::NameCodec;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Table {
    Channels,
    Zones,
    Contacts,
}

/// What a column holds; decides its text, sort key and editor.
#[derive(Clone, Copy, Debug)]
pub enum ColumnKind {
    Index,
    Name,
    Frequency {
        tx: bool,
    },
    Zone,
    TxContact,
    RxGroup,
    Mode,
    ColorCode,
    Slot,
    Field(&'static LazyLock<Setting>),
    EncryptionKey,
    ScanList,
    Emergency,
    Tone {
        tx: bool,
    },
    /// The record's remaining bytes from this offset, as hex.
    Rest(usize),
    ContactKind,
    CallId,
}

#[derive(Clone, Debug)]
pub struct Column {
    pub title: String,
    pub kind: ColumnKind,
}

/// How a cell is edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Editor {
    ReadOnly,
    Text,
    /// A drop-down of `(label, value)`; `none` is the label of an extra first entry with value 0.
    Choices {
        items: Vec<(String, i64)>,
        none: Option<&'static str>,
        searchable: bool,
    },
}

/// The input of an edit: typed text, or the value picked in a drop-down.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input {
    Text(String),
    Value(i64),
}

/// Sort keys compare numbers as numbers and everything else by case-folded text.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SortKey {
    Number(i64),
    Text(String),
}

fn column(title: &str, kind: ColumnKind) -> Column {
    Column {
        title: title.into(),
        kind,
    }
}

pub static CHANNEL_COLUMNS: LazyLock<Vec<Column>> = LazyLock::new(|| {
    let mut columns = vec![
        column("#", ColumnKind::Index),
        column("名稱", ColumnKind::Name),
        column("接收 (MHz)", ColumnKind::Frequency { tx: false }),
        column("發射 (MHz)", ColumnKind::Frequency { tx: true }),
        column("分區", ColumnKind::Zone),
        column("發話對象", ColumnKind::TxContact),
        column("接收群組", ColumnKind::RxGroup),
        column("模式", ColumnKind::Mode),
        column("色碼", ColumnKind::ColorCode),
        column("時隙", ColumnKind::Slot),
    ];
    let fields: [&'static LazyLock<Setting>; 5] = [
        &cf::POWER,
        &cf::CALL_INDICATOR,
        &cf::PRIVATE_CALL_TYPE,
        &cf::MESSAGE_TYPE,
        &cf::TX_POLICY,
    ];
    columns.extend(
        fields
            .into_iter()
            .map(|f| column(&f.label, ColumnKind::Field(f))),
    );
    columns.extend([
        column("加密列表", ColumnKind::EncryptionKey),
        column("掃描列表", ColumnKind::ScanList),
        column("緊急系統", ColumnKind::Emergency),
        column("接收亞音", ColumnKind::Tone { tx: false }),
        column("發射亞音", ColumnKind::Tone { tx: true }),
        column("其餘位元組", ColumnKind::Rest(0x14)),
    ]);
    columns
});

pub static ZONE_COLUMNS: LazyLock<Vec<Column>> = LazyLock::new(|| {
    vec![
        column("#", ColumnKind::Index),
        column("名稱", ColumnKind::Name),
        column("其餘位元組", ColumnKind::Rest(13)),
    ]
});

pub static CONTACT_COLUMNS: LazyLock<Vec<Column>> = LazyLock::new(|| {
    vec![
        column("#", ColumnKind::Index),
        column("類型", ColumnKind::ContactKind),
        column("名稱", ColumnKind::Name),
        column("呼叫 ID", ColumnKind::CallId),
    ]
});

/// The contact kinds the type filter offers.
pub const CONTACT_KINDS: [(u8, &str); 4] = [(4, "組呼"), (5, "個呼"), (6, "全呼"), (7, "無地址呼")];

pub fn columns(table: Table) -> &'static [Column] {
    match table {
        Table::Channels => &CHANNEL_COLUMNS,
        Table::Zones => &ZONE_COLUMNS,
        Table::Contacts => &CONTACT_COLUMNS,
    }
}

pub fn row_count(project: &Project, table: Table) -> usize {
    match table {
        Table::Channels => project.channels.len(),
        Table::Zones => project.zones.len(),
        Table::Contacts => project.contacts.len(),
    }
}

/// The record's bytes and its index.
fn record(project: &Project, table: Table, row: usize) -> (&[u8], u16) {
    match table {
        Table::Channels => (&project.channels[row].0[..], project.channels[row].index()),
        Table::Zones => (&project.zones[row].0[..], project.zones[row].index()),
        Table::Contacts => (project.contacts[row].raw(), project.contacts[row].index()),
    }
}

fn name(project: &Project, table: Table, row: usize, codec: NameCodec) -> String {
    match table {
        Table::Channels => project.channels[row].name(codec),
        Table::Zones => project.zones[row].name(codec),
        Table::Contacts => project.contacts[row].name(codec),
    }
}

/// `" ".join(f"{b:02x}")`, like `bytes.hex(" ")`.
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn contact_label(contact: &opendrs_core::records::Contact, codec: NameCodec) -> String {
    format!(
        "#{} {}（{} {}）",
        contact.index(),
        contact.name(codec),
        contact.kind_label(),
        contact.call_id()
    )
}

/// `{n}. {name}`, `Off`/`（無）` for 0, `{n}（不存在）` past the end.
fn numbered(number: usize, names: &[String], zero: &str) -> String {
    match number {
        0 => zero.into(),
        n if n <= names.len() => format!("{n}. {}", names[n - 1]),
        n => format!("{n}（不存在）"),
    }
}

fn channel_text(project: &Project, row: usize, kind: ColumnKind, codec: NameCodec) -> String {
    let ch = &project.channels[row];
    match kind {
        ColumnKind::Frequency { tx } => hz_to_mhz(if tx { ch.tx_hz() } else { ch.rx_hz() }),
        ColumnKind::Zone => match zone_rows(project).get(row).copied().flatten() {
            Some(z) => format!("{}. {}", z + 1, project.zones[z].name(codec)),
            None => "（無）".into(),
        },
        ColumnKind::TxContact => match ch.tx_contact() {
            0 => "（無）".into(),
            n => project
                .contacts
                .iter()
                .find(|c| c.index() == n)
                .map_or_else(|| format!("#{n}（不存在）"), |c| contact_label(c, codec)),
        },
        ColumnKind::RxGroup => {
            let names: Vec<String> = rx_groups(&project.image)
                .iter()
                .map(|g| g.name(codec))
                .collect();
            numbered(usize::from(ch.rx_group()), &names, "（無）")
        }
        ColumnKind::Mode => ch.mode_label().into(),
        ColumnKind::ColorCode => ch.color_code().to_string(),
        ColumnKind::Slot => cf::slot_label(ch),
        ColumnKind::Field(field) => field.display(&ch.0),
        ColumnKind::EncryptionKey => {
            let names: Vec<String> = encryption_keys(&project.image)
                .iter()
                .map(|k| k.alias(codec))
                .collect();
            numbered(usize::from(cf::encryption_key(ch)), &names, "Off")
        }
        ColumnKind::ScanList => {
            let names: Vec<String> = scan_lists(&project.image)
                .iter()
                .map(|s| s.name(codec))
                .collect();
            numbered(cf::get(ch, &cf::SCAN_LIST) as usize, &names, "Off")
        }
        ColumnKind::Emergency => {
            let names: Vec<String> = alarm_lists(&project.image)
                .iter()
                .map(|a| a.name(codec))
                .collect();
            numbered(cf::get(ch, &cf::EMERGENCY) as usize, &names, "Off")
        }
        ColumnKind::Tone { tx } => cf::tone_label(cf::tone(ch, tx)),
        _ => String::new(),
    }
}

/// The text a cell shows (and the search matches).
pub fn text(project: &Project, table: Table, row: usize, col: usize, codec: NameCodec) -> String {
    let kind = columns(table)[col].kind;
    let (raw, index) = record(project, table, row);
    match kind {
        ColumnKind::Index => index.to_string(),
        ColumnKind::Name => name(project, table, row, codec),
        ColumnKind::Rest(offset) => hex(raw.get(offset..).unwrap_or_default()),
        ColumnKind::ContactKind => project.contacts[row].kind_label(),
        ColumnKind::CallId => project.contacts[row].call_id().to_string(),
        _ => channel_text(project, row, kind, codec),
    }
}

/// The number behind a drop-down cell (the entry its editor starts on).
pub fn value(project: &Project, table: Table, row: usize, col: usize) -> Option<i64> {
    if table != Table::Channels {
        return None;
    }
    let ch = &project.channels[row];
    Some(match columns(table)[col].kind {
        ColumnKind::Zone => zone_rows(project)
            .get(row)
            .copied()
            .flatten()
            .map_or(-1, |z| z as i64),
        ColumnKind::TxContact => i64::from(ch.tx_contact()),
        ColumnKind::RxGroup => i64::from(ch.rx_group()),
        ColumnKind::Mode => i64::from(ch.mode()),
        ColumnKind::ColorCode => i64::from(ch.color_code()),
        ColumnKind::Slot => cf::slot(ch).map_or(-1, |s| s as i64),
        ColumnKind::Field(field) => i64::from(cf::get(ch, field)),
        ColumnKind::EncryptionKey => i64::from(cf::encryption_key(ch)),
        ColumnKind::ScanList => i64::from(cf::get(ch, &cf::SCAN_LIST)),
        ColumnKind::Emergency => i64::from(cf::get(ch, &cf::EMERGENCY)),
        ColumnKind::Tone { tx } => i64::from(cf::tone(ch, tx)),
        _ => return None,
    })
}

pub fn sort_key(
    project: &Project,
    table: Table,
    row: usize,
    col: usize,
    codec: NameCodec,
) -> SortKey {
    let kind = columns(table)[col].kind;
    let number = |n: i64| SortKey::Number(n);
    match (table, kind) {
        (_, ColumnKind::Index) => number(i64::from(record(project, table, row).1)),
        (Table::Channels, ColumnKind::Frequency { tx }) => {
            let ch = &project.channels[row];
            number(i64::from(if tx { ch.tx_hz() } else { ch.rx_hz() }))
        }
        (Table::Channels, ColumnKind::RxGroup | ColumnKind::Mode | ColumnKind::ColorCode)
        | (Table::Channels, ColumnKind::Field(_) | ColumnKind::EncryptionKey)
        | (
            Table::Channels,
            ColumnKind::ScanList | ColumnKind::Emergency | ColumnKind::Tone { .. },
        ) => number(value(project, table, row, col).unwrap_or(0)),
        (Table::Contacts, ColumnKind::ContactKind) => {
            number(i64::from(project.contacts[row].kind()))
        }
        (Table::Contacts, ColumnKind::CallId) => number(i64::from(project.contacts[row].call_id())),
        _ => SortKey::Text(casefold(&text(project, table, row, col, codec))),
    }
}

/// The band problem of a frequency cell (shown in red with this tooltip), or `None`.
pub fn frequency_problem_of(
    project: &Project,
    table: Table,
    row: usize,
    col: usize,
) -> Option<String> {
    let ColumnKind::Frequency { tx } = columns(table)[col].kind else {
        return None;
    };
    if table != Table::Channels {
        return None;
    }
    let ch = &project.channels[row];
    let hz = if tx { ch.tx_hz() } else { ch.rx_hz() };
    frequency_problem(hz, project.device().bands().as_deref())
}

fn picker(items: Vec<(String, i64)>, none: Option<&'static str>) -> Editor {
    Editor::Choices {
        items,
        none,
        searchable: true,
    }
}

fn fixed(items: Vec<(String, i64)>) -> Editor {
    Editor::Choices {
        items,
        none: None,
        searchable: false,
    }
}

/// How the cell is edited; drop-down entries come from the project where needed.
pub fn editor(project: &Project, table: Table, col: usize, codec: NameCodec) -> Editor {
    let kind = columns(table)[col].kind;
    match (table, kind) {
        (_, ColumnKind::Name)
        | (Table::Channels, ColumnKind::Frequency { .. })
        | (Table::Contacts, ColumnKind::CallId) => Editor::Text,
        (Table::Channels, ColumnKind::Zone) => picker(
            project
                .zones
                .iter()
                .enumerate()
                .map(|(i, z)| (format!("{}. {}", i + 1, z.name(codec)), i as i64))
                .collect(),
            None,
        ),
        (Table::Channels, ColumnKind::TxContact) => picker(
            project
                .contacts
                .iter()
                .map(|c| (contact_label(c, codec), i64::from(c.index())))
                .collect(),
            Some("（無）"),
        ),
        (Table::Channels, ColumnKind::RxGroup) => picker(
            rx_groups(&project.image)
                .iter()
                .filter(|g| g.in_use())
                .map(|g| (format!("{}. {}", g.number, g.name(codec)), g.number as i64))
                .collect(),
            Some("（無）"),
        ),
        (Table::Channels, ColumnKind::EncryptionKey) => picker(
            encryption_keys(&project.image)
                .iter()
                .filter(|k| k.in_use())
                .map(|k| (format!("{}. {}", k.number, k.alias(codec)), k.number as i64))
                .collect(),
            Some("（無）"),
        ),
        (Table::Channels, ColumnKind::ScanList) => picker(
            scan_lists(&project.image)
                .iter()
                .filter(|s| s.in_use())
                .map(|s| (format!("{}. {}", s.number, s.name(codec)), s.number as i64))
                .collect(),
            Some("（無）"),
        ),
        (Table::Channels, ColumnKind::Emergency) => picker(
            alarm_lists(&project.image)
                .iter()
                .map(|a| (format!("{}. {}", a.number, a.name(codec)), a.number as i64))
                .collect(),
            Some("（無）"),
        ),
        (Table::Channels, ColumnKind::Mode) => fixed(
            (0i64..)
                .zip(Channel::MODES)
                .map(|(v, label)| (label.to_string(), v))
                .collect(),
        ),
        (Table::Channels, ColumnKind::ColorCode) => {
            fixed((0..16).map(|cc| (cc.to_string(), cc)).collect())
        }
        (Table::Channels, ColumnKind::Slot) => fixed(
            (0i64..)
                .zip(cf::SLOT_LABELS)
                .map(|(v, label)| (label.to_string(), v))
                .collect(),
        ),
        (Table::Channels, ColumnKind::Field(field)) => fixed(
            field
                .choices
                .iter()
                .map(|(label, raw)| (label.clone(), i64::from(*raw)))
                .collect(),
        ),
        (Table::Channels, ColumnKind::Tone { .. }) => fixed(
            cf::tone_choices()
                .into_iter()
                .map(|(label, value)| (label, i64::from(value)))
                .collect(),
        ),
        _ => Editor::ReadOnly,
    }
}

/// The result of an edit: the new project and its undo label (`編輯`, or `變更分區` for a
/// move to another zone); `None` when nothing changed.
pub type Edit = Option<(Project, String)>;

fn as_u8(value: i64) -> Result<u8, String> {
    u8::try_from(value).map_err(|_| format!("{value} 超出範圍"))
}

fn frequency(text: &str) -> Result<u32, String> {
    let hz = if strip(text).is_empty() {
        0
    } else {
        mhz_to_hz(text).map_err(|e| e.to_string())?
    };
    u32::try_from(hz).map_err(|_| "頻率超出 32-bit 範圍".to_string())
}

fn channel_edit(
    ch: &Channel,
    kind: ColumnKind,
    input: &Input,
    codec: NameCodec,
) -> Result<Channel, String> {
    let text = |input: &Input| match input {
        Input::Text(t) => t.clone(),
        Input::Value(v) => v.to_string(),
    };
    let number = |input: &Input| -> Result<i64, String> {
        match input {
            Input::Value(v) => Ok(*v),
            Input::Text(t) => PyInt::parse(t)
                .and_then(|n| n.to_i64())
                .ok_or_else(|| format!("「{t}」不是數字")),
        }
    };
    match kind {
        ColumnKind::Name => ch.with_name(&text(input), codec).map_err(|e| e.to_string()),
        ColumnKind::Frequency { tx: false } => Ok(ch.with_rx(frequency(&text(input))?)),
        ColumnKind::Frequency { tx: true } => Ok(ch.with_tx(frequency(&text(input))?)),
        ColumnKind::TxContact => {
            let n = number(input)?;
            u16::try_from(n)
                .map(|n| ch.with_tx_contact(n))
                .map_err(|_| "聯絡人編號超出範圍".to_string())
        }
        ColumnKind::RxGroup => {
            let n = number(input)?;
            let n = u8::try_from(n).map_err(|_| "接收群組編號必須介於 0 與 32".to_string())?;
            ch.with_rx_group(n).map_err(|e| e.to_string())
        }
        ColumnKind::Mode => ch
            .with_mode(as_u8(number(input)?)?)
            .map_err(|e| e.to_string()),
        ColumnKind::ColorCode => {
            cf::with_color_code(ch, as_u8(number(input)?)?).map_err(|e| e.to_string())
        }
        ColumnKind::Slot => {
            let n = usize::try_from(number(input)?).map_err(|_| "時隙超出範圍".to_string())?;
            cf::with_slot(ch, n).map_err(|e| e.to_string())
        }
        ColumnKind::Field(field) => cf::put(ch, field, number(input)?).map_err(|e| e.to_string()),
        ColumnKind::EncryptionKey => {
            cf::with_encryption_key(ch, as_u8(number(input)?)?).map_err(|e| e.to_string())
        }
        ColumnKind::ScanList => {
            cf::put(ch, &cf::SCAN_LIST, number(input)?).map_err(|e| e.to_string())
        }
        ColumnKind::Emergency => {
            cf::put(ch, &cf::EMERGENCY, number(input)?).map_err(|e| e.to_string())
        }
        ColumnKind::Tone { tx } => {
            let value = u16::try_from(number(input)?).map_err(|_| "亞音超出範圍".to_string())?;
            cf::with_tone(ch, tx, value).map_err(|e| e.to_string())
        }
        _ => Ok(*ch),
    }
}

/// Apply an edit to one cell; the error text is shown in 「無法套用」.
pub fn apply(
    project: &Project,
    table: Table,
    row: usize,
    col: usize,
    input: &Input,
    codec: NameCodec,
) -> Result<Edit, String> {
    let Some(column) = columns(table).get(col) else {
        return Ok(None);
    };
    if row >= row_count(project, table) {
        return Ok(None); // no such cell (Qt ignores an invalid index)
    }
    if let (Table::Channels, ColumnKind::Zone) = (table, column.kind) {
        let Input::Value(zone_row) = input else {
            return Ok(None);
        };
        let target = usize::try_from(*zone_row).map_err(|_| "目標分區不存在".to_string())?;
        let (moved, _row, _edit) = move_channel(project, row, target).map_err(|e| e.0)?;
        return Ok((moved != *project).then(|| (moved, format!("變更{}", column.title))));
    }
    if let Editor::ReadOnly = editor(project, table, col, codec) {
        return Ok(None);
    }
    let edited = match table {
        Table::Channels => {
            let ch = channel_edit(&project.channels[row], column.kind, input, codec)?;
            project.with_channel(row, ch)
        }
        Table::Zones => {
            let Input::Text(t) = input else {
                return Ok(None);
            };
            let zone = project.zones[row]
                .with_name(t, codec)
                .map_err(|e| e.to_string())?;
            project.with_zone(row, zone)
        }
        Table::Contacts => {
            let contact = &project.contacts[row];
            let edited = match (column.kind, input) {
                (ColumnKind::Name, Input::Text(t)) => {
                    contact.with_name(t, codec).map_err(|e| e.to_string())?
                }
                (ColumnKind::CallId, Input::Text(t)) => {
                    let n = PyInt::parse(strip(t))
                        .ok_or_else(|| format!("「{}」不是數字", strip(t)))?;
                    let id = n
                        .to_i64()
                        .and_then(|v| u32::try_from(v).ok())
                        .ok_or_else(|| "ID 必須介於 1 與 16777215".to_string())?;
                    contact.with_call_id(id).map_err(|e| e.to_string())?
                }
                _ => return Ok(None),
            };
            project.with_contact(row, edited)
        }
    };
    let edited = edited.ok_or_else(|| "找不到這一列".to_string())?;
    Ok((edited != *project).then(|| (edited, "編輯".to_string())))
}
