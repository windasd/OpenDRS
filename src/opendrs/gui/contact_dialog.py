"""新增聯絡人: type, name and ID of one contact; the main window checks it before accepting."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtCore import QRegularExpression
from PySide6.QtGui import QRegularExpressionValidator
from PySide6.QtWidgets import (
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QFormLayout,
    QLabel,
    QLineEdit,
    QVBoxLayout,
    QWidget,
)

from ..records import NAME_SIZE, Contact

# (type, name, ID) -> why it cannot be added, or None
Check = Callable[[int, str, int], str | None]


class AddContactDialog(QDialog):
    """Asks for one contact. 確定 runs ``check`` and stays open, showing the reason, while the
    contact cannot be added (duplicate type and ID, name too long, list full…)."""

    def __init__(self, kind: int, check: Check, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("新增聯絡人")
        self._check = check
        self.kind = QComboBox()
        for value, label in Contact.KINDS.items():
            self.kind.addItem(label, value)
        self.kind.setCurrentIndex(max(self.kind.findData(kind), 0))
        self.name = QLineEdit()
        self.name.setPlaceholderText(f"最多 {NAME_SIZE} bytes（中文字每字 2 bytes）")
        self.call_id = QLineEdit()
        # Digits only (QIntValidator also takes "1,000" and "+5"); the range is checked on 確定.
        self.call_id.setValidator(QRegularExpressionValidator(QRegularExpression(r"\d{0,8}"), self))
        self.call_id.setPlaceholderText(f"1–{Contact.MAX_ID}")
        self.error = QLabel()
        self.error.setObjectName("danger")
        self.error.setWordWrap(True)
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok
                                   | QDialogButtonBox.StandardButton.Cancel)
        buttons.accepted.connect(self.accept)
        buttons.rejected.connect(self.reject)
        form = QFormLayout()
        form.addRow("類型：", self.kind)
        form.addRow("名稱：", self.name)
        form.addRow("ID：", self.call_id)
        note = QLabel("加在清單最後面；其他聯絡人的編號不變。")
        note.setObjectName("muted")
        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(note)
        layout.addWidget(self.error)
        layout.addWidget(buttons)
        self.resize(380, self.sizeHint().height())

    def values(self) -> tuple[int, str, int]:
        """(type, name, ID); ID 0 while the box is empty."""
        text = self.call_id.text().strip()
        return int(self.kind.currentData()), self.name.text().strip(), int(text) if text.isdecimal() else 0

    def accept(self) -> None:
        if not self.call_id.text().strip():
            self.error.setText("請輸入 ID")
            return
        kind, name, call_id = self.values()
        problem = self._check(kind, name, call_id)
        if problem:
            self.error.setText(problem)
            return
        super().accept()
