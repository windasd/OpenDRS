//! Immutable views over the fixed-size records of a `.F136` project (port of `records.py`).
//!
//! Every record keeps its complete raw bytes; only fields whose meaning has been confirmed are
//! exposed, so editing a name or frequency never disturbs bytes this program does not understand.

use crate::pystr::{DECIMAL_EMAX, PyDecimal};
use crate::textcodec::{NameCodec, NameEncodingError, display_name, encode_name};

pub const NAME_SIZE: usize = 10;

/// A value outside what a record field can hold.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RecordError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Name(#[from] NameEncodingError),
}

fn invalid(message: impl Into<String>) -> RecordError {
    RecordError::Invalid(message.into())
}

fn u16_at(raw: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([raw[offset], raw[offset + 1]])
}

fn u32_at(raw: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        raw[offset],
        raw[offset + 1],
        raw[offset + 2],
        raw[offset + 3],
    ])
}

fn replaced<const N: usize>(raw: &[u8; N], offset: usize, value: &[u8]) -> [u8; N] {
    let mut out = *raw;
    out[offset..offset + value.len()].copy_from_slice(value);
    out
}

fn record<const N: usize>(kind: &str, bytes: &[u8]) -> Result<[u8; N], RecordError> {
    bytes.try_into().map_err(|_| {
        invalid(format!(
            "{kind} record must be {N} bytes, got {}",
            bytes.len()
        ))
    })
}

/// 16 bytes: index u16 | kind u8 | name[10] | call id u24 (little-endian).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Contact(pub [u8; 16]);

impl Contact {
    pub const SIZE: usize = 16;
    pub const NAME_OFFSET: usize = 3;
    pub const ID_OFFSET: usize = 13;
    pub const MAX_ID: u32 = 0xFF_FFFF;
    pub const GROUP_CALL: u8 = 4;
    pub const PRIVATE_CALL: u8 = 5;

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RecordError> {
        record("Contact", bytes).map(Contact)
    }

    pub fn build(
        index: u16,
        kind: u8,
        name_field: &[u8],
        call_id: u32,
    ) -> Result<Self, RecordError> {
        if name_field.len() != NAME_SIZE {
            return Err(invalid(format!("名稱欄必須是 {NAME_SIZE} bytes")));
        }
        check_call_id(call_id)?;
        let mut raw = [0u8; 16];
        raw[..2].copy_from_slice(&index.to_le_bytes());
        raw[2] = kind;
        raw[3..13].copy_from_slice(name_field);
        raw[13..].copy_from_slice(&call_id.to_le_bytes()[..3]);
        Ok(Contact(raw))
    }

    pub fn raw(&self) -> &[u8] {
        &self.0
    }
    pub fn index(&self) -> u16 {
        u16_at(&self.0, 0)
    }
    pub fn kind(&self) -> u8 {
        self.0[2]
    }
    pub fn kind_label(&self) -> String {
        match self.kind() {
            4 => "組呼".into(),
            5 => "個呼".into(),
            6 => "全呼".into(),
            7 => "無地址呼".into(),
            other => format!("類型 {other}"),
        }
    }
    pub fn call_id(&self) -> u32 {
        u32::from_le_bytes([self.0[13], self.0[14], self.0[15], 0])
    }
    pub fn name_field(&self) -> &[u8] {
        &self.0[Self::NAME_OFFSET..Self::NAME_OFFSET + NAME_SIZE]
    }
    pub fn name(&self, codec: NameCodec) -> String {
        display_name(self.name_field(), codec)
    }
    pub fn with_name(&self, text: &str, codec: NameCodec) -> Result<Self, RecordError> {
        let field = encode_name(text, NAME_SIZE, codec)?;
        Ok(Contact(replaced(&self.0, Self::NAME_OFFSET, &field)))
    }
    pub fn with_name_field(&self, field: &[u8]) -> Result<Self, RecordError> {
        if field.len() != NAME_SIZE {
            return Err(invalid(format!("名稱欄必須是 {NAME_SIZE} bytes")));
        }
        Ok(Contact(replaced(&self.0, Self::NAME_OFFSET, field)))
    }
    pub fn with_index(&self, index: u16) -> Self {
        Contact(replaced(&self.0, 0, &index.to_le_bytes()))
    }
    pub fn with_call_id(&self, call_id: u32) -> Result<Self, RecordError> {
        check_call_id(call_id)?;
        Ok(Contact(replaced(
            &self.0,
            Self::ID_OFFSET,
            &call_id.to_le_bytes()[..3],
        )))
    }
}

fn check_call_id(call_id: u32) -> Result<(), RecordError> {
    if (1..=Contact::MAX_ID).contains(&call_id) {
        Ok(())
    } else {
        Err(invalid(format!("ID 必須介於 1 與 {}", Contact::MAX_ID)))
    }
}

/// 32 bytes: index u16 | u8 | name[10] | first u16 | count u16 | remainder kept raw.
///
/// `first` numbers the zone's first channel among all 32-byte records of part 2 (zones first,
/// then channels, counting from 1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Zone(pub [u8; 32]);

impl Zone {
    pub const SIZE: usize = 32;
    pub const NAME_OFFSET: usize = 3;
    pub const FIRST_OFFSET: usize = 0x0D;
    pub const COUNT_OFFSET: usize = 0x0F;

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RecordError> {
        record("Zone", bytes).map(Zone)
    }
    pub fn raw(&self) -> &[u8] {
        &self.0
    }
    pub fn index(&self) -> u16 {
        u16_at(&self.0, 0)
    }
    pub fn name_field(&self) -> &[u8] {
        &self.0[Self::NAME_OFFSET..Self::NAME_OFFSET + NAME_SIZE]
    }
    pub fn name(&self, codec: NameCodec) -> String {
        display_name(self.name_field(), codec)
    }
    pub fn with_name(&self, text: &str, codec: NameCodec) -> Result<Self, RecordError> {
        let field = encode_name(text, NAME_SIZE, codec)?;
        Ok(Zone(replaced(&self.0, Self::NAME_OFFSET, &field)))
    }
    pub fn first_record(&self) -> u16 {
        u16_at(&self.0, Self::FIRST_OFFSET)
    }
    pub fn channel_count(&self) -> u16 {
        u16_at(&self.0, Self::COUNT_OFFSET)
    }
    pub fn with_index(&self, index: u16) -> Self {
        Zone(replaced(&self.0, 0, &index.to_le_bytes()))
    }
    pub fn with_layout(&self, first_record: u16, channel_count: u16) -> Self {
        let mut packed = [0u8; 4];
        packed[..2].copy_from_slice(&first_record.to_le_bytes());
        packed[2..].copy_from_slice(&channel_count.to_le_bytes());
        Zone(replaced(&self.0, Self::FIRST_OFFSET, &packed))
    }
}

/// 32 bytes: index u16 | name[10] | frequency A u32 (RX) | frequency B u32 (TX) | 12 bytes raw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Channel(pub [u8; 32]);

impl Channel {
    pub const SIZE: usize = 32;
    pub const NAME_OFFSET: usize = 2;
    pub const FREQ_A_OFFSET: usize = 0x0C;
    pub const FREQ_B_OFFSET: usize = 0x10;
    pub const FLAGS_OFFSET: usize = 0x14;
    pub const DIGITAL_OFFSET: usize = 0x15;
    pub const RX_GROUP_OFFSET: usize = 0x17;
    pub const TX_CONTACT_OFFSET: usize = 0x1E;
    pub const ANALOG: u8 = 0;

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RecordError> {
        record("Channel", bytes).map(Channel)
    }
    pub fn raw(&self) -> &[u8] {
        &self.0
    }
    pub fn index(&self) -> u16 {
        u16_at(&self.0, 0)
    }
    pub fn name_field(&self) -> &[u8] {
        &self.0[Self::NAME_OFFSET..Self::NAME_OFFSET + NAME_SIZE]
    }
    pub fn name(&self, codec: NameCodec) -> String {
        display_name(self.name_field(), codec)
    }
    pub fn with_name(&self, text: &str, codec: NameCodec) -> Result<Self, RecordError> {
        let field = encode_name(text, NAME_SIZE, codec)?;
        Ok(Channel(replaced(&self.0, Self::NAME_OFFSET, &field)))
    }
    /// Receive frequency in Hz (frequency A).
    pub fn rx_hz(&self) -> u32 {
        u32_at(&self.0, Self::FREQ_A_OFFSET)
    }
    /// Transmit frequency in Hz (frequency B).
    pub fn tx_hz(&self) -> u32 {
        u32_at(&self.0, Self::FREQ_B_OFFSET)
    }
    pub fn with_rx(&self, hz: u32) -> Self {
        Channel(replaced(&self.0, Self::FREQ_A_OFFSET, &hz.to_le_bytes()))
    }
    pub fn with_tx(&self, hz: u32) -> Self {
        Channel(replaced(&self.0, Self::FREQ_B_OFFSET, &hz.to_le_bytes()))
    }
    pub fn tx_contact(&self) -> u16 {
        u16_at(&self.0, Self::TX_CONTACT_OFFSET)
    }
    pub fn with_tx_contact(&self, contact_index: u16) -> Self {
        Channel(replaced(
            &self.0,
            Self::TX_CONTACT_OFFSET,
            &contact_index.to_le_bytes(),
        ))
    }
    pub fn mode(&self) -> u8 {
        self.0[Self::FLAGS_OFFSET] >> 6
    }
    /// Labels of the channel modes 0–3.
    pub const MODES: [&'static str; 4] = [
        "類比",
        "數位",
        "類比+數位（發射類比）",
        "類比+數位（發射數位）",
    ];

    pub fn mode_label(&self) -> &'static str {
        Self::MODES[usize::from(self.mode())]
    }
    pub fn with_mode(&self, mode: u8) -> Result<Self, RecordError> {
        if mode > 3 {
            return Err(invalid("頻道模式必須是 0–3"));
        }
        let flags = (self.0[Self::FLAGS_OFFSET] & 0x3F) | (mode << 6);
        Ok(Channel(replaced(&self.0, Self::FLAGS_OFFSET, &[flags])))
    }
    pub fn color_code(&self) -> u8 {
        self.0[Self::DIGITAL_OFFSET] & 0x0F
    }
    pub fn with_color_code(&self, cc: u8) -> Result<Self, RecordError> {
        if cc > 15 {
            return Err(invalid("色碼必須介於 0 與 15"));
        }
        let byte = (self.0[Self::DIGITAL_OFFSET] & 0xF0) | cc;
        Ok(Channel(replaced(&self.0, Self::DIGITAL_OFFSET, &[byte])))
    }
    pub fn rx_group(&self) -> u8 {
        self.0[Self::RX_GROUP_OFFSET]
    }
    pub fn with_rx_group(&self, number: u8) -> Result<Self, RecordError> {
        if number > 32 {
            return Err(invalid("接收群組編號必須介於 0 與 32"));
        }
        Ok(Channel(replaced(&self.0, Self::RX_GROUP_OFFSET, &[number])))
    }
    pub fn with_index(&self, index: u16) -> Self {
        Channel(replaced(&self.0, 0, &index.to_le_bytes()))
    }
}

/// Parse a MHz string such as `"438.8"` into integer Hz, exactly (no binary floating point).
///
/// Accepts whatever Python's `Decimal` accepts (full-width digits, underscores, exponents; see
/// [`PyDecimal::parse`]); a value beyond 64 bits is rejected like invalid text.
pub fn mhz_to_hz(text: &str) -> Result<i64, RecordError> {
    let bad = || invalid(format!("無效頻率「{text}」"));
    let Some(PyDecimal::Finite {
        negative,
        digits,
        exponent,
    }) = PyDecimal::parse(text)
    else {
        return Err(bad());
    };
    if digits.is_empty() {
        // Python scales the zero too: 0E+999999999999999999 × 10^6 is past Decimal's exponent limit.
        return if exponent.saturating_add(6) <= DECIMAL_EMAX {
            Ok(0)
        } else {
            Err(bad())
        };
    }
    // value = digits × 10^(exponent + 6) Hz; `digits` ends in a non-zero digit, so a negative
    // power of ten leaves a fraction of a hertz.
    let scale = exponent.saturating_add(6);
    if scale < 0 {
        return Err(invalid("頻率解析度不可小於 1 Hz"));
    }
    let magnitude = u32::try_from(scale)
        .ok()
        .and_then(|scale| 10i128.checked_pow(scale))
        .zip(digits.parse::<i128>().ok())
        .and_then(|(power, digits)| digits.checked_mul(power))
        .ok_or_else(bad)?;
    i64::try_from(if negative { -magnitude } else { magnitude }).map_err(|_| bad())
}

pub fn hz_to_mhz(hz: u32) -> String {
    format!("{:.5}", f64::from(hz) / 1_000_000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contact_fields_and_rebuild() {
        let field = encode_name("TG9", NAME_SIZE, NameCodec::Ascii).unwrap();
        let c = Contact::build(13, Contact::GROUP_CALL, &field, 9).unwrap();
        assert_eq!((c.index(), c.kind(), c.call_id()), (13, 4, 9));
        assert_eq!(c.name(NameCodec::Ascii), "TG9");
        assert_eq!(c.with_call_id(16_777_215).unwrap().call_id(), 16_777_215);
        assert!(c.with_call_id(0).is_err());
        assert_eq!(Contact::from_bytes(c.raw()).unwrap(), c);
        assert!(Contact::from_bytes(&[0; 15]).is_err());
    }

    #[test]
    fn channel_bits_are_independent() {
        let ch = Channel([0; 32])
            .with_mode(2)
            .unwrap()
            .with_color_code(7)
            .unwrap();
        assert_eq!((ch.mode(), ch.color_code()), (2, 7));
        assert_eq!(ch.with_rx(439_850_000).rx_hz(), 439_850_000);
        assert!(ch.with_mode(4).is_err());
    }

    #[test]
    fn mhz_parsing_is_exact() {
        assert_eq!(mhz_to_hz("439.85").unwrap(), 439_850_000);
        assert_eq!(mhz_to_hz("4.3885E2").unwrap(), 438_850_000);
        assert!(
            mhz_to_hz("430.0000005")
                .unwrap_err()
                .to_string()
                .contains("1 Hz")
        );
        assert!(mhz_to_hz("abc").is_err());
    }
}
