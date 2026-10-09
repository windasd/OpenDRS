"""「關於本軟體」: version, author, environment, menu entry."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
pytest.importorskip("PySide6.QtCore")
from PySide6.QtGui import QGuiApplication  # noqa: E402
from PySide6.QtWidgets import QApplication  # noqa: E402

from opendrs import __author__, __version__  # noqa: E402
from opendrs.gui.about_dialog import AboutDialog, about_text  # noqa: E402


@pytest.fixture
def app() -> QApplication:
    return QApplication.instance() or QApplication([])


def test_package_metadata() -> None:
    assert __author__ == "windasd"
    pyproject = (Path(__file__).resolve().parents[1] / "pyproject.toml").read_text(encoding="utf-8")
    assert f'version = "{__version__}"' in pyproject and 'name = "windasd"' in pyproject


def test_about_text(tmp_path: Path) -> None:
    text = about_text(tmp_path)
    assert f"OpenDRS {__version__}" in text and "作者：windasd" in text
    assert str(tmp_path) in text and "Python" in text and "PySide6" in text


def test_about_dialog_shows_and_copies(app: QApplication, tmp_path: Path) -> None:
    dialog = AboutDialog(data_dir=tmp_path)
    assert dialog.version.text() == __version__ and dialog.author.text() == "windasd"
    copied = dialog.copy_info()
    assert QGuiApplication.clipboard().text() == copied and "windasd" in copied
    assert dialog.copy.text() == "已複製"


def test_menu_entry_opens_the_dialog(app: QApplication, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("APPDATA", str(tmp_path / "appdata"))
    from opendrs.gui.main_window import MainWindow

    win = MainWindow()
    tools = next(a.menu() for a in win.menuBar().actions() if a.text() == "工具(&T)")
    entry = next(a for a in tools.actions() if a.text() == "關於本軟體…")
    entry.trigger()
    dialogs = [w for w in app.topLevelWidgets() if isinstance(w, AboutDialog) and w.isVisible()]
    assert dialogs and dialogs[-1].windowTitle() == "關於本軟體"
    for d in dialogs:
        d.close()
    win.close()
