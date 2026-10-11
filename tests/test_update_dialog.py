"""檢查更新 dialog, the start-up check and handing over to the updater (no network)."""

from __future__ import annotations

import os
from datetime import UTC, datetime, timedelta
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from PySide6.QtWidgets import QApplication, QMessageBox

from opendrs import __url__, update
from opendrs.gui.update_dialog import UpdateDialog
from opendrs.update import PreparedUpdate, UpdateError

from .test_update import release_json

DH98 = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"


@pytest.fixture
def app(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> QApplication:
    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    return QApplication.instance() or QApplication([])


def _release(version: str) -> update.Release:
    return update.parse_release(release_json(version, b"zip"))


def _checked(dialog: UpdateDialog, app: QApplication) -> UpdateDialog:
    dialog.check()
    dialog._check.wait(10_000)
    app.processEvents()
    return dialog


def test_newer_release_can_be_installed(app: QApplication, tmp_path: Path) -> None:
    dialog = _checked(UpdateDialog(current="0.1.0", fetch=lambda: _release("0.2.0"), target=tmp_path), app)
    assert "有新版本 v0.2.0" in dialog.status.text() and dialog.install_button.isEnabled()
    assert "最新版本：0.2.0" in dialog.versions.text() and dialog.notes.isVisibleTo(dialog)


def test_up_to_date_and_failures(app: QApplication, tmp_path: Path) -> None:
    dialog = _checked(UpdateDialog(current="0.2.0", fetch=lambda: _release("0.2.0"), target=tmp_path), app)
    assert dialog.status.text() == "已是最新版本。" and not dialog.install_button.isEnabled()

    def offline() -> update.Release:
        raise UpdateError("查詢最新版本：無法連線（timed out）")

    dialog = _checked(UpdateDialog(current="0.1.0", fetch=offline, target=tmp_path), app)
    assert dialog.status.text().startswith("無法檢查更新：查詢最新版本：無法連線")


def test_running_from_source_points_to_git(app: QApplication) -> None:
    dialog = _checked(UpdateDialog(current="0.1.0", fetch=lambda: _release("0.2.0"), target=None), app)
    assert not dialog.install_button.isVisibleTo(dialog) and "git pull" in dialog.status.text()


@pytest.mark.parametrize("confirm", [True, False])
def test_install_hands_over_only_after_confirmation(app: QApplication, tmp_path: Path,
                                                    monkeypatch: pytest.MonkeyPatch, confirm: bool) -> None:
    staging = tmp_path / ".opendrs-update-0.2.0"
    prepared = PreparedUpdate("0.2.0", staging / "unpacked" / "OpenDRS", tmp_path, staging)

    def fake_prepare(release, target, progress, cancelled):
        staging.mkdir(parents=True)
        progress(5, 10)
        return prepared

    handed: list[PreparedUpdate] = []
    answer = QMessageBox.StandardButton.Yes if confirm else QMessageBox.StandardButton.No
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: answer)
    dialog = _checked(UpdateDialog(current="0.1.0", fetch=lambda: _release("0.2.0"), prepare=fake_prepare,
                                   target=tmp_path, on_install=lambda p: handed.append(p) or True), app)
    dialog.install()
    dialog._download.wait(10_000)
    app.processEvents()
    assert handed == ([prepared] if confirm else [])
    assert staging.exists() == confirm  # declined: the download is cleaned up
    assert dialog.install_button.text() == "下載並安裝"


def _window(tmp_path: Path):
    from opendrs.gui.main_window import MainWindow

    win = MainWindow()
    target = tmp_path / "p.F136"
    target.write_bytes(DH98.read_bytes())
    assert win.open_path(target)
    return win


def test_startup_check_is_daily_and_announces_a_new_version(app: QApplication, tmp_path: Path) -> None:
    from opendrs.settings import load_settings

    win = _window(tmp_path)
    labels = [a.text() for a in win.menuBar().actions()[-2].menu().actions()]
    assert "檢查更新…" in labels
    now = datetime(2026, 11, 2, 9, 0, tzinfo=UTC)
    assert win.start_update_check(lambda: _release("9.0.0"), now)
    win._auto_check.wait(10_000)
    app.processEvents()
    assert win._a_update.text() == "檢查更新…（有新版本 v9.0.0）"
    assert "v9.0.0" in win.statusBar().currentMessage()
    assert load_settings().last_update_check == now.isoformat()
    assert not win.start_update_check(lambda: _release("9.0.0"), now + timedelta(hours=1))  # not due yet
    win._set_auto_check(False)
    assert not win.start_update_check(lambda: _release("9.0.0"), now + timedelta(days=2))  # turned off
    assert load_settings().check_updates is False
    win.close()


def test_apply_update_asks_about_unsaved_changes_first(app: QApplication, tmp_path: Path,
                                                       monkeypatch: pytest.MonkeyPatch) -> None:
    win = _window(tmp_path)
    win._history.push(win._project.with_alias("QT0EST"), "edit")
    prepared = PreparedUpdate("0.2.0", tmp_path, tmp_path, tmp_path)
    launched: list[tuple] = []
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Cancel)
    assert not win.apply_update(prepared, launcher=lambda *a: launched.append(a)) and not launched
    monkeypatch.setattr(QMessageBox, "question", lambda *a, **k: QMessageBox.StandardButton.Discard)
    assert win.apply_update(prepared, launcher=lambda *a: launched.append(a))
    assert launched and launched[0][1].name == "update.log" and not win.isVisible()


def test_about_shows_the_website_and_offers_an_update_check(app: QApplication, tmp_path: Path) -> None:
    from opendrs.gui.about_dialog import AboutDialog, about_text

    assert f"網站：{__url__}" in about_text(tmp_path)
    win = _window(tmp_path)
    about = AboutDialog(win)
    assert __url__ in about.website.text() and about.check_updates.text() == "檢查更新…"
    win.close()
