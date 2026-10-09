//! RX group lists ("GCL") in the settings image (port of `rxgroup.py`).
//!
//! The firmware (FUN_03031284) loads list `n` (1..32) from image offset 0x62B + (n-1)*0x4C: a
//! 10-byte name, then u16 contact numbers (33 slots, 0 = empty; each a `Contact::index`). A
//! channel selects its list with byte +0x17 (0 = none).

use crate::records::NAME_SIZE;
use crate::textcodec::{NameCodec, NameEncodingError, display_name, encode_name};

pub const RX_GROUPS_OFFSET: usize = 0x62B;
pub const RX_GROUP_SIZE: usize = 0x4C;
pub const RX_GROUP_COUNT: usize = 32;
pub const RX_MEMBERS_OFFSET: usize = NAME_SIZE;
pub const RX_MEMBER_SLOTS: usize = (RX_GROUP_SIZE - RX_MEMBERS_OFFSET) / 2;
/// One slot always stays 0 so the list remains terminated.
pub const MAX_RX_MEMBERS: usize = 32;
pub const RX_AREA_END: usize = RX_GROUPS_OFFSET + RX_GROUP_COUNT * RX_GROUP_SIZE;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RxGroupError {
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Name(#[from] NameEncodingError),
}

fn invalid(message: impl Into<String>) -> RxGroupError {
    RxGroupError::Invalid(message.into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RxGroup {
    /// 1..32
    pub number: usize,
    pub raw: Vec<u8>,
}

impl RxGroup {
    pub fn name(&self, codec: NameCodec) -> String {
        display_name(&self.raw[..NAME_SIZE], codec)
    }

    /// Contact numbers, in stored order (empty slots skipped).
    pub fn members(&self) -> Vec<u16> {
        self.raw[RX_MEMBERS_OFFSET..RX_MEMBERS_OFFSET + 2 * RX_MEMBER_SLOTS]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .filter(|v| *v != 0 && *v != 0xFFFF)
            .collect()
    }

    pub fn in_use(&self) -> bool {
        !matches!(self.raw[0], 0x00 | 0xFF) || !self.members().is_empty()
    }
}

pub fn has_rx_groups(image: &[u8]) -> bool {
    image.len() >= RX_AREA_END
}

fn base(number: usize) -> usize {
    RX_GROUPS_OFFSET + (number - 1) * RX_GROUP_SIZE
}

pub fn rx_groups(image: &[u8]) -> Vec<RxGroup> {
    if !has_rx_groups(image) {
        return Vec::new();
    }
    (1..=RX_GROUP_COUNT)
        .map(|n| RxGroup {
            number: n,
            raw: image[base(n)..base(n) + RX_GROUP_SIZE].to_vec(),
        })
        .collect()
}

fn pack_members(members: &[u16]) -> Result<Vec<u8>, RxGroupError> {
    if members.len() > MAX_RX_MEMBERS {
        return Err(invalid(format!(
            "每個接收群組最多 {MAX_RX_MEMBERS} 個聯絡人"
        )));
    }
    if members.iter().any(|m| !(1..=0xFFFE).contains(m)) {
        return Err(invalid("聯絡人編號必須從 1 起算"));
    }
    let mut out: Vec<u8> = members.iter().flat_map(|m| m.to_le_bytes()).collect();
    out.resize(2 * RX_MEMBER_SLOTS, 0);
    Ok(out)
}

pub fn with_rx_group(
    image: &[u8],
    number: usize,
    name: &str,
    members: &[u16],
    codec: NameCodec,
) -> Result<Vec<u8>, RxGroupError> {
    if !has_rx_groups(image) {
        return Err(invalid("設定區太短，沒有接收群組區"));
    }
    if !(1..=RX_GROUP_COUNT).contains(&number) {
        return Err(invalid(format!(
            "接收群組編號必須介於 1 與 {RX_GROUP_COUNT}"
        )));
    }
    let field = encode_name(name, NAME_SIZE, codec)?;
    let packed = pack_members(members)?;
    let at = base(number);
    let mut img = image.to_vec();
    img[at..at + NAME_SIZE].copy_from_slice(&field);
    img[at + RX_MEMBERS_OFFSET..at + RX_GROUP_SIZE].copy_from_slice(&packed);
    Ok(img)
}

/// Rewrite every list's members (`None` drops one); returns `(image, members dropped)`.
pub fn remap_members(
    image: &[u8],
    remap: impl Fn(u16) -> Option<u16>,
) -> Result<(Vec<u8>, usize), RxGroupError> {
    if !has_rx_groups(image) {
        return Ok((image.to_vec(), 0));
    }
    let mut img = image.to_vec();
    let mut dropped = 0;
    for group in rx_groups(image) {
        let members = group.members();
        let kept: Vec<u16> = members.iter().filter_map(|m| remap(*m)).collect();
        dropped += members.len() - kept.len();
        if kept == members {
            continue;
        }
        let at = base(group.number) + RX_MEMBERS_OFFSET;
        img[at..at + RX_GROUP_SIZE - RX_MEMBERS_OFFSET].copy_from_slice(&pack_members(&kept)?);
    }
    Ok((img, dropped))
}

pub fn groups_containing(image: &[u8], contact_number: u16) -> Vec<usize> {
    rx_groups(image)
        .into_iter()
        .filter(|g| g.members().contains(&contact_number))
        .map(|g| g.number)
        .collect()
}
