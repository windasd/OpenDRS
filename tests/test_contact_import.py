from __future__ import annotations

import struct
from pathlib import Path

import pytest

from opendrs import f136
from opendrs.contact_import import ImportReport, NameFormat, contact_text, import_users
from opendrs.f136 import Project
from opendrs.radioid import RadioIdUser
from opendrs.records import Contact
from opendrs.textcodec import NameCodec, display_name, fit_name


def user(rid: int, call: str, first: str = "", last: str = "") -> RadioIdUser:
    return RadioIdUser(rid, call, first, last, "", "", "Taiwan")


@pytest.mark.parametrize(
    "fmt, expected",
    [(NameFormat.CALLSIGN, "QA0BD"), (NameFormat.CALLSIGN_NAME, "QA0BD James"),
     (NameFormat.NAME, "James")],
)
def test_name_formats(fmt: NameFormat, expected: str) -> None:
    assert contact_text(user(9990001, "QA0BD", "James"), fmt) == expected


def test_name_format_fallbacks() -> None:
    assert contact_text(user(9990001, "", ""), NameFormat.CALLSIGN) == "9990001"
    assert contact_text(user(9990001, "QA0BD", ""), NameFormat.NAME) == "QA0BD"
    assert {f.label for f in NameFormat} and len(NameFormat) == 3


@pytest.mark.parametrize(
    "text, codec, stored",
    [
        ("QA0BD James", NameCodec.ASCII, "QA0BD Jame"),  # cut at 10 bytes
        ("José Núñez", NameCodec.ASCII, "Jose Nunez"),    # accents folded
        ("王小明測試字", NameCodec.BIG5, "王小明測試"),        # 5 CJK = 10 bytes
        ("A王小明測試", NameCodec.BIG5, "A王小明測"),          # never split a CJK char
        ("QA0BD\r\n", NameCodec.ASCII, "QA0BD"),          # legacy CR/LF bug never written
        ("ab😀cd", NameCodec.BIG5, "abcd"),               # unrepresentable dropped
        ("ab        x", NameCodec.ASCII, "ab"),           # trailing space trimmed
    ],
)
def test_fit_name(text: str, codec: NameCodec, stored: str) -> None:
    field, got = fit_name(text, 10, codec)
    assert got == stored
    assert len(field) == 10
    assert field.rstrip(b"\x00").decode(codec.value) == stored


def test_display_shows_legacy_control_characters() -> None:
    assert display_name(b"QA0BD\r\n\x00\x00\x00", NameCodec.ASCII) == "QA0BD␍␊"


def test_import_into_real_project(small_path: Path) -> None:
    project = f136.load(small_path)
    before = len(project.contacts)
    users = [user(9990001, "QA0BD", "James"), user(9990002, "QA0ADN", "Chen")]
    updated, report = import_users(project, users, NameFormat.CALLSIGN, NameCodec.BIG5)
    assert report == ImportReport(2, 0, 0, 0, ())
    added = updated.contacts[before:]
    assert [c.index for c in added] == [before + 1, before + 2]
    assert all(c.kind == Contact.PRIVATE_CALL for c in added)
    assert [(c.name(NameCodec.BIG5), c.call_id) for c in added] == [
        ("QA0BD", 9990001), ("QA0ADN", 9990002)]
    reloaded = f136.parse(f136.serialize(updated))
    assert reloaded == updated
    # image: counts at 0x138F and the appended contact copy change, nothing else
    img_before, img_after = project.image, updated.image
    changed = [i for i in range(len(img_before)) if img_before[i] != img_after[i]]
    assert all(0x138F <= i < 0x1395 or 0x1400 <= i for i in changed)
    assert struct.unpack_from("<3H", img_after, 0x138F) == (
        len(updated.zones), len(updated.channels), len(updated.contacts))
    assert img_after[0x1400:0x1400 + 16 * len(updated.contacts)] == b"".join(
        c.raw for c in updated.contacts)


def test_existing_ids_are_overwritten_not_duplicated(sample_path: Path) -> None:
    project = f136.load(sample_path)
    existing = [c.call_id for c in project.contacts if c.kind == Contact.PRIVATE_CALL]
    users = [user(i, f"X{i % 100000}") for i in existing] + [user(9999999, "OLD"),
                                                             user(9999999, "NEW")]
    updated, report = import_users(project, users, NameFormat.CALLSIGN, NameCodec.BIG5)
    assert report.added == 1 and report.updated == len(existing) and report.unchanged == 0
    assert len(updated.contacts) == len(project.contacts) + 1
    for old, new in zip(project.contacts, updated.contacts):
        assert (new.index, new.kind, new.call_id) == (old.index, old.kind, old.call_id)
        assert new.raw[:3] == old.raw[:3] and new.raw[13:] == old.raw[13:]
        if old.kind == Contact.PRIVATE_CALL:
            assert new.name(NameCodec.BIG5) == f"X{old.call_id % 100000}"
        else:
            assert new.raw == old.raw
    assert updated.contacts[-1].name(NameCodec.BIG5) == "NEW"  # later duplicate wins
    img = updated.image
    assert img[0x1400:0x1400 + 16 * len(updated.contacts)] == b"".join(
        c.raw for c in updated.contacts)


def test_same_name_counts_as_unchanged(small_path: Path) -> None:
    project, _ = import_users(f136.load(small_path), [user(9991234, "QA0BA")],
                              NameFormat.CALLSIGN, NameCodec.ASCII)
    again, report = import_users(project, [user(9991234, "QA0BA")],
                                 NameFormat.CALLSIGN, NameCodec.ASCII)
    assert (report.added, report.updated, report.unchanged) == (0, 0, 1)
    assert again.contacts == project.contacts


def test_capacity_limit(small_path: Path) -> None:
    project = f136.load(small_path)
    room = 3
    users = [user(9991000 + i, f"C{i}") for i in range(5)]
    updated, report = import_users(project, users, NameFormat.CALLSIGN, NameCodec.ASCII,
                                   capacity=len(project.contacts) + room)
    assert (report.added, report.over_capacity) == (room, 2)
    assert len(updated.contacts) == len(project.contacts) + room
    assert "超過容量" in report.summary()


def test_report_lists_shortened_names() -> None:
    _, report = import_users(Project(), [user(9990001, "QA0BD", "Jonathan")],
                             NameFormat.CALLSIGN_NAME, NameCodec.ASCII)
    assert report.shortened == ((9990001, "QA0BD Jonathan", "QA0BD Jona"),)
    assert "截短" in report.summary()


def test_import_into_empty_project_creates_section() -> None:
    updated, report = import_users(Project(), [user(9990001, "QA0BD")],
                                   NameFormat.CALLSIGN, NameCodec.ASCII)
    assert report.added == 1 and updated.section_order == ("Contacts",)
    assert updated.contacts[0].index == 1
    assert f136.serialize(updated).startswith(b"Contacts\t1\t0\t5\t81\t65")  # "QA"


def test_contact_build_validation() -> None:
    field = bytes(10)
    with pytest.raises(ValueError):
        Contact.build(70000, 5, field, 1)
    with pytest.raises(ValueError):
        Contact.build(1, 5, b"short", 1)
    with pytest.raises(ValueError):
        Contact.build(1, 5, field, 0)
    assert Contact.build(1, 7, field, 16777184).kind_label == "無地址呼"


@pytest.mark.parametrize(
    "existing, replace",
    [
        ("QA0BD", True),            # outdated callsign: DMR ID kept, callsign changed
        ("QA0BD James", True),
        ("QA0BD␍␊James", True),     # legacy name with CR/LF
        ("James", True),            # matches RadioID first name
        ("Christ", True),           # truncated first name
        ("9990001", True),          # bare ID
        ("", True),
        ("QA0AKL/P", True),
        ("老王", False),
        ("台中中繼", False),
        ("Dad", False),
        ("TG46001", False),
    ],
)
def test_is_radioid_style_name(existing: str, replace: bool) -> None:
    from opendrs.contact_import import is_radioid_style_name

    u = RadioIdUser(9990001, "QA0AGX", "Christopher" if existing == "Christ" else "James",
                    "", "", "", "Taiwan")
    assert is_radioid_style_name(existing, u) is replace


def test_custom_names_are_kept_and_callsigns_updated() -> None:
    from opendrs.contact_import import NewContact, append_contacts

    base, _ = append_contacts(Project(), [NewContact(Contact.PRIVATE_CALL, "QA0BD", 9990001),
                                          NewContact(Contact.PRIVATE_CALL, "老王", 9990002)],
                              NameCodec.BIG5)
    users = [user(9990001, "QA0AGX"), user(9990002, "QA0AGQ"), user(9990003, "QA0AGP")]
    updated, report = import_users(base, users, NameFormat.CALLSIGN, NameCodec.BIG5)
    names = [c.name(NameCodec.BIG5) for c in updated.contacts]
    assert names == ["QA0AGX", "老王", "QA0AGP"]
    assert (report.added, report.updated, report.unchanged, report.kept) == (1, 1, 0, 1)
    assert "保留原名稱" in report.summary()
    assert updated.contacts[1].raw == base.contacts[1].raw
