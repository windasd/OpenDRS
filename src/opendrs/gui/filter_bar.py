"""One table page: search / filter controls above a sortable table."""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QComboBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QPushButton,
    QTableView,
    QVBoxLayout,
    QWidget,
)

from ..records import Contact
from .models import RecordTableModel
from .record_filter import (
    RecordFilterProxy,
    RowFilter,
    control_char_rows,
    duplicate_id_rows,
    duplicate_name_rows,
    kind_filter,
)

ALL_COLUMNS = "全部欄位"
# Extra "顯示" choices per table; the contacts table also gets a type filter.
_SHOW_CHOICES: dict[str, tuple[tuple[str, RowFilter | None], ...]] = {
    "contacts": (("全部", None), ("名稱含換行字元", control_char_rows),
                 ("名稱重複", duplicate_name_rows), ("類型與 ID 重複", duplicate_id_rows)),
    "zones": (("全部", None), ("名稱含換行字元", control_char_rows),
              ("名稱重複", duplicate_name_rows)),
    "channels": (("全部", None), ("名稱含換行字元", control_char_rows),
                 ("名稱重複", duplicate_name_rows)),
}


def _combo(items: tuple[tuple[str, object], ...], tip: str) -> QComboBox:
    box = QComboBox()
    for label, value in items:
        box.addItem(label, value)
    box.setToolTip(tip)
    return box


class TablePage(QWidget):
    """Search box, column scope, optional type / show filters, count and a sortable view.

    Clicking a column header sorts (again: descending, third time: original order).
    Sorting and filtering never change the record order saved to the project.
    """

    def __init__(self, key: str, model: RecordTableModel, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._model = model
        self.proxy = RecordFilterProxy(self)
        self.proxy.setSourceModel(model)

        self.search = QLineEdit(placeholderText="搜尋（多個關鍵字以空白分隔，全部符合才列出）")
        self.search.setClearButtonEnabled(True)
        self.search.textChanged.connect(self.proxy.set_text)

        titles = [model.headerData(c, Qt.Orientation.Horizontal) for c in range(model.columnCount())]
        scope_items: tuple[tuple[str, object], ...] = ((ALL_COLUMNS, None),) + tuple(
            (str(t), (c,)) for c, t in enumerate(titles))
        self.scope = _combo(scope_items, "搜尋範圍")
        self.scope.currentIndexChanged.connect(
            lambda _i: self.proxy.set_columns(self.scope.currentData()))

        self.kind: QComboBox | None = None
        if key == "contacts":
            self.kind = _combo((("全部類型", None),) + tuple(
                (label, kind) for kind, label in Contact.KINDS.items()), "只顯示此類型")
            self.kind.currentIndexChanged.connect(self._on_kind)

        self.show_box = _combo(_SHOW_CHOICES[key], "快速篩選")
        self.show_box.currentIndexChanged.connect(
            lambda _i: self.proxy.set_row_filter("show", self.show_box.currentData()))

        reset = QPushButton("清除篩選與排序")
        reset.clicked.connect(self.reset)
        self.count = QLabel()
        self.count.setObjectName("muted")
        self.count.setMinimumWidth(120)

        self.view = QTableView()
        self.view.setModel(self.proxy)
        header = self.view.horizontalHeader()
        header.setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        header.setStretchLastSection(True)
        header.setSortIndicatorClearable(True)
        header.setSortIndicator(-1, Qt.SortOrder.AscendingOrder)
        self.view.setSortingEnabled(True)
        self.view.setAlternatingRowColors(True)
        self.view.verticalHeader().setVisible(False)

        changed = [self.search.textChanged, self.scope.currentIndexChanged,
                   self.show_box.currentIndexChanged]
        if self.kind is not None:
            changed.append(self.kind.currentIndexChanged)
        for signal in (*changed, self.proxy.rowsInserted, self.proxy.rowsRemoved,
                       self.proxy.modelReset, self.proxy.layoutChanged):
            signal.connect(self._update_count)

        bar = QHBoxLayout()
        bar.addWidget(self.search, 1)
        bar.addWidget(QLabel("範圍："))
        bar.addWidget(self.scope)
        if self.kind is not None:
            bar.addWidget(self.kind)
        bar.addWidget(QLabel("顯示："))
        bar.addWidget(self.show_box)
        bar.addWidget(reset)
        bar.addWidget(self.count)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.addLayout(bar)
        layout.addWidget(self.view)
        self._update_count()

    def _on_kind(self, _index: int) -> None:
        assert self.kind is not None
        kind = self.kind.currentData()
        self.proxy.set_row_filter("kind", None if kind is None else kind_filter(kind))

    def _update_count(self, *_args: object) -> None:
        shown, total = self.proxy.rowCount(), self._model.rowCount()
        self.count.setText(f"顯示 {shown:,} / {total:,} 筆" if self.proxy.is_filtered()
                           else f"共 {total:,} 筆")

    def sort_by(self, column: int, order: Qt.SortOrder = Qt.SortOrder.AscendingOrder) -> None:
        self.view.sortByColumn(column, order)

    def reset(self) -> None:
        """Clear every filter and return to the project's own record order."""
        self.search.clear()
        for box in (self.scope, self.kind, self.show_box):
            if box is not None:
                box.setCurrentIndex(0)
        self.view.horizontalHeader().setSortIndicator(-1, Qt.SortOrder.AscendingOrder)
        self.proxy.sort(-1)
        self._update_count()
