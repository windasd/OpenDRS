from __future__ import annotations

import os
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
pytest.importorskip("PySide6.QtWidgets")
from PySide6.QtCore import Qt  # noqa: E402
from PySide6.QtWidgets import QApplication, QMessageBox  # noqa: E402

from opendrs import f136, radioid  # noqa: E402
from opendrs.contact_import import NameFormat  # noqa: E402
from opendrs.gui.radioid_dialog import RadioIdDialog  # noqa: E402
from opendrs.radioid import UserDb, parse_csv  # noqa: E402

from .test_radioid import CSV, opener_for  # noqa: E402


@pytest.fixture(scope="module")
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


@pytest.fixture
def db() -> UserDb:
    return UserDb(parse_csv(CSV).users)


def test_defaults_to_taiwan_and_marks_existing(app: QApplication, db: UserDb,
                                               small_path: Path, tmp_path: Path) -> None:
    project = f136.load(small_path)  # already holds 9990001 as a private contact? check
    dlg = RadioIdDialog(project, tmp_path, db=db)
    assert dlg.query().country == "Taiwan"
    model = dlg._model
    assert {u.callsign for u in model.rows()} == {"QA0BD", "QA0ADN", "QA0AHJ"}
    assert "符合 3 筆" in dlg._counts.text()


def test_check_select_all_and_result(app: QApplication, db: UserDb, tmp_path: Path) -> None:
    dlg = RadioIdDialog(f136.Project(), tmp_path, db=db)
    model = dlg._model
    assert model.setData(model.index(0, 0), Qt.CheckState.Checked, Qt.ItemDataRole.CheckStateRole)
    assert len(dlg.selected_users()) == 1
    model.set_all(True)
    assert [u.radio_id for u in dlg.selected_users()] == [9990001, 9990002, 9990010]
    assert "匯入 3 筆" in dlg._buttons.button(dlg._buttons.StandardButton.Ok).text()
    model.set_all(False)
    assert dlg.selected_users() == []
    assert not model.setData(model.index(0, 1), "x", Qt.ItemDataRole.EditRole)


def test_search_and_prefix_filters(app: QApplication, db: UserDb, tmp_path: Path) -> None:
    dlg = RadioIdDialog(f136.Project(), tmp_path, db=db)
    dlg._region.setCurrentIndex(0)  # 全部地區
    dlg._prefix.setText("999")
    dlg._search.setText("chen")
    dlg._apply_filter()
    assert [u.callsign for u in dlg._model.rows()] == ["QA0ADN"]
    dlg._prefix.setText("")
    dlg._search.setText("")
    dlg._apply_filter()
    assert dlg._model.rows() == []  # empty query shows nothing rather than 300k rows
    dlg._fmt.setCurrentIndex(1)
    assert dlg.name_format() is NameFormat.CALLSIGN_NAME


def test_existing_contacts_can_be_checked_to_overwrite(app: QApplication, db: UserDb, tmp_path: Path) -> None:
    from opendrs.contact_import import import_users
    from opendrs.textcodec import NameCodec

    project, _ = import_users(f136.Project(), [db.users[0]], NameFormat.CALLSIGN, NameCodec.ASCII)
    dlg = RadioIdDialog(project, tmp_path, db=db)
    model = dlg._model
    row = next(i for i, u in enumerate(model.rows()) if u.radio_id == db.users[0].radio_id)
    idx = model.index(row, 0)
    assert model.data(idx) == "已存在"
    assert model.flags(idx) & Qt.ItemFlag.ItemIsUserCheckable
    assert model.setData(idx, Qt.CheckState.Checked, Qt.ItemDataRole.CheckStateRole)
    assert db.users[0].radio_id in model.checked
    assert dlg._new_count() == 0  # overwrite does not use capacity
    assert "已存在 1）" in dlg._counts.text()
    model.set_all(True)
    assert db.users[0].radio_id in model.checked


def test_background_load_from_cache(app: QApplication, tmp_path: Path, qtbot_wait=None) -> None:
    radioid.download(tmp_path, opener=opener_for(CSV))
    dlg = RadioIdDialog(f136.Project(), tmp_path)
    assert dlg._thread is not None
    dlg._thread.wait(10_000)
    app.processEvents()
    assert dlg._db is not None and len(dlg._db) == 7
    assert "7 筆" in dlg._status.text()
    dlg.reject()


def test_main_window_import_flow(app: QApplication, small_path: Path, tmp_path: Path,
                                 db: UserDb, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    monkeypatch.setattr(QMessageBox, "information", lambda *a, **k: None)
    from opendrs.gui.main_window import MainWindow

    target = tmp_path / "p.F136"
    target.write_bytes(small_path.read_bytes())
    win = MainWindow()
    win.open_path(target)
    before = len(win._project.contacts)
    report = win.apply_import(list(db.search(radioid.Query(country="Taiwan"))),
                              NameFormat.CALLSIGN)
    assert report.added + report.updated + report.unchanged == 3
    assert len(win._project.contacts) == before + report.added
    assert win.is_dirty()
    assert win.save()
    assert len(f136.load(target).contacts) == before + report.added
