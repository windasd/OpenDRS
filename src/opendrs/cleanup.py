"""Remove control characters (mostly CR/LF) that the legacy CPS wrote into names.

Works on bytes, independent of the selected name codec: in Big5 and GBK every
trail byte is >= 0x40, so a byte below 0x20 can never be half of a CJK
character. Only the name body (up to the first 0x00/0xFF terminator) is
rewritten; the terminator and every byte after it are left exactly as they were.
"""

from __future__ import annotations

from dataclasses import dataclass, replace

from .codeplug import with_synced_image
from .f136 import Project
from .records import NAME_SIZE
from .textcodec import PAD_BYTES

CONTROL = frozenset(range(0x01, 0x20)) | {0x7F}


def _body_len(field: bytes) -> int:
    for i, b in enumerate(field):
        if b in PAD_BYTES:
            return i
    return len(field)


def clean_field(field: bytes) -> bytes:
    """Return ``field`` with control bytes removed from its name body.

    Leading/trailing control runs are dropped; interior runs become one space.
    The freed bytes are filled with the field's own terminator byte.
    """
    n = _body_len(field)
    body = field[:n]
    if not any(b in CONTROL for b in body):
        return field
    parts: list[bytes] = []
    current = bytearray()
    for b in body:
        if b in CONTROL:
            if current:
                parts.append(bytes(current))
                current = bytearray()
        else:
            current.append(b)
    if current:
        parts.append(bytes(current))
    cleaned = b" ".join(p.strip(b" ") for p in parts if p.strip(b" "))
    pad = field[n] if n < len(field) else 0x00
    return cleaned + bytes([pad]) * (n - len(cleaned)) + field[n:]


def _clean_record(rec):  # noqa: ANN001, ANN202
    off = rec.NAME_OFFSET
    field = rec.raw[off : off + NAME_SIZE]
    new_field = clean_field(field)
    if new_field == field:
        return rec
    return type(rec)(rec.raw[:off] + new_field + rec.raw[off + NAME_SIZE :])


def needs_cleaning(rec) -> bool:  # noqa: ANN001
    field = rec.raw[rec.NAME_OFFSET : rec.NAME_OFFSET + NAME_SIZE]
    return any(b in CONTROL for b in field[: _body_len(field)])


@dataclass(frozen=True)
class CleanupReport:
    contacts: int
    zones: int
    channels: int

    @property
    def total(self) -> int:
        return self.contacts + self.zones + self.channels

    def summary(self) -> str:
        parts = [f"{label} {n} 筆" for label, n in
                 (("聯絡人", self.contacts), ("分區", self.zones), ("頻道", self.channels)) if n]
        return "、".join(parts) if parts else "沒有"


def scan(project: Project) -> CleanupReport:
    return CleanupReport(
        sum(map(needs_cleaning, project.contacts)),
        sum(map(needs_cleaning, project.zones)),
        sum(map(needs_cleaning, project.channels)),
    )


def clean_names(project: Project) -> tuple[Project, CleanupReport]:
    report = scan(project)
    if report.total == 0:
        return project, report
    cleaned = replace(
        project,
        contacts=tuple(map(_clean_record, project.contacts)),
        zones=tuple(map(_clean_record, project.zones)),
        channels=tuple(map(_clean_record, project.channels)),
    )
    return with_synced_image(cleaned), report
