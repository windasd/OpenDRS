from __future__ import annotations

import os
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
QtCore = pytest.importorskip("PySide6.QtCore")
from PySide6.QtCore import Qt  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from opendrs import f136  # noqa: E402
from opendrs.gui.models import make_models  # noqa: E402
from opendrs.textcodec import NameCodec  # noqa: E402


@pytest.fixture(scope="module")
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


def test_channel_model_shows_and_edits(app: QApplication, small_path: Path) -> None:
    project = f136.load(small_path)
    model = make_models()["channels"]
    model.set_project(project, NameCodec.BIG5)
    assert model.rowCount() == 3 and model.columnCount() == 21
    assert model.data(model.index(0, 2)) == "438.80000"
    changed: list[object] = []
    model.project_changed.connect(changed.append)
    assert model.setData(model.index(0, 1), "測試")
    assert changed[-1].channels[0].name(NameCodec.BIG5) == "測試"


def test_invalid_edit_emits_error(app: QApplication, small_path: Path) -> None:
    model = make_models()["channels"]
    model.set_project(f136.load(small_path), NameCodec.ASCII)
    errors: list[str] = []
    model.edit_failed.connect(errors.append)
    assert not model.setData(model.index(0, 1), "中文")
    assert not model.setData(model.index(0, 2), "abc")
    assert len(errors) == 2
    assert not model.setData(model.index(0, 0), "5")  # read-only column


def test_out_of_band_cell_is_highlighted(app: QApplication, small_path: Path) -> None:
    model = make_models()["channels"]
    model.set_project(f136.load(small_path), NameCodec.BIG5)
    assert model.setData(model.index(0, 2), "300")
    tip = model.data(model.index(0, 2), Qt.ItemDataRole.ToolTipRole)
    assert tip and "超出本機頻段" in tip
    assert model.data(model.index(0, 3), Qt.ItemDataRole.ToolTipRole) is None


def test_contact_model_id_edit(app: QApplication, small_path: Path) -> None:
    model = make_models()["contacts"]
    model.set_project(f136.load(small_path), NameCodec.BIG5)
    assert model.data(model.index(0, 3)) == "46001"
    assert model.setData(model.index(0, 3), "46002")
    assert model.data(model.index(0, 3)) == "46002"
    assert model.headerData(3, Qt.Orientation.Horizontal) == "呼叫 ID"


def test_main_window_opens_and_saves(app: QApplication, small_path: Path, tmp_path: Path,
                                     monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    from opendrs.gui.main_window import MainWindow

    target = tmp_path / "p.F136"
    target.write_bytes(small_path.read_bytes())
    win = MainWindow()
    assert win.open_path(target)
    assert "DRS-8500" in win._dev_labels["model"].text()
    assert not win.is_dirty()
    win._models["channels"].setData(win._models["channels"].index(0, 1), "X")
    assert win.is_dirty() and win.windowTitle().startswith("*")
    assert win.save()
    assert not win.is_dirty()
    assert f136.load(target).channels[0].name(NameCodec.ASCII) == "X"


def test_main_window_cleans_control_chars(app: QApplication, tmp_path: Path,
                                          monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox

    from opendrs.gui.main_window import MainWindow

    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: None)
    src = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"
    target = tmp_path / "dh.F136"
    target.write_bytes(src.read_bytes())
    win = MainWindow()
    win.open_path(target)
    hint = win._problems.item(0).text()
    assert "聯絡人 343 筆" in hint and "清除名稱中的換行字元" in hint

    monkeypatch.setattr(QMessageBox, "question",
                        lambda *a, **k: QMessageBox.StandardButton.No)
    assert win.clean_control_chars() == 0
    assert not win.is_dirty()

    asked: list[str] = []

    def yes(*args, **kwargs):  # noqa: ANN002, ANN003, ANN202
        asked.append(args[2])
        return QMessageBox.StandardButton.Yes

    monkeypatch.setattr(QMessageBox, "question", yes)
    assert win.clean_control_chars() == 343
    assert "「QA0BD␍␊」→「QA0BD」" in asked[0]
    assert win.is_dirty()
    assert "換行字元" not in win._problems.item(0).text()
    assert win.clean_control_chars() == 0  # nothing left; information box only
    assert win.save()
    saved = f136.load(target)
    assert saved.contacts[15].name(NameCodec.BIG5) == "QA0BD"
    assert (tmp_path / "dh.F136.bak").read_bytes() == src.read_bytes()


def _window(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, src: Path):  # noqa: ANN202
    from PySide6.QtWidgets import QMessageBox

    from opendrs.gui.main_window import MainWindow

    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: None)
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Yes)
    target = tmp_path / src.name
    target.write_bytes(src.read_bytes())
    win = MainWindow()
    assert win.open_path(target)
    return win, target


def test_undo_redo_across_edit_import_and_cleanup(app: QApplication, tmp_path: Path,
                                                  monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.contact_import import NameFormat
    from opendrs.radioid import RadioIdUser

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    original = win._project
    assert not win._a_undo.isEnabled()
    win._models["channels"].setData(win._models["channels"].index(0, 1), "EDIT")
    assert win._a_undo.isEnabled() and win._a_undo.text() == "復原 編輯"
    win.clean_control_chars()
    win.apply_import([RadioIdUser(9999999, "QA0AGR", "", "", "", "", "Taiwan")], NameFormat.CALLSIGN)
    assert win._tabs.tabText(3) == "聯絡人（360）"
    win.undo()
    assert win._tabs.tabText(3) == "聯絡人（359）" and win._a_redo.text() == "重做 匯入聯絡人"
    win.undo()
    win.undo()
    assert win._project == original and not win.is_dirty()
    win.redo()
    assert win._models["channels"].data(win._models["channels"].index(0, 1)) == "EDIT"
    assert win.is_dirty()


def test_search_filter(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    proxy = win._proxies["contacts"]
    assert proxy.rowCount() == 359
    win._filters["contacts"].setText("qa0bd")
    assert proxy.rowCount() == 1
    assert proxy.data(proxy.index(0, 3)) == "9990001"
    win._filters["contacts"].setText("9990")
    assert 300 < proxy.rowCount() < 359
    win._tabs.setCurrentIndex(3)
    win._focus_filter()


def test_csv_export_import_and_recent_menu(app: QApplication, small_path: Path, tmp_path: Path,
                                           monkeypatch: pytest.MonkeyPatch) -> None:
    win, target = _window(tmp_path, monkeypatch, small_path)
    out = tmp_path / "c.csv"
    assert win.export_contacts_csv(out)
    assert out.read_bytes().startswith(b"\xef\xbb\xbf")
    extra = tmp_path / "extra.csv"
    extra.write_bytes("名稱,ID\r\n測試甲,9991111\r\n壞,xx\r\n".encode("big5"))
    report = win.import_contacts_csv(extra)
    assert report is not None and report.added == 1
    assert win._project.contacts[-1].name(NameCodec.BIG5) == "測試甲"
    win.undo()
    assert win._project.contacts[-1].call_id != 9991111
    actions = [a.text() for a in win._recent_menu.actions()]
    assert str(target) in actions


def test_compare_with(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fix = Path(__file__).parent / "fixtures"
    win, _ = _window(tmp_path, monkeypatch, fix / "mmdvm_20241124.F136")
    dlg = win.compare_with(fix / "mmdvm_20241127.F136")
    assert "■ 分區（1 → 2 筆）" in dlg.text()
    assert "mmdvm_20241127.F136" in dlg._title


def test_device_page_shows_dmr_id(app: QApplication, tmp_path: Path,
                                  monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.gui.device_editing import describe_dmr_id
    from opendrs.radioid import RadioIdUser

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    assert win._dev_labels["dmr_id"].text().startswith("尚未下載 RadioID")
    assert win._dmr_edit.text() == "9998765" and win._alias_edit.text() == "QA0AFK"
    assert win._dev_labels["firmware"].text() == "308E.BBKTARSCB.003"
    assert win._dev_labels["firmware_date"].text() == "Mar 9 2024"
    user = RadioIdUser(9998765, "QA0AJS", "Da-Wei", "", "Tainan", "Southern Taiwan", "Taiwan")
    text = describe_dmr_id(9998765, "QA0AFK", user)
    assert text.startswith("RadioID：QA0AJS Da-Wei（Tainan、Taiwan）")
    assert "別名 QA0AFK 與 RadioID 登記的呼號 QA0AJS 不同" in text
    assert "⚠" not in describe_dmr_id(9998765, "qa0ajs", user)
    assert describe_dmr_id(9998765, "", None) == "RadioID 查無 9998765"


def test_device_page_edits_dmr_id_and_alias(app: QApplication, tmp_path: Path,
                                            monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox

    from opendrs import f136
    from opendrs.radioid import RadioIdUser

    warnings: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda _p, _t, msg: warnings.append(msg))
    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    win._dmr_edit.setText("9990999")
    win._apply_dmr_id()
    assert win._project.device.dmr_id == 9990999 and win.is_dirty()
    assert win._a_undo.text() == "復原 修改 DMR ID"
    win._dmr_edit.setText("0")
    win._apply_dmr_id()
    assert warnings and win._dmr_edit.text() == "9990999"
    win._alias_edit.setText("QA0AJS")
    win._apply_alias()
    assert win._project.device.alias == "QA0AJS"
    win._apply_alias()  # unchanged text: no new history entry
    win.undo()
    assert win._project.device.alias == "QA0AFK" and win._alias_edit.text() == "QA0AFK"
    win.undo()
    assert win._project.device.dmr_id == 9998765 and not win.is_dirty()
    # The RadioID button appears only when the registered callsign differs from the alias.
    user = RadioIdUser(9998765, "QA0AJS", "Da-Wei", "", "Tainan", "", "Taiwan")
    win._radioid_lookups[9998765] = user
    monkeypatch.setattr("opendrs.radioid.cache_info", lambda _d: object())
    win._refresh()
    assert not win._alias_from_radioid_btn.isHidden()
    win.alias_from_radioid()
    assert win._project.device.alias == "QA0AJS" and win._alias_from_radioid_btn.isHidden()
    assert win.save()
    assert f136.load(target).device.alias == "QA0AJS"


def test_channel_zone_and_tx_contact_columns(app: QApplication, tmp_path: Path,
                                             monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QStyleOptionViewItem

    from opendrs.gui.models import CHANNEL_TX_CONTACT_COLUMN, VALUE_ROLE

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    model = win._models["channels"]
    assert model.data(model.index(0, 4)) == "1. MMDVM"
    assert model.data(model.index(23, 4)) == "4. FRS_TW"
    tx = model.index(0, CHANNEL_TX_CONTACT_COLUMN)
    assert model.data(tx).startswith("#2 TG46600_TW") and model.data(tx, VALUE_ROLE) == 2
    # Edit through the picker on the (sortable, filterable) view.
    page = win._pages["channels"]
    pidx = page.proxy.mapFromSource(tx)
    editor = win._contact_picker.createEditor(page.view, QStyleOptionViewItem(), pidx)
    win._contact_picker.setEditorData(editor, pidx)
    assert editor.currentData() == 2
    editor.setCurrentIndex(editor.findData(13))
    win._contact_picker.setModelData(editor, page.proxy, pidx)
    assert win._project.channels[0].tx_contact == 13 and win.is_dirty()
    editor.setEditText("no such contact")
    win._contact_picker.setModelData(editor, page.proxy, pidx)
    assert win._project.channels[0].tx_contact == 13


def test_add_channel_action(app: QApplication, tmp_path: Path,
                            monkeypatch: pytest.MonkeyPatch) -> None:
    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    win._pages["channels"].sort_by(1)  # sorted view: selection must map to the right record
    win.select_row("channels", 8)  # HW_3, zone 2
    assert win.selected_row("channels") == 8
    new_row = win.add_channel()
    assert new_row == 9 and win.selected_row("channels") == 9
    assert win._project.channels[9].name(NameCodec.BIG5) == "HW_3"
    assert win._tabs.tabText(1) == "頻道（25）"
    assert win._a_undo.text() == "復原 新增頻道"
    assert win.save() and len(f136.load(target).channels) == 25
    win.undo()
    assert len(win._project.channels) == 24


def test_delete_channel_action(app: QApplication, tmp_path: Path,
                               monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox

    asked: list[str] = []
    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    monkeypatch.setattr(QMessageBox, "question",
                        lambda *a, **k: asked.append(a[2]) or QMessageBox.StandardButton.Yes)
    warned: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: warned.append(a[2]))
    win._pages["channels"].sort_by(1, Qt.SortOrder.DescendingOrder)
    win.select_row("channels", 12)  # FRS_TW_3
    assert win.delete_channel()
    assert "FRS_TW_3" in asked[-1] and "掃描清單中有 1 筆" in asked[-1] and "11 筆" in asked[-1]
    assert win._tabs.tabText(1) == "頻道（23）"
    assert win._a_undo.text() == "復原 刪除頻道 FRS_TW_3"
    assert win.save() and len(f136.load(target).channels) == 23
    win.select_row("channels", 9)  # CHT_1, only channel of its zone
    assert not win.delete_channel() and "只剩這個頻道" in warned[-1]
    win.undo()
    assert len(win._project.channels) == 24
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.No)
    win.select_row("channels", 0)
    assert not win.delete_channel() and len(win._project.channels) == 24


def test_zone_actions(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox

    asked: list[str] = []
    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    monkeypatch.setattr(QMessageBox, "question",
                        lambda *a, **k: asked.append(a[2]) or QMessageBox.StandardButton.Yes)
    win.select_row("channels", 2)  # TG9
    zone_row = win.add_zone("TESTZONE")
    assert zone_row == 4 and win._tabs.tabText(2) == "分區（5）" and win._tabs.tabText(1) == "頻道（25）"
    assert win._models["channels"].data(win._models["channels"].index(24, 4)) == "5. TESTZONE"
    assert win.selected_row("zones") == 4
    win.select_row("zones", 1)  # HighWay, on the zones tab
    assert win.delete_zone()
    assert "HighWay" in asked[-1] and "14 筆的分區編號" in asked[-1]
    assert [z.name(NameCodec.BIG5) for z in win._project.zones] == ["MMDVM", "HiKing", "FRS_TW", "TESTZONE"]
    assert win._a_undo.text() == "復原 刪除分區 HighWay"
    assert win.save() and len(f136.load(target).zones) == 4
    win.undo()
    win.undo()
    assert len(win._project.zones) == 4 and len(win._project.channels) == 24


def test_scan_list_tab_and_edit(app: QApplication, tmp_path: Path,
                                monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.gui.scan_page import ScanListDialog, channel_entries
    from opendrs.scanlist import scan_lists

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    tab = win._tabs.indexOf(win._scan_page)
    assert win._tabs.tabText(tab) == "掃描清單（2）"
    table = win._scan_page.table
    assert table.rowCount() == 16 and table.item(1, 1).text() == "ScanFRS_TW"
    assert table.item(1, 2).text() == "14" and table.item(1, 3).text().startswith("FRS_TW／FRS_TW_1")
    assert table.item(5, 1).text() == "（未使用）"
    # Dialog: existing members ticked; ticking 17 disables OK.
    dlg = ScanListDialog(win._project, scan_lists(win._project.image)[1], NameCodec.BIG5)
    assert dlg.entries() == [(4, n) for n in range(1, 15)]
    for i in range(dlg.channels.count()):
        dlg.channels.item(i).setCheckState(Qt.CheckState.Checked)
    assert "超過上限" in dlg.count.text()
    assert not dlg.buttons.button(dlg.buttons.StandardButton.Ok).isEnabled()
    assert len(channel_entries(win._project)) == 24
    # Apply an edit through the window.
    assert win.apply_scan_list(1, "TGscan", [(1, 1), (1, 2)])
    assert table.item(0, 1).text() == "TGscan" and table.item(0, 3).text() == "MMDVM／TG46600、MMDVM／TG46621"
    assert win.is_dirty() and win._a_undo.text() == "復原 編輯掃描清單 1"
    warned: list[str] = []
    from PySide6.QtWidgets import QMessageBox
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: warned.append(a[2]))
    assert not win.apply_scan_list(1, "x" * 11, [])
    assert warned
    # Deleting a channel updates the scan tab.
    win.select_row("channels", 10)  # FRS_TW_1
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Yes)
    assert win.delete_channel()
    assert table.item(1, 2).text() == "13"


def test_channel_mode_and_slot_editors(app: QApplication, tmp_path: Path,
                                       monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox, QStyleOptionViewItem

    from opendrs import channel_fields as cf
    from opendrs.gui.models import CHANNEL_COLOR_CODE_COLUMN as CC
    from opendrs.gui.models import (
        CHANNEL_COLUMNS,
        CHANNEL_ENCRYPTION_COLUMN,
        CHANNEL_MODE_COLUMN,
        CHANNEL_SLOT_COLUMN,
    )

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    model, page = win._models["channels"], win._pages["channels"]
    titles = [c.title for c in CHANNEL_COLUMNS]
    assert titles[CHANNEL_MODE_COLUMN] == "模式" and titles[CHANNEL_SLOT_COLUMN] == "時隙"
    assert model.data(model.index(0, CHANNEL_MODE_COLUMN)) == "數位"
    assert model.data(model.index(0, CC)) == "1" and model.data(model.index(0, CHANNEL_SLOT_COLUMN)) == "On"
    assert model.data(model.index(0, titles.index("發射權限"))) == "Impolite"
    assert model.data(model.index(0, CHANNEL_ENCRYPTION_COLUMN)) == "Off"
    for col, value in ((CHANNEL_MODE_COLUMN, 0), (CHANNEL_SLOT_COLUMN, 2), (titles.index("功率"), 1)):
        delegate = win._choice_pickers[col]
        pidx = page.proxy.mapFromSource(model.index(0, col))
        editor = delegate.createEditor(page.view, QStyleOptionViewItem(), pidx)
        delegate.setEditorData(editor, pidx)
        editor.setCurrentIndex(editor.findData(value))
        delegate.setModelData(editor, page.proxy, pidx)
    ch = win._project.channels[0]
    assert (ch.mode, cf.slot_label(ch), cf.POWER.display(ch.raw)) == (0, "時隙2", "高")
    pidx = page.proxy.mapFromSource(model.index(0, CHANNEL_ENCRYPTION_COLUMN))
    editor = win._key_picker.createEditor(page.view, QStyleOptionViewItem(), pidx)
    assert [editor.itemText(i) for i in range(editor.count())] == ["（無）", "1. windasd"]
    editor.setCurrentIndex(1)
    win._key_picker.setModelData(editor, page.proxy, pidx)
    assert cf.encryption_key(win._project.channels[0]) == 1
    assert model.setData(model.index(0, CC), "9")
    assert win._project.channels[0].color_code == 9 and win._project.channels[0].raw[0x1D] >> 4 == 9
    errors: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: errors.append(a[2]))
    assert not model.setData(model.index(0, CC), "16")
    assert errors and "色碼" in errors[-1]


def test_rx_group_tab_column_and_contact_delete(app: QApplication, tmp_path: Path,
                                                monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtCore import QItemSelectionModel
    from PySide6.QtWidgets import QMessageBox, QStyleOptionViewItem

    from opendrs.gui.group_page import RxGroupDialog
    from opendrs.gui.models import CHANNEL_RX_GROUP_COLUMN
    from opendrs.rxgroup import rx_groups

    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    tab = win._tabs.indexOf(win._group_page)
    assert win._tabs.tabText(tab) == "接收群組（1）"
    assert win._group_page.table.item(0, 1).text() == "MMDVM_TG"
    assert win._group_page.table.item(0, 2).text() == "9"
    model = win._models["channels"]
    assert model.data(model.index(0, CHANNEL_RX_GROUP_COLUMN)) == "1. MMDVM_TG"
    assert model.data(model.index(5, CHANNEL_RX_GROUP_COLUMN)) == "（無）"
    # Picker lists only groups in use, plus none.
    page = win._pages["channels"]
    pidx = page.proxy.mapFromSource(model.index(5, CHANNEL_RX_GROUP_COLUMN))
    editor = win._group_picker.createEditor(page.view, QStyleOptionViewItem(), pidx)
    assert [editor.itemText(i) for i in range(editor.count())] == ["（無）", "1. MMDVM_TG"]
    editor.setCurrentIndex(1)
    win._group_picker.setModelData(editor, page.proxy, pidx)
    assert win._project.channels[5].rx_group == 1
    # Group dialog: search filter and member values.
    dlg = RxGroupDialog(win._project, rx_groups(win._project.image)[0], NameCodec.BIG5)
    assert dlg.values() == [1, 2, 3, 4, 5, 6, 10, 12, 13]
    dlg.search.setText("TG466")
    visible = [dlg.items.item(i).text() for i in range(dlg.items.count()) if not dlg.items.item(i).isHidden()]
    assert visible and all("TG466" in v for v in visible)
    assert win.apply_rx_group(2, "TW", [2, 10])
    assert win._tabs.tabText(tab) == "接收群組（2）"
    # Delete two contacts selected in a sorted view.
    asked: list[str] = []
    monkeypatch.setattr(QMessageBox, "question",
                        lambda *a, **k: asked.append(a[2]) or QMessageBox.StandardButton.Yes)
    cpage = win._pages["contacts"]
    cpage.sort_by(3, Qt.SortOrder.DescendingOrder)
    sel = cpage.view.selectionModel()
    for row in (2, 299):
        idx = cpage.proxy.mapFromSource(win._models["contacts"].index(row, 2))
        sel.select(idx, QItemSelectionModel.SelectionFlag.Select)
    assert win.selected_rows("contacts") == [2, 299]
    assert win.delete_contacts()
    assert "刪除 2 筆聯絡人" in asked[-1] and "接收群組中有 1 筆" in asked[-1]
    assert win._tabs.tabText(3) == f"聯絡人（{359 - 2}）"
    assert win._group_page.table.item(0, 2).text() == "8"
    assert win.save() and len(f136.load(target).contacts) == 357
    win.undo()
    assert len(win._project.contacts) == 359
    # A contact used by a channel cannot be deleted.
    warned: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: warned.append(a[2]))
    sel.clearSelection()
    sel.select(cpage.proxy.mapFromSource(win._models["contacts"].index(1, 2)),
               QItemSelectionModel.SelectionFlag.Select)
    assert not win.delete_contacts() and "TG46600" in warned[-1]
    # Ctrl+F on the list tabs does nothing instead of failing.
    win._tabs.setCurrentIndex(tab)
    win._focus_filter()


def test_settings_tab_edits_bits(app: QApplication, tmp_path: Path,
                                 monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QComboBox

    from opendrs.radio_settings import by_key

    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    page = win._settings_page
    assert win._tabs.indexOf(page) > 0 and page.page_list.item(0).text() == "基本設置"
    backlight = page.editor("backlight")
    assert isinstance(backlight, QComboBox) and backlight.currentText() == "On"
    assert not page.editor("power_save_ratio").isEnabled()  # power saving is off
    before = win._project.image
    backlight.setCurrentIndex(backlight.findText("Auto"))
    after = win._project.image
    assert by_key("backlight").display(after) == "Auto" and win.is_dirty()
    assert [i for i in range(len(before)) if before[i] != after[i]] == [0x95]
    assert after[0x95] & ~0x28 == before[0x95] & ~0x28  # language / keylock bits kept
    assert win._a_undo.text() == "復原 基本設置：背景燈開關"
    page.editor("power_save").setCurrentIndex(1)
    assert page.editor("power_save_ratio").isEnabled()
    code = page.editor("dtmf_code")
    assert code.text() == "1234"
    code.setText("5678")
    code.editingFinished.emit()
    assert win._project.image[0x1364:0x1368] == b"5678"
    win.undo()
    assert win.save() and by_key("backlight").display(f136.load(target).image) == "Auto"
    win.undo()
    win.undo()
    assert win._project.image == before and backlight.currentText() == "On"


def test_quick_message_tab(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox

    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    table = win._quickmsg_page.table
    assert table.rowCount() == 16 and table.item(0, 0).text() == ""
    table.item(0, 0).setText("CHECK!")
    assert win._a_undo.text() == "復原 常用短信 1" and win._project.image[0xFAE:0xFB4] == b"CHECK!"
    warned: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: warned.append(a[2]))
    table.item(1, 0).setText("x" * 41)
    assert warned and table.item(1, 0).text() == ""
    assert win.save()


def test_alarm_encryption_tab(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox

    from opendrs.alarm_enc import alarm_lists, encryption_keys, gps_channel

    win, target = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    page = win._alarm_page
    assert page.alarms.rowCount() == 4 and page.alarms.item(0, 0).text() == "1"
    assert page.alarms.cellWidget(0, 3).currentText() == "Off" and page.gps.currentText() == "Off"
    page.alarms.cellWidget(0, 1).setCurrentIndex(page.alarms.cellWidget(0, 1).findText("靜默"))
    assert alarm_lists(win._project.image)[0].alarm_type == 3
    channel = page.alarms.cellWidget(1, 3)
    channel.setCurrentIndex(channel.findText("FRS_TW／FRS_TW_2"))
    assert alarm_lists(win._project.image)[1].channel == (4, 2)
    page.gps.setCurrentIndex(page.gps.findText("MMDVM／TG9"))
    assert gps_channel(win._project.image) == (1, 3) and win._a_undo.text() == "復原 GPS上報信道"
    assert page.keys.item(0, 0).text() == "windasd" and page.keys.item(0, 1).text() == "12 34 56 78 9A BC"
    warned: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: warned.append(a[2]))
    page.keys.item(1, 0).setText("TW")
    assert encryption_keys(win._project.image)[1].in_use and not warned  # zero key until typed
    page.keys.item(1, 1).setText("010203040506")
    assert encryption_keys(win._project.image)[1].key == bytes(range(1, 7))
    page.keys.item(1, 1).setText("BAD")
    assert warned and "金鑰" in warned[-1]
    assert win.save() and gps_channel(f136.load(target).image) == (1, 3)


def test_theme_switch_and_icons(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtCore import QCoreApplication, QEvent

    from opendrs.gui import theme
    from opendrs.settings import load_settings

    for old in app.topLevelWidgets():  # windows left by earlier tests would all be re-styled
        old.deleteLater()
    QCoreApplication.sendPostedEvents(None, QEvent.Type.DeferredDelete)
    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    assert not theme.icon("save").isNull() and not theme.app_icon().isNull()
    assert all(not act.icon().isNull() for act, _ in win._icon_actions)
    win.set_theme(theme.ThemeMode.DARK)
    assert theme.tokens() is theme.DARK and app.palette().color(app.palette().ColorRole.Window).name() == "#1b1f24"
    assert "QToolBar" in app.styleSheet() and load_settings().theme == "dark"
    assert win._theme_actions[theme.ThemeMode.DARK].isChecked()
    win.set_theme(theme.ThemeMode.LIGHT)
    assert theme.tokens() is theme.LIGHT and load_settings().theme == "light"
    assert theme.ThemeMode.AUTO.label == "跟隨系統"
    assert theme.resolve(theme.ThemeMode.AUTO, app) in (theme.LIGHT, theme.DARK)
    assert win._problems_box.title() == "問題清單（1）"
    app.setStyleSheet("")


def test_channel_dropdowns_open_on_one_click(app: QApplication, tmp_path: Path,
                                             monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtCore import QCoreApplication, QEvent
    from PySide6.QtTest import QTest
    from PySide6.QtWidgets import QComboBox

    from opendrs import channel_fields as cf
    from opendrs.gui.models import (
        CHANNEL_COLOR_CODE_COLUMN,
        CHANNEL_COLUMNS,
        CHANNEL_MODE_COLUMN,
    )

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    win.resize(1280, 760)
    win.show()
    win._tabs.setCurrentIndex(1)
    page = win._pages["channels"]
    view = page.view
    titles = [c.title for c in CHANNEL_COLUMNS]
    for title in ("模式", "色碼", "時隙", "功率", "呼叫指示", "個呼類型", "短信類型", "發射權限",
                  "加密列表", "掃描列表", "緊急系統", "接收亞音", "發射亞音", "發話對象", "接收群組"):
        assert view.itemDelegateForColumn(titles.index(title)) is not None, title

    def click_and_pick(col: int, value: int) -> None:
        index = page.proxy.index(0, col)
        view.scrollTo(index)
        app.processEvents()
        QTest.mouseClick(view.viewport(), Qt.MouseButton.LeftButton, Qt.KeyboardModifier.NoModifier,
                         view.visualRect(index).center())
        QCoreApplication.sendPostedEvents(None, QEvent.Type.None_)
        app.processEvents()
        boxes = [b for b in view.findChildren(QComboBox) if b.isVisible()]
        assert len(boxes) == 1, f"one click should open the editor for column {col}"
        box = boxes[0]
        box.setCurrentIndex(box.findData(value))
        box.activated.emit(box.currentIndex())  # what picking an entry in the list emits
        app.processEvents()

    click_and_pick(CHANNEL_MODE_COLUMN, 0)
    assert win._project.channels[0].mode == 0
    click_and_pick(CHANNEL_COLOR_CODE_COLUMN, 5)
    assert win._project.channels[0].color_code == 5 and win._project.channels[0].raw[0x1D] >> 4 == 5
    click_and_pick(titles.index("功率"), 1)
    assert cf.POWER.display(win._project.channels[0].raw) == "高"
    assert view.state() == view.State.NoState  # editor closed after the pick
    win.close()


def test_zone_column_moves_channel(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QMessageBox, QStyleOptionViewItem

    from opendrs.gui.models import CHANNEL_ZONE_COLUMN, VALUE_ROLE

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    model, page = win._models["channels"], win._pages["channels"]
    index = page.proxy.mapFromSource(model.index(12, CHANNEL_ZONE_COLUMN))
    assert model.data(model.index(12, CHANNEL_ZONE_COLUMN)) == "4. FRS_TW" and index.data(VALUE_ROLE) == 3
    editor = win._zone_picker.createEditor(page.view, QStyleOptionViewItem(), index)
    assert [editor.itemText(i) for i in range(editor.count())] == ["1. MMDVM", "2. HighWay", "3. HiKing", "4. FRS_TW"]
    win._zone_picker.setEditorData(editor, index)
    assert editor.currentIndex() == 3
    editor.setCurrentIndex(1)
    win._zone_picker.setModelData(editor, page.proxy, index)
    assert [c.name(NameCodec.BIG5) for c in win._project.channels][9] == "FRS_TW_3"
    assert model.data(model.index(9, CHANNEL_ZONE_COLUMN)) == "2. HighWay"
    assert win._a_undo.text() == "復原 變更分區" and win._tabs.tabText(2) == "分區（4）"
    warned: list[str] = []
    monkeypatch.setattr(QMessageBox, "warning", lambda *a, **k: warned.append(a[2]))
    assert not model.setData(model.index(10, CHANNEL_ZONE_COLUMN), 0)  # CHT_1: only channel of HiKing
    assert warned and "只剩" in warned[-1]
    win.undo()
    assert [c.name(NameCodec.BIG5) for c in win._project.channels][12] == "FRS_TW_3"


def test_ordering_batch_and_device_ids(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.gui.batch_dialog import BatchAddDialog, BatchPlan

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    assert (win._dev_labels["factory_number"].text(), win._dev_labels["serial_number"].text(),
            win._dev_labels["model_number"].text()) == ("123456", "0001", "DR8600")
    win.select_row("channels", 12)
    assert win.shift_channel(-1) and win.selected_row("channels") == 11
    assert win._project.channels[11].name(NameCodec.BIG5) == "FRS_TW_3"
    win.select_row("contacts", 1)
    win._pages["contacts"].view.selectionModel().select(
        win._pages["contacts"].proxy.mapFromSource(win._models["contacts"].index(1, 1)),
        win._pages["contacts"].view.selectionModel().SelectionFlag.ClearAndSelect)
    assert win.shift_contact(1) and win._project.channels[0].tx_contact == 3
    dialog = BatchAddDialog(win._project.channels[23], NameCodec.BIG5, win._project.device.bands, "4. FRS_TW")
    plan = dialog.plan()
    assert (plan.name_prefix, plan.first_number, plan.step_hz, plan.tx_offset_hz) == ("FRS_TW_", 15, 12_500, 0)
    dialog.start.setText("999.0")
    assert not dialog.buttons.button(dialog.buttons.StandardButton.Ok).isEnabled() and "⚠" in dialog.preview.text()
    win.select_row("channels", 23)
    before = len(win._project.channels)
    assert win.batch_add_channels(BatchPlan(2, 467_687_500, 12_500, 0, "X", 1))
    assert len(win._project.channels) == before + 2 and win._a_undo.text() == "復原 批次新增頻道 2 台"


def test_firmware_update_dialog_from_menu(app: QApplication, tmp_path: Path,
                                          monkeypatch: pytest.MonkeyPatch) -> None:
    from opendrs.firmware_check import DEFAULT_IAP_EXE
    from opendrs.settings import load_settings

    win, _ = _window(tmp_path, monkeypatch, Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    assert any(a.text().startswith("韌體升級") for a in win.findChildren(type(win._a_read)))
    dialog = win.firmware_update()
    assert dialog._radio_firmware == "308E.BBKTARSCB.003" and dialog._radio_date == "Mar 9 2024"
    assert dialog.exe.text() == str(DEFAULT_IAP_EXE) and not dialog.launch.isEnabled()
    custom = tmp_path / "tools" / "IAP+Flash.exe"
    dialog.exe.setText(str(custom))
    dialog.reject()
    app.processEvents()
    assert load_settings().iap_path == str(custom)
