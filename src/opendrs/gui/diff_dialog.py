"""Show a project comparison report."""

from __future__ import annotations

from pathlib import Path

from PySide6.QtGui import QFontDatabase
from PySide6.QtWidgets import (
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QLabel,
    QMessageBox,
    QPlainTextEdit,
    QVBoxLayout,
)


class DiffDialog(QDialog):
    def __init__(self, title: str, lines: list[str], parent=None) -> None:
        super().__init__(parent)
        self.setWindowTitle("比較專案")
        self.resize(900, 600)
        layout = QVBoxLayout(self)
        layout.addWidget(QLabel(title))
        self._text = QPlainTextEdit("\n".join(lines))
        self._text.setReadOnly(True)
        self._text.setFont(QFontDatabase.systemFont(QFontDatabase.SystemFont.FixedFont))
        layout.addWidget(self._text, 1)
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Save
                                   | QDialogButtonBox.StandardButton.Close)
        buttons.button(QDialogButtonBox.StandardButton.Save).setText("儲存報告…")
        buttons.accepted.connect(self._save)
        buttons.rejected.connect(self.reject)
        layout.addWidget(buttons)
        self._title = title

    def text(self) -> str:
        return self._text.toPlainText()

    def _save(self) -> None:
        path, _ = QFileDialog.getSaveFileName(self, "儲存比較報告", "diff.txt", "文字檔 (*.txt)")
        if not path:
            return
        try:
            Path(path).write_text(self._title + "\r\n\r\n" + self.text().replace("\n", "\r\n"),
                                  encoding="utf-8-sig", newline="")  # already CRLF
        except OSError as exc:
            QMessageBox.critical(self, "儲存失敗", str(exc))
