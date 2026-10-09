"""Scan-list tab: the 16 list slots with their members, and an editor dialog."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtWidgets import QComboBox, QWidget

from ..f136 import Project
from ..layout import zone_rows
from ..scanlist import (
    SCAN_ENTRY_COUNT,
    TX_MODES,
    Entry,
    ScanList,
    has_scan_lists,
    scan_lists,
)
from ..textcodec import NameCodec
from .slot_page import MemberPickerDialog, SlotTablePage


def channel_entries(project: Project) -> list[Entry]:
    """Every channel as its scan entry (zone number, number in zone), in list order."""
    seen: dict[int, int] = {}
    out: list[Entry] = []
    for zone_row in zone_rows(project):
        if zone_row is None:
            continue
        seen[zone_row] = seen.get(zone_row, 0) + 1
        out.append((zone_row + 1, seen[zone_row]))
    return out


def entry_label(project: Project, entry: Entry, codec: NameCodec) -> str:
    zone_no, channel_no = entry
    if not 1 <= zone_no <= len(project.zones):
        return f"分區 {zone_no}／第 {channel_no} 台（不存在）"
    zone = project.zones[zone_no - 1]
    if not 1 <= channel_no <= zone.channel_count:
        return f"{zone.name(codec)}／第 {channel_no} 台（不存在）"
    row = zone.first_record - len(project.zones) - 1 + channel_no - 1
    return f"{zone.name(codec)}／{project.channels[row].name(codec)}"


class ScanListDialog(MemberPickerDialog):
    """Rename one list and tick up to 16 channels (scanned in channel-list order)."""

    def __init__(self, project: Project, scan: ScanList, codec: NameCodec,
                 parent: QWidget | None = None) -> None:
        choices = [(entry_label(project, e, codec), e) for e in channel_entries(project)]
        super().__init__(f"掃描清單 {scan.number}", scan.name(codec), choices,
                         set(scan.entries), SCAN_ENTRY_COUNT, "頻道", parent)
        self.channels = self.items
        self.talkback = QComboBox()
        self.talkback.addItems(["Off", "On"])
        self.talkback.setCurrentIndex(int(scan.talkback))
        self.tx_mode = QComboBox()
        for label, value in TX_MODES:
            self.tx_mode.addItem(label, value)
        self.tx_mode.setCurrentIndex(max(self.tx_mode.findData(scan.tx_mode), 0))
        self.designated = QComboBox()
        self.designated.addItem("Off", 0)
        for entry in channel_entries(project):
            self.designated.addItem(entry_label(project, entry, codec), entry[0] << 16 | entry[1])
        zone, channel = scan.designated
        found = self.designated.findData(zone << 16 | channel)
        self.designated.setCurrentIndex(max(found, 0))
        # A stale value the legacy CPS shows as "Off" is kept unless the user picks another.
        self._stale_designated = scan.designated if found < 0 and scan.designated != (0, 0) else None
        self.designated.currentIndexChanged.connect(self._forget_stale)
        self.form.addRow("回講：", self.talkback)
        self.form.addRow("掃描發射模式：", self.tx_mode)
        self.form.addRow("指定信道：", self.designated)
        self.tx_mode.currentIndexChanged.connect(self._sync_designated)
        self._sync_designated()

    def _forget_stale(self, *_args: object) -> None:
        self._stale_designated = None

    def _sync_designated(self, *_args: object) -> None:
        self.designated.setEnabled(self.tx_mode.currentData() == 2)

    def settings(self) -> tuple[bool, int, Entry]:
        value = self.designated.currentData() or 0
        designated = self._stale_designated or (value >> 16, value & 0xFFFF)
        return bool(self.talkback.currentIndex()), self.tx_mode.currentData(), designated

    def entries(self) -> list[Entry]:
        return self.values()


class ScanListPage(SlotTablePage):
    def __init__(self, on_edit: Callable[[int], None], parent: QWidget | None = None) -> None:
        super().__init__("雙擊一列可編輯。清單最多 16 台頻道，依頻道清單順序掃描；"
                         "刪除頻道或分區時，掃描清單會自動跟著調整。", on_edit, parent)

    def set_project(self, project: Project, codec: NameCodec) -> int:
        rows = [(s.name(codec), len(s.entries),
                 "、".join(entry_label(project, e, codec) for e in s.entries), s.in_use)
                for s in scan_lists(project.image)]
        return self.set_rows(rows, has_scan_lists(project.image))
