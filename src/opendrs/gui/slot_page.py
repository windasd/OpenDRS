"""Shared UI for fixed-slot lists (scan lists, RX group lists): a table of slots and
a dialog that renames one slot and ticks its members."""

from __future__ import annotations

from collections.abc import Callable, Hashable, Sequence

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QAbstractItemView,
    QDialog,
    QDialogButtonBox,
    QFormLayout,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QPushButton,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

SLOT_COLUMNS = ("#", "名稱", "數量", "成員")


class MemberPickerDialog(QDialog):
    """Rename a slot and tick up to ``limit`` members; members keep the given order."""

    def __init__(self, title: str, name: str, choices: Sequence[tuple[str, Hashable]],
                 checked: set, limit: int, unit: str, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle(title)
        self.resize(480, 600)
        self._limit, self._unit = limit, unit
        self.name = QLineEdit(name)
        self.name.setPlaceholderText("名稱（最多 10 bytes）")
        self.search = QLineEdit(placeholderText="篩選清單…")
        self.search.setClearButtonEnabled(True)
        self.search.textChanged.connect(self._filter)
        self.items = QListWidget()
        for label, value in choices:
            item = QListWidgetItem(label)
            item.setData(Qt.ItemDataRole.UserRole, value)
            item.setFlags(item.flags() | Qt.ItemFlag.ItemIsUserCheckable)
            item.setCheckState(Qt.CheckState.Checked if value in checked else Qt.CheckState.Unchecked)
            self.items.addItem(item)
        self.count = QLabel()
        self.items.itemChanged.connect(self._update_count)
        self.buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok
                                        | QDialogButtonBox.StandardButton.Cancel)
        self.buttons.accepted.connect(self.accept)
        self.buttons.rejected.connect(self.reject)
        form = QFormLayout()
        form.addRow("名稱：", self.name)
        self.form = form  # subclasses add their own settings rows
        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(QLabel(f"勾選要加入的{unit}："))
        layout.addWidget(self.search)
        layout.addWidget(self.items, 1)
        layout.addWidget(self.count)
        layout.addWidget(self.buttons)
        self._update_count()

    def values(self) -> list:
        return [self.items.item(i).data(Qt.ItemDataRole.UserRole)
                for i in range(self.items.count())
                if self.items.item(i).checkState() == Qt.CheckState.Checked]

    def _filter(self, text: str) -> None:
        terms = text.casefold().split()
        for i in range(self.items.count()):
            item = self.items.item(i)
            label = item.text().casefold()
            item.setHidden(not all(t in label for t in terms))

    def _update_count(self, *_args: object) -> None:
        n = len(self.values())
        over = n > self._limit
        self.count.setText(f"已勾選 {n} / {self._limit} 個{self._unit}" + ("　⚠ 超過上限" if over else ""))
        self.buttons.button(QDialogButtonBox.StandardButton.Ok).setEnabled(not over)


class SlotTablePage(QWidget):
    """Read-only table of slots; double-click or the button calls ``on_edit(number)``."""

    def __init__(self, note: str, on_edit: Callable[[int], None],
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._on_edit = on_edit
        self.table = QTableWidget(0, len(SLOT_COLUMNS))
        self.table.setHorizontalHeaderLabels(SLOT_COLUMNS)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.table.verticalHeader().setVisible(False)
        header = self.table.horizontalHeader()
        header.setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        header.setStretchLastSection(True)
        self.table.cellDoubleClicked.connect(lambda row, _col: self._on_edit(row + 1))
        self.edit_button = QPushButton("編輯選取的清單…")
        self.edit_button.clicked.connect(self._edit_selected)
        self.note = QLabel(note)
        self.note.setWordWrap(True)
        bar = QHBoxLayout()
        bar.addWidget(self.note, 1)
        bar.addWidget(self.edit_button)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.addLayout(bar)
        layout.addWidget(self.table)

    def selected_number(self) -> int | None:
        rows = self.table.selectionModel().selectedRows()
        return rows[0].row() + 1 if rows else None

    def _edit_selected(self) -> None:
        number = self.selected_number()
        if number is not None:
            self._on_edit(number)

    def set_rows(self, rows: Sequence[tuple[str, int, str, bool]], enabled: bool) -> int:
        """rows: (name, member count, members text, in use); returns slots in use."""
        self.table.setRowCount(len(rows))
        self.setEnabled(enabled)
        for row, (name, count, members, in_use) in enumerate(rows):
            values = (str(row + 1), name if in_use else "（未使用）", str(count), members)
            for col, text in enumerate(values):
                item = QTableWidgetItem(text)
                if not in_use:
                    item.setForeground(Qt.GlobalColor.gray)
                self.table.setItem(row, col, item)
        return sum(1 for r in rows if r[3])
