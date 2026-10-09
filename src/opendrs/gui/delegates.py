"""Cell editors that pick from another table."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtCore import (
    QAbstractItemModel,
    QEvent,
    QModelIndex,
    QPersistentModelIndex,
    QRect,
    QSize,
    QSortFilterProxyModel,
    Qt,
    QTimer,
)
from PySide6.QtGui import QPainter
from PySide6.QtWidgets import (
    QAbstractItemView,
    QApplication,
    QComboBox,
    QCompleter,
    QStyle,
    QStyledItemDelegate,
    QStyleOptionViewItem,
    QWidget,
)

from ..f136 import Project
from ..rxgroup import rx_groups
from ..textcodec import NameCodec
from .models import VALUE_ROLE, RecordTableModel, contact_label

Index = QModelIndex | QPersistentModelIndex


def _source(model: QAbstractItemModel) -> RecordTableModel:
    while isinstance(model, QSortFilterProxyModel):
        model = model.sourceModel()
    assert isinstance(model, RecordTableModel)
    return model


Choices = Callable[[Project, NameCodec], list[tuple[str, int]]]


def contact_choices(project: Project, codec: NameCodec) -> list[tuple[str, int]]:
    return [(contact_label(c, codec), c.index) for c in project.contacts]


def encryption_key_choices(project: Project, codec: NameCodec) -> list[tuple[str, int]]:
    from ..alarm_enc import encryption_keys

    return [(f"{k.number}. {k.alias(codec)}", k.number) for k in encryption_keys(project.image) if k.in_use]


def scan_list_choices(project: Project, codec: NameCodec) -> list[tuple[str, int]]:
    from ..scanlist import scan_lists

    return [(f"{s.number}. {s.name(codec)}", s.number) for s in scan_lists(project.image) if s.in_use]


def alarm_list_choices(project: Project, codec: NameCodec) -> list[tuple[str, int]]:
    from ..alarm_enc import alarm_lists

    return [(f"{a.number}. {a.name(codec)}", a.number) for a in alarm_lists(project.image)]


def zone_choices(project: Project, codec: NameCodec) -> list[tuple[str, int]]:
    """Zones by row (the value is the 0-based zone row)."""
    return [(f"{i + 1}. {z.name(codec)}", i) for i, z in enumerate(project.zones)]


def rx_group_choices(project: Project, codec: NameCodec) -> list[tuple[str, int]]:
    return [(f"{g.number}. {g.name(codec)}", g.number) for g in rx_groups(project.image) if g.in_use]


class DropdownDelegate(QStyledItemDelegate):
    """A cell that looks and acts like a drop-down: an arrow is drawn on the right, one
    click opens the list, and picking an entry applies it at once."""

    ARROW_WIDTH = 16

    def paint(self, painter: QPainter, option: QStyleOptionViewItem, index: Index) -> None:  # noqa: D102
        super().paint(painter, option, index)
        if not index.flags() & Qt.ItemFlag.ItemIsEditable:
            return
        arrow = QStyleOptionViewItem(option)
        r = option.rect
        arrow.rect = QRect(r.right() - self.ARROW_WIDTH, r.top() + (r.height() - 10) // 2, 10, 10)
        style = option.widget.style() if option.widget is not None else QApplication.style()
        painter.save()
        painter.setOpacity(0.55)
        style.drawPrimitive(QStyle.PrimitiveElement.PE_IndicatorArrowDown, arrow, painter, option.widget)
        painter.restore()

    def sizeHint(self, option: QStyleOptionViewItem, index: Index) -> QSize:  # noqa: N802
        hint = super().sizeHint(option, index)
        return QSize(hint.width() + self.ARROW_WIDTH + 6, hint.height())  # room for the arrow

    def editorEvent(self, event: QEvent, model: QAbstractItemModel,  # noqa: N802
                    option: QStyleOptionViewItem, index: Index) -> bool:
        view = self.parent()
        if (event.type() == QEvent.Type.MouseButtonRelease and isinstance(view, QAbstractItemView)
                and index.flags() & Qt.ItemFlag.ItemIsEditable
                and event.button() == Qt.MouseButton.LeftButton):
            view.setCurrentIndex(index)
            view.edit(index)
            return True
        return super().editorEvent(event, model, option, index)

    def _drop_down(self, box: QComboBox) -> QComboBox:
        def commit(_row: int) -> None:
            self.commitData.emit(box)
            self.closeEditor.emit(box, QStyledItemDelegate.EndEditHint.NoHint)

        box.activated.connect(commit)
        QTimer.singleShot(0, box.showPopup)
        return box

    def setEditorData(self, editor: QWidget, index: Index) -> None:  # noqa: N802
        assert isinstance(editor, QComboBox)
        editor.setCurrentIndex(max(editor.findData(index.data(VALUE_ROLE)), 0))


class ProjectPickerDelegate(DropdownDelegate):
    """Searchable drop-down of choices drawn from the current project (plus "（無）" = 0);
    stores the chosen number."""

    def __init__(self, choices: Choices, parent: QWidget | None = None, none_label: str | None = "（無）") -> None:
        super().__init__(parent)
        self._choices = choices
        self._none_label = none_label

    def createEditor(self, parent: QWidget, _option: QStyleOptionViewItem,  # noqa: N802
                     index: Index) -> QWidget:
        source = _source(index.model())
        box = QComboBox(parent)
        box.setEditable(True)
        box.setInsertPolicy(QComboBox.InsertPolicy.NoInsert)
        if self._none_label is not None:
            box.addItem(self._none_label, 0)
        for label, value in self._choices(source.project, source.codec):
            box.addItem(label, value)
        completer = box.completer()
        completer.setFilterMode(Qt.MatchFlag.MatchContains)
        completer.setCompletionMode(QCompleter.CompletionMode.PopupCompletion)
        return self._drop_down(box)

    def setModelData(self, editor: QWidget, model: QAbstractItemModel,  # noqa: N802
                     index: Index) -> None:
        assert isinstance(editor, QComboBox)
        row = editor.findText(editor.currentText())
        if row < 0:  # typed text that names no choice: keep the old value
            return
        model.setData(index, editor.itemData(row), Qt.ItemDataRole.EditRole)


class ChoiceDelegate(DropdownDelegate):
    """Drop-down of fixed (label, value) choices; stores the value."""

    def __init__(self, choices: list[tuple[str, int]], parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._choices = choices

    def createEditor(self, parent: QWidget, _option: QStyleOptionViewItem,  # noqa: N802
                     _index: Index) -> QWidget:
        box = QComboBox(parent)
        box.setMaxVisibleItems(16)
        for label, value in self._choices:
            box.addItem(label, value)
        return self._drop_down(box)

    def setModelData(self, editor: QWidget, model: QAbstractItemModel,  # noqa: N802
                     index: Index) -> None:
        assert isinstance(editor, QComboBox)
        model.setData(index, editor.currentData(), Qt.ItemDataRole.EditRole)
