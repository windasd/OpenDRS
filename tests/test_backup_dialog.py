from __future__ import annotations

import os
from datetime import datetime
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
pytest.importorskip("PySide6.QtCore")
from PySide6.QtWidgets import QApplication, QDialog, QMessageBox

from opendrs import f136
from opendrs.backups import (
    cleanup_candidates,
    delete_backup,
    list_backups,
    parse_name,
    size_text,
)
from opendrs.gui.backup_dialog import (
    COUNT_COL,
    LAST_WRITE_NOTE,
    NOTE_COL,
    SIZE_COL,
    BackupDialog,
)

FIXTURES = Path(__file__).parent / "fixtures"
# name stem -> fixture project (None = an unreadable .F136), newest last
BACKUPS = {
    "20261001_090000_read_DRS-8600": "mmdvm_20241124",
    "20261002_100000_before_write_DRS-8500": "mmdvm_20241127",
    "20261003_110000_read_DRS-8500": "dh98vu_20250811",
    "20261004_120000_read_DRS-8500": None,
}
# Files that look related but are not backups: never listed, never deleted.
BYSTANDERS = ("notes.txt", "random.F136", "20261003_110000_read_DRS-8500.F136.bak",
              "20261005_130000_read_DRS-8500.bin", "20261306_000000_read_DRS-8500.F136",
              "20261007_000000_restore_DRS-8500.F136")


@pytest.fixture(scope="module")
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


def make_backups(folder: Path) -> Path:
    folder.mkdir(parents=True, exist_ok=True)
    for stem, fixture in BACKUPS.items():
        data = (FIXTURES / f"{fixture}.F136").read_bytes() if fixture else b"12\t34\tgarbage\r\n"
        (folder / f"{stem}.F136").write_bytes(data)
        if fixture:  # the unreadable one has no .bin
            (folder / f"{stem}.bin").write_bytes(bytes(2048 * (len(stem) % 3 + 1)))
    for name in BYSTANDERS:
        (folder / name).write_bytes(b"keep me")
    return folder


def names(folder: Path) -> set[str]:
    return {p.name for p in folder.iterdir()}


@pytest.fixture
def folder(tmp_path: Path) -> Path:
    return make_backups(tmp_path / "backups")


def stub_dialog(folder: Path, opened: list[Path] | None = None, compared: list[Path] | None = None,
                open_ok: bool = True) -> BackupDialog:
    def open_backup(path: Path) -> bool:
        (opened if opened is not None else []).append(path)
        return open_ok

    def compare_backup(path: Path) -> QDialog | None:
        (compared if compared is not None else []).append(path)
        return QDialog()

    return BackupDialog(folder, open_backup, compare_backup)


# ------------------------------------------------------------------ backups.py


def test_lists_backups_newest_first_with_labels(folder: Path) -> None:
    backups = list_backups(folder)
    assert [b.path.name for b in backups] == [f"{s}.F136" for s in reversed(BACKUPS)]
    assert [b.kind_label for b in backups] == ["讀取", "讀取", "寫入前", "讀取"]
    assert [b.model for b in backups] == ["DRS-8500", "DRS-8500", "DRS-8500", "DRS-8600"]
    assert backups[0].time_text == "2026-10-04 12:00:00"
    assert backups[0].when == datetime(2026, 10, 4, 12, 0, 0)  # noqa: DTZ001 - local, like the names
    write = backups[2]
    assert write.raw_path == folder / "20261002_100000_before_write_DRS-8500.bin"
    assert write.size == write.path.stat().st_size + write.raw_path.stat().st_size
    assert write.files() == [write.path, write.raw_path]


def test_summary_counts_and_unreadable_backup(folder: Path) -> None:
    broken, dh, write, oldest = list_backups(folder)
    assert (dh.counts.zones, dh.counts.channels, dh.counts.contacts) == (4, 24, 359)
    assert (write.counts.zones, write.counts.channels, write.counts.contacts) == (2, 3, 9)
    assert (oldest.counts.zones, oldest.counts.channels, oldest.counts.contacts) == (1, 1, 8)
    assert broken.counts is None and broken.error
    assert broken.files() == [broken.path]  # no .bin
    assert dh.error == ""


def test_parse_name_accepts_only_backup_names() -> None:
    assert parse_name("20261009_141359_read_DRS-8500.F136") == (
        datetime(2026, 10, 9, 14, 13, 59), "read", "DRS-8500")  # noqa: DTZ001
    assert parse_name("20261009_141359_before_write_radio.F136")[1:] == ("before_write", "radio")
    for name in BYSTANDERS + ("20261009_141359_read_DRS-8500.bin", "20261009_141359_read_.F136",
                              "20261009_141359_read_X.F136\n", "２０２６1009_141359_read_X.F136"):
        assert parse_name(name) is None, name
    assert list_backups(Path("no such folder")) == []


def test_delete_backup_removes_its_two_files_only(folder: Path) -> None:
    before = names(folder)
    target = list_backups(folder)[1]  # 20261003 read
    removed = delete_backup(target)
    assert {p.name for p in removed} == {"20261003_110000_read_DRS-8500.F136", "20261003_110000_read_DRS-8500.bin"}
    assert names(folder) == before - {p.name for p in removed}
    assert "20261003_110000_read_DRS-8500.F136.bak" in names(folder)


def test_cleanup_candidates_keep_newest_and_last_before_write(folder: Path) -> None:
    backups = list_backups(folder)  # broken(read) dh(read) write(before_write) oldest(read)
    names_of = lambda bs: [b.path.stem for b in bs]
    assert names_of(cleanup_candidates(backups, 1)) == ["20261003_110000_read_DRS-8500",
                                                        "20261001_090000_read_DRS-8600"]
    assert names_of(cleanup_candidates(backups, 2)) == ["20261001_090000_read_DRS-8600"]
    assert names_of(cleanup_candidates(backups, 3)) == ["20261001_090000_read_DRS-8600"]
    assert cleanup_candidates(backups, 4) == [] and cleanup_candidates(backups, 99) == []
    reads_only = [b for b in backups if b.kind == "read"]
    assert names_of(cleanup_candidates(reads_only, 1)) == ["20261003_110000_read_DRS-8500",
                                                           "20261001_090000_read_DRS-8600"]
    with pytest.raises(ValueError):
        cleanup_candidates(backups, 0)


def test_size_text() -> None:
    assert size_text(17) == "17 bytes"
    assert size_text(47_938) == "46.8 KB"
    assert size_text(3 * 1024 * 1024 + 200_000) == "3.2 MB"


# ------------------------------------------------------------------ dialog


def test_dialog_table_rows_and_labels(app: QApplication, folder: Path) -> None:
    dlg = stub_dialog(folder)
    table = dlg.table
    assert table.rowCount() == 4
    rows = [[table.item(r, c).text() for c in range(table.columnCount())] for r in range(4)]
    assert rows[0][:SIZE_COL + 1] == ["2026-10-04 12:00:00", "讀取", "DRS-8500", "—", "—", "—", "15 bytes"]
    assert rows[0][NOTE_COL].startswith("無法讀取：第 1 行") and table.item(0, NOTE_COL).toolTip()
    assert rows[1][:COUNT_COL + 3] == ["2026-10-03 11:00:00", "讀取", "DRS-8500", "4", "24", "359"]
    assert rows[2][:COUNT_COL + 3] == ["2026-10-02 10:00:00", "寫入前", "DRS-8500", "2", "3", "9"]
    assert rows[3][1:COUNT_COL + 3] == ["讀取", "DRS-8600", "1", "1", "8"]
    assert rows[1][SIZE_COL] == size_text(dlg.backups[1].size)
    assert [r[NOTE_COL] for r in rows[1:]] == ["", LAST_WRITE_NOTE, ""]
    assert table.item(1, 0).toolTip() == "20261003_110000_read_DRS-8500.F136、20261003_110000_read_DRS-8500.bin"
    assert dlg.summary.text().startswith("共 4 份備份，合計 ")
    assert not dlg.open_btn.isEnabled() and not dlg.delete_btn.isEnabled()
    dlg.select(2)
    assert dlg.selected() is dlg.backups[2] and dlg.open_btn.isEnabled() and dlg.compare_btn.isEnabled()
    assert not dlg.cleanup_btn.isEnabled()  # 4 backups, keep 10
    dlg.keep.setValue(2)
    assert dlg.cleanup_btn.isEnabled()


def test_dialog_empty_folder(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    shown: list[str] = []
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: shown.append(a[2]))
    dlg = stub_dialog(tmp_path / "missing")
    assert dlg.table.rowCount() == 0 and "目前沒有備份" in dlg.summary.text()
    assert not dlg.open_folder() and "還不存在" in shown[-1]
    assert not dlg.open_selected() and dlg.compare_selected() is None and not dlg.delete_selected()


def test_dialog_open_folder(app: QApplication, folder: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtGui import QDesktopServices

    urls: list[str] = []
    monkeypatch.setattr(QDesktopServices, "openUrl", lambda url: urls.append(url.toLocalFile()) or True)
    assert stub_dialog(folder).open_folder()
    assert Path(urls[-1]) == folder


def test_dialog_delete_confirms_and_removes_both_files(app: QApplication, folder: Path,
                                                       monkeypatch: pytest.MonkeyPatch) -> None:
    asked: list[str] = []
    answer = [QMessageBox.StandardButton.No]
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: asked.append(a[2]) or answer[0])
    before = names(folder)
    dlg = stub_dialog(folder)
    dlg.select(2)  # the before_write backup
    assert not dlg.delete_selected() and names(folder) == before
    assert "20261002_100000_before_write_DRS-8500.bin" in asked[-1] and "最近一次寫入" in asked[-1]
    answer[0] = QMessageBox.StandardButton.Yes
    assert dlg.delete_selected()
    assert names(folder) == before - {"20261002_100000_before_write_DRS-8500.F136",
                                      "20261002_100000_before_write_DRS-8500.bin"}
    assert dlg.table.rowCount() == 3 and dlg.selected() is None
    assert [b.kind for b in dlg.backups] == ["read"] * 3
    dlg.select(0)  # unreadable, no .bin
    assert dlg.delete_selected() and "最近一次寫入" not in asked[-1]
    assert names(folder) == before - {"20261002_100000_before_write_DRS-8500.F136",
                                      "20261002_100000_before_write_DRS-8500.bin",
                                      "20261004_120000_read_DRS-8500.F136"}
    assert set(BYSTANDERS) <= names(folder)


def test_dialog_cleanup_keeps_newest_and_last_before_write(app: QApplication, folder: Path,
                                                           monkeypatch: pytest.MonkeyPatch) -> None:
    asked: list[str] = []
    monkeypatch.setattr(QMessageBox, "question",
                        lambda *a, **k: asked.append(a[2]) or QMessageBox.StandardButton.Yes)
    dlg = stub_dialog(folder)
    dlg.keep.setValue(1)
    assert dlg.cleanup() == 2
    assert "將刪除 2 份較舊的備份（2026-10-01 09:00:00 ～ 2026-10-03 11:00:00" in asked[-1]
    assert "另外保留最近一次寫入前的備份（2026-10-02 10:00:00）" in asked[-1]
    assert sorted(n for n in names(folder) if n not in BYSTANDERS) == [
        "20261002_100000_before_write_DRS-8500.F136", "20261002_100000_before_write_DRS-8500.bin",
        "20261004_120000_read_DRS-8500.F136"]
    assert set(BYSTANDERS) <= names(folder)
    assert [b.path.stem for b in dlg.backups] == ["20261004_120000_read_DRS-8500",
                                                 "20261002_100000_before_write_DRS-8500"]
    assert dlg.cleanup() == 0 and not dlg.cleanup_btn.isEnabled()  # nothing more to delete


def test_dialog_cleanup_cancelled(app: QApplication, folder: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.No)
    before = names(folder)
    dlg = stub_dialog(folder)
    dlg.keep.setValue(1)
    assert dlg.cleanup() == 0 and names(folder) == before and len(dlg.backups) == 4


def test_dialog_open_and_compare_call_back(app: QApplication, folder: Path) -> None:
    opened: list[Path] = []
    compared: list[Path] = []
    dlg = stub_dialog(folder, opened, compared, open_ok=False)
    dlg.select(1)
    report = dlg.compare_selected()
    assert compared == [folder / "20261003_110000_read_DRS-8500.F136"] and report is not None
    report.close()
    assert not dlg.open_selected() and opened == compared and dlg.result() == 0  # stays open
    dlg = stub_dialog(folder, opened, compared, open_ok=True)
    dlg.select(3)
    assert dlg.open_selected() and opened[-1].name == "20261001_090000_read_DRS-8600.F136"
    assert dlg.result() == QDialog.DialogCode.Accepted


# ------------------------------------------------------------------ main window


def _window(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    from opendrs.gui.main_window import MainWindow

    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: None)
    folder = make_backups(tmp_path / "appdata" / "OpenDRS" / "backups")
    project = tmp_path / "current.F136"
    project.write_bytes((FIXTURES / "dh98vu_20250811.F136").read_bytes())
    return MainWindow(), folder, project


def test_menu_entry_and_busy_radio(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    win, folder, _ = _window(tmp_path, monkeypatch)
    radio_menu = next(a.menu() for a in win.menuBar().actions() if a.text() == "對講機(&R)")
    assert "備份管理…" in [a.text() for a in radio_menu.actions()]
    dlg = win.backup_manager()
    assert dlg is not None and dlg.table.rowCount() == 4 and dlg.backups[0].path.parent == folder
    dlg.reject()
    monkeypatch.setattr(win._radio, "is_running", lambda: True)
    assert win.backup_manager() is None


def test_open_backup_through_main_window(app: QApplication, tmp_path: Path,
                                         monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QFileDialog

    win, folder, project = _window(tmp_path, monkeypatch)
    assert win.open_path(project)
    recent = win._settings.recent_files
    win._models["channels"].setData(win._models["channels"].index(0, 1), "EDIT")
    assert win.is_dirty()
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Cancel)
    dlg = win.backup_manager()
    dlg.select(2)  # before_write: mmdvm_20241127
    assert not dlg.open_selected()  # unsaved changes kept
    assert win.is_dirty() and len(win._project.channels) == 24 and dlg.isVisible()

    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Discard)
    backup = folder / "20261002_100000_before_write_DRS-8500.F136"
    original = backup.read_bytes()
    assert dlg.open_selected() and not dlg.isVisible()
    assert len(win._project.zones) == 2 and len(win._project.channels) == 3 and not win.is_dirty()
    assert win._path is None and backup.name in win.windowTitle()
    assert win._settings.recent_files == recent  # backups stay out of 最近開啟
    # Saving asks for a new name; the backup itself is untouched.
    saved_as = tmp_path / "from_backup.F136"
    monkeypatch.setattr(QFileDialog, "getSaveFileName", lambda *a, **k: (str(saved_as), ""))
    win._models["channels"].setData(win._models["channels"].index(0, 1), "NEW")
    assert win.save()
    assert backup.read_bytes() == original and not backup.with_name(backup.name + ".bak").exists()
    assert f136.load(saved_as).channels[0].name(win.codec) == "NEW"


def test_compare_backup_through_main_window(app: QApplication, tmp_path: Path,
                                            monkeypatch: pytest.MonkeyPatch) -> None:
    win, _folder, project = _window(tmp_path, monkeypatch)
    dlg = win.backup_manager()
    dlg.select(1)
    assert dlg.compare_selected() is None  # no project open yet
    assert win.open_path(project)
    report = dlg.compare_selected()
    assert report is not None and "20261003_110000_read_DRS-8500.F136" in report._title
    assert "current.F136" in report._title and report.isVisible()
    report.close()
    critical: list[str] = []
    monkeypatch.setattr(QMessageBox, "critical", lambda *a, **k: critical.append(a[2]))
    dlg.select(0)  # unreadable
    assert dlg.compare_selected() is None and "20261004_120000_read_DRS-8500.F136" in critical[-1]
    dlg.reject()
