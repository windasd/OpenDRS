"""Entry point: ``python -m opendrs [project.F136]`` (or ``OpenDRS.exe``).

Extra options: ``--version`` and ``--self-test REPORT.json`` (see selftest.py).
"""

from __future__ import annotations

import logging
import logging.handlers
import sys
from pathlib import Path

LOG_BYTES = 1_000_000
LOG_BACKUPS = 3


def _setup_logging() -> None:
    """Console when there is one; the windowed .exe has none, so it logs to a file."""
    fmt = "%(asctime)s %(levelname)s %(name)s: %(message)s"
    if sys.stderr is not None:
        logging.basicConfig(level=logging.INFO, format=fmt)
        return
    from .settings import default_dir

    folder = default_dir() / "logs"
    try:
        folder.mkdir(parents=True, exist_ok=True)
        handler: logging.Handler = logging.handlers.RotatingFileHandler(
            folder / "opendrs.log", maxBytes=LOG_BYTES, backupCount=LOG_BACKUPS, encoding="utf-8")
    except OSError:
        handler = logging.NullHandler()  # read-only profile: run without a log
    logging.basicConfig(level=logging.INFO, format=fmt, handlers=[handler])


def main(argv: list[str] | None = None) -> int:
    args = sys.argv[1:] if argv is None else argv
    from . import __version__

    if args[:1] == ["--version"]:
        if sys.stdout is not None:
            print(__version__)
        return 0
    if args[:1] == ["--self-test"]:
        from .selftest import run

        return run(Path(args[1]) if len(args) > 1 else Path("opendrs_selftest.json"))

    from .settings import migrate_legacy_dir

    migrated = migrate_legacy_dir()  # before logging, which creates the new data folder
    _setup_logging()
    if migrated:
        logging.getLogger(__name__).info(migrated)
    from PySide6.QtWidgets import QApplication

    from .gui.main_window import MainWindow
    from .gui.theme import ThemeMode, app_icon, apply_theme
    from .gui.translations import install_qt_translations
    from .settings import load_settings

    app = QApplication(sys.argv[:1])
    app.setApplicationName("OpenDRS")
    app.setApplicationVersion(__version__)
    app.setWindowIcon(app_icon())
    install_qt_translations(app)
    settings = load_settings()
    try:
        mode = ThemeMode(settings.theme)
    except ValueError:
        mode = ThemeMode.AUTO
    apply_theme(app, mode)
    window = MainWindow(settings)
    if args:
        window.open_path(Path(args[0]))
    window.show()
    if migrated:
        window.statusBar().showMessage(migrated, 20_000)
    return app.exec()


if __name__ == "__main__":
    raise SystemExit(main())
