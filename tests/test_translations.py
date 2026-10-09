"""Standard Qt buttons appear in Traditional Chinese."""

from __future__ import annotations

import os

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from PySide6.QtWidgets import QApplication, QDialogButtonBox

from opendrs.gui.translations import install_qt_translations


def _close_text() -> str:
    box = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
    return box.button(QDialogButtonBox.StandardButton.Close).text().replace("&", "")


def test_standard_buttons_are_translated() -> None:
    app = QApplication.instance() or QApplication([])
    installed = install_qt_translations(app)
    try:
        assert len(installed) == 1
        assert _close_text() == "關閉"
    finally:
        for translator in installed:
            app.removeTranslator(translator)
    assert _close_text() == "Close"


def test_missing_catalog_is_harmless() -> None:
    app = QApplication.instance() or QApplication([])
    assert install_qt_translations(app, locale="xx_XX") == []
