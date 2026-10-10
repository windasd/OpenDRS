"""Export what the Python GUI's record tables show and do, as the oracle for the egui port.

Uses the real Qt models (gui/models.py), the drop-down choice lists (gui/delegates.py) and the
filter proxy (gui/record_filter.py): every cell's text, sort key, editor value and band problem,
the choices of every drop-down, filtered/sorted row orders, and the outcome of cell edits.

Run from the repository root (needs PySide6):
    PYTHONPATH=../opendrs-oracle/src python rust/tools/make_parity_gui.py > rust/opendrs-gui/tests/parity_table.json
"""

from __future__ import annotations

import hashlib
from datetime import datetime
import json
import os
import sys
from pathlib import Path

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")

from PySide6.QtCore import QCoreApplication, Qt  # noqa: E402

from opendrs import f136  # noqa: E402
from opendrs.gui import delegates  # noqa: E402
from opendrs.gui.filter_bar import _SHOW_CHOICES  # noqa: E402
from opendrs.gui.models import SORT_ROLE, VALUE_ROLE, make_models  # noqa: E402
from opendrs.gui.print_report import report_html  # noqa: E402
from opendrs.gui.record_filter import RecordFilterProxy, kind_filter  # noqa: E402
from opendrs.textcodec import NameCodec  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
FIX = ROOT / "rust" / "fixtures"
FIXTURES = ["dh98vu_20250811.F136", "mmdvm_20241124.F136", "mmdvm_20241127.F136", "capture/written_project.F136"]
CODEC = NameCodec.BIG5
DISPLAY = Qt.ItemDataRole.DisplayRole
TOOLTIP = Qt.ItemDataRole.ToolTipRole
PICKERS = {4: (delegates.zone_choices, None), 5: (delegates.contact_choices, "（無）"),
           6: (delegates.rx_group_choices, "（無）"), 15: (delegates.encryption_key_choices, "（無）"),
           16: (delegates.scan_list_choices, "（無）"), 17: (delegates.alarm_list_choices, "（無）")}


def sha(project: f136.Project) -> str:
    return hashlib.sha256(f136.serialize(project)).hexdigest()


def plain(value: object) -> object:
    return value if value is None or isinstance(value, (int, str)) else str(value)


def table_facts(model) -> dict:  # noqa: ANN001
    rows, cols = model.rowCount(), model.columnCount()
    cell = lambda r, c, role: model.data(model.index(r, c), role)  # noqa: E731
    problems = {f"{r},{c}": cell(r, c, TOOLTIP) for r in range(rows) for c in range(cols) if cell(r, c, TOOLTIP)}
    return {
        "titles": [model.headerData(c, Qt.Orientation.Horizontal) for c in range(cols)],
        "editable": [bool(model.flags(model.index(0, c)) & Qt.ItemFlag.ItemIsEditable) if rows else None
                     for c in range(cols)],
        "text": [[cell(r, c, DISPLAY) for c in range(cols)] for r in range(rows)],
        "sort": [[plain(cell(r, c, SORT_ROLE)) for c in range(cols)] for r in range(rows)],
        "value": [[plain(cell(r, c, VALUE_ROLE)) for c in range(cols)] for r in range(rows)],
        "problems": problems,
    }


def choices(project: f136.Project, models: dict) -> dict:  # noqa: ANN001
    out = {}
    for col, (fn, none) in PICKERS.items():
        out[str(col)] = {"none": none, "items": [[label, value] for label, value in fn(project, CODEC)]}
    from opendrs.gui.models import CHANNEL_COLUMNS
    for col, column in enumerate(CHANNEL_COLUMNS):
        if column.choices:
            out[str(col)] = {"none": None, "items": [[label, value] for label, value in column.choices]}
    return out


def filter_cases(project: f136.Project) -> list[dict]:
    cases = []
    kinds = {"channels": 0, "zones": 0, "contacts": 0}
    specs = [
        ("channels", "", None, None, "全部", None), ("channels", "tg", None, None, "全部", None),
        ("channels", "tg 4", None, None, "全部", (1, "asc")), ("channels", "439", 2, None, "全部", (2, "desc")),
        ("channels", "", None, None, "名稱重複", None), ("channels", "", None, None, "名稱含換行字元", None),
        ("channels", "", None, None, "全部", (4, "asc")), ("channels", "", None, None, "全部", (5, "desc")),
        ("channels", "數位", 7, None, "全部", (2, "asc")), ("channels", "", None, None, "全部", (9, "asc")),
        ("channels", "off", None, None, "全部", (18, "desc")), ("channels", "ＴＧ", None, None, "全部", None),
        ("zones", "", None, None, "全部", (1, "desc")), ("zones", "z", None, None, "名稱重複", None),
        ("contacts", "", None, 5, "全部", (3, "desc")), ("contacts", "tg", 2, None, "全部", (2, "asc")),
        ("contacts", "", None, None, "類型與 ID 重複", None), ("contacts", "", None, None, "名稱含換行字元", (0, "desc")),
        ("contacts", "46", None, 4, "名稱重複", (3, "asc")), ("contacts", "", None, None, "全部", (1, "asc")),
    ]
    models = make_models()
    for model in models.values():
        model.set_project(project, CODEC)
    for table, text, scope, kind, show, sort in specs:
        model = models[table]
        proxy = RecordFilterProxy()
        proxy.setSourceModel(model)
        proxy.set_text(text)
        proxy.set_columns(None if scope is None else (scope,))
        if kind is not None:
            proxy.set_row_filter("kind", kind_filter(kind))
        proxy.set_row_filter("show", dict(_SHOW_CHOICES[table])[show])
        if sort is not None:
            order = Qt.SortOrder.AscendingOrder if sort[1] == "asc" else Qt.SortOrder.DescendingOrder
            proxy.sort(sort[0], order)
        rows = [proxy.mapToSource(proxy.index(r, 0)).row() for r in range(proxy.rowCount())]
        cases.append({"table": table, "text": text, "scope": scope, "kind": kind, "show": show,
                      "sort": sort, "rows": rows, "filtered": proxy.is_filtered()})
        kinds[table] += 1
    return cases


def edit_cases(project: f136.Project) -> list[dict]:
    specs: list[tuple[str, int, int, object]] = [
        ("channels", 0, 1, "NEW NAME"), ("channels", 0, 1, "名稱太長太長太長"), ("channels", 1, 1, "A\nB"),
        ("channels", 0, 1, project.channels[0].name(CODEC)), ("channels", 0, 2, "439.5"), ("channels", 0, 2, ""),
        ("channels", 0, 2, " 900 "), ("channels", 0, 2, "abc"), ("channels", 0, 3, "１４５.５"),
        ("channels", 0, 3, "430.0000005"), ("channels", 0, 3, "99999"),
        ("channels", 0, 4, 1), ("channels", 0, 4, 0), ("channels", 0, 4, 99), ("channels", 2, 4, len(project.zones) - 1),
        ("channels", 0, 5, 2), ("channels", 0, 5, 0), ("channels", 0, 5, 9999),
        ("channels", 0, 6, 1), ("channels", 0, 6, 0), ("channels", 0, 6, 33),
        ("channels", 0, 7, 0), ("channels", 0, 7, 3), ("channels", 0, 7, 4),
        ("channels", 0, 8, 15), ("channels", 0, 8, 16), ("channels", 0, 9, 0), ("channels", 0, 9, 2), ("channels", 0, 9, 3),
        ("channels", 0, 10, 0), ("channels", 0, 10, 2), ("channels", 0, 11, 1), ("channels", 0, 12, 1),
        ("channels", 0, 13, 1), ("channels", 0, 14, 3), ("channels", 0, 14, 9),
        ("channels", 0, 15, 1), ("channels", 0, 15, 0), ("channels", 0, 15, 11),
        ("channels", 0, 16, 1), ("channels", 0, 16, 17), ("channels", 0, 17, 1), ("channels", 0, 17, 5),
        ("channels", 0, 18, 1 << 8 | 5), ("channels", 0, 18, 0), ("channels", 0, 19, 3 << 8 | 2), ("channels", 0, 19, 0xFFFF),
        ("channels", 0, 0, "5"), ("channels", 0, 20, "00"),
        ("zones", 0, 1, "ZONE X"), ("zones", 0, 1, "很長很長很長的名稱"), ("zones", 0, 0, "3"),
        ("contacts", 0, 2, "RENAMED"), ("contacts", 0, 3, " 123 "), ("contacts", 0, 3, "0"),
        ("contacts", 0, 3, "16777216"), ("contacts", 0, 3, "abc"), ("contacts", 0, 3, "１２３"), ("contacts", 0, 1, "5"),
    ]
    out = []
    for table, row, col, value in specs:
        models = make_models()
        model = models[table]
        model.set_project(project, CODEC)
        outcome: dict = {"result": None}
        model.edit_failed.connect(lambda message, o=outcome: o.update(result={"error": message}))
        model.project_changed.connect(lambda p, o=outcome: o.update(
            result=None if p == project else {"sha": sha(p), "label": "編輯"}))
        model.project_restructured.connect(lambda p, what, o=outcome: o.update(
            result={"sha": sha(p), "label": f"變更{what}"}))
        try:
            model.setData(model.index(row, col), value, Qt.ItemDataRole.EditRole)
        except ValueError as exc:  # int(t) of a non-number escapes the model (Python's English message)
            outcome["result"] = {"error": str(exc), "python_exception": True}
        out.append({"table": table, "row": row, "col": col,
                    "input": {"value": value} if isinstance(value, int) else {"text": value}, **outcome})
    return out


def main() -> None:
    app = QCoreApplication.instance() or QCoreApplication([])  # noqa: F841
    fixtures = {}
    for name in FIXTURES:
        project = f136.load(FIX / name)
        models = make_models()
        for model in models.values():
            model.set_project(project, CODEC)
        fixtures[name] = {"tables": {key: table_facts(m) for key, m in models.items()},
                          "choices": choices(project, models), "filters": filter_cases(project),
                          "edits": edit_cases(project),
                          "report": report_html(project, CODEC, "測試報表", now=datetime(2026, 10, 10, 12, 0))}
    sys.stdout.reconfigure(encoding="utf-8", newline="\n")
    json.dump({"fixtures": fixtures}, sys.stdout, ensure_ascii=False, indent=0)


if __name__ == "__main__":
    main()
