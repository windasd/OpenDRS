"""Export what the Python model-editing modules compute, as the oracle for the Rust port's stage 2.

Covers channel CSV export/import, zone/channel layout edits, contact deletion, the RX group, scan,
alarm, encryption, GPS and quick-message lists, radio settings, name clean-up and project diffs.
Each case is (op, args, out); the Rust test (opendrs-core/tests/parity_model.rs) runs the same op
with the same args and compares.

Run from the repository root with the Python package importable:
    PYTHONPATH=../opendrs-oracle/src python rust/tools/make_parity_model.py > rust/opendrs-core/tests/parity_model.json
"""

from __future__ import annotations

import csv
import hashlib
import io
import json
import sys
from collections.abc import Callable
from pathlib import Path

from opendrs import (
    alarm_enc,
    channels_csv,
    cleanup,
    contact_edit,
    diff,
    f136,
    layout,
    quickmsg,
    rxgroup,
    scanlist,
)
from opendrs import radio_settings as rs
from opendrs.textcodec import NameCodec

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "rust" / "fixtures"
FIXTURES = ["dh98vu_20250811.F136", "mmdvm_20241124.F136", "mmdvm_20241127.F136", "capture/written_project.F136"]
CODEC = NameCodec.BIG5
I64_MAX = 2**63 - 1
# Over the csv module's 131072-character field limit (built the same way by the Rust test).
HUGE_CSV = "﻿分區,頻道,名稱\r\n1,1,".encode() + b"x" * 200_000 + b"\r\n"


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def psha(project: f136.Project) -> str:
    return sha(f136.serialize(project))


def guarded(fn: Callable[[], object]) -> object:
    try:
        return fn()
    except ValueError as exc:  # every module error (and NameEncodingError) is a ValueError
        return {"error": str(exc)}


def csv_bytes(rows: list[list[str]], encoding: str = "utf-8-sig", delimiter: str = ",") -> bytes:
    buf = io.StringIO(newline="")
    csv.writer(buf, lineterminator="\r\n", delimiter=delimiter).writerows(rows)
    return buf.getvalue().encode(encoding)


# ---------------------------------------------------------------- results
def edit_out(edit: scanlist.ScanEdit) -> list[int]:
    return [edit.removed, edit.renumbered, edit.refs_changed]


def row_out(project: f136.Project, row: int) -> dict:
    return {"sha": psha(project), "row": row}


def edited_out(project: f136.Project, edit: scanlist.ScanEdit) -> dict:
    return {"sha": psha(project), "edit": edit_out(edit)}


def moved_out(project: f136.Project, row: int, edit: scanlist.ScanEdit) -> dict:
    return {"sha": psha(project), "row": row, "edit": edit_out(edit)}


def rows_out(project: f136.Project, rows: range) -> dict:
    return {"sha": psha(project), "rows": [rows.start, rows.stop]}


def deletion_out(project: f136.Project, deletion: contact_edit.ContactDeletion) -> dict:
    return {"sha": psha(project),
            "deletion": [deletion.removed, deletion.group_members_dropped, deletion.renumbered]}


def export_out(data: bytes) -> dict:
    return {"sha": sha(data), "text": data.decode("utf-8-sig")}


def import_out(result: channels_csv.ChannelImport) -> dict:
    return {"sha": psha(result.project), "updated": result.updated, "added": result.added,
            "unchanged": result.unchanged, "errors": list(result.errors), "encoding": result.encoding,
            "summary": result.summary()}


# ---------------------------------------------------------------- ops
def op_layout(p: f136.Project, op: str, args: list) -> object:
    actions: dict[str, Callable[[], object]] = {
        "zone_rows": lambda: layout.zone_rows(p),
        "check_layout": lambda: layout.check_layout(p),
        "channel_position": lambda: list(layout.channel_position(p, *args)),
        "add_channel": lambda: row_out(*layout.add_channel(p, *args)),
        "delete_channel": lambda: edited_out(*layout.delete_channel(p, *args)),
        "add_zone": lambda: row_out(*layout.add_zone(p, args[0], args[1], CODEC)),
        "delete_zone": lambda: edited_out(*layout.delete_zone(p, *args)),
        "move_channel": lambda: moved_out(*layout.move_channel(p, *args)),
        "shift_channel": lambda: row_out(*layout.shift_channel(p, *args)),
        "add_channels": lambda: rows_out(*layout.add_channels(p, *args, codec=CODEC)),
    }
    return guarded(actions[op])


def op_contacts(p: f136.Project, op: str, args: list) -> object:
    actions: dict[str, Callable[[], object]] = {
        "delete_contacts": lambda: deletion_out(*contact_edit.delete_contacts(p, args[0], CODEC)),
        "shift_contact": lambda: row_out(*contact_edit.shift_contact(p, *args)),
        "channels_calling": lambda: contact_edit.channels_calling(p, set(args[0]), CODEC),
        "add_contact": lambda: row_out(*contact_edit.add_contact(p, args[0], args[1], args[2], CODEC, *args[3:])),
    }
    return guarded(actions[op])


def alarm_fields(fields: dict) -> dict:
    return {key: tuple(value) if key == "channel" else value for key, value in fields.items()}


def op_lists(p: f136.Project, op: str, args: list) -> object:
    img = p.image
    actions: dict[str, Callable[[], object]] = {
        "rx_groups": lambda: [[g.name(CODEC), list(g.members), g.in_use] for g in rxgroup.rx_groups(img)],
        "with_rx_group": lambda: sha(rxgroup.with_rx_group(img, args[0], args[1], args[2], CODEC)),
        "groups_containing": lambda: rxgroup.groups_containing(img, args[0]),
        "scan_lists": lambda: [[s.name(CODEC), [list(e) for e in s.entries], s.talkback, s.tx_mode,
                                list(s.designated), s.in_use] for s in scanlist.scan_lists(img)],
        "with_scan_list": lambda: sha(scanlist.with_scan_list(img, args[0], args[1], [tuple(e) for e in args[2]],
                                                              CODEC)),
        "with_scan_settings": lambda: sha(scanlist.with_scan_settings(img, args[0], args[1], args[2],
                                                                      tuple(args[3]))),
        "references": lambda: scanlist.references(img, *args),
        "alarm_lists": lambda: [[a.name(CODEC), a.alarm_type, a.mode, list(a.channel), a.impolite_attempts,
                                 a.mic_seconds] for a in alarm_enc.alarm_lists(img)],
        "with_alarm": lambda: sha(alarm_enc.with_alarm(img, args[0], codec=CODEC, **alarm_fields(args[1]))),
        "encryption_keys": lambda: [[k.alias(CODEC), k.key.hex(), k.in_use] for k in alarm_enc.encryption_keys(img)],
        "parse_key_hex": lambda: alarm_enc.parse_key_hex(args[0]).hex(),
        "with_encryption_key": lambda: sha(alarm_enc.with_encryption_key(img, args[0], args[1],
                                                                         bytes.fromhex(args[2]), CODEC)),
        "gps_channel": lambda: list(alarm_enc.gps_channel(img)),
        "with_gps_channel": lambda: sha(alarm_enc.with_gps_channel(img, tuple(args[0]))),
        "quick_messages": lambda: quickmsg.quick_messages(img, CODEC),
        "with_quick_message": lambda: sha(quickmsg.with_quick_message(img, args[0], args[1], CODEC)),
    }
    return guarded(actions[op])


def op_settings(p: f136.Project, op: str, args: list) -> object:
    img = p.image
    actions: dict[str, Callable[[], object]] = {
        "settings": lambda: [[s.key, s.display(img) if isinstance(s, rs.Setting) else s.text(img)]
                             for s in rs.SETTINGS],
        "with_raw": lambda: sha(rs.by_key(args[0]).with_raw(img, args[1])),
        "with_value": lambda: sha(rs.by_key(args[0]).with_value(img, args[1])),
        "with_text": lambda: sha(rs.by_key(args[0]).with_text(img, args[1])),
    }
    return guarded(actions[op])


def op_misc(p: f136.Project, op: str, args: list) -> object:
    def cleanup_out() -> dict:
        report = cleanup.scan(p)
        cleaned, again = cleanup.clean_names(p)
        return {"report": [report.contacts, report.zones, report.channels], "total": report.total,
                "summary": report.summary(), "sha": psha(cleaned),
                "cleaned": [again.contacts, again.zones, again.channels]}

    actions: dict[str, Callable[[], object]] = {
        "cleanup": cleanup_out,
        "export_csv": lambda: export_out(channels_csv.export_channels(p, CODEC)),
        "import_csv": lambda: import_out(channels_csv.import_channels(p, bytes.fromhex(args[0]), CODEC)),
        "import_csv_huge": lambda: import_out(channels_csv.import_channels(p, HUGE_CSV, CODEC)),
    }
    return guarded(actions[op])


GROUPS: tuple[Callable[[f136.Project, str, list], object], ...] = (
    op_layout, op_contacts, op_lists, op_settings, op_misc)


def run(p: f136.Project, op: str, args: list) -> object:
    """The op's result; each group raises KeyError(op) for the ops it does not have."""
    for group in GROUPS:
        try:
            return group(p, op, args)
        except KeyError as exc:
            if exc.args != (op,):
                raise
    raise KeyError(op)


# ---------------------------------------------------------------- cases
def layout_cases(p: f136.Project) -> list[tuple[str, list]]:
    last = len(p.channels) - 1
    zones = len(p.zones)
    cases: list[tuple[str, list]] = [("zone_rows", []), ("check_layout", [])]
    cases += [("channel_position", [r]) for r in range(len(p.channels) + 1)]
    cases += [("add_channel", [0]), ("add_channel", [last]), ("add_channel", [last + 1])]
    cases += [("delete_channel", [r]) for r in (0, 1, last, last + 1)]
    cases += [("add_zone", [0, "新分區"]), ("add_zone", [last, "ZONE_X"]), ("add_zone", [last + 1, "x"]),
              ("add_zone", [0, "名稱超過十個位元組的分區"])]
    cases += [("delete_zone", [z]) for z in (0, zones - 1, zones)]
    cases += [("move_channel", [0, zones - 1]), ("move_channel", [last, 0]), ("move_channel", [0, 0]),
              ("move_channel", [1, zones]), ("move_channel", [last + 1, 0])]
    cases += [("shift_channel", [0, 1]), ("shift_channel", [0, -1]), ("shift_channel", [1, -1]),
              ("shift_channel", [last, 1]), ("shift_channel", [0, 2])]
    cases += [("add_channels", [0, 3, 439_000_000, 12_500, -5_000_000, "批次", 1]),
              ("add_channels", [last, 2, 145_500_000, 25_000, 0, "很長的名稱前綴", 98]),
              ("add_channels", [0, 0, 439_000_000, 12_500, 0, "X", 1]),
              ("add_channels", [0, 65, 439_000_000, 12_500, 0, "X", 1]),
              ("add_channels", [0, 2, 4_294_967_295, 1, 0, "X", 1]),
              ("add_channels", [0, 1, 1_000_000, 0, -1_000_000, "X", 1]),
              # past 64 bits in the Rust port's arithmetic (Python's integers have no limit)
              ("add_channels", [0, 1, I64_MAX, 0, 1, "X", 1]), ("add_channels", [0, 1, 439_000_000, 0, I64_MAX, "X", 1]),
              ("add_channels", [0, 2, 439_000_000, I64_MAX, 0, "X", 1]),
              ("add_channels", [0, 2, 439_000_000, 12_500, 0, "", I64_MAX]),
              ("add_channels", [0, 2, 439_000_000, 12_500, 0, "N", -I64_MAX - 1])]
    return cases


def contact_cases(p: f136.Project) -> list[tuple[str, list]]:
    used = {ch.tx_contact for ch in p.channels}
    unused = [r for r, c in enumerate(p.contacts) if c.index not in used]
    called = [r for r, c in enumerate(p.contacts) if c.index in used]
    n = len(p.contacts)
    cases: list[tuple[str, list]] = [("delete_contacts", [unused[:1]]), ("delete_contacts", [unused[:5]]),
                                     ("delete_contacts", [[]]), ("delete_contacts", [[n]])]
    if called:
        cases.append(("delete_contacts", [called[:2] + unused[:1]]))
    cases += [("shift_contact", [0, 1]), ("shift_contact", [0, -1]), ("shift_contact", [n - 1, 1]),
              ("shift_contact", [1, -1]), ("shift_contact", [n, 1]), ("shift_contact", [0, 3])]
    first_contact = p.channels[0].tx_contact if p.channels else 0
    cases.append(("channels_calling", [[first_contact, 9999]]))
    cases += [("add_contact", [4, "NEWTG", 9_990_001]), ("add_contact", [5, "  Bob  ", 9_990_002]),
              ("add_contact", [6, "中文名稱", 16_777_215]), ("add_contact", [7, "X", 1]),
              ("add_contact", [4, "名稱超過十個位元組", 9_990_003]), ("add_contact", [4, "", 5]),
              ("add_contact", [4, "\u3000", 5]), ("add_contact", [4, "A\nB", 5]),
              ("add_contact", [4, "A\x7fB", 5]), ("add_contact", [3, "X", 5]), ("add_contact", [8, "X", 5]),
              ("add_contact", [4, "X", 0]), ("add_contact", [4, "X", 16_777_216]),
              ("add_contact", [4, "\U0001F600", 5]), ("add_contact", [4, "X", 123, n]),
              ("add_contact", [4, "X", 123, n + 1])]
    if p.contacts:
        cases.append(("add_contact", [p.contacts[0].kind, "DUP", p.contacts[0].call_id]))
    return cases


def list_cases() -> list[tuple[str, list]]:
    keys = ("12 34 56 78 9A BC", "123456789abc", "12-34-56-78-9a-bc", "", "1234", "1234567890ABCD", "zz",
            " 0102 0304 0506 ", "１２３４５６７８９０ab")
    return [
        ("rx_groups", []), ("with_rx_group", [1, "群組Ａ", [1, 2, 3]]), ("with_rx_group", [32, "", []]),
        ("with_rx_group", [0, "X", [1]]), ("with_rx_group", [33, "X", [1]]),
        ("with_rx_group", [2, "X", list(range(1, 40))]), ("with_rx_group", [2, "很長很長很長的名稱", [1]]),
        ("groups_containing", [1]), ("groups_containing", [2]),
        ("scan_lists", []), ("with_scan_list", [1, "掃描", [[1, 1], [1, 2]]]), ("with_scan_list", [16, "", []]),
        ("with_scan_list", [17, "X", []]), ("with_scan_list", [1, "X", [[1, i] for i in range(1, 20)]]),
        ("with_scan_settings", [1, True, 2, [1, 3]]), ("with_scan_settings", [1, False, 0, [0, 0]]),
        ("with_scan_settings", [1, False, 5, [0, 0]]), ("with_scan_settings", [0, False, 0, [0, 0]]),
        ("references", [1, None]), ("references", [1, 1]), ("references", [2, 1]),
        ("alarm_lists", []),
        ("with_alarm", [1, {"name": "警報", "alarm_type": 2, "mode": 1, "channel": [1, 2],
                            "impolite_attempts": 3, "mic_seconds": 10}]),
        ("with_alarm", [4, {"name": "X"}]), ("with_alarm", [1, {"mic_seconds": 151}]),
        ("with_alarm", [1, {"alarm_type": 9}]), ("with_alarm", [5, {"name": "X"}]),
        ("with_alarm", [1, {"mode": 7}]), ("with_alarm", [1, {"channel": [0, 0]}]),
        ("encryption_keys", []),
        *[("parse_key_hex", [text]) for text in keys],
        ("with_encryption_key", [1, "密鑰", "0102030405ab"]), ("with_encryption_key", [10, "", "000000000000"]),
        ("with_encryption_key", [11, "X", "0102030405ab"]), ("with_encryption_key", [1, "X", "0102"]),
        ("gps_channel", []), ("with_gps_channel", [[1, 2]]), ("with_gps_channel", [[0, 0]]),
        ("quick_messages", []), ("with_quick_message", [1, "測試訊息"]), ("with_quick_message", [16, ""]),
        ("with_quick_message", [17, "X"]), ("with_quick_message", [2, "長" * 30]),
    ]


def settings_cases() -> list[tuple[str, list]]:
    cases: list[tuple[str, list]] = [("settings", [])]
    for s in rs.SETTINGS:
        if isinstance(s, rs.TextSetting):
            cases += [("with_text", [s.key, " 123*# "]), ("with_text", [s.key, "中"]),
                      ("with_text", [s.key, "9" * (s.size + 1)]), ("with_text", [s.key, ""])]
            continue
        raws = [v for _, v in s.choices] or [s.minimum // s.scale, s.maximum // s.scale]
        cases += [("with_raw", [s.key, raws[-1]]), ("with_raw", [s.key, raws[0]]),
                  ("with_raw", [s.key, -1]), ("with_raw", [s.key, (s.mask >> s.shift) + 1])]
        if not s.choices:
            cases += [("with_value", [s.key, s.maximum]), ("with_value", [s.key, s.maximum + s.scale]),
                      ("with_value", [s.key, s.minimum + 1])]
    return cases


def edited_rows(p: f136.Project, rows: list[list[str]]) -> list[bytes]:
    """An edit of channel 4 by label, name and number, and a file with several bad rows."""
    head = rows[0]
    col = head.index
    edited = list(rows[4])
    for column, value in (("名稱", "TN6A_NEW"), ("接收MHz", "431.6"), ("功率", "低"), ("時隙", "TS1"),
                          ("發話對象", p.contacts[0].name(CODEC) if p.contacts else ""), ("接收亞音", "88.5"),
                          ("發射亞音", "D023N")):
        edited[col(column)] = value
    bad = list(rows[2])
    bad[col("頻道")] = "1"
    bad[col("接收MHz")] = "900"
    ref = list(rows[3])
    ref[col("發話對象")] = "999"
    return [csv_bytes([head, edited]), csv_bytes([head, rows[2], bad, ["99", "", "1", "X"], ref, ["1", "", "", ""]])]


def import_cases(p: f136.Project) -> list[tuple[str, list]]:
    exported = channels_csv.export_channels(p, CODEC)
    rows = list(csv.reader(io.StringIO(exported.decode("utf-8-sig"), newline="")))
    zone2 = "2" if len(p.zones) > 1 else "1"
    scenarios: list[bytes] = [exported, b"", csv_bytes([["名稱"], ["x"]])]
    if len(rows) > 4:
        scenarios += edited_rows(p, rows)
    scenarios += [
        csv_bytes([["Zone", "Ch", "Name", "RX (MHz)", "TX (MHz)", "Mode"],
                   [zone2, "", "NEW_ANALOG", "145.5", "145.5", "analog"]]),
        csv_bytes([["分區", "頻道", "名稱"], ["1", "1", "測試頻道"]], "big5"),
        csv_bytes([["分區", "頻道", "名稱"], ["1", "1", "測試頻道"]], "utf-16"),
        csv_bytes([["分區", "頻道", "名稱"], ["1", "9", "SECOND"], ["1", "8", "FIRST"]]),
        csv_bytes([["分區", "頻道", "名稱"], ["1", "1", "AAA"], ["1", "1", "BBB"]]),
        csv_bytes([["分區", "頻道", "發話對象"], ["1", "1", "13 WRONG_NAME"], ["1", "2", "2"], ["1", "3", "#2"]]),
        csv_bytes([["分區", "頻道", "名稱", "接收MHz", "接收亞音"], ["1", "1", "X", "inf", "88.5"],
                   ["1", "2", "X", "nan", "88.5"], ["1", "3", "X", "0", "88.5"], ["1", "4", "X", "439.5", "snan"],
                   ["1", "²", "X", "439.5", "88.5"], ["²", "1", "X", "439.5", "88.5"],
                   ["1", "5", "X", "439.50000001", "D999"], ["1", "6", "X", "1E999999999", "67"]]),
        csv_bytes([["分區", "分區名稱", "頻道", "名稱"], ["4", "MMDVM", "1", "MOVED"], ["1", "MMDVM", "1", "OK"]]),
        csv_bytes([["分區", "頻道", "名稱", "接收MHz", "發射MHz"], [zone2, "", "NEW", "145.5", "145.5"],
                   [zone2, "", "NEW", "145.5", "145.5"]]),
        "分區;頻道;接收MHz\r\n1;1;439.5 MHz\r\n".encode("utf-8-sig"),
        csv_bytes([["分區", "頻道", "接收MHz"], ["1", "2", "439.5"]], delimiter="\t"),
        csv_bytes([["分區", "頻道", "色碼", "接收MHz", "接收亞音", "發話對象", "模式", "時隙"],
                   ["１", "２", "１５", "４３９.５", "８８.５",
                    "\x1c１\x1f", "DMR", "slot2"],
                   ["1", "3", "16", "439.5", "D023I", "無", "類比+數位（發射數位）", "On"],
                   ["1", "4", "²", "439.5mhz", "OFF", "none", "fm", "時隙3"]]),
        csv_bytes([["分區", "頻道", "名稱", "掃描列表", "緊急系統", "加密列表", "接收群組"],
                   ["1", "1", "A\nB", "1", "0", "無", "1"], ["1", "2", "　Ｂ　", "17", "5", "11", "33"],
                   ["1", "3", "C", "", "", "", ""], [], ["", "", "", ""]]),
        csv_bytes([["zone", "ch", "NAME", "power", "callindicator", "privatecalltype", "messagetype", "admit"],
                   ["1", "1", "PW", "高", "開", "x", "y", "z"]]),
        '分區,頻道,名稱\r\n1,1,"unterminated\r\n'.encode(),
        b"\xff\xfe\x00\x00garbage",
    ]
    return [("import_csv", [data.hex()]) for data in scenarios] + [("import_csv_huge", [])]


def fixture_cases(p: f136.Project) -> list[dict]:
    cases = [("export_csv", []), ("cleanup", []), *layout_cases(p), *contact_cases(p), *list_cases(),
             *settings_cases(), *import_cases(p)]
    return [{"op": op, "args": args, "out": run(p, op, args)} for op, args in cases]


def diff_cases(projects: dict[str, f136.Project]) -> list[dict]:
    dh98 = projects["dh98vu_20250811.F136"]
    change = csv_bytes([["分區", "頻道", "名稱", "接收MHz"], ["1", "1", "CHANGED", "431.5"]])
    edited = channels_csv.import_channels(dh98, change, CODEC).project
    smaller, _ = layout.delete_zone(dh98, 0)
    renamed = dh98.with_contact(0, dh98.contacts[0].with_name("RENAMED", CODEC))
    pairs = [("mmdvm_20241124.F136", "mmdvm_20241127.F136", projects["mmdvm_20241124.F136"],
              projects["mmdvm_20241127.F136"]),
             ("dh98", "dh98+edit", dh98, edited), ("dh98", "dh98-zone1", dh98, smaller),
             ("dh98-zone1", "dh98", smaller, dh98), ("dh98", "dh98", dh98, dh98), ("dh98", "renamed", dh98, renamed)]
    out = []
    for a_name, b_name, a, b in pairs:
        ranges = [[r.start, r.old.hex(), r.new.hex()] for r in diff.byte_ranges(a.image, b.image)]
        out.append({"old": a_name, "new": b_name, "old_sha": psha(a), "new_sha": psha(b),
                    "lines": diff.diff_projects(a, b, CODEC), "image_ranges": ranges})
    return out


def main() -> None:
    projects = {name: f136.load(FIX / name) for name in FIXTURES}
    fixtures = {name: fixture_cases(p) for name, p in projects.items()}
    json.dump({"fixtures": fixtures, "diffs": diff_cases(projects)}, sys.stdout, ensure_ascii=False, indent=0)


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    main()
