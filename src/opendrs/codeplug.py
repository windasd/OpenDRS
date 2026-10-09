"""Layout of the programming transfer ("codeplug stream").

Established from a USB capture of the legacy CPS (2026-10-08) and the radio's
transfer handlers in the firmware (FUN_030272fc / FUN_03027488 / FUN_03027670):

* The stream is a whole number of 2048-byte blocks in two parts.
* Part 1 is radio flash 0x3C0000 onwards: the 0x1400-byte settings area followed
  by the contact records (16 bytes each) at 0x1400.
* Part 2 is radio flash 0x400000 onwards: zone records then channel records
  (32 bytes each), back to back.
* The settings area holds the counts at 0x138F: zones, channels, contacts (u16 LE).
  The radio derives the block counts from them with exactly these formulas.

A ``.F136`` project stores part 1 as its "image" (contacts included) and also the
contact / zone / channel records as labelled rows; the two copies must agree.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, replace
from typing import TYPE_CHECKING

from .records import Channel, Contact, Zone

if TYPE_CHECKING:  # pragma: no cover
    from .f136 import Project

BLOCK = 0x800
SETTINGS_SIZE = 0x1400
COUNTS_OFFSET = 0x138F
CONTACTS_OFFSET = 0x1400
CONTACT_SIZE = 16
RECORD_SIZE = 32
MAX_CONTACTS = 1000  # radio treats >= 0x3E9 as corrupt (FUN_030272fc)
DATE_OFFSET = 0x60
DATE_SIZE = 16
# When a write completes the radio sets its clock from the text at DATE_OFFSET
# (FUN_03027aec, called at 0x03027DA8; minutes resolution, seconds untouched). It parses
# year first only, so the legacy CPS's US-style M/d/yyyy stamp leaves the clock alone.
DATE_FORMAT = "%Y/%m/%d %H:%M"
PART1_ADDR = 0x3C0000
PART2_ADDR = 0x400000
IMAGE_ROW = 100


class CodeplugError(ValueError):
    """The project cannot be turned into a valid transfer, or a transfer is malformed."""


@dataclass(frozen=True)
class Counts:
    zones: int
    channels: int
    contacts: int

    @property
    def part1_blocks(self) -> int:
        return (self.contacts * CONTACT_SIZE + 0x1BFF) >> 11

    @property
    def part2_blocks(self) -> int:
        return ((self.zones + self.channels) * RECORD_SIZE + 0x7FF) >> 11

    @property
    def total_blocks(self) -> int:
        return self.part1_blocks + self.part2_blocks

    @property
    def stream_size(self) -> int:
        return self.total_blocks * BLOCK

    @classmethod
    def of(cls, project: Project) -> Counts:
        return cls(len(project.zones), len(project.channels), len(project.contacts))


def _round_up(value: int, step: int) -> int:
    return -(-value // step) * step


def image_length_for(contacts: int) -> int:
    """Image length the legacy CPS uses when it saves a freshly read radio."""
    return _round_up(CONTACTS_OFFSET + contacts * CONTACT_SIZE, IMAGE_ROW) + IMAGE_ROW


def sync_image(image: bytes, contacts_raw: bytes, zones: int, channels: int) -> bytes:
    """Bring the settings image in line with the record rows.

    Writes the counts at 0x138F and the contact records at 0x1400, growing the
    image if needed. Returns ``image`` unchanged when it is already consistent
    (true for every project the legacy CPS saves) or when there is no settings
    image at all.
    """
    if len(image) < COUNTS_OFFSET + 6:
        return image
    end = CONTACTS_OFFSET + len(contacts_raw)
    img = bytearray(image)
    if len(img) < end:
        img += bytes(image_length_for(len(contacts_raw) // CONTACT_SIZE) - len(img))
    img[COUNTS_OFFSET:COUNTS_OFFSET + 6] = struct.pack(
        "<3H", zones, channels, len(contacts_raw) // CONTACT_SIZE)
    img[CONTACTS_OFFSET:end] = contacts_raw
    return bytes(img)


def counts_in_image(image: bytes) -> Counts | None:
    if len(image) < COUNTS_OFFSET + 6:
        return None
    return Counts(*struct.unpack_from("<3H", image, COUNTS_OFFSET))


def encode_date(text: str) -> bytes:
    try:
        raw = text.encode("ascii")
    except UnicodeEncodeError as exc:
        raise CodeplugError("寫入時間只能包含 ASCII 字元") from exc
    if len(raw) > DATE_SIZE:
        raise CodeplugError(f"寫入時間超過 {DATE_SIZE} 個字元")
    return raw + bytes(DATE_SIZE - len(raw))


def check_writable(project: Project) -> list[str]:
    """Reasons ``project`` must not be sent to a radio (empty when it is fine)."""
    reasons: list[str] = []
    if len(project.image) < SETTINGS_SIZE:
        reasons.append("專案沒有設定區（請先從對講機讀取，或開啟由寫頻軟體儲存的專案）")
    if len(project.contacts) > MAX_CONTACTS:
        reasons.append(f"聯絡人 {len(project.contacts)} 筆，超過對講機上限 {MAX_CONTACTS} 筆")
    if not project.zones:
        reasons.append("專案沒有任何分區")
    reasons.extend(project.problems())
    return reasons


def build_stream(project: Project, date_text: str | None = None) -> bytes:
    """Serialise ``project`` into the exact bytes the radio expects during a write."""
    reasons = check_writable(project)
    if reasons:
        raise CodeplugError("\n".join(reasons))
    counts = Counts.of(project)
    contacts_raw = b"".join(c.raw for c in project.contacts)
    image = sync_image(project.image, contacts_raw, counts.zones, counts.channels)
    part1 = bytearray(counts.part1_blocks * BLOCK)
    if any(image[len(part1):]):
        raise CodeplugError("設定區在聯絡人之後還有資料，超出傳輸範圍")
    part1[:min(len(image), len(part1))] = image[:len(part1)]
    if date_text is not None:
        part1[DATE_OFFSET:DATE_OFFSET + DATE_SIZE] = encode_date(date_text)
    part2 = bytearray(counts.part2_blocks * BLOCK)
    records = b"".join(r.raw for r in (*project.zones, *project.channels))
    part2[:len(records)] = records
    return bytes(part1 + part2)


def parse_stream(stream: bytes, counts: Counts) -> Project:
    """Turn a transfer read from the radio back into a project."""
    from .f136 import Project

    if counts.contacts > MAX_CONTACTS:
        raise CodeplugError(f"對講機回報聯絡人 {counts.contacts} 筆，超過上限")
    if len(stream) != counts.stream_size:
        raise CodeplugError(f"收到 {len(stream)} bytes，預期 {counts.stream_size} bytes")
    p1_len = counts.part1_blocks * BLOCK
    part1, part2 = stream[:p1_len], stream[p1_len:]
    if counts_in_image(part1) != counts:
        raise CodeplugError("設定區內的計數與對講機回報不一致")
    contacts = tuple(Contact(part1[o:o + CONTACT_SIZE])
                     for o in range(CONTACTS_OFFSET, CONTACTS_OFFSET + counts.contacts * CONTACT_SIZE,
                                    CONTACT_SIZE))
    zones = tuple(Zone(part2[i * RECORD_SIZE:(i + 1) * RECORD_SIZE]) for i in range(counts.zones))
    channels = tuple(Channel(part2[(counts.zones + i) * RECORD_SIZE:(counts.zones + i + 1) * RECORD_SIZE])
                     for i in range(counts.channels))
    image_len = image_length_for(counts.contacts)
    image = part1[:image_len] + bytes(max(0, image_len - len(part1)))
    return Project(contacts=contacts, zones=zones, channels=channels, image=image,
                   section_order=("Contacts", "Zone", "CH"))


def with_synced_image(project: Project) -> Project:
    image = sync_image(project.image, b"".join(c.raw for c in project.contacts),
                       len(project.zones), len(project.channels))
    return project if image == project.image else replace(project, image=image)
