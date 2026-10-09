"""Sorting and filtering for the record tables.

Both only change what the view shows: the order and numbering of records in the
project stay exactly as they are (the settings image may refer to contacts by
number, so the list itself is never reordered).
"""

from __future__ import annotations

from collections import Counter
from collections.abc import Callable

from PySide6.QtCore import (
    QModelIndex,
    QObject,
    QPersistentModelIndex,
    QSortFilterProxyModel,
)

from ..cleanup import needs_cleaning
from ..records import Contact
from ..textcodec import NameCodec
from .models import SORT_ROLE, RecordTableModel

# Given every record of the table, return the row numbers to keep.
RowFilter = Callable[[tuple, NameCodec], frozenset[int]]


def kind_filter(kind: int) -> RowFilter:
    def rows(records: tuple, _codec: NameCodec) -> frozenset[int]:
        return frozenset(i for i, r in enumerate(records) if r.kind == kind)

    return rows


def control_char_rows(records: tuple, _codec: NameCodec) -> frozenset[int]:
    return frozenset(i for i, r in enumerate(records) if needs_cleaning(r))


def duplicate_name_rows(records: tuple, codec: NameCodec) -> frozenset[int]:
    names = [r.name(codec).strip().casefold() for r in records]
    seen = Counter(names)
    return frozenset(i for i, n in enumerate(names) if seen[n] > 1)


def duplicate_id_rows(records: tuple[Contact, ...], _codec: NameCodec) -> frozenset[int]:
    keys = [(r.kind, r.call_id) for r in records]
    seen = Counter(keys)
    return frozenset(i for i, k in enumerate(keys) if seen[k] > 1)


class RecordFilterProxy(QSortFilterProxyModel):
    """Whitespace-separated terms must all match (case-insensitive) within the chosen
    columns; every named row filter must also accept the row."""

    def __init__(self, parent: QObject | None = None) -> None:
        super().__init__(parent)
        self._terms: tuple[str, ...] = ()
        self._columns: tuple[int, ...] | None = None
        self._row_filters: dict[str, RowFilter] = {}
        self._cache: dict[str, tuple[object, NameCodec, frozenset[int]]] = {}
        self.setSortRole(SORT_ROLE)
        self.setDynamicSortFilter(True)

    def setSourceModel(self, model: RecordTableModel) -> None:  # noqa: N802
        super().setSourceModel(model)
        # A row filter may depend on other rows (duplicates), so an edit to one record
        # can change which rows pass; Qt alone would only re-check the edited row.
        model.dataChanged.connect(self._refilter_if_row_filters)

    def _refilter_if_row_filters(self, *_args: object) -> None:
        if self._row_filters:
            self.beginFilterChange()
            self._end_filter_change()

    def set_text(self, text: str) -> None:
        self.beginFilterChange()
        self._terms = tuple(t.casefold() for t in text.split())
        self._end_filter_change()

    def set_columns(self, columns: tuple[int, ...] | None) -> None:
        """Restrict the text search to these source columns (None = all)."""
        self.beginFilterChange()
        self._columns = columns
        self._end_filter_change()

    def set_row_filter(self, name: str, row_filter: RowFilter | None) -> None:
        self.beginFilterChange()
        if row_filter is None:
            self._row_filters.pop(name, None)
        else:
            self._row_filters[name] = row_filter
        self._cache.pop(name, None)
        self._end_filter_change()

    def _end_filter_change(self) -> None:
        self.endFilterChange(QSortFilterProxyModel.Direction.Rows)

    def is_filtered(self) -> bool:
        return bool(self._terms or self._row_filters)

    def _accepted_rows(self, name: str, row_filter: RowFilter) -> frozenset[int]:
        model = self.sourceModel()
        assert isinstance(model, RecordTableModel)
        # Projects are immutable, so identity tells whether the cached result is current.
        hit = self._cache.get(name)
        if hit is not None and hit[0] is model.project and hit[1] is model.codec:
            return hit[2]
        rows = row_filter(model.records(), model.codec)
        self._cache[name] = (model.project, model.codec, rows)
        return rows

    def filterAcceptsRow(self, source_row: int,  # noqa: N802
                         source_parent: QModelIndex | QPersistentModelIndex) -> bool:
        for name, row_filter in self._row_filters.items():
            if source_row not in self._accepted_rows(name, row_filter):
                return False
        if not self._terms:
            return True
        model = self.sourceModel()
        columns = self._columns if self._columns is not None else range(model.columnCount())
        texts = [str(model.data(model.index(source_row, c, source_parent)) or "").casefold()
                 for c in columns]
        return all(any(term in text for text in texts) for term in self._terms)
