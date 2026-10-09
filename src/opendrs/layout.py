"""Zones and channels: which zone owns each channel, and structural edits that keep
every reference valid.

Part 2 holds the zones followed by the channels, 32 bytes each. A zone names its
channels by ``first_record`` (1-based record number in part 2) and ``channel_count``,
so a zone's channels are contiguous, zones follow each other in order, and the first
channel record is number ``zone_count + 1``. A channel's own index field counts from 1
inside its zone; a zone's index field counts zones from 1.

Scan lists (see scanlist.py) name channels as (zone number, channel number in zone).
The radio drops entries naming a channel that does not exist but trusts the rest, so
every edit that shifts numbers must rewrite them:

* add a channel at a zone's end, add a zone at the end: no number changes;
* delete a channel: its entries go, higher channel numbers in that zone drop by one;
* delete a zone: its entries go, higher zone numbers drop by one.
"""

from __future__ import annotations

from dataclasses import replace

from .alarm_enc import ALARM_COUNT, alarm_base
from .codeplug import with_synced_image
from .f136 import Project
from .records import Channel, Zone
from .scanlist import (
    SCAN_LIST_COUNT,
    SCAN_LIST_SIZE,
    SCAN_LISTS_OFFSET,
    Entry,
    ScanEdit,
    remap_entries,
)
from .textcodec import NameCodec

# Other single (zone u16, channel u16) references in the settings image: the DMR
# service "GPS上報信道" and the alarm channel of each digital alarm list.
CHANNEL_REFS = (
    (0x1354, 0x1356),
    *((alarm_base(n) + 0x0C, alarm_base(n) + 0x0E) for n in range(1, ALARM_COUNT + 1)),
    # each scan list's designated channel (指定信道)
    *((SCAN_LISTS_OFFSET + n * SCAN_LIST_SIZE + 0x0C, SCAN_LISTS_OFFSET + n * SCAN_LIST_SIZE + 0x0E)
      for n in range(SCAN_LIST_COUNT)),
)


def _remap_refs(image: bytes, remap, zone_count: int) -> tuple[bytes, int]:  # noqa: ANN001
    img = bytearray(image)
    changed = 0
    for zone_at, channel_at in CHANNEL_REFS:
        if len(img) < max(zone_at, channel_at) + 2:
            continue
        entry = (int.from_bytes(img[zone_at:zone_at + 2], "little"),
                 int.from_bytes(img[channel_at:channel_at + 2], "little"))
        if entry == (0, 0) or not 1 <= entry[0] <= zone_count:
            continue  # Off, or a stale value the legacy CPS shows as Off: leave it alone
        new = remap(entry) or (0, 0)
        if new != entry:
            changed += 1
            img[zone_at:zone_at + 2] = new[0].to_bytes(2, "little")
            img[channel_at:channel_at + 2] = new[1].to_bytes(2, "little")
    return bytes(img), changed


def _remap_all(image: bytes, remap, zone_count: int) -> tuple[bytes, ScanEdit]:  # noqa: ANN001
    image, edit = remap_entries(image, remap)
    image, refs = _remap_refs(image, remap, zone_count)
    return image, replace(edit, refs_changed=refs)


class LayoutError(ValueError):
    """The zone table does not describe the channel list the way the radio expects."""


def zone_rows(project: Project) -> list[int | None]:
    """Zone row (0-based) for every channel row; ``None`` where no zone covers it."""
    owner: list[int | None] = [None] * len(project.channels)
    first_channel = len(project.zones) + 1
    for z, zone in enumerate(project.zones):
        start = zone.first_record - first_channel
        for row in range(max(start, 0), min(start + zone.channel_count, len(owner))):
            owner[row] = z
    return owner


def check_layout(project: Project) -> None:
    """Raise unless the zones tile the channel list contiguously, in order."""
    expected = len(project.zones) + 1
    for z, zone in enumerate(project.zones, start=1):
        if zone.first_record != expected:
            raise LayoutError(f"分區 {z} 的起始位置 {zone.first_record} 與預期 {expected} 不符")
        expected += zone.channel_count
    if expected - len(project.zones) - 1 != len(project.channels):
        raise LayoutError("分區內的頻道數合計與頻道清單筆數不符")


def channel_position(project: Project, row: int) -> tuple[int, int]:
    """(zone row, position in zone), both 0-based."""
    check_layout(project)
    if not 0 <= row < len(project.channels):
        raise LayoutError("請先選擇頻道")
    zone_row = zone_rows(project)[row]
    if zone_row is None:
        raise LayoutError("這個頻道不屬於任何分區")
    zone = project.zones[zone_row]
    return zone_row, row - (zone.first_record - len(project.zones) - 1)


def zone_channel_rows(project: Project, zone_row: int) -> range:
    zone = project.zones[zone_row]
    start = zone.first_record - len(project.zones) - 1
    return range(start, start + zone.channel_count)


def _relayout(zones: list[Zone], counts: list[int]) -> tuple[Zone, ...]:
    """Renumber zones 1..n and lay their channels out back to back."""
    first = len(zones) + 1
    out = []
    for i, (zone, count) in enumerate(zip(zones, counts, strict=True), start=1):
        out.append(zone.with_index(i).with_layout(first, count))
        first += count
    return tuple(out)


def _finish(project: Project, zones: tuple[Zone, ...], channels: list[Channel],
            image: bytes) -> Project:
    updated = replace(project, zones=zones, channels=tuple(channels), image=image)
    check_layout(updated)
    return with_synced_image(updated)


def add_channel(project: Project, source_row: int) -> tuple[Project, int]:
    """Copy channel ``source_row`` to the end of the same zone; returns (project, new row)."""
    zone_row, _ = channel_position(project, source_row)
    rows = zone_channel_rows(project, zone_row)
    channels = list(project.channels)
    channels.insert(rows.stop, project.channels[source_row].with_index(len(rows) + 1))
    counts = [z.channel_count for z in project.zones]
    counts[zone_row] += 1
    zones = _relayout(list(project.zones), counts)
    return _finish(project, zones, channels, project.image), rows.stop


def delete_channel(project: Project, row: int) -> tuple[Project, ScanEdit]:
    """Remove channel ``row``: renumber the rest of its zone, move later zones up and
    fix the scan lists. A zone's last remaining channel cannot be deleted."""
    zone_row, position = channel_position(project, row)
    rows = zone_channel_rows(project, zone_row)
    if len(rows) <= 1:
        raise LayoutError(f"分區 {zone_row + 1} 只剩這個頻道，不能刪除"
                          "（分區至少要有一個頻道；要整個移除請用「刪除分區」）")
    channels = list(project.channels)
    del channels[row]
    for r in range(row, rows.stop - 1):
        channels[r] = channels[r].with_index(channels[r].index - 1)
    counts = [z.channel_count for z in project.zones]
    counts[zone_row] -= 1
    zone_no, channel_no = zone_row + 1, position + 1

    def remap(entry: Entry) -> Entry | None:
        zone, channel = entry
        if zone != zone_no or channel < channel_no:
            return entry
        return None if channel == channel_no else (zone, channel - 1)

    image, edit = _remap_all(project.image, remap, len(project.zones))
    zones = _relayout(list(project.zones), counts)
    return _finish(project, zones, channels, image), edit


def add_zone(project: Project, source_row: int, name: str,
             codec: NameCodec) -> tuple[Project, int]:
    """Append a zone named ``name`` holding a copy of channel ``source_row``.

    Returns (project, new zone row). No existing zone or channel number changes, so
    the scan lists stay valid.
    """
    check_layout(project)
    if not 0 <= source_row < len(project.channels):
        raise LayoutError("請先選擇要放進新分區的頻道")
    template = project.zones[-1] if project.zones else Zone(bytes(Zone.SIZE))
    zones = [*project.zones, template.with_name(name, codec)]
    channels = [*project.channels, project.channels[source_row].with_index(1)]
    counts = [z.channel_count for z in project.zones] + [1]
    return _finish(project, _relayout(zones, counts), channels, project.image), len(zones) - 1


def delete_zone(project: Project, zone_row: int) -> tuple[Project, ScanEdit]:
    """Remove a zone and all its channels; later zones move up and the scan lists follow."""
    check_layout(project)
    if not 0 <= zone_row < len(project.zones):
        raise LayoutError("請先選擇分區")
    if len(project.zones) <= 1:
        raise LayoutError("至少要保留一個分區")
    rows = zone_channel_rows(project, zone_row)
    channels = [ch for r, ch in enumerate(project.channels) if r not in rows]
    zones = [z for i, z in enumerate(project.zones) if i != zone_row]
    zone_no = zone_row + 1

    def remap(entry: Entry) -> Entry | None:
        zone, channel = entry
        if zone < zone_no:
            return entry
        return None if zone == zone_no else (zone - 1, channel)

    image, edit = _remap_all(project.image, remap, len(project.zones))
    relaid = _relayout(zones, [z.channel_count for z in zones])
    return _finish(project, relaid, channels, image), edit


def move_channel(project: Project, row: int, target_zone_row: int) -> tuple[Project, int, ScanEdit]:
    """Move channel ``row`` to the end of zone ``target_zone_row``.

    Returns (project, new row, scan edit). The rest of the source zone closes up, and
    every scan-list / GPS / alarm reference follows: the moved channel's entries point
    at its new (zone, channel) number, later channels of the source zone drop by one.
    """
    zone_row, position = channel_position(project, row)
    if not 0 <= target_zone_row < len(project.zones):
        raise LayoutError("目標分區不存在")
    if target_zone_row == zone_row:
        return project, row, ScanEdit(0, 0)
    if project.zones[zone_row].channel_count <= 1:
        raise LayoutError(f"分區 {zone_row + 1} 只剩這個頻道，不能移走（分區至少要有一個頻道）")
    groups = [list(project.channels[r] for r in zone_channel_rows(project, z))
              for z in range(len(project.zones))]
    moved = groups[zone_row].pop(position)
    groups[target_zone_row].append(moved)
    new_number = len(groups[target_zone_row])
    channels = [ch.with_index(i) for group in groups for i, ch in enumerate(group, start=1)]
    src_no, dst_no, old_no = zone_row + 1, target_zone_row + 1, position + 1

    def remap(entry: Entry) -> Entry | None:
        zone, channel = entry
        if zone != src_no or channel < old_no:
            return entry
        return (dst_no, new_number) if channel == old_no else (zone, channel - 1)

    image, edit = _remap_all(project.image, remap, len(project.zones))
    zones = _relayout(list(project.zones), [len(g) for g in groups])
    new_row = sum(len(g) for g in groups[:target_zone_row]) + new_number - 1
    return _finish(project, zones, channels, image), new_row, edit


def shift_channel(project: Project, row: int, delta: int) -> tuple[Project, int]:
    """Move channel ``row`` up (delta -1) or down (+1) inside its zone; returns
    (project, new row). Scan-list / GPS / alarm entries of the two swapped channels
    are swapped too, so every reference keeps naming the same physical channel."""
    if delta not in (-1, 1):
        raise LayoutError("一次只能移動一格")
    zone_row, position = channel_position(project, row)
    rows = zone_channel_rows(project, zone_row)
    target = row + delta
    if target not in rows:
        raise LayoutError("已經在分區的" + ("第一台" if delta < 0 else "最後一台"))
    channels = list(project.channels)
    channels[row], channels[target] = channels[target], channels[row]
    channels[row] = channels[row].with_index(position + 1)
    channels[target] = channels[target].with_index(position + 1 + delta)
    zone_no, a, b = zone_row + 1, position + 1, position + 1 + delta

    def remap(entry: Entry) -> Entry | None:
        zone, channel = entry
        if zone != zone_no or channel not in (a, b):
            return entry
        return (zone, b if channel == a else a)

    image, _edit = _remap_all(project.image, remap, len(project.zones))
    return _finish(project, project.zones, channels, image), target


MAX_BATCH = 64


def add_channels(project: Project, source_row: int, count: int, start_rx_hz: int, step_hz: int,
                 tx_offset_hz: int, name_prefix: str, first_number: int,
                 codec: NameCodec) -> tuple[Project, range]:
    """Append ``count`` copies of channel ``source_row`` to the end of its zone.

    Channel k (0-based) receives on ``start_rx_hz + k * step_hz``, transmits
    ``tx_offset_hz`` away from that, and is named ``name_prefix`` + its number
    (shortened at a character boundary to fit the 10-byte name field). Returns the
    project and the rows of the new channels.
    """
    from .textcodec import fit_name

    if not 1 <= count <= MAX_BATCH:
        raise LayoutError(f"一次可新增 1 到 {MAX_BATCH} 台")
    zone_row, _ = channel_position(project, source_row)
    template = project.channels[source_row]
    first_row = zone_channel_rows(project, zone_row).stop
    for k in range(count):
        rx = start_rx_hz + k * step_hz
        tx = rx + tx_offset_hz
        if not (0 < rx <= 0xFFFFFFFF and 0 < tx <= 0xFFFFFFFF):
            raise LayoutError(f"第 {k + 1} 台的頻率超出範圍")
        field, _stored = fit_name(f"{name_prefix}{first_number + k}", 10, codec)
        channel = template.with_rx(rx).with_tx(tx)
        channel = Channel(channel.raw[:2] + field + channel.raw[12:])
        # Appending at the zone end never moves the template, so source_row stays valid.
        project, row = add_channel(project, source_row)
        channels = list(project.channels)
        channels[row] = channel.with_index(channels[row].index)
        project = replace(project, channels=tuple(channels))
    return with_synced_image(project), range(first_row, first_row + count)
