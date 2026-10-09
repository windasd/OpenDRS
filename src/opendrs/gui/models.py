"""Qt table models over the immutable project records.

Each edit produces a new record via the record's ``with_*`` method; validation
errors are reported through ``edit_failed`` instead of being silently dropped.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass

from PySide6.QtCore import (
    QAbstractTableModel,
    QModelIndex,
    QPersistentModelIndex,
    Qt,
    Signal,
)
from PySide6.QtGui import QBrush, QColor

from .. import channel_fields as cf
from ..device import frequency_problem
from ..f136 import Project
from ..radio_settings import Setting
from ..records import Channel, Contact, Zone, hz_to_mhz, mhz_to_hz
from ..textcodec import NameCodec, NameEncodingError

Index = QModelIndex | QPersistentModelIndex
_ERROR_BG = QBrush(QColor("#ffd6d6"))
# Typed sort key (int for numbers, casefolded text otherwise) so "10" sorts after "9".
SORT_ROLE = Qt.ItemDataRole.UserRole + 1
# The raw number behind a column whose display text is looked up (e.g. a contact number).
VALUE_ROLE = Qt.ItemDataRole.UserRole + 2


@dataclass(frozen=True)
class Column:
    title: str
    get: Callable[[object, NameCodec], str]
    set: Callable[[object, str, NameCodec], object] | None = None
    hz: Callable[[object], int] | None = None
    sort: Callable[[object], int] | None = None
    # Display text that needs the whole project (other tables), given the row number.
    lookup: Callable[[Project, int, NameCodec], str] | None = None
    value: Callable[[object], int] | None = None  # VALUE_ROLE
    choices: tuple[tuple[str, int], ...] = ()  # fixed drop-down (label, VALUE_ROLE value)
    # Edits that change more than the record itself (e.g. moving a channel to another
    # zone): returns the whole new project and the window refreshes every view.
    restructure: Callable[[Project, int, int], Project] | None = None
    row_value: Callable[[Project, int], int] | None = None  # VALUE_ROLE needing the project


def _move_to_zone(project: Project, row: int, zone_row: int) -> Project:
    from ..layout import move_channel

    return move_channel(project, row, zone_row)[0]


def _zone_row(project: Project, row: int) -> int:
    from ..layout import zone_rows

    zone_row = zone_rows(project)[row]
    return -1 if zone_row is None else zone_row


def _zone_name(project: Project, row: int, codec: NameCodec) -> str:
    from ..layout import zone_rows

    zone_row = zone_rows(project)[row]
    if zone_row is None:
        return "（無）"
    return f"{zone_row + 1}. {project.zones[zone_row].name(codec)}"


def _group_name(project: Project, row: int, codec: NameCodec) -> str:
    from ..rxgroup import rx_groups

    number = project.channels[row].rx_group
    if number == 0:
        return "（無）"
    groups = rx_groups(project.image)
    if not 1 <= number <= len(groups):
        return f"{number}（不存在）"
    return f"{number}. {groups[number - 1].name(codec)}"


def _field_column(field: Setting) -> Column:
    return Column(field.label, lambda r, c: field.display(r.raw),
                  lambda r, t, c: cf.put(r, field, int(t)),
                  value=lambda r: cf.get(r, field), sort=lambda r: cf.get(r, field),
                  choices=field.choices)


def _key_name(project: Project, row: int, codec: NameCodec) -> str:
    from ..alarm_enc import encryption_keys

    number = cf.encryption_key(project.channels[row])
    if number == 0:
        return "Off"
    keys = encryption_keys(project.image)
    if not 1 <= number <= len(keys):
        return f"{number}（不存在）"
    return f"{number}. {keys[number - 1].alias(codec)}"


def _scan_name(project: Project, row: int, codec: NameCodec) -> str:
    from ..scanlist import scan_lists

    number = cf.get(project.channels[row], cf.SCAN_LIST)
    lists = scan_lists(project.image)
    if number == 0:
        return "Off"
    return f"{number}. {lists[number - 1].name(codec)}" if number <= len(lists) else f"{number}（不存在）"


def _alarm_name(project: Project, row: int, codec: NameCodec) -> str:
    from ..alarm_enc import alarm_lists

    number = cf.get(project.channels[row], cf.EMERGENCY)
    lists = alarm_lists(project.image)
    if number == 0:
        return "Off"
    return f"{number}. {lists[number - 1].name(codec)}" if number <= len(lists) else f"{number}（不存在）"


def contact_label(contact: Contact, codec: NameCodec) -> str:
    return f"#{contact.index} {contact.name(codec)}（{contact.kind_label} {contact.call_id}）"


def _tx_contact_name(project: Project, row: int, codec: NameCodec) -> str:
    number = project.channels[row].tx_contact
    if number == 0:
        return "（無）"
    contact = next((c for c in project.contacts if c.index == number), None)
    return contact_label(contact, codec) if contact else f"#{number}（不存在）"


def _freq_setter(method: str) -> Callable[[object, str, NameCodec], object]:
    def apply(rec: object, text: str, _codec: NameCodec) -> object:
        return getattr(rec, method)(mhz_to_hz(text) if text.strip() else 0)

    return apply


CHANNEL_COLUMNS = (
    Column("#", lambda r, c: str(r.index), sort=lambda r: r.index),
    Column("名稱", lambda r, c: r.name(c), lambda r, t, c: r.with_name(t, c)),
    Column("接收 (MHz)", lambda r, c: hz_to_mhz(r.freq_a_hz), _freq_setter("with_freq_a"),
           lambda r: r.freq_a_hz, lambda r: r.freq_a_hz),
    Column("發射 (MHz)", lambda r, c: hz_to_mhz(r.freq_b_hz), _freq_setter("with_freq_b"),
           lambda r: r.freq_b_hz, lambda r: r.freq_b_hz),
    Column("分區", lambda r, c: "", lookup=_zone_name, restructure=_move_to_zone,
           row_value=lambda p, row: _zone_row(p, row)),
    Column("發話對象", lambda r, c: "", lambda r, t, c: r.with_tx_contact(int(t)),
           lookup=_tx_contact_name, value=lambda r: r.tx_contact),
    Column("接收群組", lambda r, c: "", lambda r, t, c: r.with_rx_group(int(t)),
           lookup=_group_name, value=lambda r: r.rx_group,
           sort=lambda r: r.rx_group),
    Column("模式", lambda r, c: r.mode_label, lambda r, t, c: r.with_mode(int(t)),
           sort=lambda r: r.mode, value=lambda r: r.mode,
           choices=tuple((v, k) for k, v in Channel.MODES.items())),
    Column("色碼", lambda r, c: str(r.color_code), lambda r, t, c: cf.with_color_code(r, int(t)),
           sort=lambda r: r.color_code, value=lambda r: r.color_code,
           choices=tuple((str(cc), cc) for cc in range(16))),
    Column("時隙", lambda r, c: cf.slot_label(r), lambda r, t, c: cf.with_slot(r, int(t)),
           value=lambda r: cf.slot(r) if cf.slot(r) is not None else -1,
           choices=tuple((label, i) for i, label in enumerate(cf.SLOT_LABELS))),
    *(_field_column(f) for f in cf.FIELDS),
    Column("加密列表", lambda r, c: "", lambda r, t, c: cf.with_encryption_key(r, int(t)),
           lookup=_key_name, value=cf.encryption_key, sort=cf.encryption_key),
    Column("掃描列表", lambda r, c: "", lambda r, t, c: cf.put(r, cf.SCAN_LIST, int(t)),
           lookup=_scan_name, value=lambda r: cf.get(r, cf.SCAN_LIST), sort=lambda r: cf.get(r, cf.SCAN_LIST)),
    Column("緊急系統", lambda r, c: "", lambda r, t, c: cf.put(r, cf.EMERGENCY, int(t)),
           lookup=_alarm_name, value=lambda r: cf.get(r, cf.EMERGENCY), sort=lambda r: cf.get(r, cf.EMERGENCY)),
    Column("接收亞音", lambda r, c: cf.tone_label(cf.tone(r, False)),
           lambda r, t, c: cf.with_tone(r, False, int(t)), value=lambda r: cf.tone(r, False),
           sort=lambda r: cf.tone(r, False), choices=tuple(cf.tone_choices())),
    Column("發射亞音", lambda r, c: cf.tone_label(cf.tone(r, True)),
           lambda r, t, c: cf.with_tone(r, True, int(t)), value=lambda r: cf.tone(r, True),
           sort=lambda r: cf.tone(r, True), choices=tuple(cf.tone_choices())),
    Column("其餘位元組", lambda r, c: r.raw[0x14:].hex(" ")),
)
CHANNEL_ZONE_COLUMN = 4
CHANNEL_TX_CONTACT_COLUMN = 5
CHANNEL_RX_GROUP_COLUMN = 6
CHANNEL_MODE_COLUMN = 7
CHANNEL_COLOR_CODE_COLUMN = 8
CHANNEL_SLOT_COLUMN = 9
CHANNEL_ENCRYPTION_COLUMN = CHANNEL_SLOT_COLUMN + len(cf.FIELDS) + 1
CHANNEL_SCAN_COLUMN = CHANNEL_ENCRYPTION_COLUMN + 1
CHANNEL_EMERGENCY_COLUMN = CHANNEL_ENCRYPTION_COLUMN + 2
ZONE_COLUMNS = (
    Column("#", lambda r, c: str(r.index), sort=lambda r: r.index),
    Column("名稱", lambda r, c: r.name(c), lambda r, t, c: r.with_name(t, c)),
    Column("其餘位元組", lambda r, c: r.raw[13:].hex(" ")),
)
CONTACT_COLUMNS = (
    Column("#", lambda r, c: str(r.index), sort=lambda r: r.index),
    Column("類型", lambda r, c: r.kind_label, sort=lambda r: r.kind),
    Column("名稱", lambda r, c: r.name(c), lambda r, t, c: r.with_name(t, c)),
    Column("呼叫 ID", lambda r, c: str(r.call_id),
           lambda r, t, c: r.with_call_id(int(t.strip())), sort=lambda r: r.call_id),
)


class RecordTableModel(QAbstractTableModel):
    edit_failed = Signal(str)
    project_changed = Signal(object)
    project_restructured = Signal(object, str)  # whole new project, history label

    def __init__(self, kind: str, columns: tuple[Column, ...]) -> None:
        super().__init__()
        self._kind = kind
        self._columns = columns
        self._project = Project()
        self._codec = NameCodec.BIG5

    def _records(self) -> tuple:
        return getattr(self._project, self._kind)

    def records(self) -> tuple:
        return self._records()

    @property
    def project(self) -> Project:
        return self._project

    @property
    def codec(self) -> NameCodec:
        return self._codec

    def set_project(self, project: Project, codec: NameCodec) -> None:
        self.beginResetModel()
        self._project, self._codec = project, codec
        self.endResetModel()

    def rowCount(self, parent: Index = QModelIndex()) -> int:  # noqa: B008, N802
        return 0 if parent.isValid() else len(self._records())

    def columnCount(self, parent: Index = QModelIndex()) -> int:  # noqa: B008, N802
        return 0 if parent.isValid() else len(self._columns)

    def headerData(self, section: int, orientation: Qt.Orientation,  # noqa: N802
                   role: int = Qt.ItemDataRole.DisplayRole) -> object:
        if role == Qt.ItemDataRole.DisplayRole and orientation == Qt.Orientation.Horizontal:
            return self._columns[section].title
        return None

    def flags(self, index: Index) -> Qt.ItemFlag:
        base = Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable
        if index.isValid() and (self._columns[index.column()].set is not None
                                or self._columns[index.column()].restructure is not None):
            return base | Qt.ItemFlag.ItemIsEditable
        return base

    def data(self, index: Index, role: int = Qt.ItemDataRole.DisplayRole) -> object:
        if not index.isValid():
            return None
        rec = self._records()[index.row()]
        col = self._columns[index.column()]
        if role in (Qt.ItemDataRole.DisplayRole, Qt.ItemDataRole.EditRole):
            if col.lookup is not None:
                return col.lookup(self._project, index.row(), self._codec)
            return col.get(rec, self._codec)
        if role == VALUE_ROLE and col.row_value is not None:
            return col.row_value(self._project, index.row())
        if role == VALUE_ROLE and col.value is not None:
            return col.value(rec)
        if role == SORT_ROLE:
            if col.sort is not None:
                return col.sort(rec)
            return str(self.data(index, Qt.ItemDataRole.DisplayRole)).casefold()
        if col.hz is not None and role in (Qt.ItemDataRole.BackgroundRole,
                                           Qt.ItemDataRole.ToolTipRole):
            problem = frequency_problem(col.hz(rec), self._project.device.bands)
            if problem:
                return _ERROR_BG if role == Qt.ItemDataRole.BackgroundRole else problem
        return None

    def setData(self, index: Index, value: object,  # noqa: N802
                role: int = Qt.ItemDataRole.EditRole) -> bool:
        if role != Qt.ItemDataRole.EditRole or not index.isValid():
            return False
        col = self._columns[index.column()]
        if col.restructure is not None:
            try:
                project = col.restructure(self._project, index.row(), int(value))
            except ValueError as exc:  # LayoutError is a ValueError
                self.edit_failed.emit(str(exc))
                return False
            if project is not self._project:
                self.project_restructured.emit(project, col.title)
            return True
        if col.set is None:
            return False
        rec = self._records()[index.row()]
        try:
            new_rec = col.set(rec, str(value), self._codec)
        except (NameEncodingError, ValueError) as exc:
            self.edit_failed.emit(str(exc))
            return False
        updater = {"channels": Project.with_channel, "zones": Project.with_zone,
                   "contacts": Project.with_contact}[self._kind]
        self._project = updater(self._project, index.row(), new_rec)
        row_l, row_r = self.index(index.row(), 0), self.index(index.row(), self.columnCount() - 1)
        self.dataChanged.emit(row_l, row_r)
        self.project_changed.emit(self._project)
        return True


def make_models() -> dict[str, RecordTableModel]:
    return {
        "channels": RecordTableModel("channels", CHANNEL_COLUMNS),
        "zones": RecordTableModel("zones", ZONE_COLUMNS),
        "contacts": RecordTableModel("contacts", CONTACT_COLUMNS),
    }


__all__ = ["SORT_ROLE", "Channel", "Contact", "RecordTableModel", "Zone", "make_models"]
