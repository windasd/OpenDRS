"""``opendrs --self-test REPORT.json``: prove that a build (the PyInstaller .exe in particular)
starts and has every part it needs, without a display or a radio.

Each check exercises something a frozen build can silently lose: Qt platform and image
plugins, Qt's translations, print/PDF support, SVG icons, TLS for RadioID downloads,
pyserial's port enumeration, the Big5/GBK codecs. The report is a JSON file because a
windowed .exe has no console; the exit code is 0 only when every check passed.
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
from collections.abc import Callable
from pathlib import Path

from . import __version__


def _qt() -> str:
    from PySide6.QtWidgets import QApplication

    app = QApplication.instance() or QApplication([sys.argv[0]])
    return f"platform {app.platformName()}"


def _translations() -> str:
    from PySide6.QtWidgets import QApplication, QDialogButtonBox

    from .gui.translations import install_qt_translations

    app = QApplication.instance()
    installed = install_qt_translations(app)
    try:
        box = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        text = box.button(QDialogButtonBox.StandardButton.Close).text().replace("&", "")
    finally:
        for translator in installed:
            app.removeTranslator(translator)
    if text != "關閉":
        raise RuntimeError(f"Close 按鈕顯示「{text}」（找到 {len(installed)} 個翻譯檔）")
    return text


def _icon() -> str:
    from .gui.theme import app_icon

    if app_icon().pixmap(32, 32).isNull():
        raise RuntimeError("SVG 圖示無法繪製")
    return "32×32"


def _main_window() -> str:
    from .gui.main_window import MainWindow
    from .settings import Settings

    win = MainWindow(Settings())
    try:
        menus = [a.text() for a in win.menuBar().actions()]
    finally:
        win.close()
        win.deleteLater()
    return "、".join(menus)


def _pdf(folder: Path) -> str:
    from .gui.print_report import write_pdf

    out = folder / "selftest.pdf"
    write_pdf("<h1>自我測試</h1><table><tr><td>頻道</td></tr></table>", out)
    if not out.read_bytes().startswith(b"%PDF-"):
        raise RuntimeError("不是 PDF")
    return f"{out.stat().st_size} bytes"


def _printer() -> str:
    from PySide6.QtPrintSupport import QPrinter

    printer = QPrinter(QPrinter.PrinterMode.HighResolution)
    return printer.pageLayout().pageSize().name()


def _ssl() -> str:
    import ssl

    ssl.create_default_context()
    return ssl.OPENSSL_VERSION


def _serial() -> str:
    from .radio_link import list_ports

    return f"{len(list_ports())} 個連接埠"


def _update() -> str:
    from . import update

    if not update.parse_version("v0.2.0") > update.parse_version("0.1.0"):
        raise RuntimeError("版本比較錯誤")
    if "Move-Item" not in update.updater_script():
        raise RuntimeError("更新腳本不完整")
    return update.API_LATEST


def _codecs() -> str:
    for codec in ("big5", "gbk", "cp950"):
        "測試頻道".encode(codec)
    return "big5、gbk、cp950"


def run(report: Path) -> int:
    """Run every check in an isolated data folder; write ``report``; 0 when all passed."""
    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    saved = os.environ.get("APPDATA")
    results: dict[str, str] = {}
    with tempfile.TemporaryDirectory(prefix="opendrs_selftest_", ignore_cleanup_errors=True) as tmp:
        os.environ["APPDATA"] = tmp  # never read or write the user's settings
        checks: list[tuple[str, Callable[[], str]]] = [
            ("qt", _qt), ("translations", _translations), ("icon", _icon), ("main_window", _main_window),
            ("pdf", lambda: _pdf(Path(tmp))), ("printer", _printer), ("ssl", _ssl), ("serial", _serial),
            ("codecs", _codecs), ("update", _update),
        ]
        try:
            for name, check in checks:
                try:
                    results[name] = "ok: " + check()
                except Exception as exc:  # noqa: BLE001 - every failure belongs in the report
                    results[name] = f"FAIL: {type(exc).__name__}: {exc}"
        finally:
            if saved is None:
                os.environ.pop("APPDATA", None)
            else:
                os.environ["APPDATA"] = saved
    ok = all(v.startswith("ok") for v in results.values())
    report.write_text(json.dumps({"version": __version__, "frozen": bool(getattr(sys, "frozen", False)),
                                  "ok": ok, "checks": results}, ensure_ascii=False, indent=2), encoding="utf-8")
    return 0 if ok else 1
