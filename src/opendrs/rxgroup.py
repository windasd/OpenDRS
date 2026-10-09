"""RX group lists ("GCL") in the settings image.

Confirmed in the firmware: FUN_03031284 loads list ``n`` (1..32) from flash
0x3C062B + (n-1)*0x4C — image offset 0x62B, right after the scan lists — reading a
10-byte name, then u16 contact numbers from +0x0A (33 slots, 0 = empty; each is a
``Contact.index``, read via the same contact base 0x3C13F as a channel's TX contact).
A channel selects its list with byte +0x17 (0 = none): in the real projects every
digital channel holds 1, list 1 "MMDVM_TG" holding the talkgroup contacts.
"""

from __future__ import annotations

import struct
from collections.abc import Callable, Iterable
from dataclasses import dataclass

from .records import NAME_SIZE
from .textcodec import NameCodec, display_name, encode_name

RX_GROUPS_OFFSET = 0x62B
RX_GROUP_SIZE = 0x4C
RX_GROUP_COUNT = 32
RX_MEMBERS_OFFSET = NAME_SIZE
RX_MEMBER_SLOTS = (RX_GROUP_SIZE - RX_MEMBERS_OFFSET) // 2  # 33 in the record
MAX_RX_MEMBERS = 32  # one slot always left 0 so the list stays terminated
RX_AREA_END = RX_GROUPS_OFFSET + RX_GROUP_COUNT * RX_GROUP_SIZE


class RxGroupError(ValueError):
    pass


@dataclass(frozen=True)
class RxGroup:
    number: int  # 1..32
    raw: bytes

    def name(self, codec: NameCodec) -> str:
        return display_name(self.raw[:NAME_SIZE], codec)

    @property
    def members(self) -> tuple[int, ...]:
        """Contact numbers, in stored order (empty slots skipped)."""
        values = struct.unpack_from(f"<{RX_MEMBER_SLOTS}H", self.raw, RX_MEMBERS_OFFSET)
        return tuple(v for v in values if v not in (0, 0xFFFF))

    @property
    def in_use(self) -> bool:
        return self.raw[0] not in (0x00, 0xFF) or bool(self.members)


def has_rx_groups(image: bytes) -> bool:
    return len(image) >= RX_AREA_END


def _base(number: int) -> int:
    return RX_GROUPS_OFFSET + (number - 1) * RX_GROUP_SIZE


def rx_groups(image: bytes) -> tuple[RxGroup, ...]:
    if not has_rx_groups(image):
        return ()
    return tuple(RxGroup(n, image[_base(n):_base(n) + RX_GROUP_SIZE])
                 for n in range(1, RX_GROUP_COUNT + 1))


def _pack_members(members: Iterable[int]) -> bytes:
    items = list(members)
    if len(items) > MAX_RX_MEMBERS:
        raise RxGroupError(f"每個接收群組最多 {MAX_RX_MEMBERS} 個聯絡人")
    if any(not 1 <= m <= 0xFFFE for m in items):
        raise RxGroupError("聯絡人編號必須從 1 起算")
    items += [0] * (RX_MEMBER_SLOTS - len(items))
    return struct.pack(f"<{RX_MEMBER_SLOTS}H", *items)


def with_rx_group(image: bytes, number: int, name: str, members: Iterable[int],
                  codec: NameCodec) -> bytes:
    if not has_rx_groups(image):
        raise RxGroupError("設定區太短，沒有接收群組區")
    if not 1 <= number <= RX_GROUP_COUNT:
        raise RxGroupError(f"接收群組編號必須介於 1 與 {RX_GROUP_COUNT}")
    field = encode_name(name, NAME_SIZE, codec)
    packed = _pack_members(members)
    base = _base(number)
    img = bytearray(image)
    img[base:base + NAME_SIZE] = field
    img[base + RX_MEMBERS_OFFSET:base + RX_GROUP_SIZE] = packed
    return bytes(img)


def remap_members(image: bytes, remap: Callable[[int], int | None]) -> tuple[bytes, int]:
    """Rewrite every list's members (None drops one); returns (image, members dropped)."""
    if not has_rx_groups(image):
        return image, 0
    img = bytearray(image)
    dropped = 0
    for group in rx_groups(image):
        kept = []
        for member in group.members:
            new = remap(member)
            if new is None:
                dropped += 1
            else:
                kept.append(new)
        if tuple(kept) == group.members:
            continue
        base = _base(group.number) + RX_MEMBERS_OFFSET
        img[base:base + RX_GROUP_SIZE - RX_MEMBERS_OFFSET] = _pack_members(kept)
    return bytes(img), dropped


def groups_containing(image: bytes, contact_number: int) -> list[int]:
    return [g.number for g in rx_groups(image) if contact_number in g.members]
