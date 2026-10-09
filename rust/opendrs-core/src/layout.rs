//! Zones and channels: which zone owns each channel, and structural edits that keep every
//! reference valid (port of `layout.py`).
//!
//! Part 2 holds the zones followed by the channels. A zone names its channels by `first_record`
//! (1-based record number in part 2) and `channel_count`, so a zone's channels are contiguous and
//! the first channel record is number `zone_count + 1`. Scan lists, the GPS report channel, the
//! alarm channels and each scan list's designated channel name channels as (zone, channel in
//! zone); every edit that shifts numbers rewrites them:
//!
//! * add a channel at a zone's end, add a zone at the end: no number changes;
//! * delete a channel: its references go, higher channel numbers in that zone drop by one;
//! * delete a zone: its references go, higher zone numbers drop by one.

use std::ops::Range;

use crate::alarm_enc::{ALARM_COUNT, alarm_base};
use crate::codeplug::with_synced_image;
use crate::f136::Project;
use crate::records::{Channel, NAME_SIZE, Zone};
use crate::scanlist::{
    Entry, SCAN_LIST_COUNT, SCAN_LIST_SIZE, SCAN_LISTS_OFFSET, ScanEdit, remap_entries,
};
use crate::textcodec::{NameCodec, fit_name};

pub const MAX_BATCH: usize = 64;

/// The zone table does not describe the channel list the way the radio expects, or an edit is
/// not possible.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct LayoutError(pub String);

fn error(message: impl Into<String>) -> LayoutError {
    LayoutError(message.into())
}

/// Other single (zone u16, channel u16) references in the settings image: the GPS report
/// channel, each alarm list's alarm channel, each scan list's designated channel.
fn channel_refs() -> Vec<(usize, usize)> {
    let mut refs = vec![(0x1354, 0x1356)];
    refs.extend((1..=ALARM_COUNT).map(|n| (alarm_base(n) + 0x0C, alarm_base(n) + 0x0E)));
    refs.extend((0..SCAN_LIST_COUNT).map(|n| {
        let at = SCAN_LISTS_OFFSET + n * SCAN_LIST_SIZE;
        (at + 0x0C, at + 0x0E)
    }));
    refs
}

fn u16_at(img: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([img[at], img[at + 1]])
}

fn remap_refs(
    image: &[u8],
    remap: &impl Fn(Entry) -> Option<Entry>,
    zone_count: usize,
) -> (Vec<u8>, usize) {
    let mut img = image.to_vec();
    let mut changed = 0;
    for (zone_at, channel_at) in channel_refs() {
        if img.len() < zone_at.max(channel_at) + 2 {
            continue;
        }
        let entry = (u16_at(&img, zone_at), u16_at(&img, channel_at));
        if entry == (0, 0) || !(1..=zone_count).contains(&usize::from(entry.0)) {
            continue; // Off, or a stale value the legacy CPS shows as Off: leave it alone
        }
        let new = remap(entry).unwrap_or((0, 0));
        if new != entry {
            changed += 1;
            img[zone_at..zone_at + 2].copy_from_slice(&new.0.to_le_bytes());
            img[channel_at..channel_at + 2].copy_from_slice(&new.1.to_le_bytes());
        }
    }
    (img, changed)
}

fn remap_all(
    image: &[u8],
    remap: impl Fn(Entry) -> Option<Entry>,
    zone_count: usize,
) -> Result<(Vec<u8>, ScanEdit), LayoutError> {
    let (image, edit) = remap_entries(image, &remap).map_err(|e| error(e.to_string()))?;
    let (image, refs) = remap_refs(&image, &remap, zone_count);
    Ok((
        image,
        ScanEdit {
            refs_changed: refs,
            ..edit
        },
    ))
}

/// Zone row (0-based) for every channel row; `None` where no zone covers it.
pub fn zone_rows(project: &Project) -> Vec<Option<usize>> {
    let mut owner = vec![None; project.channels.len()];
    let first_channel = project.zones.len() as i64 + 1;
    for (z, zone) in project.zones.iter().enumerate() {
        let start = i64::from(zone.first_record()) - first_channel;
        let end = (start + i64::from(zone.channel_count())).min(owner.len() as i64);
        for row in start.max(0)..end.max(0) {
            owner[row as usize] = Some(z);
        }
    }
    owner
}

/// Fails unless the zones tile the channel list contiguously, in order.
pub fn check_layout(project: &Project) -> Result<(), LayoutError> {
    let mut expected = project.zones.len() + 1;
    for (z, zone) in project.zones.iter().enumerate().map(|(i, z)| (i + 1, z)) {
        if usize::from(zone.first_record()) != expected {
            return Err(error(format!(
                "分區 {z} 的起始位置 {} 與預期 {expected} 不符",
                zone.first_record()
            )));
        }
        expected += usize::from(zone.channel_count());
    }
    if expected - project.zones.len() - 1 != project.channels.len() {
        return Err(error("分區內的頻道數合計與頻道清單筆數不符"));
    }
    Ok(())
}

/// `(zone row, position in zone)`, both 0-based.
pub fn channel_position(project: &Project, row: usize) -> Result<(usize, usize), LayoutError> {
    check_layout(project)?;
    if row >= project.channels.len() {
        return Err(error("請先選擇頻道"));
    }
    let zone_row = zone_rows(project)[row].ok_or_else(|| error("這個頻道不屬於任何分區"))?;
    Ok((zone_row, row - zone_channel_rows(project, zone_row).start))
}

/// Channel rows of zone `zone_row` (meaningful once `check_layout` passed).
///
/// Empty for a zone that does not exist, and never past the end of the channel list (a project
/// whose layout [`check_layout`] rejects may claim more).
pub fn zone_channel_rows(project: &Project, zone_row: usize) -> Range<usize> {
    let Some(zone) = project.zones.get(zone_row) else {
        return 0..0;
    };
    let total = project.channels.len();
    let start = usize::from(zone.first_record())
        .saturating_sub(project.zones.len() + 1)
        .min(total);
    start..(start + usize::from(zone.channel_count())).min(total)
}

fn as_u16(value: usize) -> Result<u16, LayoutError> {
    u16::try_from(value).map_err(|_| error("數量超出 16-bit 範圍"))
}

/// Renumber zones 1..n and lay their channels out back to back.
fn relayout(zones: &[Zone], counts: &[usize]) -> Result<Vec<Zone>, LayoutError> {
    let mut first = zones.len() + 1;
    let mut out = Vec::with_capacity(zones.len());
    for (i, (zone, count)) in zones.iter().zip(counts).enumerate() {
        out.push(
            zone.with_index(as_u16(i + 1)?)
                .with_layout(as_u16(first)?, as_u16(*count)?),
        );
        first += count;
    }
    Ok(out)
}

fn finish(
    project: &Project,
    zones: Vec<Zone>,
    channels: Vec<Channel>,
    image: Vec<u8>,
) -> Result<Project, LayoutError> {
    let updated = Project {
        zones,
        channels,
        image,
        ..project.clone()
    };
    check_layout(&updated)?;
    Ok(with_synced_image(&updated))
}

fn counts(project: &Project) -> Vec<usize> {
    project
        .zones
        .iter()
        .map(|z| usize::from(z.channel_count()))
        .collect()
}

/// Copy channel `source_row` to the end of the same zone; returns `(project, new row)`.
pub fn add_channel(project: &Project, source_row: usize) -> Result<(Project, usize), LayoutError> {
    let (zone_row, _) = channel_position(project, source_row)?;
    let rows = zone_channel_rows(project, zone_row);
    let mut channels = project.channels.clone();
    channels.insert(
        rows.end,
        project.channels[source_row].with_index(as_u16(rows.len() + 1)?),
    );
    let mut counts = counts(project);
    counts[zone_row] += 1;
    let zones = relayout(&project.zones, &counts)?;
    Ok((
        finish(project, zones, channels, project.image.clone())?,
        rows.end,
    ))
}

/// Remove channel `row`: renumber the rest of its zone, move later zones up and fix every
/// reference. A zone's last remaining channel cannot be deleted.
pub fn delete_channel(project: &Project, row: usize) -> Result<(Project, ScanEdit), LayoutError> {
    let (zone_row, position) = channel_position(project, row)?;
    let rows = zone_channel_rows(project, zone_row);
    if rows.len() <= 1 {
        return Err(error(format!(
            "分區 {} 只剩這個頻道，不能刪除（分區至少要有一個頻道；要整個移除請用「刪除分區」）",
            zone_row + 1
        )));
    }
    let mut channels = project.channels.clone();
    channels.remove(row);
    for ch in &mut channels[row..rows.end - 1] {
        let index = ch
            .index()
            .checked_sub(1)
            .ok_or_else(|| error("頻道編號超出範圍"))?;
        *ch = ch.with_index(index);
    }
    let mut counts = counts(project);
    counts[zone_row] -= 1;
    let (zone_no, channel_no) = (as_u16(zone_row + 1)?, as_u16(position + 1)?);
    let remap = |(zone, channel): Entry| {
        if zone != zone_no || channel < channel_no {
            Some((zone, channel))
        } else if channel == channel_no {
            None
        } else {
            Some((zone, channel - 1))
        }
    };
    let (image, edit) = remap_all(&project.image, remap, project.zones.len())?;
    let zones = relayout(&project.zones, &counts)?;
    Ok((finish(project, zones, channels, image)?, edit))
}

/// Append a zone named `name` holding a copy of channel `source_row`; returns
/// `(project, new zone row)`. No existing number changes, so every reference stays valid.
pub fn add_zone(
    project: &Project,
    source_row: usize,
    name: &str,
    codec: NameCodec,
) -> Result<(Project, usize), LayoutError> {
    check_layout(project)?;
    if source_row >= project.channels.len() {
        return Err(error("請先選擇要放進新分區的頻道"));
    }
    let template = project.zones.last().copied().unwrap_or(Zone([0; 32]));
    let mut zones = project.zones.clone();
    zones.push(
        template
            .with_name(name, codec)
            .map_err(|e| error(e.to_string()))?,
    );
    let mut channels = project.channels.clone();
    channels.push(project.channels[source_row].with_index(1));
    let mut counts = counts(project);
    counts.push(1);
    let new_row = zones.len() - 1;
    let zones = relayout(&zones, &counts)?;
    Ok((
        finish(project, zones, channels, project.image.clone())?,
        new_row,
    ))
}

/// Remove a zone and all its channels; later zones move up and every reference follows.
pub fn delete_zone(project: &Project, zone_row: usize) -> Result<(Project, ScanEdit), LayoutError> {
    check_layout(project)?;
    if zone_row >= project.zones.len() {
        return Err(error("請先選擇分區"));
    }
    if project.zones.len() <= 1 {
        return Err(error("至少要保留一個分區"));
    }
    let rows = zone_channel_rows(project, zone_row);
    let channels: Vec<Channel> = project
        .channels
        .iter()
        .enumerate()
        .filter(|(r, _)| !rows.contains(r))
        .map(|(_, c)| *c)
        .collect();
    let zones: Vec<Zone> = project
        .zones
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != zone_row)
        .map(|(_, z)| *z)
        .collect();
    let zone_no = as_u16(zone_row + 1)?;
    let remap = |(zone, channel): Entry| {
        if zone < zone_no {
            Some((zone, channel))
        } else if zone == zone_no {
            None
        } else {
            Some((zone - 1, channel))
        }
    };
    let (image, edit) = remap_all(&project.image, remap, project.zones.len())?;
    let zone_counts: Vec<usize> = zones
        .iter()
        .map(|z| usize::from(z.channel_count()))
        .collect();
    let relaid = relayout(&zones, &zone_counts)?;
    Ok((finish(project, relaid, channels, image)?, edit))
}

/// Move channel `row` to the end of zone `target_zone_row`; returns `(project, new row, edit)`.
/// The rest of the source zone closes up and every reference follows.
pub fn move_channel(
    project: &Project,
    row: usize,
    target_zone_row: usize,
) -> Result<(Project, usize, ScanEdit), LayoutError> {
    let (zone_row, position) = channel_position(project, row)?;
    if target_zone_row >= project.zones.len() {
        return Err(error("目標分區不存在"));
    }
    if target_zone_row == zone_row {
        return Ok((project.clone(), row, ScanEdit::default()));
    }
    if project.zones[zone_row].channel_count() <= 1 {
        return Err(error(format!(
            "分區 {} 只剩這個頻道，不能移走（分區至少要有一個頻道）",
            zone_row + 1
        )));
    }
    let mut groups: Vec<Vec<Channel>> = (0..project.zones.len())
        .map(|z| {
            zone_channel_rows(project, z)
                .map(|r| project.channels[r])
                .collect()
        })
        .collect();
    let moved = groups[zone_row].remove(position);
    groups[target_zone_row].push(moved);
    let new_number = groups[target_zone_row].len();
    let channels = groups
        .iter()
        .flat_map(|g| {
            g.iter()
                .enumerate()
                .map(|(i, ch)| as_u16(i + 1).map(|n| ch.with_index(n)))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let (src_no, dst_no, old_no, new_no) = (
        as_u16(zone_row + 1)?,
        as_u16(target_zone_row + 1)?,
        as_u16(position + 1)?,
        as_u16(new_number)?,
    );
    let remap = |(zone, channel): Entry| {
        if zone != src_no || channel < old_no {
            Some((zone, channel))
        } else if channel == old_no {
            Some((dst_no, new_no))
        } else {
            Some((zone, channel - 1))
        }
    };
    let (image, edit) = remap_all(&project.image, remap, project.zones.len())?;
    let zones = relayout(
        &project.zones,
        &groups.iter().map(Vec::len).collect::<Vec<_>>(),
    )?;
    let new_row = groups[..target_zone_row]
        .iter()
        .map(Vec::len)
        .sum::<usize>()
        + new_number
        - 1;
    Ok((finish(project, zones, channels, image)?, new_row, edit))
}

/// Move channel `row` up (`delta` -1) or down (+1) inside its zone; returns `(project, new row)`.
/// References to the two swapped channels are swapped too.
pub fn shift_channel(
    project: &Project,
    row: usize,
    delta: i32,
) -> Result<(Project, usize), LayoutError> {
    if delta != -1 && delta != 1 {
        return Err(error("一次只能移動一格"));
    }
    let (zone_row, position) = channel_position(project, row)?;
    let rows = zone_channel_rows(project, zone_row);
    let target = row
        .checked_add_signed(delta as isize)
        .filter(|t| rows.contains(t));
    let Some(target) = target else {
        return Err(error(format!(
            "已經在分區的{}",
            if delta < 0 {
                "第一台"
            } else {
                "最後一台"
            }
        )));
    };
    let mut channels = project.channels.clone();
    channels.swap(row, target);
    let (a, b) = (
        as_u16(position + 1)?,
        as_u16(position.checked_add_signed(delta as isize).unwrap_or(0) + 1)?,
    );
    channels[row] = channels[row].with_index(a);
    channels[target] = channels[target].with_index(b);
    let zone_no = as_u16(zone_row + 1)?;
    let remap = |(zone, channel): Entry| {
        if zone != zone_no || (channel != a && channel != b) {
            Some((zone, channel))
        } else {
            Some((zone, if channel == a { b } else { a }))
        }
    };
    let (image, _edit) = remap_all(&project.image, remap, project.zones.len())?;
    Ok((
        finish(project, project.zones.clone(), channels, image)?,
        target,
    ))
}

/// A batch of channels to append (see [`add_channels`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BatchPlan {
    pub count: usize,
    pub start_rx_hz: i64,
    pub step_hz: i64,
    pub tx_offset_hz: i64,
    pub name_prefix: String,
    pub first_number: i64,
}

/// Append `plan.count` copies of channel `source_row` to the end of its zone. Channel k receives
/// on `start_rx_hz + k × step_hz`, transmits `tx_offset_hz` away from that, and is named the
/// prefix + its number (shortened at a character boundary to fit). Returns the project and the
/// rows of the new channels.
pub fn add_channels(
    project: &Project,
    source_row: usize,
    plan: &BatchPlan,
    codec: NameCodec,
) -> Result<(Project, Range<usize>), LayoutError> {
    if !(1..=MAX_BATCH).contains(&plan.count) {
        return Err(error(format!("一次可新增 1 到 {MAX_BATCH} 台")));
    }
    let (zone_row, _) = channel_position(project, source_row)?;
    let template = project.channels[source_row];
    let first_row = zone_channel_rows(project, zone_row).end;
    let mut current = project.clone();
    for k in 0..plan.count {
        // i128: an i64 plus a small multiple of an i64 cannot overflow, like Python's integers.
        let k_wide = k as i128;
        let rx = i128::from(plan.start_rx_hz) + k_wide * i128::from(plan.step_hz);
        let tx = rx + i128::from(plan.tx_offset_hz);
        let (Ok(rx), Ok(tx)) = (u32::try_from(rx), u32::try_from(tx)) else {
            return Err(error(format!("第 {} 台的頻率超出範圍", k + 1)));
        };
        if rx == 0 || tx == 0 {
            return Err(error(format!("第 {} 台的頻率超出範圍", k + 1)));
        }
        let number = i128::from(plan.first_number) + k_wide;
        let (field, _stored) = fit_name(&format!("{}{number}", plan.name_prefix), NAME_SIZE, codec);
        let mut raw = template.with_rx(rx).with_tx(tx).0;
        raw[2..2 + NAME_SIZE].copy_from_slice(&field);
        // Appending at the zone end never moves the template, so source_row stays valid.
        let (next, row) = add_channel(&current, source_row)?;
        let mut channels = next.channels.clone();
        channels[row] = Channel(raw).with_index(channels[row].index());
        current = Project { channels, ..next };
    }
    Ok((
        with_synced_image(&current),
        first_row..first_row + plan.count,
    ))
}
