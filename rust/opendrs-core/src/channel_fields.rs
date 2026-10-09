//! Channel record bit fields, as the legacy CPS channel grid edits them (port of
//! `channel_fields.py`). Offsets are within the 32-byte channel record:
//!
//! * +0x14: bits 7-6 channel type, bit 5 power (高), bit 2 call indicator, bit 1 private-call
//!   type (Oacsu), bit 0 together with +0x1D bits 0-2 the 時隙 choice;
//! * +0x15: bits 0-3 color code (repeated in +0x1D bits 4-7), bit 4 confirmed data, bits 6-5 TX
//!   policy;
//! * +0x16 encryption key number, +0x17 RX group list, +0x18 scan list, +0x19 emergency alarm
//!   list, +0x1E TX contact;
//! * +0x1A squelch types (bits 0-1 RX, bits 2-3 TX: 0 off, 1 CTCSS, 2 DCS normal, 3 DCS inverted),
//!   +0x1B RX code, +0x1C TX code — an index into the CTCSS or DCS list.

use std::sync::LazyLock;

use crate::radio_settings::{Setting, SettingError, choices, off_on};
use crate::records::{Channel, RecordError};

pub const PAGE: &str = "頻道";

fn field(key: &str, label: &str, offset: usize, mask: u32, opts: Vec<(String, u32)>) -> Setting {
    Setting::new(key, PAGE, label, offset, mask).with_choices(opts)
}

pub static POWER: LazyLock<Setting> =
    LazyLock::new(|| field("power", "功率", 0x14, 0x20, choices(&["低", "高"], 0)));
pub static CALL_INDICATOR: LazyLock<Setting> =
    LazyLock::new(|| field("call_indicator", "呼叫指示", 0x14, 0x04, off_on()));
pub static PRIVATE_CALL_TYPE: LazyLock<Setting> = LazyLock::new(|| {
    field(
        "private_call_type",
        "個呼類型",
        0x14,
        0x02,
        choices(&["Patcs", "Oacsu"], 0),
    )
});
pub static MESSAGE_TYPE: LazyLock<Setting> = LazyLock::new(|| {
    field(
        "message_type",
        "短信類型",
        0x15,
        0x10,
        choices(&["非確認數據", "確認數據"], 0),
    )
});
/// Raw 0 appears in projects written by older tools; the legacy grid shows it as Impolite.
pub static TX_POLICY: LazyLock<Setting> = LazyLock::new(|| {
    let opts = [
        ("Impolite", 1),
        ("Polite to CC", 2),
        ("Polite to ALL", 3),
        ("Impolite（舊值 0）", 0),
    ];
    field(
        "tx_policy",
        "發射權限",
        0x15,
        0x60,
        opts.iter().map(|(l, v)| (l.to_string(), *v)).collect(),
    )
});
pub static SCAN_LIST: LazyLock<Setting> =
    LazyLock::new(|| Setting::new("scan_list", PAGE, "掃描列表", 0x18, 0x3F).with_range(0, 16));
pub static EMERGENCY: LazyLock<Setting> =
    LazyLock::new(|| Setting::new("emergency", PAGE, "緊急系統", 0x19, 0xFF).with_range(0, 4));

pub fn fields() -> [&'static Setting; 5] {
    [
        &POWER,
        &CALL_INDICATOR,
        &PRIVATE_CALL_TYPE,
        &MESSAGE_TYPE,
        &TX_POLICY,
    ]
}

/// 時隙: (bit 0 of +0x14, bits 0-2 of +0x1D) for each legacy option.
pub const SLOT_CHOICES: [(&str, (u8, u8)); 3] =
    [("On", (0, 0)), ("時隙1", (0, 5)), ("時隙2", (1, 7))];
pub const SLOT_LABELS: [&str; 3] = ["On", "時隙1", "時隙2"];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FieldError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Setting(#[from] SettingError),
    #[error(transparent)]
    Record(#[from] RecordError),
}

fn with_bytes(channel: &Channel, edit: impl FnOnce(&mut [u8; 32])) -> Channel {
    let mut raw = channel.0;
    edit(&mut raw);
    Channel(raw)
}

pub fn get(channel: &Channel, field: &Setting) -> u32 {
    (u32::from(channel.0[field.offset]) & field.mask) >> field.shift()
}

pub fn put(channel: &Channel, field: &Setting, raw: i64) -> Result<Channel, FieldError> {
    Ok(Channel::from_bytes(&field.with_raw(&channel.0, raw)?)?)
}

/// Index into SLOT_CHOICES, or `None` for a combination the legacy CPS never writes.
pub fn slot(channel: &Channel) -> Option<usize> {
    let state = (channel.0[0x14] & 1, channel.0[0x1D] & 7);
    SLOT_CHOICES.iter().position(|(_, bits)| *bits == state)
}

pub fn slot_label(channel: &Channel) -> String {
    match slot(channel) {
        Some(i) => SLOT_LABELS[i].to_string(),
        None => format!("未知（{}/{}）", channel.0[0x14] & 1, channel.0[0x1D] & 7),
    }
}

pub fn with_slot(channel: &Channel, index: usize) -> Result<Channel, FieldError> {
    let (_, (bit0, low3)) = SLOT_CHOICES
        .get(index)
        .ok_or_else(|| FieldError::Invalid("時隙選項無效".into()))?;
    Ok(with_bytes(channel, |raw| {
        raw[0x14] = (raw[0x14] & 0xFE) | bit0;
        raw[0x1D] = (raw[0x1D] & 0xF8) | low3;
    }))
}

/// The legacy CPS writes the color code twice: +0x15 low and +0x1D high nibble.
pub fn with_color_code(channel: &Channel, cc: u8) -> Result<Channel, FieldError> {
    let updated = channel.with_color_code(cc)?;
    Ok(with_bytes(&updated, |raw| {
        raw[0x1D] = (raw[0x1D] & 0x0F) | (cc << 4)
    }))
}

pub fn encryption_key(channel: &Channel) -> u8 {
    channel.0[0x16]
}

pub fn with_encryption_key(channel: &Channel, number: u8) -> Result<Channel, FieldError> {
    if number > 10 {
        return Err(FieldError::Invalid("加密列表編號必須介於 0 與 10".into()));
    }
    Ok(with_bytes(channel, |raw| raw[0x16] = number))
}

// ---------------------------------------------------------------- CTCSS / DCS
pub const CTCSS: [&str; 51] = [
    "62.5", "67.0", "69.3", "71.9", "74.4", "77.0", "79.7", "82.5", "85.4", "88.5", "91.5", "94.8",
    "97.4", "100.0", "103.5", "107.2", "110.9", "114.8", "118.8", "123.0", "127.3", "131.8",
    "136.5", "141.3", "146.2", "151.4", "156.7", "159.8", "162.2", "165.5", "167.9", "171.3",
    "173.8", "177.3", "179.9", "183.5", "186.2", "189.9", "192.8", "196.6", "199.5", "203.5",
    "206.5", "210.7", "218.1", "225.7", "229.1", "233.6", "241.8", "250.3", "254.1",
];
pub const DCS: [&str; 108] = [
    "017", "023", "025", "026", "031", "032", "036", "043", "047", "050", "051", "053", "054",
    "065", "071", "072", "073", "074", "114", "115", "116", "122", "125", "131", "132", "134",
    "143", "145", "152", "155", "156", "162", "165", "172", "174", "205", "212", "223", "225",
    "226", "243", "244", "245", "246", "251", "252", "255", "261", "263", "265", "266", "271",
    "274", "306", "311", "315", "325", "331", "332", "343", "346", "351", "356", "364", "365",
    "371", "411", "412", "413", "423", "431", "432", "445", "446", "452", "454", "455", "462",
    "464", "465", "466", "503", "506", "516", "523", "526", "532", "546", "565", "606", "612",
    "624", "627", "631", "632", "645", "646", "654", "662", "664", "703", "712", "723", "731",
    "732", "734", "743", "754",
];
pub const TONE_TYPE_OFFSET: usize = 0x1A;
pub const RX_CODE_OFFSET: usize = 0x1B;
pub const TX_CODE_OFFSET: usize = 0x1C;
pub const OFF: u16 = 0;

/// All legacy options as `(label, value)` with value = type << 8 | code.
pub fn tone_choices() -> Vec<(String, u16)> {
    let mut out = vec![("Off".to_string(), 0)];
    out.extend(
        (0u16..)
            .zip(CTCSS)
            .map(|(i, hz)| (format!("{hz} Hz"), 1 << 8 | i)),
    );
    out.extend(
        (0u16..)
            .zip(DCS)
            .map(|(i, code)| (format!("D{code}N"), 2 << 8 | i)),
    );
    out.extend(
        (0u16..)
            .zip(DCS)
            .map(|(i, code)| (format!("D{code}I"), 3 << 8 | i)),
    );
    out
}

pub fn tone_label(value: u16) -> String {
    let (kind, code) = (value >> 8, usize::from(value & 0xFF));
    let table: Option<&[&str]> = match kind {
        1 => Some(&CTCSS),
        2 | 3 => Some(&DCS),
        _ => None,
    };
    match (kind, table.and_then(|t| t.get(code))) {
        (0, _) => "Off".to_string(),
        (1, Some(hz)) => format!("{hz} Hz"),
        (2, Some(c)) => format!("D{c}N"),
        (3, Some(c)) => format!("D{c}I"),
        _ => format!("未知（{kind}/{code}）"),
    }
}

pub fn tone(channel: &Channel, tx: bool) -> u16 {
    let kind = u16::from((channel.0[TONE_TYPE_OFFSET] >> if tx { 2 } else { 0 }) & 3);
    let code = u16::from(channel.0[if tx { TX_CODE_OFFSET } else { RX_CODE_OFFSET }]);
    kind << 8 | if kind != 0 { code } else { 0 }
}

pub fn with_tone(channel: &Channel, tx: bool, value: u16) -> Result<Channel, FieldError> {
    if !tone_choices().iter().any(|(_, v)| *v == value) {
        return Err(FieldError::Invalid("亞音選項無效".into()));
    }
    let (kind, code) = ((value >> 8) as u8, (value & 0xFF) as u8);
    let shift = if tx { 2 } else { 0 };
    Ok(with_bytes(channel, |raw| {
        raw[TONE_TYPE_OFFSET] = (raw[TONE_TYPE_OFFSET] & !(3 << shift)) | (kind << shift);
        raw[if tx { TX_CODE_OFFSET } else { RX_CODE_OFFSET }] = code;
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_round_trip() {
        let ch = Channel([0; 32]);
        let d023n = tone_choices()
            .into_iter()
            .find(|(l, _)| l == "D023N")
            .unwrap()
            .1;
        let ch = with_tone(&ch, true, d023n).unwrap();
        assert_eq!(tone_label(tone(&ch, true)), "D023N");
        assert_eq!(tone(&ch, false), OFF);
        assert!(with_tone(&ch, false, 1 << 8 | 51).is_err());
        assert_eq!(tone_label(5 << 8), "未知（5/0）");
    }

    #[test]
    fn slots_and_color_code() {
        let ch = with_slot(&Channel([0; 32]), 2).unwrap();
        assert_eq!(slot_label(&ch), "時隙2");
        let ch = with_color_code(&ch, 6).unwrap();
        assert_eq!((ch.color_code(), ch.0[0x1D] >> 4), (6, 6));
        assert_eq!(slot_label(&ch), "時隙2"); // the color code nibble does not touch the slot bits
    }
}
