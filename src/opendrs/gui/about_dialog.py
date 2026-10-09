"""「關於本軟體」dialog: name, version, author, environment, data folder."""

from __future__ import annotations

import platform
import sys
from pathlib import Path

import PySide6
from PySide6.QtCore import Qt, QUrl, qVersion
from PySide6.QtGui import QDesktopServices, QGuiApplication
from PySide6.QtWidgets import (
    QDialog,
    QDialogButtonBox,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from .. import __author__, __version__, hardware
from ..settings import default_dir
from . import theme

APP_NAME = "OpenDRS"
SUMMARY = "Kydera DR-8500UV／HORA DH-98VU 對講機寫頻軟體：專案編輯、讀寫頻、韌體升級（IAP）、通話錄音讀取。"
RADIO = (f"{hardware.supported_text()}；韌體 308E.TARSCB.003（BK4819 機型回報 308E.BBKTARSCB.003）")
NOTES = ("聯絡人匯入使用 RadioID.net 的公開資料；錄音解碼呼叫原廠附帶的 DVSI ambep2.exe"
         "（授權軟體，本程式不附帶也不重寫）。")


def environment() -> str:
    return (f"Python {platform.python_version()}、PySide6 {PySide6.__version__}（Qt {qVersion()}）、"
            f"{platform.system()} {platform.release()}（{platform.machine()}）")


def about_text(data_dir: Path | None = None) -> str:
    """Plain-text version information (for the clipboard and bug reports)."""
    folder = data_dir or default_dir()
    return "\n".join([
        f"{APP_NAME} {__version__}",
        f"作者：{__author__}",
        f"支援機型：{RADIO}",
        f"執行環境：{environment()}",
        f"程式位置：{Path(sys.argv[0]).resolve() if sys.argv and sys.argv[0] else '?'}",
        f"資料夾：{folder}",
    ])


class AboutDialog(QDialog):
    def __init__(self, parent: QWidget | None = None, data_dir: Path | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("關於本軟體")
        self._dir = data_dir or default_dir()

        icon = QLabel()
        icon.setPixmap(theme.app_icon().pixmap(64, 64))
        title = QLabel(APP_NAME)
        font = title.font()
        font.setPointSizeF(font.pointSizeF() * 1.6)
        font.setBold(True)
        title.setFont(font)
        subtitle = QLabel(f"版本 {__version__}　·　作者 {__author__}")
        summary = QLabel(SUMMARY)
        summary.setWordWrap(True)
        heading = QVBoxLayout()
        for w in (title, subtitle, summary):
            heading.addWidget(w)
        top = QHBoxLayout()
        top.addWidget(icon, 0, Qt.AlignmentFlag.AlignTop)
        top.addLayout(heading, 1)

        self.version = QLabel(__version__)
        self.author = QLabel(__author__)
        folder = QLabel(f'<a href="{QUrl.fromLocalFile(str(self._dir)).toString()}">{self._dir}</a>')
        folder.setOpenExternalLinks(True)
        form = QFormLayout()
        form.addRow("版本", self.version)
        form.addRow("作者", self.author)
        form.addRow("支援機型", _wrapped(RADIO))
        form.addRow("執行環境", _wrapped(environment()))
        form.addRow("資料夾", folder)
        notes = _wrapped(NOTES)
        notes.setObjectName("muted")

        self.copy = QPushButton("複製版本資訊")
        self.copy.setAutoDefault(False)
        self.copy.clicked.connect(self.copy_info)
        open_dir = QPushButton("開啟資料夾")
        open_dir.setAutoDefault(False)
        open_dir.clicked.connect(lambda: QDesktopServices.openUrl(QUrl.fromLocalFile(str(self._dir))))
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        buttons.addButton(self.copy, QDialogButtonBox.ButtonRole.ActionRole)
        buttons.addButton(open_dir, QDialogButtonBox.ButtonRole.ActionRole)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addLayout(top)
        layout.addSpacing(8)
        layout.addLayout(form)
        layout.addWidget(notes)
        layout.addWidget(buttons)
        self.resize(560, self.sizeHint().height())

    def copy_info(self) -> str:
        text = about_text(self._dir)
        QGuiApplication.clipboard().setText(text)
        self.copy.setText("已複製")
        return text


def _wrapped(text: str) -> QLabel:
    label = QLabel(text)
    label.setWordWrap(True)
    label.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
    return label
