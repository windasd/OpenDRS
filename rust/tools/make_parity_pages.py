"""Export what the Python GUI's settings-area pages show and do, as the oracle for the egui port.

Builds the real widgets (gui/scan_page.py, group_page.py, settings_page.py, quickmsg_page.py,
alarm_page.py) offscreen for each fixture, plus patched and truncated variants, and records
their texts, choices, enabled states and the member editors' initial state; then applies edits
through gui/list_ops.py and records the undo label and resulting image (or the warning).

Run from the repository root (needs PySide6):
    PYTHONPATH=src python rust/tools/make_parity_pages.py > rust/opendrs-gui/tests/parity_pages.json
"""

from __future__ import annotations

import hashlib
import json
import os
import struct
import sys
from dataclasses import replace
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import Qt  # noqa: E402
from PySide6.QtWidgets import QApplication, QComboBox, QLineEdit, QMessageBox, QSpinBox  # noqa: E402

from opendrs import f136  # noqa: E402
from opendrs.alarm_enc import ENC_KEYS_OFFSET, ENC_KEY_SIZE, GPS_CHANNEL_OFFSET, GPS_ZONE_OFFSET, alarm_base  # noqa: E402
from opendrs.gui.alarm_page import AlarmEncryptionPage, channel_choices  # noqa: E402
from opendrs.gui.group_page import RxGroupDialog, RxGroupPage, member_labels  # noqa: E402
from opendrs.gui.list_ops import ListOpsMixin  # noqa: E402
from opendrs.gui.quickmsg_page import QuickMessagePage  # noqa: E402
from opendrs.gui.scan_page import ScanListDialog, ScanListPage, channel_entries, entry_label  # noqa: E402
from opendrs.gui.settings_page import SettingsPage  # noqa: E402
from opendrs.quickmsg import with_quick_message  # noqa: E402
from opendrs.radio_settings import TextSetting, by_key, by_page, pages  # noqa: E402
from opendrs.rxgroup import RX_GROUP_SIZE, RX_GROUPS_OFFSET, rx_groups  # noqa: E402
from opendrs.scanlist import SCAN_LIST_SIZE, SCAN_LISTS_OFFSET, scan_lists  # noqa: E402
from opendrs.textcodec import NameCodec  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "tests" / "fixtures"
FIXTURES = ["dh98vu_20250811.F136", "mmdvm_20241124.F136", "mmdvm_20241127.F136", "capture/written_project.F136"]
CODEC = NameCodec.BIG5
SEARCHES = ["", "tg466", "frs 1", "ＴＧ", "zzz"]


def sha(image: bytes) -> str:
    return hashlib.sha256(image).hexdigest()


def force_raw(image: bytearray, key: str, raw: int) -> None:
    """Store ``raw`` in a setting's bits even when it is not a valid choice."""
    s = by_key(key)
    word = int.from_bytes(image[s.offset:s.offset + s.size], "little")
    word = (word & ~s.mask) | ((raw << (s.mask & -s.mask).bit_length() - 1) & s.mask)
    image[s.offset:s.offset + s.size] = word.to_bytes(s.size, "little")


def patched(project: f136.Project) -> f136.Project:
    """Values the fixtures lack: missing channels, stale references, unknown settings."""
    img = bytearray(project.image)
    scan3 = SCAN_LISTS_OFFSET + 2 * SCAN_LIST_SIZE
    img[scan3:scan3 + 10] = b"ZZ".ljust(10, b"\0")
    img[scan3 + 0x0B] = 0x2C  # talkback on, unknown TX mode 3
    struct.pack_into("<HH", img, scan3 + 0x0C, 1, 2)
    struct.pack_into("<8H", img, scan3 + 0x10, 9, 1, 1, 99, 1, 1, 4, 2)
    scan4 = SCAN_LISTS_OFFSET + 3 * SCAN_LIST_SIZE
    img[scan4:scan4 + 10] = b"Y".ljust(10, b"\0")
    img[scan4 + 0x0B] = 0x08  # designated-channel mode with a stale target
    struct.pack_into("<HH", img, scan4 + 0x0C, 2, 99)
    group2 = RX_GROUPS_OFFSET + RX_GROUP_SIZE
    img[group2:group2 + 10] = b"G2".ljust(10, b"\0")
    struct.pack_into("<3H", img, group2 + 10, 1, 9999, 3)
    struct.pack_into("<H", img, GPS_ZONE_OFFSET, 2)
    struct.pack_into("<H", img, GPS_CHANNEL_OFFSET, 99)
    struct.pack_into("<HH", img, alarm_base(1) + 0x0C, 1, 1)
    struct.pack_into("<HH", img, alarm_base(2) + 0x0C, 3, 50)
    img[alarm_base(2) + 0x0A] = (img[alarm_base(2) + 0x0A] & 0xF0) | 7
    img[alarm_base(3) + 0x10] &= 0xF0
    img[alarm_base(3) + 0x11] = 200
    key2 = ENC_KEYS_OFFSET + ENC_KEY_SIZE
    img[key2:key2 + 16] = b"K2".ljust(10, b"\0") + bytes.fromhex("0a0b0c0d0e0f")
    force_raw(img, "tot", 200)
    force_raw(img, "vox_level", 15)
    force_raw(img, "power_save", 0)
    force_raw(img, "tone_key", 1)
    force_raw(img, "tone_low_battery", 0)
    dtmf = by_key("dtmf_code")
    img[dtmf.offset:dtmf.offset + dtmf.size] = b"12#".ljust(dtmf.size, b"\0")
    image = with_quick_message(bytes(img), 2, "測試訊息", CODEC)
    return replace(project, image=image)


def variants() -> list[tuple[str, str, f136.Project]]:
    out = []
    for name in FIXTURES:
        out.append((name, "plain", f136.load(FIX / name)))
    base = f136.load(FIX / FIXTURES[0])
    out.append((FIXTURES[0], "patched", patched(base)))
    for size in (0x1000, 0x700, 0x200):
        out.append((FIXTURES[0], f"short_{size:#x}", replace(base, image=base.image[:size])))
    return out


def diff(old: bytes, new: bytes) -> list[list]:
    """The new image as patches on the fixture's (same length or truncated)."""
    if len(new) < len(old):
        return [["truncate", len(new)]]
    return [[i, f"{b:02x}"] for i, (a, b) in enumerate(zip(old, new)) if a != b]


def combo(box: QComboBox) -> dict:
    return {"items": [box.itemText(i) for i in range(box.count())], "index": box.currentIndex(),
            "enabled": box.isEnabled()}


def table_rows(table) -> list[list[str]]:  # noqa: ANN001
    return [[table.item(r, c).text() if table.item(r, c) else "" for c in range(table.columnCount())]
            for r in range(table.rowCount())]


def visible(dialog) -> dict:  # noqa: ANN001
    out = {}
    for text in SEARCHES:
        dialog.search.setText(text)
        out[text] = [i for i in range(dialog.items.count()) if not dialog.items.item(i).isHidden()]
    dialog.search.setText("")
    return out


def scan_facts(project: f136.Project) -> dict:
    page = ScanListPage(lambda _n: None)
    in_use = page.set_project(project, CODEC)
    dialogs = []
    for scan in scan_lists(project.image):
        dlg = ScanListDialog(project, scan, CODEC)
        dialogs.append({
            "title": dlg.windowTitle(), "name": dlg.name.text(), "checked": [list(e) for e in dlg.entries()],
            "count": dlg.count.text(), "ok": dlg.buttons.button(dlg.buttons.StandardButton.Ok).isEnabled(),
            "talkback": combo(dlg.talkback), "tx_mode": combo(dlg.tx_mode), "designated": combo(dlg.designated),
            "settings": [dlg.settings()[0], dlg.settings()[1], list(dlg.settings()[2])],
        })
    first = ScanListDialog(project, scan_lists(project.image)[0], CODEC) if scan_lists(project.image) else None
    return {
        "enabled": page.isEnabled(), "in_use": in_use, "rows": table_rows(page.table),
        "grey": [page.table.item(r, 0).foreground().color().name() for r in range(page.table.rowCount())],
        "entries": [list(e) for e in channel_entries(project)],
        "choices": [first.items.item(i).text() for i in range(first.items.count())] if first else [],
        "visible": visible(first) if first else {},
        "dialogs": dialogs,
    }


def entry_labels(project: f136.Project) -> list[list]:
    """entry_label for real, missing-zone and missing-channel entries."""
    zones = len(project.zones)
    probes = [(z, c) for z in range(0, zones + 2) for c in (0, 1, 2, 99)]
    return [[z, c, entry_label(project, (z, c), CODEC)] for z, c in probes if z or c]


def group_facts(project: f136.Project) -> dict:
    page = RxGroupPage(lambda _n: None)
    in_use = page.set_project(project, CODEC)
    dialogs = []
    for group in rx_groups(project.image):
        dlg = RxGroupDialog(project, group, CODEC)
        dialogs.append({"title": dlg.windowTitle(), "name": dlg.name.text(), "checked": dlg.values(),
                        "count": dlg.count.text(),
                        "ok": dlg.buttons.button(dlg.buttons.StandardButton.Ok).isEnabled()})
    first = RxGroupDialog(project, rx_groups(project.image)[0], CODEC) if rx_groups(project.image) else None
    probes = [1, 2, 3, 9999, 0xFFFE]
    return {
        "enabled": page.isEnabled(), "in_use": in_use, "rows": table_rows(page.table),
        "choices": [first.items.item(i).text() for i in range(first.items.count())] if first else [],
        "visible": visible(first) if first else {},
        "member_labels": [[m, member_labels(project, (m,), CODEC)[0]] for m in probes],
        "dialogs": dialogs,
    }


def settings_facts(project: f136.Project) -> dict:
    page = SettingsPage(lambda _s, _v: None)
    page.set_image(project.image)
    out = []
    for name in pages():
        for s in by_page(name):
            editor = page.editor(s.key)
            fact = {"key": s.key, "page": name, "enabled": editor.isEnabled(), "tip": editor.toolTip()}
            if isinstance(editor, QComboBox):
                fact.update(kind="choice", text=editor.currentText(), count=editor.count(),
                            extra=[editor.itemText(i) for i in range(len(s.choices), editor.count())])
            elif isinstance(editor, QSpinBox):
                fact.update(kind="number", value=editor.value(), suffix=editor.suffix())
            elif isinstance(editor, QLineEdit):
                fact.update(kind="text", text=editor.text(), limit=editor.maxLength())
            out.append(fact)
    return {"pages": [page.page_list.item(i).text() for i in range(page.page_list.count())], "settings": out}


def quick_facts(project: f136.Project) -> dict:
    page = QuickMessagePage(lambda _n, _t: None)
    page.set_image(project.image, CODEC)
    return {"enabled": page.isEnabled(), "header": page.table.horizontalHeaderItem(0).text(),
            "texts": [r[0] for r in table_rows(page.table)]}


def alarm_facts(project: f136.Project) -> dict:
    page = AlarmEncryptionPage(lambda *_a: None)
    page.set_project(project, CODEC)
    rows = []
    for r in range(page.alarms.rowCount()):
        cell = page.alarms.cellWidget
        rows.append({
            "name": page.alarms.item(r, 0).text(),
            "type": combo(cell(r, 1)), "mode": combo(cell(r, 2)),
            # The channel lists are long and shared: keep the entries past the shared ones.
            "channel": {"index": cell(r, 3).currentIndex(), "text": cell(r, 3).currentText(),
                        "count": cell(r, 3).count()},
            "attempts": cell(r, 4).value(), "mic": cell(r, 5).value(), "mic_suffix": cell(r, 5).suffix(),
        })
    return {
        "enabled": page.isEnabled(), "rows": rows,
        "headers": [page.alarms.horizontalHeaderItem(c).text() for c in range(page.alarms.columnCount())],
        "channel_choices": [label for label, _ in channel_choices(project, CODEC)],
        "gps": {"index": page.gps.currentIndex(), "text": page.gps.currentText(), "count": page.gps.count()},
        "key_headers": [page.keys.horizontalHeaderItem(c).text() for c in range(page.keys.columnCount())],
        "keys": table_rows(page.keys),
    }


class Host(ListOpsMixin):
    """The parts of MainWindow that list_ops uses."""

    def __init__(self, project: f136.Project) -> None:
        self._project = project
        self.codec = CODEC
        self.result: dict | None = None

    def _commit(self, project: f136.Project, label: str) -> None:
        self.result = {"label": label, "patches": diff(self._project.image, project.image),
                       "sha": sha(project.image)}

    def _refresh(self) -> None:
        pass


def edit_cases(project: f136.Project) -> list[dict]:
    specs: list[tuple[str, list]] = [
        ("scan", [1, " New ", [[4, 1], [4, 2]], [True, 2, [4, 3]]]),
        ("scan", [2, "名稱太長太長太長", [[4, 1]], [False, 0, [0, 0]]]),
        ("scan", [3, "x", [[1, 1]] * 17, [False, 0, [0, 0]]]),
        ("scan", [4, "abc", [], [False, 3, [0, 0]]]),
        ("scan", [16, "Last", [[1, 1]], [False, 1, [261, 6]]]),
        ("scan", [1, "測試", [[1, 2], [1, 1]], [True, 0, [0, 0]]]),
        ("group", [1, " TW ", [2, 10]]),
        ("group", [2, "x", list(range(1, 34))]),
        ("group", [3, "名稱太長太長太長", []]),
        ("group", [32, "", [5]]),
        ("setting", ["tot", 3]), ("setting", ["tot", 255]), ("setting", ["language", 2]),
        ("setting", ["vox_level", 5]), ("setting", ["enc_level", 7]),
        ("setting", ["dtmf_code", " 12AB "]), ("setting", ["dtmf_code", "12E"]),
        ("setting", ["dtmf_code", "123456789012"]),
        ("quick", [1, "Hello"]), ("quick", [2, "測試訊息"]), ("quick", [3, "x" * 41]), ("quick", [16, ""]),
        ("quick", [4, " pad "]),
        ("alarm", ["alarm", 1, "name", "AL1"]), ("alarm", ["alarm", 1, "name", " sp "]),
        ("alarm", ["alarm", 1, "alarm_type", 3]), ("alarm", ["alarm", 1, "mode", 2]),
        ("alarm", ["alarm", 1, "channel", [4, 3]]), ("alarm", ["alarm", 2, "impolite_attempts", 15]),
        ("alarm", ["alarm", 2, "mic_seconds", 150]), ("alarm", ["alarm", 1, "name", "名稱太長太長太長"]),
        ("alarm", ["gps", 0, "channel", [4, 3]]), ("alarm", ["gps", 0, "channel", [0, 0]]),
        ("alarm", ["key", 1, "key", ["K1", "12 34 56 78 9a bc"]]), ("alarm", ["key", 2, "key", ["K2", ""]]),
        ("alarm", ["key", 1, "key", ["", "12 34"]]), ("alarm", ["key", 3, "key", ["K3", "123"]]),
        ("alarm", ["key", 3, "key", ["K3", "12 34 56 78 9A BC DE"]]), ("alarm", ["key", 4, "key", ["  ", "zz"]]),
        ("alarm", ["key", 5, "key", [" K5 ", "123456789abc"]]),
    ]
    out = []
    warned: list = []
    QMessageBox.warning = staticmethod(lambda _parent, title, text: warned.append([title, text]))
    for kind, args in specs:
        host = Host(project)
        warned.clear()
        if kind == "scan":
            n, name, entries, (talkback, mode, designated) = args
            host.apply_scan_list(n, name, [tuple(e) for e in entries], (talkback, mode, tuple(designated)))
        elif kind == "group":
            host.apply_rx_group(*args)
        elif kind == "setting":
            host.apply_setting(by_key(args[0]), args[1])
        elif kind == "quick":
            host.apply_quick_message(*args)
        else:
            what, n, field, value = args
            value = tuple(value) if isinstance(value, list) else value
            host.apply_alarm_edit(what, n, field, value)
        out.append({"kind": kind, "args": args, "result": host.result,
                    "warning": warned[0] if warned else None})
    return out


def main() -> None:
    app = QApplication.instance() or QApplication([])  # noqa: F841
    facts = []
    for name, variant, project in variants():
        base = f136.load(FIX / name)
        facts.append({
            "fixture": name, "variant": variant, "patches": diff(base.image, project.image),
            "scan": scan_facts(project), "entry_labels": entry_labels(project), "groups": group_facts(project),
            "settings": settings_facts(project), "quick": quick_facts(project), "alarm": alarm_facts(project),
            "edits": edit_cases(project),
        })
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    json.dump({"variants": facts}, sys.stdout, ensure_ascii=False, indent=0)


if __name__ == "__main__":
    main()
