"""常用短信 tab: 16 editable rows."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtWidgets import (
    QHeaderView,
    QLabel,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from ..quickmsg import (
    QUICK_MSG_COUNT,
    QUICK_MSG_SIZE,
    has_quick_messages,
    quick_messages,
)
from ..textcodec import NameCodec


class QuickMessagePage(QWidget):
    """Edits call ``apply(number, text)``; the window re-fills the table afterwards."""

    def __init__(self, apply: Callable[[int, str], None], parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._apply = apply
        self._filling = False
        self.table = QTableWidget(QUICK_MSG_COUNT, 1)
        self.table.setHorizontalHeaderLabels([f"常用短信內容（最多 {QUICK_MSG_SIZE} bytes，中文每字 2 bytes）"])
        self.table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.Stretch)
        self.table.itemChanged.connect(self._changed)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.addWidget(QLabel("雙擊一列可編輯；對講機的「常用短信」選單會列出這些內容。"))
        layout.addWidget(self.table)

    def _changed(self, item: QTableWidgetItem) -> None:
        if not self._filling:
            self._apply(item.row() + 1, item.text())

    def set_image(self, image: bytes, codec: NameCodec) -> None:
        self._filling = True
        try:
            self.setEnabled(has_quick_messages(image))
            for row, text in enumerate(quick_messages(image, codec) or [""] * QUICK_MSG_COUNT):
                self.table.setItem(row, 0, QTableWidgetItem(text))
        finally:
            self._filling = False
