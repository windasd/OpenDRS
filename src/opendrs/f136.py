"""Read and write ``.F136`` project files (also the legacy ``backup.ini`` format).

Layout, as produced by the original CPS:

* CRLF lines, the last one also terminated by CRLF.
* Optional labelled rows first, e.g. ``Contacts\\t1\\t0\\t4\\t...`` — one fixed-size
  record per row, values in decimal, separated by single tabs, no trailing tab.
* Then the settings image as unlabelled rows of up to 100 decimal byte values.

The content is pure ASCII digits, so the file itself has no text-encoding issue;
names inside the records are handled by :mod:`opendrs.textcodec`.
Unmodified projects are written back byte-for-byte identical.
"""

from __future__ import annotations

import os
import shutil
import tempfile
from dataclasses import dataclass, field, replace
from pathlib import Path

from . import device as _device
from .device import DeviceInfo, frequency_problem
from .records import Channel, Contact, Zone
from .textcodec import NameCodec

ROW_WIDTH = 100
EOL = b"\r\n"

_RECORD_TYPES: dict[str, type[Contact] | type[Zone] | type[Channel]] = {
    "Contacts": Contact,
    "Zone": Zone,
    "CH": Channel,
}


class F136FormatError(ValueError):
    """The file does not follow the project layout."""


@dataclass(frozen=True)
class Project:
    contacts: tuple[Contact, ...] = ()
    zones: tuple[Zone, ...] = ()
    channels: tuple[Channel, ...] = ()
    image: bytes = b""
    section_order: tuple[str, ...] = ()
    unknown_rows: tuple[tuple[str, bytes], ...] = field(default=())

    @property
    def device(self) -> DeviceInfo:
        return DeviceInfo.from_image(self.image)

    def with_dmr_id(self, dmr_id: int) -> Project:
        return replace(self, image=_device.with_dmr_id(self.image, dmr_id))

    def with_alias(self, alias: str) -> Project:
        return replace(self, image=_device.with_alias(self.image, alias))

    def with_contact(self, row: int, contact: Contact) -> Project:
        from .codeplug import with_synced_image

        return with_synced_image(replace(self, contacts=_set(self.contacts, row, contact)))

    def with_zone(self, row: int, zone: Zone) -> Project:
        return replace(self, zones=_set(self.zones, row, zone))

    def with_channel(self, row: int, channel: Channel) -> Project:
        return replace(self, channels=_set(self.channels, row, channel))

    def problems(self) -> list[str]:
        """Conditions that must be fixed before saving."""
        from .codeplug import MAX_CONTACTS

        bands = self.device.bands
        out: list[str] = []
        if len(self.contacts) > MAX_CONTACTS:
            out.append(f"聯絡人 {len(self.contacts)} 筆，超過對講機上限 {MAX_CONTACTS} 筆")
        contact_numbers = {c.index for c in self.contacts}
        for row, ch in enumerate(self.channels, start=1):
            if ch.tx_contact and ch.tx_contact not in contact_numbers:
                out.append(f"頻道第 {row} 列 發話對象：聯絡人 #{ch.tx_contact} 不存在")
            for label, hz in (("接收頻率", ch.freq_a_hz), ("發射頻率", ch.freq_b_hz)):
                reason = frequency_problem(hz, bands)
                if reason:
                    out.append(f"頻道第 {row} 列 {label}：{reason}")
        return out

    def names_decodable(self, codec: NameCodec) -> bool:
        return not any(
            r.name(codec).startswith("‹") for r in (*self.contacts, *self.zones, *self.channels)
        )


def _set(items: tuple, row: int, value: object) -> tuple:
    if not 0 <= row < len(items):
        raise IndexError(row)
    return items[:row] + (value,) + items[row + 1 :]


def _parse_values(fields: list[bytes], line_no: int) -> bytes:
    try:
        values = [int(f) for f in fields]
    except ValueError as exc:
        raise F136FormatError(f"第 {line_no} 行含非數字欄位") from exc
    if any(not 0 <= v <= 255 for v in values):
        raise F136FormatError(f"第 {line_no} 行有超出 0–255 的數值")
    return bytes(values)


def parse(data: bytes) -> Project:
    if data.startswith(b"\xef\xbb\xbf"):
        data = data[3:]
    if data and not data.endswith(EOL):
        data += EOL
    records: dict[str, list] = {k: [] for k in _RECORD_TYPES}
    order: list[str] = []
    unknown: list[tuple[str, bytes]] = []
    image = bytearray()
    for line_no, line in enumerate(data.split(EOL)[:-1], start=1):
        if not line:
            raise F136FormatError(f"第 {line_no} 行是空行")
        fields = line.split(b"\t")
        if fields[0][:1].isdigit():
            image += _parse_values(fields, line_no)
            continue
        label = fields[0].decode("ascii", errors="replace")
        payload = _parse_values(fields[1:], line_no)
        if label not in order:
            order.append(label)
        cls = _RECORD_TYPES.get(label)
        if cls is None:
            unknown.append((label, payload))
            continue
        try:
            records[label].append(cls(payload))
        except ValueError as exc:
            raise F136FormatError(f"第 {line_no} 行 {label}：{exc}") from exc
    return Project(
        contacts=tuple(records["Contacts"]),
        zones=tuple(records["Zone"]),
        channels=tuple(records["CH"]),
        image=bytes(image),
        section_order=tuple(order),
        unknown_rows=tuple(unknown),
    )


def _row(label: str | None, payload: bytes) -> bytes:
    numbers = b"\t".join(str(b).encode("ascii") for b in payload)
    return (label.encode("ascii") + b"\t" + numbers if label else numbers) + EOL


def serialize(project: Project) -> bytes:
    """Write the project. The settings image's contact copy and counts are synced to
    the rows first (a no-op for projects saved by the legacy CPS)."""
    from .codeplug import with_synced_image

    project = with_synced_image(project)
    out = bytearray()
    by_label: dict[str, list[bytes]] = {
        "Contacts": [r.raw for r in project.contacts],
        "Zone": [r.raw for r in project.zones],
        "CH": [r.raw for r in project.channels],
    }
    for label, payload in project.unknown_rows:
        by_label.setdefault(label, []).append(payload)
    order = list(project.section_order) + [k for k in by_label if k not in project.section_order]
    for label in order:
        for payload in by_label.get(label, []):
            out += _row(label, payload)
    for start in range(0, len(project.image), ROW_WIDTH):
        out += _row(None, project.image[start : start + ROW_WIDTH])
    return bytes(out)


def load(path: str | os.PathLike[str]) -> Project:
    return parse(Path(path).read_bytes())


def save(project: Project, path: str | os.PathLike[str], *, keep_backup: bool = True) -> None:
    """Atomically write ``project``; refuses to save while :meth:`Project.problems` is non-empty."""
    issues = project.problems()
    if issues:
        raise F136FormatError("尚有問題未修正：\n" + "\n".join(issues))
    target = Path(path)
    if keep_backup and target.exists():
        backup = target.with_name(target.name + ".bak")
        if not backup.exists():
            shutil.copy2(target, backup)
    fd, tmp = tempfile.mkstemp(dir=target.parent, prefix=".opendrs-", suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as fh:
            fh.write(serialize(project))
        os.replace(tmp, target)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
