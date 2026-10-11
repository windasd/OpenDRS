//! Device information held in the settings image, and frequency-band rules (port of `device.py`).

use std::sync::LazyLock;

use regex::Regex;

use crate::pystr::{PyInt, strip};

pub const BAND_LABEL_OFFSET: usize = 0x50;
pub const DATE_OFFSET: usize = 0x60;
pub const MODEL_OFFSET: usize = 0x70;
pub const ALIAS_OFFSET: usize = 0x80;
pub const FIELD_SIZE: usize = 16;
/// The radio's own DMR ID: u24 little-endian right after the alias.
pub const DMR_ID_OFFSET: usize = 0x90;
pub const DMR_ID_SIZE: usize = 3;
/// Firmware ID and build date, NUL-terminated; it runs past 16 bytes into the next slot.
pub const FIRMWARE_OFFSET: usize = 0x30;
pub const FIRMWARE_SIZE: usize = 0x20;
pub const MAX_DMR_ID: u32 = 16_776_415;
/// The alias field is 16 bytes and the DMR ID follows; keep one NUL so the radio finds the end.
pub const MAX_ALIAS_LEN: usize = FIELD_SIZE - 1;

const MHZ: u32 = 1_000_000;

/// An inclusive frequency range in Hz.
pub type Band = (u32, u32);

/// Factory band variants of this product family ("136-480" is dual band VHF + UHF), plus the
/// label the band-widened test firmware reports.
pub fn bands_for(label: &str) -> Option<Vec<Band>> {
    let m = |lo: u32, hi: u32| (lo * MHZ, hi * MHZ);
    let bands = match label {
        "136-480" => vec![m(136, 174), m(400, 480)],
        // Not a factory variant: the band-widened test firmware (mkpatch_band_record.py) reports it.
        "136-520" => vec![m(136, 180), m(400, 520)],
        "136-174" => vec![m(136, 174)],
        "140-174" => vec![m(140, 174)],
        "150-170" => vec![m(150, 170)],
        "240-250" => vec![m(240, 250)],
        "350-400" => vec![m(350, 400)],
        "400-470" => vec![m(400, 470)],
        "400-480" => vec![m(400, 480)],
        "410-440" => vec![m(410, 440)],
        "430-450" => vec![m(430, 450)],
        "430-480" => vec![m(430, 480)],
        "450-520" => vec![m(450, 520)],
        "30-32" => vec![m(30, 32)],
        "66-68" => vec![m(66, 68)],
        "66-88" => vec![m(66, 88)],
        _ => return None,
    };
    Some(bands)
}

static BUILD_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?P<version>.+?)\.(?P<date>[A-Z][a-z]{2} +\d{1,2} +\d{4})$").expect("valid regex")
});

/// Printable ASCII up to the first 00/FF, other bytes shown as `?`, trimmed.
pub(crate) fn ascii_field(image: &[u8], offset: usize, size: usize) -> String {
    let end = (offset + size).min(image.len());
    let field = if offset < image.len() {
        &image[offset..end]
    } else {
        &[][..]
    };
    let text: String = field
        .iter()
        .take_while(|b| **b != 0x00 && **b != 0xFF)
        .map(|&b| {
            if (0x20..0x7F).contains(&b) {
                b as char
            } else {
                '?'
            }
        })
        .collect();
    text.trim().to_string()
}

fn dmr_id(image: &[u8]) -> Option<u32> {
    let field = image.get(DMR_ID_OFFSET..DMR_ID_OFFSET + DMR_ID_SIZE)?;
    let value = u32::from_le_bytes([field[0], field[1], field[2], 0]);
    (value != 0 && value != 0xFF_FFFF).then_some(value)
}

/// Older projects hold binary junk in the identity fields; show nothing rather than `?`s.
fn clean_id(text: String) -> String {
    if text.contains('?') {
        String::new()
    } else {
        text
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    pub band_label: String,
    pub write_date: String,
    pub model: String,
    pub alias: String,
    pub dmr_id: Option<u32>,
    pub firmware: String,
    pub firmware_date: String,
    pub factory_number: String,
    pub serial_number: String,
    pub model_number: String,
}

impl DeviceInfo {
    pub fn from_image(image: &[u8]) -> Self {
        let firmware_text = ascii_field(image, FIRMWARE_OFFSET, FIRMWARE_SIZE);
        let (firmware, firmware_date) = match BUILD_DATE.captures(&firmware_text) {
            Some(c) => (
                c["version"].to_string(),
                c["date"].split_whitespace().collect::<Vec<_>>().join(" "),
            ),
            None => (firmware_text.clone(), String::new()),
        };
        DeviceInfo {
            band_label: ascii_field(image, BAND_LABEL_OFFSET, FIELD_SIZE),
            write_date: ascii_field(image, DATE_OFFSET, FIELD_SIZE),
            model: ascii_field(image, MODEL_OFFSET, FIELD_SIZE),
            alias: ascii_field(image, ALIAS_OFFSET, FIELD_SIZE),
            dmr_id: dmr_id(image),
            firmware,
            firmware_date,
            factory_number: clean_id(ascii_field(image, 0x00, FIELD_SIZE)),
            serial_number: clean_id(ascii_field(image, 0x10, FIELD_SIZE)),
            model_number: clean_id(ascii_field(image, 0x20, FIELD_SIZE)),
        }
    }

    pub fn bands(&self) -> Option<Vec<Band>> {
        bands_for(self.band_label.trim_start_matches('F'))
    }
}

/// A reason the frequency is unusable on this device, or `None`.
pub fn frequency_problem(hz: u32, bands: Option<&[Band]>) -> Option<String> {
    frequency_problem_hz(i64::from(hz), bands)
}

/// [`frequency_problem`] for any integer (a typed-in value can be negative or past 32 bits).
pub fn frequency_problem_hz(hz: i64, bands: Option<&[Band]>) -> Option<String> {
    let bands = bands?;
    if hz == 0
        || bands
            .iter()
            .any(|(lo, hi)| (i64::from(*lo)..=i64::from(*hi)).contains(&hz))
    {
        return None;
    }
    let ranges = bands
        .iter()
        .map(|(lo, hi)| format!("{}–{}", lo / MHZ, hi / MHZ))
        .collect::<Vec<_>>();
    Some(format!(
        "{:.5} MHz 超出本機頻段（{} MHz）",
        hz as f64 / f64::from(MHZ),
        ranges.join("、")
    ))
}

/// A device field that cannot take the given value.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DeviceError(pub String);

fn patch(image: &[u8], offset: usize, value: &[u8]) -> Result<Vec<u8>, DeviceError> {
    if image.len() < offset + value.len() {
        return Err(DeviceError(
            "設定區太短，無法寫入此欄位（請先開啟專案或從對講機讀取）".into(),
        ));
    }
    let mut out = image.to_vec();
    out[offset..offset + value.len()].copy_from_slice(value);
    Ok(out)
}

/// Store the radio's own DMR ID; the byte after it (another setting) is untouched.
pub fn with_dmr_id(image: &[u8], dmr_id: u32) -> Result<Vec<u8>, DeviceError> {
    if !(1..=MAX_DMR_ID).contains(&dmr_id) {
        return Err(DeviceError(format!("DMR ID 必須介於 1 與 {MAX_DMR_ID}")));
    }
    patch(image, DMR_ID_OFFSET, &dmr_id.to_le_bytes()[..DMR_ID_SIZE])
}

pub fn with_alias(image: &[u8], alias: &str) -> Result<Vec<u8>, DeviceError> {
    let text = alias.trim();
    if text.chars().count() > MAX_ALIAS_LEN {
        return Err(DeviceError(format!("別名最多 {MAX_ALIAS_LEN} 個字元")));
    }
    if text.chars().any(|ch| !(' '..'\x7f').contains(&ch)) {
        return Err(DeviceError("別名只能使用英數字與一般符號（ASCII）".into()));
    }
    let mut field = text.as_bytes().to_vec();
    field.resize(FIELD_SIZE, 0);
    patch(image, ALIAS_OFFSET, &field)
}

/// `int(text.strip())` within 1–[`MAX_DMR_ID`] (full-width digits and `9_998_765` are accepted, as
/// in Python).
pub fn parse_dmr_id(text: &str) -> Result<u32, DeviceError> {
    let trimmed = strip(text);
    let value = PyInt::parse(trimmed)
        .ok_or_else(|| DeviceError(format!("「{trimmed}」不是有效的 DMR ID（只能是數字）")))?;
    value
        .to_i64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| (1..=MAX_DMR_ID).contains(v))
        .ok_or_else(|| DeviceError(format!("DMR ID 必須介於 1 與 {MAX_DMR_ID}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_and_problems() {
        let bands = bands_for("136-480").unwrap();
        assert_eq!(frequency_problem(439_850_000, Some(&bands)), None);
        assert_eq!(frequency_problem(0, Some(&bands)), None);
        assert_eq!(
            frequency_problem(900_000_000, Some(&bands)).unwrap(),
            "900.00000 MHz 超出本機頻段（136–174、400–480 MHz）"
        );
        assert_eq!(frequency_problem(900_000_000, None), None);
    }

    #[test]
    fn band_widened_test_firmware_label() {
        let bands = bands_for("136-520").unwrap();
        assert_eq!(bands, vec![(136 * MHZ, 180 * MHZ), (400 * MHZ, 520 * MHZ)]);
        for ok in [136, 178, 180, 400, 500, 520] {
            assert_eq!(frequency_problem(ok * MHZ, Some(&bands)), None, "{ok}");
        }
        for bad in [135, 181, 250, 399, 521] {
            assert!(
                frequency_problem(bad * MHZ, Some(&bands)).is_some(),
                "{bad}"
            );
        }
    }

    #[test]
    fn alias_and_id_validation() {
        let image = vec![0u8; 0x100];
        let with = with_alias(&image, " QT0EST ").unwrap();
        assert_eq!(DeviceInfo::from_image(&with).alias, "QT0EST");
        assert!(with_alias(&image, "測試").is_err());
        assert!(with_alias(&image, "1234567890123456").is_err());
        assert_eq!(
            DeviceInfo::from_image(&with_dmr_id(&image, 9_998_765).unwrap()).dmr_id,
            Some(9_998_765)
        );
        assert!(with_dmr_id(&image[..0x91], 1).is_err());
        assert_eq!(parse_dmr_id(" 9998765 ").unwrap(), 9_998_765);
        assert!(parse_dmr_id("abc").is_err() && parse_dmr_id("0").is_err());
        assert_eq!(parse_dmr_id("９９９８７６５").unwrap(), 9_998_765);
        assert_eq!(parse_dmr_id("9_998_765").unwrap(), 9_998_765);
        let huge = parse_dmr_id(&"9".repeat(23)).unwrap_err().0;
        assert!(huge.starts_with("DMR ID 必須介於"), "{huge}");
    }
}
