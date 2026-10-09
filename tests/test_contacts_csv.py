from __future__ import annotations

from pathlib import Path

import pytest

from opendrs import f136
from opendrs.contact_import import NewContact, append_contacts
from opendrs.contacts_csv import HEADER, export_contacts, parse_contacts_csv
from opendrs.f136 import Project
from opendrs.records import Contact
from opendrs.textcodec import NameCodec


def test_export_is_excel_friendly_utf8(small_path: Path) -> None:
    project = f136.load(small_path)
    data = export_contacts(project, NameCodec.BIG5)
    assert data.startswith(b"\xef\xbb\xbf")
    text = data.decode("utf-8-sig")
    lines = text.split("\r\n")
    assert lines[0] == ",".join(HEADER)
    assert lines[1] == "1,組呼,TG46001_CN,46001"
    assert len([ln for ln in lines if ln]) == len(project.contacts) + 1


def test_export_then_import_round_trip(sample_path: Path) -> None:
    project = f136.load(sample_path)
    result = parse_contacts_csv(export_contacts(project, NameCodec.BIG5))
    assert result.errors == ()
    assert [(e.kind, e.call_id) for e in result.entries] == [
        (c.kind, c.call_id) for c in project.contacts]
    _, report = append_contacts(project, list(result.entries), NameCodec.BIG5)
    assert report.added == 0 and report.updated + report.unchanged == len(project.contacts)
    assert report.unchanged > 0


@pytest.mark.parametrize("encoding", ["utf-8", "utf-8-sig", "big5"])
def test_import_chinese_headers_and_legacy_encodings(encoding: str) -> None:
    text = "編號,類型,名稱,呼叫ID\r\n1,個呼,王小明,9991234\r\n2,組呼,台灣,46600\r\n,,,\r\n"
    result = parse_contacts_csv(text.encode(encoding))
    assert result.errors == ()
    assert result.entries == (NewContact(5, "王小明", 9991234), NewContact(4, "台灣", 46600))


def test_import_gbk_and_minimal_columns() -> None:
    data = "名称,号码\r\n张伟,4600001\r\n".encode("gbk")
    result = parse_contacts_csv(data)
    assert result.encoding == "gbk"
    assert result.entries == (NewContact(5, "张伟", 4600001),)  # type defaults to private


@pytest.mark.parametrize("data, error", [
    (b"", "空"),
    (b"Foo,Bar\r\n1,2\r\n", "缺少欄位"),
])
def test_import_unusable_files(data: bytes, error: str) -> None:
    result = parse_contacts_csv(data)
    assert result.entries == () and error in result.errors[0]


def test_import_reports_bad_rows_and_keeps_good_ones() -> None:
    data = (b"Name,ID,CallType\r\n"
            b"OK,9990001,private\r\n"
            b"BadId,abc,\r\n"
            b"TooBig,99999999,\r\n"
            b"BadType,9990002,weird\r\n"
            b",9990003,All Call\r\n")
    result = parse_contacts_csv(data)
    assert [e.text for e in result.entries] == ["OK", "9990003"]
    assert result.entries[1].kind == 6
    assert len(result.errors) == 3
    assert any("第 3 列" in e for e in result.errors)


def test_append_keeps_existing_indices_and_mixed_types(small_path: Path) -> None:
    project = f136.load(small_path)
    n = len(project.contacts)
    entries = [NewContact(4, "TG99", 99), NewContact(5, "QA0BD", 9990001)]
    updated, report = append_contacts(project, entries, NameCodec.BIG5)
    assert updated.contacts[:n] == project.contacts
    assert [(c.index, c.kind) for c in updated.contacts[n:]] == [(n + 1, 4), (n + 2, 5)]
    assert report.added == 2


def test_same_id_different_type_is_not_a_duplicate() -> None:
    base, _ = append_contacts(Project(), [NewContact(4, "TG", 9990001)], NameCodec.ASCII)
    updated, report = append_contacts(base, [NewContact(5, "P", 9990001)], NameCodec.ASCII)
    assert report.added == 1
    assert [c.kind for c in updated.contacts] == [Contact.GROUP_CALL, Contact.PRIVATE_CALL]
