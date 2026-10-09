"""Qt's own Traditional Chinese strings for standard widgets (Close / Yes / Cancel,
file and print dialogs), shipped with PySide6."""

from __future__ import annotations

import logging

from PySide6.QtCore import QCoreApplication, QLibraryInfo, QTranslator

log = logging.getLogger(__name__)

LOCALE = "zh_TW"
CATALOGS = ("qtbase",)


def install_qt_translations(app: QCoreApplication, locale: str = LOCALE) -> list[QTranslator]:
    """Install the catalogs that exist; the app keeps working in English without them."""
    folder = QLibraryInfo.path(QLibraryInfo.LibraryPath.TranslationsPath)
    installed = []
    for catalog in CATALOGS:
        translator = QTranslator(app)  # parented: lives as long as the application
        if translator.load(f"{catalog}_{locale}", folder) and app.installTranslator(translator):
            installed.append(translator)
        else:
            log.info("Qt translation %s_%s not found in %s", catalog, locale, folder)
    return installed
