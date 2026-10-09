"""Scan lists in the settings image.

Confirmed in the firmware (FUN_0303caf0 / FUN_0303ca90 / FUN_0303cc74 / FUN_0303cf88):
16 fixed slots of 0x50 bytes from image offset 0x12B, each

    name[10] | 6 bytes of scan settings (kept as-is) | 16 x (zone u16, channel u16)

with zone and channel counted from 1 (channel = position inside the zone) and (0, 0)
marking an empty entry. The radio counts the members at run time and drops entries
that name a channel that does not exist, moving the later ones up.
"""

from __future__ import annotations

import struct
from collections.abc import Callable, Iterable
from dataclasses import dataclass

from .records import NAME_SIZE
from .textcodec import NameCodec, display_name, encode_name

SCAN_LISTS_OFFSET = 0x12B
SCAN_LIST_SIZE = 0x50
SCAN_LIST_COUNT = 16
SCAN_SETTINGS_OFFSET = NAME_SIZE
SCAN_ENTRIES_OFFSET = 0x10
SCAN_ENTRY_COUNT = 16
SCAN_AREA_END = SCAN_LISTS_OFFSET + SCAN_LIST_COUNT * SCAN_LIST_SIZE

Entry = tuple[int, int]  # (zone number, channel number in zone), 1-based


# Scan settings inside each list (mapped by driving the legacy CPS scan-list page):
# +0x0B bit 5 talkback (回講), +0x0B bits 2-3 scan TX mode, +0x0C zone u16 and +0x0E
# channel u16 the designated channel (指定信道). +0x0A is left untouched.
SETTINGS_OFFSET = 0x0B
TALKBACK_BIT = 0x20
TX_MODE_MASK = 0x0C
TX_MODES = (("當前信道", 0), ("最後活動信道", 1), ("指定信道", 2))
DESIGNATED_ZONE_OFFSET = 0x0C
DESIGNATED_CHANNEL_OFFSET = 0x0E


class ScanListError(ValueError):
    pass


@dataclass(frozen=True)
class ScanList:
    number: int  # 1..16
    raw: bytes

    @property
    def name_field(self) -> bytes:
        return self.raw[:NAME_SIZE]

    def name(self, codec: NameCodec) -> str:
        return display_name(self.name_field, codec)

    @property
    def in_use(self) -> bool:
        """A slot the legacy CPS created: it has a name or at least one member."""
        return self.raw[0] not in (0x00, 0xFF) or bool(self.entries)

    @property
    def talkback(self) -> bool:
        return bool(self.raw[SETTINGS_OFFSET] & TALKBACK_BIT)

    @property
    def tx_mode(self) -> int:
        return (self.raw[SETTINGS_OFFSET] & TX_MODE_MASK) >> 2

    @property
    def designated(self) -> Entry:
        """(zone, channel); values the legacy CPS shows as Off (0 or out of range) pass through."""
        return (struct.unpack_from("<H", self.raw, DESIGNATED_ZONE_OFFSET)[0],
                struct.unpack_from("<H", self.raw, DESIGNATED_CHANNEL_OFFSET)[0])

    @property
    def entries(self) -> tuple[Entry, ...]:
        out = []
        for i in range(SCAN_ENTRY_COUNT):
            zone, channel = struct.unpack_from("<HH", self.raw, SCAN_ENTRIES_OFFSET + 4 * i)
            if zone not in (0, 0xFFFF) and channel not in (0, 0xFFFF):
                out.append((zone, channel))
        return tuple(out)


def has_scan_lists(image: bytes) -> bool:
    return len(image) >= SCAN_AREA_END


def scan_lists(image: bytes) -> tuple[ScanList, ...]:
    if not has_scan_lists(image):
        return ()
    return tuple(ScanList(n + 1, image[_base(n + 1):_base(n + 1) + SCAN_LIST_SIZE])
                 for n in range(SCAN_LIST_COUNT))


def _base(number: int) -> int:
    return SCAN_LISTS_OFFSET + (number - 1) * SCAN_LIST_SIZE


def _pack_entries(entries: Iterable[Entry]) -> bytes:
    items = list(entries)
    if len(items) > SCAN_ENTRY_COUNT:
        raise ScanListError(f"每個掃描清單最多 {SCAN_ENTRY_COUNT} 個頻道")
    for zone, channel in items:
        if not (1 <= zone <= 0xFFFE and 1 <= channel <= 0xFFFE):
            raise ScanListError("掃描清單項目的分區與頻道編號必須從 1 起算")
    items += [(0, 0)] * (SCAN_ENTRY_COUNT - len(items))
    return b"".join(struct.pack("<HH", *e) for e in items)


def with_scan_list(image: bytes, number: int, name: str, entries: Iterable[Entry],
                   codec: NameCodec) -> bytes:
    """Set one list's name and members; its 6 settings bytes are kept unchanged."""
    if not has_scan_lists(image):
        raise ScanListError("設定區太短，沒有掃描清單區")
    if not 1 <= number <= SCAN_LIST_COUNT:
        raise ScanListError(f"掃描清單編號必須介於 1 與 {SCAN_LIST_COUNT}")
    field = encode_name(name, NAME_SIZE, codec)
    packed = _pack_entries(entries)
    base = _base(number)
    img = bytearray(image)
    img[base:base + NAME_SIZE] = field
    img[base + SCAN_ENTRIES_OFFSET:base + SCAN_LIST_SIZE] = packed
    return bytes(img)


@dataclass(frozen=True)
class ScanEdit:
    """What a structural change did to the scan lists."""

    removed: int  # entries whose channel no longer exists
    renumbered: int  # entries now pointing at a new (zone, channel) number
    refs_changed: int = 0  # GPS / alarm channel references cleared or renumbered


def remap_entries(image: bytes, remap: Callable[[Entry], Entry | None]) -> tuple[bytes, ScanEdit]:
    """Rewrite every list: ``remap`` returns the new entry, or None to drop it.
    Dropped entries close up, as the firmware itself does."""
    if not has_scan_lists(image):
        return image, ScanEdit(0, 0)
    img = bytearray(image)
    removed = renumbered = 0
    for lst in scan_lists(image):
        kept: list[Entry] = []
        for entry in lst.entries:
            new = remap(entry)
            if new is None:
                removed += 1
                continue
            renumbered += new != entry
            kept.append(new)
        if tuple(kept) == lst.entries:
            continue
        base = _base(lst.number) + SCAN_ENTRIES_OFFSET
        img[base:base + 4 * SCAN_ENTRY_COUNT] = _pack_entries(kept)
    return bytes(img), ScanEdit(removed, renumbered)


def references(image: bytes, zone: int, channel: int | None = None) -> int:
    """Entries naming this channel (or, with channel None, any channel of the zone)."""
    return sum(1 for lst in scan_lists(image) for z, c in lst.entries
               if z == zone and (channel is None or c == channel))


def with_scan_settings(image: bytes, number: int, talkback: bool, tx_mode: int,
                       designated: Entry) -> bytes:
    """Set one list's talkback, scan TX mode and designated channel ((0, 0) = Off)."""
    if not has_scan_lists(image) or not 1 <= number <= SCAN_LIST_COUNT:
        raise ScanListError("掃描清單不存在")
    if tx_mode not in {v for _, v in TX_MODES}:
        raise ScanListError("掃描發射模式無效")
    base = _base(number)
    img = bytearray(image)
    flags = img[base + SETTINGS_OFFSET] & ~(TALKBACK_BIT | TX_MODE_MASK) & 0xFF
    img[base + SETTINGS_OFFSET] = flags | (TALKBACK_BIT if talkback else 0) | (tx_mode << 2)
    struct.pack_into("<HH", img, base + DESIGNATED_ZONE_OFFSET, *designated)
    return bytes(img)

