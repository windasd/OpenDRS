//! Scan lists in the settings image (port of `scanlist.py`).
//!
//! 16 fixed slots of 0x50 bytes from image offset 0x12B, each
//! `name[10] | 6 bytes of scan settings | 16 × (zone u16, channel u16)`, zone and channel counted
//! from 1 (channel = position inside the zone), (0, 0) marking an empty entry. Scan settings:
//! +0x0B bit 5 talkback, +0x0B bits 2-3 scan TX mode, +0x0C zone u16 and +0x0E channel u16 the
//! designated channel; +0x0A is left untouched.

use crate::records::NAME_SIZE;
use crate::textcodec::{NameCodec, NameEncodingError, display_name, encode_name};

pub const SCAN_LISTS_OFFSET: usize = 0x12B;
pub const SCAN_LIST_SIZE: usize = 0x50;
pub const SCAN_LIST_COUNT: usize = 16;
pub const SCAN_SETTINGS_OFFSET: usize = NAME_SIZE;
pub const SCAN_ENTRIES_OFFSET: usize = 0x10;
pub const SCAN_ENTRY_COUNT: usize = 16;
pub const SCAN_AREA_END: usize = SCAN_LISTS_OFFSET + SCAN_LIST_COUNT * SCAN_LIST_SIZE;
pub const SETTINGS_OFFSET: usize = 0x0B;
pub const TALKBACK_BIT: u8 = 0x20;
pub const TX_MODE_MASK: u8 = 0x0C;
pub const TX_MODES: [(&str, u8); 3] = [("當前信道", 0), ("最後活動信道", 1), ("指定信道", 2)];
pub const DESIGNATED_ZONE_OFFSET: usize = 0x0C;
pub const DESIGNATED_CHANNEL_OFFSET: usize = 0x0E;

/// `(zone number, channel number in zone)`, 1-based.
pub type Entry = (u16, u16);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ScanListError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Name(#[from] NameEncodingError),
}

fn invalid(message: impl Into<String>) -> ScanListError {
    ScanListError::Invalid(message.into())
}

fn u16_at(raw: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([raw[at], raw[at + 1]])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanList {
    /// 1..16
    pub number: usize,
    pub raw: Vec<u8>,
}

impl ScanList {
    pub fn name_field(&self) -> &[u8] {
        &self.raw[..NAME_SIZE]
    }
    pub fn name(&self, codec: NameCodec) -> String {
        display_name(self.name_field(), codec)
    }
    /// A slot the legacy CPS created: it has a name or at least one member.
    pub fn in_use(&self) -> bool {
        !matches!(self.raw[0], 0x00 | 0xFF) || !self.entries().is_empty()
    }
    pub fn talkback(&self) -> bool {
        self.raw[SETTINGS_OFFSET] & TALKBACK_BIT != 0
    }
    pub fn tx_mode(&self) -> u8 {
        (self.raw[SETTINGS_OFFSET] & TX_MODE_MASK) >> 2
    }
    /// `(zone, channel)`; values the legacy CPS shows as Off (0 or out of range) pass through.
    pub fn designated(&self) -> Entry {
        (
            u16_at(&self.raw, DESIGNATED_ZONE_OFFSET),
            u16_at(&self.raw, DESIGNATED_CHANNEL_OFFSET),
        )
    }
    pub fn entries(&self) -> Vec<Entry> {
        (0..SCAN_ENTRY_COUNT)
            .map(|i| {
                let at = SCAN_ENTRIES_OFFSET + 4 * i;
                (u16_at(&self.raw, at), u16_at(&self.raw, at + 2))
            })
            .filter(|(z, c)| !matches!(z, 0 | 0xFFFF) && !matches!(c, 0 | 0xFFFF))
            .collect()
    }
}

pub fn has_scan_lists(image: &[u8]) -> bool {
    image.len() >= SCAN_AREA_END
}

fn base(number: usize) -> usize {
    SCAN_LISTS_OFFSET + (number - 1) * SCAN_LIST_SIZE
}

pub fn scan_lists(image: &[u8]) -> Vec<ScanList> {
    if !has_scan_lists(image) {
        return Vec::new();
    }
    (1..=SCAN_LIST_COUNT)
        .map(|n| ScanList {
            number: n,
            raw: image[base(n)..base(n) + SCAN_LIST_SIZE].to_vec(),
        })
        .collect()
}

fn pack_entries(entries: &[Entry]) -> Result<Vec<u8>, ScanListError> {
    if entries.len() > SCAN_ENTRY_COUNT {
        return Err(invalid(format!(
            "每個掃描清單最多 {SCAN_ENTRY_COUNT} 個頻道"
        )));
    }
    if entries
        .iter()
        .any(|(z, c)| !(1..=0xFFFE).contains(z) || !(1..=0xFFFE).contains(c))
    {
        return Err(invalid("掃描清單項目的分區與頻道編號必須從 1 起算"));
    }
    let mut out: Vec<u8> = entries
        .iter()
        .flat_map(|(z, c)| [z.to_le_bytes(), c.to_le_bytes()].concat())
        .collect();
    out.resize(4 * SCAN_ENTRY_COUNT, 0);
    Ok(out)
}

/// Set one list's name and members; its 6 settings bytes are kept unchanged.
pub fn with_scan_list(
    image: &[u8],
    number: usize,
    name: &str,
    entries: &[Entry],
    codec: NameCodec,
) -> Result<Vec<u8>, ScanListError> {
    if !has_scan_lists(image) {
        return Err(invalid("設定區太短，沒有掃描清單區"));
    }
    if !(1..=SCAN_LIST_COUNT).contains(&number) {
        return Err(invalid(format!(
            "掃描清單編號必須介於 1 與 {SCAN_LIST_COUNT}"
        )));
    }
    let field = encode_name(name, NAME_SIZE, codec)?;
    let packed = pack_entries(entries)?;
    let at = base(number);
    let mut img = image.to_vec();
    img[at..at + NAME_SIZE].copy_from_slice(&field);
    img[at + SCAN_ENTRIES_OFFSET..at + SCAN_LIST_SIZE].copy_from_slice(&packed);
    Ok(img)
}

/// What a structural change did to the scan lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScanEdit {
    /// Entries whose channel no longer exists.
    pub removed: usize,
    /// Entries now pointing at a new (zone, channel) number.
    pub renumbered: usize,
    /// GPS / alarm channel references cleared or renumbered.
    pub refs_changed: usize,
}

/// Rewrite every list: `remap` returns the new entry, or `None` to drop it. Dropped entries close
/// up, as the firmware itself does.
pub fn remap_entries(
    image: &[u8],
    remap: impl Fn(Entry) -> Option<Entry>,
) -> Result<(Vec<u8>, ScanEdit), ScanListError> {
    if !has_scan_lists(image) {
        return Ok((image.to_vec(), ScanEdit::default()));
    }
    let mut img = image.to_vec();
    let mut edit = ScanEdit::default();
    for list in scan_lists(image) {
        let entries = list.entries();
        let mut kept = Vec::with_capacity(entries.len());
        for entry in &entries {
            match remap(*entry) {
                None => edit.removed += 1,
                Some(new) => {
                    edit.renumbered += usize::from(new != *entry);
                    kept.push(new);
                }
            }
        }
        if kept == entries {
            continue;
        }
        let at = base(list.number) + SCAN_ENTRIES_OFFSET;
        img[at..at + 4 * SCAN_ENTRY_COUNT].copy_from_slice(&pack_entries(&kept)?);
    }
    Ok((img, edit))
}

/// Entries naming this channel (or, with `channel` None, any channel of the zone).
pub fn references(image: &[u8], zone: u16, channel: Option<u16>) -> usize {
    scan_lists(image)
        .iter()
        .flat_map(ScanList::entries)
        .filter(|(z, c)| *z == zone && channel.is_none_or(|ch| *c == ch))
        .count()
}

/// Set one list's talkback, scan TX mode and designated channel ((0, 0) = Off).
pub fn with_scan_settings(
    image: &[u8],
    number: usize,
    talkback: bool,
    tx_mode: u8,
    designated: Entry,
) -> Result<Vec<u8>, ScanListError> {
    if !has_scan_lists(image) || !(1..=SCAN_LIST_COUNT).contains(&number) {
        return Err(invalid("掃描清單不存在"));
    }
    if !TX_MODES.iter().any(|(_, v)| *v == tx_mode) {
        return Err(invalid("掃描發射模式無效"));
    }
    let at = base(number);
    let mut img = image.to_vec();
    let flags = img[at + SETTINGS_OFFSET] & !(TALKBACK_BIT | TX_MODE_MASK);
    img[at + SETTINGS_OFFSET] = flags | if talkback { TALKBACK_BIT } else { 0 } | (tx_mode << 2);
    img[at + DESIGNATED_ZONE_OFFSET..at + DESIGNATED_ZONE_OFFSET + 2]
        .copy_from_slice(&designated.0.to_le_bytes());
    img[at + DESIGNATED_CHANNEL_OFFSET..at + DESIGNATED_CHANNEL_OFFSET + 2]
        .copy_from_slice(&designated.1.to_le_bytes());
    Ok(img)
}
