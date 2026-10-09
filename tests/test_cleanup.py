from __future__ import annotations

from pathlib import Path

import pytest

from opendrs import f136
from opendrs.cleanup import CONTROL, clean_field, clean_names, needs_cleaning, scan
from opendrs.f136 import Project
from opendrs.records import NAME_SIZE, Channel, Contact, Zone
from opendrs.textcodec import NameCodec

DH98 = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"


def _name_body(rec) -> bytes:  # noqa: ANN001
    field = rec.raw[rec.NAME_OFFSET : rec.NAME_OFFSET + NAME_SIZE]
    for i, b in enumerate(field):
        if b in (0, 0xFF):
            return field[:i]
    return field


def test_real_project_343_contacts_cleaned() -> None:
    project = f136.load(DH98)
    assert scan(project).contacts == 343
    cleaned, report = clean_names(project)
    assert (report.contacts, report.zones, report.channels) == (343, 0, 0)
    assert report.summary() == "聯絡人 343 筆"
    for before, after in zip(project.contacts, cleaned.contacts, strict=True):
        assert not any(b in CONTROL for b in _name_body(after))
        expected = before.name(NameCodec.BIG5).replace("␍", "").replace("␊", "").strip()
        assert after.name(NameCodec.BIG5) == expected
        changed = [i for i in range(16) if before.raw[i] != after.raw[i]]
        assert all(Contact.NAME_OFFSET <= i < Contact.NAME_OFFSET + NAME_SIZE for i in changed)
    first = cleaned.contacts[15]
    assert first.name(NameCodec.BIG5) == "QA0BD" and first.call_id == 9990001
    # the settings image carries a copy of the contacts; it is cleaned too, nothing else moves
    changed = [i for i in range(len(project.image)) if project.image[i] != cleaned.image[i]]
    assert changed and all(0x1400 <= i < 0x1400 + 16 * len(project.contacts) for i in changed)
    assert cleaned.image[0x1400:0x1400 + 16 * len(cleaned.contacts)] == b"".join(
        c.raw for c in cleaned.contacts)
    assert cleaned.zones == project.zones and cleaned.channels == project.channels


def test_cleaning_is_idempotent_and_round_trips() -> None:
    cleaned, _ = clean_names(f136.load(DH98))
    again, report = clean_names(cleaned)
    assert report.total == 0 and again is cleaned
    assert f136.parse(f136.serialize(cleaned)) == cleaned


def test_clean_projects_are_untouched(small_path: Path) -> None:
    project = f136.load(small_path)
    out, report = clean_names(project)
    assert report.total == 0 and out is project
    assert report.summary() == "沒有"


@pytest.mark.parametrize(
    "field, expected",
    [
        (b"QA0BD\r\n\x00\x00\x00", b"QA0BD\x00\x00\x00\x00\x00"),
        (b"AB\r\nCD\x00\x00\x00\x00", b"AB CD\x00\x00\x00\x00\x00"),
        (b"\r\nAB\x00\x00\x00\x00\x00\x00", b"AB\x00\x00\x00\x00\x00\x00\x00\x00"),
        (b"AB \r\n CD\x00\x00", b"AB CD\x00\x00\x00\x00\x00"),
        (b"\r\n\r\n\x00\x00\x00\x00\x00\x00", bytes(10)),
        (b"BV2\r\n\xff\xff\xff\xff\xff", b"BV2\xff\xff\xff\xff\xff\xff\xff"),  # keeps 0xFF padding
        (b"AB\r\x00\x12\x34\x56\x78\x9a\xbc", b"AB\x00\x00\x12\x34\x56\x78\x9a\xbc"),  # tail untouched
        (b"ABCDEFGH\r\n", b"ABCDEFGH\x00\x00"),  # full field, no terminator
        (b"PLAIN\x00\x00\x00\x00\x00", b"PLAIN\x00\x00\x00\x00\x00"),
    ],
)
def test_clean_field(field: bytes, expected: bytes) -> None:
    assert len(field) == 10
    assert clean_field(field) == expected


@pytest.mark.parametrize("codec, text", [(NameCodec.BIG5, "臺北中繼"), (NameCodec.GBK, "台北中继")])
def test_cjk_names_survive(codec: NameCodec, text: str) -> None:
    body = text.encode(codec.value)
    field = body + b"\r\n" + bytes(10 - len(body) - 2)
    rec = Channel(bytes(2) + field + bytes(20))
    assert needs_cleaning(rec)
    cleaned, report = clean_names(Project(channels=(rec,)))
    assert report.channels == 1
    assert cleaned.channels[0].name(codec) == text


def test_zone_names_are_cleaned() -> None:
    zone = Zone(bytes(3) + b"Z1\n" + bytes(7) + bytes(19))
    cleaned, report = clean_names(Project(zones=(zone,)))
    assert report.zones == 1 and cleaned.zones[0].name(NameCodec.ASCII) == "Z1"
