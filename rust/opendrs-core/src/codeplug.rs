//! Layout of the programming transfer ("codeplug stream"), port of `codeplug.py`.
//!
//! * The stream is a whole number of 2048-byte blocks in two parts.
//! * Part 1 is radio flash 0x3C0000 onwards: the 0x1400-byte settings area followed by the contact
//!   records (16 bytes each) at 0x1400.
//! * Part 2 is radio flash 0x400000 onwards: zone records then channel records (32 bytes each).
//! * The settings area holds the counts at 0x138F: zones, channels, contacts (u16 LE); the radio
//!   derives the block counts from them with exactly these formulas.

use crate::f136::Project;
use crate::records::{Channel, Contact, Zone};

pub const BLOCK: usize = 0x800;
pub const SETTINGS_SIZE: usize = 0x1400;
pub const COUNTS_OFFSET: usize = 0x138F;
pub const CONTACTS_OFFSET: usize = 0x1400;
pub const CONTACT_SIZE: usize = 16;
pub const RECORD_SIZE: usize = 32;
/// The radio treats a count >= 0x3E9 as corrupt (FUN_030272fc).
pub const MAX_CONTACTS: usize = 1000;
pub const DATE_OFFSET: usize = 0x60;
pub const DATE_SIZE: usize = 16;
/// When a write completes the radio sets its clock from the text at DATE_OFFSET; it parses year
/// first only (FUN_03027aec).
pub const DATE_FORMAT: &str = "%Y/%m/%d %H:%M";
pub const PART1_ADDR: u32 = 0x3C_0000;
pub const PART2_ADDR: u32 = 0x40_0000;
pub const IMAGE_ROW: usize = 100;

/// The project cannot be turned into a valid transfer, or a transfer is malformed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct CodeplugError(pub String);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counts {
    pub zones: usize,
    pub channels: usize,
    pub contacts: usize,
}

impl Counts {
    pub fn new(zones: usize, channels: usize, contacts: usize) -> Self {
        Counts {
            zones,
            channels,
            contacts,
        }
    }
    pub fn of(project: &Project) -> Self {
        Counts::new(
            project.zones.len(),
            project.channels.len(),
            project.contacts.len(),
        )
    }
    pub fn part1_blocks(&self) -> usize {
        (self.contacts * CONTACT_SIZE + 0x1BFF) >> 11
    }
    pub fn part2_blocks(&self) -> usize {
        ((self.zones + self.channels) * RECORD_SIZE + 0x7FF) >> 11
    }
    pub fn total_blocks(&self) -> usize {
        self.part1_blocks() + self.part2_blocks()
    }
    pub fn stream_size(&self) -> usize {
        self.total_blocks() * BLOCK
    }
}

/// Image length the legacy CPS uses when it saves a freshly read radio.
pub fn image_length_for(contacts: usize) -> usize {
    (CONTACTS_OFFSET + contacts * CONTACT_SIZE).div_ceil(IMAGE_ROW) * IMAGE_ROW + IMAGE_ROW
}

fn u16_clamped(value: usize) -> [u8; 2] {
    u16::try_from(value).unwrap_or(u16::MAX).to_le_bytes()
}

/// Bring the settings image in line with the record rows: the counts at 0x138F and the contact
/// records at 0x1400, growing the image if needed. Unchanged for a consistent image or one too
/// short to hold the counts.
pub fn sync_image(image: &[u8], contacts_raw: &[u8], zones: usize, channels: usize) -> Vec<u8> {
    if image.len() < COUNTS_OFFSET + 6 {
        return image.to_vec();
    }
    let contacts = contacts_raw.len() / CONTACT_SIZE;
    let end = CONTACTS_OFFSET + contacts_raw.len();
    let mut img = image.to_vec();
    if img.len() < end {
        img.resize(image_length_for(contacts), 0);
    }
    let counts = [
        u16_clamped(zones),
        u16_clamped(channels),
        u16_clamped(contacts),
    ]
    .concat();
    img[COUNTS_OFFSET..COUNTS_OFFSET + 6].copy_from_slice(&counts);
    img[CONTACTS_OFFSET..end].copy_from_slice(contacts_raw);
    img
}

pub fn counts_in_image(image: &[u8]) -> Option<Counts> {
    let raw = image.get(COUNTS_OFFSET..COUNTS_OFFSET + 6)?;
    let at = |i: usize| usize::from(u16::from_le_bytes([raw[i], raw[i + 1]]));
    Some(Counts::new(at(0), at(2), at(4)))
}

pub fn encode_date(text: &str) -> Result<[u8; DATE_SIZE], CodeplugError> {
    if !text.is_ascii() {
        return Err(CodeplugError("寫入時間只能包含 ASCII 字元".into()));
    }
    if text.len() > DATE_SIZE {
        return Err(CodeplugError(format!("寫入時間超過 {DATE_SIZE} 個字元")));
    }
    let mut out = [0u8; DATE_SIZE];
    out[..text.len()].copy_from_slice(text.as_bytes());
    Ok(out)
}

/// Reasons `project` must not be sent to a radio (empty when it is fine).
pub fn check_writable(project: &Project) -> Vec<String> {
    let mut reasons = Vec::new();
    if project.image.len() < SETTINGS_SIZE {
        reasons.push("專案沒有設定區（請先從對講機讀取，或開啟由寫頻軟體儲存的專案）".to_string());
    }
    if project.contacts.len() > MAX_CONTACTS {
        reasons.push(format!(
            "聯絡人 {} 筆，超過對講機上限 {MAX_CONTACTS} 筆",
            project.contacts.len()
        ));
    }
    if project.zones.is_empty() {
        reasons.push("專案沒有任何分區".to_string());
    }
    reasons.extend(project.problems());
    reasons
}

fn contacts_raw(project: &Project) -> Vec<u8> {
    project
        .contacts
        .iter()
        .flat_map(|c| c.raw().iter().copied())
        .collect()
}

/// Serialise `project` into the exact bytes the radio expects during a write.
pub fn build_stream(project: &Project, date_text: Option<&str>) -> Result<Vec<u8>, CodeplugError> {
    let reasons = check_writable(project);
    if !reasons.is_empty() {
        return Err(CodeplugError(reasons.join("\n")));
    }
    let counts = Counts::of(project);
    let image = sync_image(
        &project.image,
        &contacts_raw(project),
        counts.zones,
        counts.channels,
    );
    let mut part1 = vec![0u8; counts.part1_blocks() * BLOCK];
    if image.iter().skip(part1.len()).any(|b| *b != 0) {
        return Err(CodeplugError(
            "設定區在聯絡人之後還有資料，超出傳輸範圍".into(),
        ));
    }
    let n = image.len().min(part1.len());
    part1[..n].copy_from_slice(&image[..n]);
    if let Some(text) = date_text {
        part1[DATE_OFFSET..DATE_OFFSET + DATE_SIZE].copy_from_slice(&encode_date(text)?);
    }
    let mut part2 = vec![0u8; counts.part2_blocks() * BLOCK];
    let records: Vec<u8> = project
        .zones
        .iter()
        .map(Zone::raw)
        .chain(project.channels.iter().map(Channel::raw))
        .flatten()
        .copied()
        .collect();
    part2[..records.len()].copy_from_slice(&records);
    Ok([part1, part2].concat())
}

/// Turn a transfer read from the radio back into a project.
pub fn parse_stream(stream: &[u8], counts: Counts) -> Result<Project, CodeplugError> {
    if counts.contacts > MAX_CONTACTS {
        return Err(CodeplugError(format!(
            "對講機回報聯絡人 {} 筆，超過上限",
            counts.contacts
        )));
    }
    if stream.len() != counts.stream_size() {
        return Err(CodeplugError(format!(
            "收到 {} bytes，預期 {} bytes",
            stream.len(),
            counts.stream_size()
        )));
    }
    let (part1, part2) = stream.split_at(counts.part1_blocks() * BLOCK);
    if counts_in_image(part1) != Some(counts) {
        return Err(CodeplugError("設定區內的計數與對講機回報不一致".into()));
    }
    let bad = |e: crate::records::RecordError| CodeplugError(e.to_string());
    let contacts = (0..counts.contacts)
        .map(|i| Contact::from_bytes(&part1[CONTACTS_OFFSET + i * CONTACT_SIZE..][..CONTACT_SIZE]))
        .collect::<Result<Vec<_>, _>>()
        .map_err(bad)?;
    let record = |i: usize| &part2[i * RECORD_SIZE..(i + 1) * RECORD_SIZE];
    let zones = (0..counts.zones)
        .map(|i| Zone::from_bytes(record(i)))
        .collect::<Result<Vec<_>, _>>();
    let channels = (0..counts.channels)
        .map(|i| Channel::from_bytes(record(counts.zones + i)))
        .collect::<Result<Vec<_>, _>>();
    let image_len = image_length_for(counts.contacts);
    let mut image = part1[..image_len.min(part1.len())].to_vec();
    image.resize(image_len, 0);
    Ok(Project {
        contacts,
        zones: zones.map_err(bad)?,
        channels: channels.map_err(bad)?,
        image,
        section_order: vec!["Contacts".into(), "Zone".into(), "CH".into()],
        unknown_rows: Vec::new(),
    })
}

pub fn with_synced_image(project: &Project) -> Project {
    let image = sync_image(
        &project.image,
        &contacts_raw(project),
        project.zones.len(),
        project.channels.len(),
    );
    Project {
        image,
        ..project.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_formulas() {
        let counts = Counts::new(4, 27, 360);
        assert_eq!((counts.part1_blocks(), counts.part2_blocks()), (6, 1));
        assert_eq!(counts.stream_size(), 7 * BLOCK);
        assert_eq!(image_length_for(360), 11_000);
    }

    #[test]
    fn dates_must_fit() {
        assert_eq!(&encode_date("2026/10/09 10:33").unwrap()[..4], b"2026");
        assert!(encode_date("2026/10/09 10:33:00").is_err());
        assert!(encode_date("２０２６").is_err());
    }
}
