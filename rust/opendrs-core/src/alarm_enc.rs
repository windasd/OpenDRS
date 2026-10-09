//! Digital alarm lists, the encryption key list and the GPS report channel (port of
//! `alarm_enc.py`).
//!
//! * Alarm lists: 4 records of 0x12 bytes from 0xE3 — name[10] | +0x0A low nibble alarm type
//!   (常規 1, 禁止 2, 靜默 3, 靜默帶語音 4; high nibble kept) | +0x0B mode | +0x0C zone u16 + +0x0E
//!   channel u16 (alarm channel, 0 = Off) | +0x10 low nibble impolite attempts (1-15) | +0x11
//!   emergency-MIC duration in seconds (0-150).
//! * Encryption: level at 0x122E, then 10 keys of 16 bytes from 0x122F: alias[10] + 6 key bytes.

use crate::records::NAME_SIZE;
use crate::textcodec::{NameCodec, NameEncodingError, display_name, encode_name};

pub const ALARM_OFFSET: usize = 0xE3;
pub const ALARM_SIZE: usize = 0x12;
pub const ALARM_COUNT: usize = 4;
pub const ALARM_TYPES: [(&str, u8); 4] = [("常規", 1), ("禁止", 2), ("靜默", 3), ("靜默帶語音", 4)];
pub const ALARM_MODES: [(&str, u8); 3] = [
    ("緊急報警", 1),
    ("緊急報警和呼叫", 2),
    ("緊急報警和語音", 3),
];
pub const MAX_MIC_SECONDS: u8 = 150;

pub const ENC_LEVEL_OFFSET: usize = 0x122E;
pub const ENC_KEYS_OFFSET: usize = 0x122F;
pub const ENC_KEY_SIZE: usize = 16;
pub const ENC_KEY_BYTES: usize = 6;
pub const ENC_KEY_COUNT: usize = 10;
pub const ENC_END: usize = ENC_KEYS_OFFSET + ENC_KEY_SIZE * ENC_KEY_COUNT;

pub const GPS_ZONE_OFFSET: usize = 0x1354;
pub const GPS_CHANNEL_OFFSET: usize = 0x1356;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ListEditError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Name(#[from] NameEncodingError),
}

fn invalid(message: impl Into<String>) -> ListEditError {
    ListEditError::Invalid(message.into())
}

fn patch(image: &[u8], offset: usize, value: &[u8]) -> Result<Vec<u8>, ListEditError> {
    if image.len() < offset + value.len() {
        return Err(invalid("設定區太短，無法寫入"));
    }
    let mut out = image.to_vec();
    out[offset..offset + value.len()].copy_from_slice(value);
    Ok(out)
}

fn u16_at(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([raw[at], raw[at + 1]])
}

// ---------------------------------------------------------------- alarm lists
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlarmList {
    pub number: usize,
    pub raw: Vec<u8>,
}

impl AlarmList {
    pub fn name(&self, codec: NameCodec) -> String {
        display_name(&self.raw[..NAME_SIZE], codec)
    }
    pub fn alarm_type(&self) -> u8 {
        self.raw[0x0A] & 0x0F
    }
    pub fn mode(&self) -> u8 {
        self.raw[0x0B]
    }
    pub fn channel(&self) -> (u16, u16) {
        (u16_at(&self.raw, 0x0C), u16_at(&self.raw, 0x0E))
    }
    pub fn impolite_attempts(&self) -> u8 {
        self.raw[0x10] & 0x0F
    }
    pub fn mic_seconds(&self) -> u8 {
        self.raw[0x11]
    }
}

pub fn alarm_base(number: usize) -> usize {
    ALARM_OFFSET + (number - 1) * ALARM_SIZE
}

pub fn alarm_lists(image: &[u8]) -> Vec<AlarmList> {
    if image.len() < ALARM_OFFSET + ALARM_COUNT * ALARM_SIZE {
        return Vec::new();
    }
    (1..=ALARM_COUNT)
        .map(|n| AlarmList {
            number: n,
            raw: image[alarm_base(n)..alarm_base(n) + ALARM_SIZE].to_vec(),
        })
        .collect()
}

/// Fields to change on one alarm list; `None` keeps the stored bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AlarmEdit {
    pub name: Option<String>,
    pub alarm_type: Option<u8>,
    pub mode: Option<u8>,
    pub channel: Option<(u16, u16)>,
    pub impolite_attempts: Option<u8>,
    pub mic_seconds: Option<u8>,
}

/// Change some fields of alarm list `number` (1-4); others keep their bytes.
pub fn with_alarm(
    image: &[u8],
    number: usize,
    edit: &AlarmEdit,
    codec: NameCodec,
) -> Result<Vec<u8>, ListEditError> {
    if !(1..=ALARM_COUNT).contains(&number) {
        return Err(invalid(format!("報警列表編號必須介於 1 與 {ALARM_COUNT}")));
    }
    let lists = alarm_lists(image);
    let mut rec = lists
        .get(number - 1)
        .ok_or_else(|| invalid("設定區太短，沒有報警列表"))?
        .raw
        .clone();
    if let Some(name) = &edit.name {
        rec[..NAME_SIZE].copy_from_slice(&encode_name(name, NAME_SIZE, codec)?);
    }
    if let Some(alarm_type) = edit.alarm_type {
        if !ALARM_TYPES.iter().any(|(_, v)| *v == alarm_type) {
            return Err(invalid("報警類型無效"));
        }
        rec[0x0A] = (rec[0x0A] & 0xF0) | alarm_type;
    }
    if let Some(mode) = edit.mode {
        if !ALARM_MODES.iter().any(|(_, v)| *v == mode) {
            return Err(invalid("報警模式無效"));
        }
        rec[0x0B] = mode;
    }
    if let Some((zone, channel)) = edit.channel {
        rec[0x0C..0x0E].copy_from_slice(&zone.to_le_bytes());
        rec[0x0E..0x10].copy_from_slice(&channel.to_le_bytes());
    }
    if let Some(attempts) = edit.impolite_attempts {
        if !(1..=15).contains(&attempts) {
            return Err(invalid("不禮貌嘗試次數必須介於 1 與 15"));
        }
        rec[0x10] = (rec[0x10] & 0xF0) | attempts;
    }
    if let Some(seconds) = edit.mic_seconds {
        if seconds > MAX_MIC_SECONDS {
            return Err(invalid(format!(
                "緊急麥克風持續時間必須介於 0 與 {MAX_MIC_SECONDS} 秒"
            )));
        }
        rec[0x11] = seconds;
    }
    patch(image, alarm_base(number), &rec)
}

// ---------------------------------------------------------------- encryption keys
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptionKey {
    pub number: usize,
    pub raw: Vec<u8>,
}

impl EncryptionKey {
    pub fn alias(&self, codec: NameCodec) -> String {
        display_name(&self.raw[..NAME_SIZE], codec)
    }
    pub fn key(&self) -> &[u8] {
        &self.raw[NAME_SIZE..NAME_SIZE + ENC_KEY_BYTES]
    }
    pub fn in_use(&self) -> bool {
        !matches!(self.raw[0], 0x00 | 0xFF)
    }
}

pub fn encryption_keys(image: &[u8]) -> Vec<EncryptionKey> {
    if image.len() < ENC_END {
        return Vec::new();
    }
    (1..=ENC_KEY_COUNT)
        .map(|n| {
            let at = ENC_KEYS_OFFSET + (n - 1) * ENC_KEY_SIZE;
            EncryptionKey {
                number: n,
                raw: image[at..at + ENC_KEY_SIZE].to_vec(),
            }
        })
        .collect()
}

pub fn parse_key_hex(text: &str) -> Result<Vec<u8>, ListEditError> {
    // "".join(text.split()), then bytes.fromhex: hex digit pairs only (no sign, no 0x).
    let digits: String = crate::pystr::split(text).collect();
    let bad = || invalid("金鑰必須是 12 個十六進位數字（例如 12 34 56 78 9A BC）");
    if digits.len() % 2 != 0 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let key = (0..digits.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&digits[i..i + 2], 16).map_err(|_| bad()))
        .collect::<Result<Vec<u8>, _>>()?;
    if key.len() != ENC_KEY_BYTES {
        return Err(invalid("金鑰必須是 6 bytes（12 個十六進位數字）"));
    }
    Ok(key)
}

/// Set key `number` (1-10). An empty alias clears the slot (alias and key zeroed).
pub fn with_encryption_key(
    image: &[u8],
    number: usize,
    alias: &str,
    key: &[u8],
    codec: NameCodec,
) -> Result<Vec<u8>, ListEditError> {
    if !(1..=ENC_KEY_COUNT).contains(&number) {
        return Err(invalid(format!("金鑰編號必須介於 1 與 {ENC_KEY_COUNT}")));
    }
    if key.len() != ENC_KEY_BYTES {
        return Err(invalid("金鑰必須是 6 bytes"));
    }
    let record = if alias.trim().is_empty() {
        vec![0; ENC_KEY_SIZE]
    } else {
        [encode_name(alias.trim(), NAME_SIZE, codec)?, key.to_vec()].concat()
    };
    patch(
        image,
        ENC_KEYS_OFFSET + (number - 1) * ENC_KEY_SIZE,
        &record,
    )
}

// ---------------------------------------------------------------- GPS report channel
/// DMR service "GPS上報信道" as (zone, channel in zone); (0, 0) = Off.
pub fn gps_channel(image: &[u8]) -> (u16, u16) {
    if image.len() < GPS_CHANNEL_OFFSET + 2 {
        return (0, 0);
    }
    (
        u16_at(image, GPS_ZONE_OFFSET),
        u16_at(image, GPS_CHANNEL_OFFSET),
    )
}

pub fn with_gps_channel(image: &[u8], entry: (u16, u16)) -> Result<Vec<u8>, ListEditError> {
    let image = patch(image, GPS_ZONE_OFFSET, &entry.0.to_le_bytes())?;
    patch(&image, GPS_CHANNEL_OFFSET, &entry.1.to_le_bytes())
}
