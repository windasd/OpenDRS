"""Editing the fixed-slot lists (scan lists, RX group lists) and general radio
settings from the main window."""

from __future__ import annotations

from dataclasses import replace

from PySide6.QtWidgets import QMessageBox

from ..alarm_enc import (
    ListEditError,
    parse_key_hex,
    with_alarm,
    with_encryption_key,
    with_gps_channel,
)
from ..quickmsg import with_quick_message
from ..radio_settings import Setting, SettingError, TextSetting
from ..rxgroup import RxGroupError, rx_groups, with_rx_group
from ..scanlist import ScanListError, scan_lists, with_scan_list, with_scan_settings
from ..textcodec import NameEncodingError
from .group_page import RxGroupDialog
from .scan_page import ScanListDialog


class ListOpsMixin:
    """Scan-list and RX-group-list editing for MainWindow."""

    def edit_scan_list(self, number: int) -> bool:
        """Open the editor for scan list ``number`` (1-16) and apply the result."""
        lists = scan_lists(self._project.image)
        if not 1 <= number <= len(lists):
            return False
        dialog = ScanListDialog(self._project, lists[number - 1], self.codec, parent=self)
        if dialog.exec() != ScanListDialog.DialogCode.Accepted:
            return False
        return self.apply_scan_list(number, dialog.name.text(), dialog.entries(), dialog.settings())

    def apply_scan_list(self, number: int, name: str, entries: list,
                        settings: tuple[bool, int, tuple[int, int]] | None = None) -> bool:
        try:
            image = with_scan_list(self._project.image, number, name.strip(), entries, self.codec)
            if settings is not None:
                image = with_scan_settings(image, number, *settings)
        except (ScanListError, NameEncodingError) as exc:
            QMessageBox.warning(self, "掃描清單", str(exc))
            return False
        self._commit(replace(self._project, image=image), f"編輯掃描清單 {number}")
        return True

    def edit_rx_group(self, number: int) -> bool:
        """Open the editor for RX group list ``number`` (1-32) and apply the result."""
        groups = rx_groups(self._project.image)
        if not 1 <= number <= len(groups):
            return False
        dialog = RxGroupDialog(self._project, groups[number - 1], self.codec, parent=self)
        if dialog.exec() != RxGroupDialog.DialogCode.Accepted:
            return False
        return self.apply_rx_group(number, dialog.name.text(), dialog.values())

    def apply_rx_group(self, number: int, name: str, members: list) -> bool:
        try:
            image = with_rx_group(self._project.image, number, name.strip(), members, self.codec)
        except (RxGroupError, NameEncodingError) as exc:
            QMessageBox.warning(self, "接收群組", str(exc))
            return False
        self._commit(replace(self._project, image=image), f"編輯接收群組 {number}")
        return True

    def apply_setting(self, setting: Setting | TextSetting, value: int | str) -> bool:
        """Change one general radio setting (bits or a text field of the settings image)."""
        try:
            if isinstance(setting, TextSetting):
                image = setting.with_text(self._project.image, str(value))
            else:
                image = setting.with_raw(self._project.image, int(value))
        except SettingError as exc:
            QMessageBox.warning(self, "對講機設定", str(exc))
            self._refresh()
            return False
        self._commit(replace(self._project, image=image), f"{setting.page}：{setting.label}")
        return True

    def apply_quick_message(self, number: int, text: str) -> bool:
        try:
            image = with_quick_message(self._project.image, number, text, self.codec)
        except (ValueError, NameEncodingError) as exc:
            QMessageBox.warning(self, "常用短信", str(exc))
            self._refresh()
            return False
        self._commit(replace(self._project, image=image), f"常用短信 {number}")
        return True

    def apply_alarm_edit(self, kind: str, number: int, field: str, value: object) -> bool:
        """One edit from the 報警與加密 tab: an alarm-list field, the GPS channel or a key."""
        image = self._project.image
        try:
            if kind == "alarm":
                image = with_alarm(image, number, codec=self.codec, **{field: value})
                label = f"報警列表 {number}"
            elif kind == "gps":
                image = with_gps_channel(image, value)
                label = "GPS上報信道"
            else:
                alias, key_text = value
                key = parse_key_hex(key_text) if alias.strip() and key_text.strip() else bytes(6)
                image = with_encryption_key(image, number, alias, key, self.codec)
                label = f"加密金鑰 {number}"
        except (ListEditError, NameEncodingError) as exc:
            QMessageBox.warning(self, "報警與加密", str(exc))
            self._refresh()
            return False
        self._commit(replace(self._project, image=image), label)
        return True

