"""Export / import the contact list as CSV.

Exports are UTF-8 with a BOM and CRLF line ends, which Excel opens without
mojibake. Imports accept UTF-8 (with or without BOM) and legacy Big5 / GBK
files, English or Chinese headers, and call types given as a number or a label.
"""

from __future__ import annotations

import csv
import io
from dataclasses import dataclass

from .contact_import import NewContact
from .f136 import Project
from .records import Contact
from .textcodec import NameCodec, read_legacy_text

HEADER = ("No", "CallType", "Name", "ID")

_COLUMN_ALIASES = {
    "no": "no", "編號": "no", "编号": "no", "序號": "no",
    "calltype": "type", "type": "type", "類型": "type", "类型": "type",
    "呼叫類型": "type", "呼叫类型": "type",
    "name": "name", "名稱": "name", "名称": "name", "姓名": "name",
    "id": "id", "callid": "id", "呼叫id": "id", "號碼": "id", "号码": "id",
}
_TYPE_ALIASES = {
    "4": 4, "組呼": 4, "组呼": 4, "group": 4, "group call": 4, "tg": 4,
    "5": 5, "個呼": 5, "个呼": 5, "private": 5, "private call": 5,
    "6": 6, "全呼": 6, "all": 6, "all call": 6,
    "7": 7, "無地址呼": 7, "无地址呼": 7,
}


def export_contacts(project: Project, codec: NameCodec) -> bytes:
    buf = io.StringIO(newline="")
    writer = csv.writer(buf, lineterminator="\r\n")
    writer.writerow(HEADER)
    for c in project.contacts:
        writer.writerow((c.index, c.kind_label, c.name(codec), c.call_id))
    return buf.getvalue().encode("utf-8-sig")


@dataclass(frozen=True)
class CsvParseResult:
    entries: tuple[NewContact, ...]
    errors: tuple[str, ...]
    encoding: str


def parse_contacts_csv(data: bytes) -> CsvParseResult:
    text, encoding = read_legacy_text(data)
    rows = list(csv.reader(io.StringIO(text, newline="")))
    if not rows:
        return CsvParseResult((), ("檔案是空的",), encoding)
    columns = {_COLUMN_ALIASES.get(h.strip().casefold().replace(" ", "")): i
               for i, h in enumerate(rows[0])}
    columns.pop(None, None)
    missing = [c for c in ("name", "id") if c not in columns]
    if missing:
        return CsvParseResult((), (f"缺少欄位：{'、'.join(missing)}（需要 Name 與 ID）",), encoding)
    entries: list[NewContact] = []
    errors: list[str] = []
    for line_no, row in enumerate(rows[1:], start=2):
        if not any(cell.strip() for cell in row):
            continue

        def cell(key: str) -> str:
            i = columns.get(key)
            return row[i].strip() if i is not None and i < len(row) else ""

        try:
            call_id = int(cell("id"))
        except ValueError:
            errors.append(f"第 {line_no} 列：ID「{cell('id')}」不是數字")
            continue
        if not 1 <= call_id <= Contact.MAX_ID:
            errors.append(f"第 {line_no} 列：ID {call_id} 超出範圍")
            continue
        raw_type = cell("type").casefold()
        kind = _TYPE_ALIASES.get(raw_type, None) if raw_type else Contact.PRIVATE_CALL
        if kind is None:
            errors.append(f"第 {line_no} 列：無法辨識的類型「{cell('type')}」")
            continue
        name = cell("name") or str(call_id)
        entries.append(NewContact(kind, name, call_id))
    return CsvParseResult(tuple(entries), tuple(errors), encoding)
