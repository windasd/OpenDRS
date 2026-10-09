"""「報警與加密」tab: digital alarm lists, the GPS report channel and encryption keys."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtWidgets import (
    QComboBox,
    QGroupBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QSpinBox,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from ..alarm_enc import (
    ALARM_MODES,
    ALARM_TYPES,
    MAX_MIC_SECONDS,
    alarm_lists,
    encryption_keys,
    gps_channel,
)
from ..f136 import Project
from ..scanlist import Entry
from ..textcodec import NameCodec
from .scan_page import channel_entries, entry_label

# (kind, number, field, value): kind "alarm" / "gps" / "key"
EditFn = Callable[[str, int, str, object], None]
ALARM_COLUMNS = ("名稱", "報警類型", "報警模式", "報警信道", "不禮貌嘗試次數", "緊急麥克風持續時間")


def pack(entry: Entry) -> int:
    """Combo boxes compare stored data by value only for plain ints, not tuples."""
    return entry[0] << 16 | entry[1]


def unpack(value: int) -> Entry:
    return (value >> 16, value & 0xFFFF)


def channel_choices(project: Project, codec: NameCodec) -> list[tuple[str, int]]:
    return [("Off", 0)] + [(entry_label(project, e, codec), pack(e)) for e in channel_entries(project)]


class AlarmEncryptionPage(QWidget):
    def __init__(self, edit: EditFn, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._edit = edit
        self._filling = False
        self.alarms = QTableWidget(0, len(ALARM_COLUMNS))
        self.alarms.setHorizontalHeaderLabels(ALARM_COLUMNS)
        self.alarms.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        self.alarms.horizontalHeader().setStretchLastSection(True)
        self.alarms.itemChanged.connect(self._alarm_name_changed)
        self.gps = QComboBox()
        self.gps.currentIndexChanged.connect(
            lambda _i: self._emit("gps", 0, "channel", None if self.gps.currentData() is None
                                  else unpack(self.gps.currentData())))
        self.keys = QTableWidget(0, 2)
        self.keys.setHorizontalHeaderLabels(["加密秘鑰別名（最多 10 bytes，清空即刪除）", "加密初始化密鑰（6 bytes 十六進位）"])
        self.keys.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.Stretch)
        self.keys.itemChanged.connect(self._key_changed)

        alarm_box = QGroupBox("數字報警列表")
        QVBoxLayout(alarm_box).addWidget(self.alarms)
        gps_box = QGroupBox("GPS")
        row = QHBoxLayout(gps_box)
        row.addWidget(QLabel("GPS上報信道："))
        row.addWidget(self.gps, 1)
        key_box = QGroupBox("加密列表（加密等級在「對講機設定 › 加密」）")
        QVBoxLayout(key_box).addWidget(self.keys)
        layout = QVBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.addWidget(alarm_box)
        layout.addWidget(gps_box)
        layout.addWidget(key_box, 1)

    def _emit(self, kind: str, number: int, field: str, value: object) -> None:
        if not self._filling and value is not None:
            self._edit(kind, number, field, value)

    def _alarm_name_changed(self, item: QTableWidgetItem) -> None:
        if item.column() == 0:
            self._emit("alarm", item.row() + 1, "name", item.text())

    def _key_changed(self, item: QTableWidgetItem) -> None:
        row = item.row()
        alias = self.keys.item(row, 0).text() if self.keys.item(row, 0) else ""
        key = self.keys.item(row, 1).text() if self.keys.item(row, 1) else ""
        self._emit("key", row + 1, "key", (alias, key))

    def _combo(self, choices: list[tuple[str, object]], current: object, on_change: Callable[[object], None]) -> QComboBox:
        box = QComboBox()
        for label, value in choices:
            box.addItem(label, value)
        index = box.findData(current)
        if index < 0:
            box.addItem(f"未知值 {current}", current)
            index = box.count() - 1
        box.setCurrentIndex(index)
        box.currentIndexChanged.connect(lambda _i, b=box: on_change(b.currentData()))
        return box

    def set_project(self, project: Project, codec: NameCodec) -> None:
        self._filling = True
        try:
            image = project.image
            lists = alarm_lists(image)
            self.setEnabled(bool(lists))
            channels = channel_choices(project, codec)
            self.alarms.setRowCount(len(lists))
            for row, a in enumerate(lists):
                n = a.number
                self.alarms.setItem(row, 0, QTableWidgetItem(a.name(codec)))
                self.alarms.setCellWidget(row, 1, self._combo(list(ALARM_TYPES), a.alarm_type,
                                                              lambda v, n=n: self._emit("alarm", n, "alarm_type", v)))
                self.alarms.setCellWidget(row, 2, self._combo(list(ALARM_MODES), a.mode,
                                                              lambda v, n=n: self._emit("alarm", n, "mode", v)))
                self.alarms.setCellWidget(row, 3, self._combo(channels, pack(a.channel),
                                                              lambda v, n=n: self._emit("alarm", n, "channel",
                                                                                        unpack(v))))
                self.alarms.setCellWidget(row, 4, self._spin(1, 15, "", a.impolite_attempts,
                                                             lambda v, n=n: self._emit("alarm", n, "impolite_attempts", v)))
                self.alarms.setCellWidget(row, 5, self._spin(0, MAX_MIC_SECONDS, "S", a.mic_seconds,
                                                             lambda v, n=n: self._emit("alarm", n, "mic_seconds", v)))
            self.gps.clear()
            for label, value in channels:
                self.gps.addItem(label, value)
            current = gps_channel(image)
            index = self.gps.findData(pack(current))
            if index < 0:
                self.gps.addItem(f"分區 {current[0]}／第 {current[1]} 台（不存在）", pack(current))
                index = self.gps.count() - 1
            self.gps.setCurrentIndex(index)
            keys = encryption_keys(image)
            self.keys.setRowCount(len(keys))
            for row, k in enumerate(keys):
                self.keys.setItem(row, 0, QTableWidgetItem(k.alias(codec) if k.in_use else ""))
                self.keys.setItem(row, 1, QTableWidgetItem(k.key.hex(" ").upper() if k.in_use else ""))
        finally:
            self._filling = False

    def _spin(self, lo: int, hi: int, suffix: str, value: int, on_change: Callable[[int], None]) -> QSpinBox:
        spin = QSpinBox()
        spin.setRange(lo, hi)
        spin.setSuffix(suffix)
        spin.setValue(max(lo, min(hi, value)))
        spin.editingFinished.connect(lambda s=spin: on_change(s.value()) if s.value() != value else None)
        return spin

