"""Device page: model / band info, and the editable alias and DMR ID."""

from __future__ import annotations

from PySide6.QtCore import Qt
from PySide6.QtGui import QIntValidator
from PySide6.QtWidgets import (
    QFormLayout,
    QGroupBox,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QMessageBox,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from .. import radioid
from ..device import DMR_ID_OFFSET, DMR_ID_SIZE, MAX_ALIAS_LEN, MAX_DMR_ID, parse_dmr_id
from ..settings import default_dir


def describe_dmr_id(dmr_id: int, alias: str, user: radioid.RadioIdUser | None,
                    cache_available: bool = True) -> str:
    """The device page's DMR ID line, with the RadioID registration when known."""
    if not cache_available:
        return "尚未下載 RadioID 資料，無法查詢呼號"
    if user is None:
        return f"RadioID 查無 {dmr_id}"
    place = "、".join(p for p in (user.city, user.country) if p)
    text = f"RadioID：{user.callsign} {user.first_name}".rstrip()
    if place:
        text += f"（{place}）"
    if alias and user.callsign and alias.casefold() != user.callsign.casefold():
        text += f"\n⚠ 別名 {alias} 與 RadioID 登記的呼號 {user.callsign} 不同（呼號可能已變更）"
    return text


class DeviceEditingMixin:
    """Builds the device page and applies alias / DMR ID edits for MainWindow."""

    def _device_page(self) -> QWidget:
        page = QWidget()
        outer = QVBoxLayout(page)
        outer.setContentsMargins(16, 12, 16, 12)
        outer.setSpacing(12)
        title = QLabel("裝置資訊")
        title.setObjectName("pageTitle")
        outer.addWidget(title)
        self._dev_labels = {}
        groups = (("對講機", (("model", "機型"), ("model_number", "型號"), ("firmware", "韌體版本"),
                               ("firmware_date", "版本日期"), ("band", "頻段標示"), ("bands", "允許頻段"),
                               ("date", "寫入時間"), ("factory_number", "出廠編號"), ("serial_number", "序號"))),
                  ("本機身分", (("alias", "別名"), ("dmr_id", "本機 DMR ID"))),
                  ("專案", (("img", "設定區大小"),)))
        row = QHBoxLayout()
        row.setSpacing(12)
        for heading, fields in groups:
            box = QGroupBox(heading)
            form = QFormLayout(box)
            form.setHorizontalSpacing(14)
            form.setVerticalSpacing(10)
            for key, label in fields:
                value = QLabel("—")
                value.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
                self._dev_labels[key] = value
                editor = self._device_editor(key)
                form.addRow(label, value if editor is None else editor)
            (outer if heading == "本機身分" else row).addWidget(box)
            if heading == "對講機":
                outer.addLayout(row)
        note = QLabel("頻道記錄 +0x0C 為接收頻率、+0x10 為發射頻率（2026-10-08 以實機讀寫擷取確認）。")
        note.setObjectName("muted")
        note.setWordWrap(True)
        outer.addWidget(note)
        outer.addStretch(1)
        return page

    def _device_editor(self, key: str) -> QWidget | None:
        """Alias and DMR ID are editable: a line edit (Enter or leaving the box applies)."""
        if key not in ("alias", "dmr_id"):
            return None
        edit = QLineEdit()
        edit.setMaximumWidth(160)
        box = QWidget()
        row = QHBoxLayout(box)
        row.setContentsMargins(0, 0, 0, 0)
        row.addWidget(edit)
        if key == "alias":
            edit.setMaxLength(MAX_ALIAS_LEN)
            edit.setToolTip(f"最多 {MAX_ALIAS_LEN} 個英數字元")
            edit.editingFinished.connect(self._apply_alias)
            self._alias_edit = edit
            row.addWidget(self._dev_labels[key], 1)
            self._dev_labels[key].setText(f"最多 {MAX_ALIAS_LEN} 個英數字元；按 Enter 或離開欄位即套用")
        else:
            edit.setValidator(QIntValidator(1, MAX_DMR_ID, edit))
            edit.setToolTip(f"1–{MAX_DMR_ID}")
            edit.editingFinished.connect(self._apply_dmr_id)
            self._dmr_edit = edit
            self._dmr_info = self._dev_labels[key]
            row.addWidget(self._dmr_info, 1)
            self._alias_from_radioid_btn = QPushButton("以 RadioID 呼號更新別名")
            self._alias_from_radioid_btn.clicked.connect(self.alias_from_radioid)
            row.addWidget(self._alias_from_radioid_btn)
        return box

    def _apply_device_change(self, edit: QLineEdit, current: str, change, label: str) -> None:  # noqa: ANN001
        text = edit.text().strip()
        if text == current:
            return
        try:
            project = change(text)
        except ValueError as exc:
            edit.setText(current)
            QMessageBox.warning(self, label, str(exc))
            return
        self._commit(project, label)

    def _apply_alias(self) -> None:
        self._apply_device_change(self._alias_edit, self._project.device.alias,
                                  self._project.with_alias, "修改別名")

    def _apply_dmr_id(self) -> None:
        current = self._project.device.dmr_id
        self._apply_device_change(self._dmr_edit, "" if current is None else str(current),
                                  lambda text: self._project.with_dmr_id(parse_dmr_id(text)),
                                  "修改 DMR ID")

    def alias_from_radioid(self) -> None:
        dev = self._project.device
        user = self._radioid_lookups.get(dev.dmr_id) if dev.dmr_id is not None else None
        if user is None or not user.callsign:
            return
        try:
            project = self._project.with_alias(user.callsign)
        except ValueError as exc:
            QMessageBox.warning(self, "修改別名", str(exc))
            return
        self._commit(project, "別名改為 RadioID 呼號")

    def _describe_dmr_id(self, dmr_id: int | None, alias: str) -> str:
        if dmr_id is None:
            return "未設定"
        cache = default_dir() / "radioid"
        if radioid.cache_info(cache) is None:
            return describe_dmr_id(dmr_id, alias, None, cache_available=False)
        if dmr_id not in self._radioid_lookups:
            self._radioid_lookups[dmr_id] = radioid.lookup_cached(cache, dmr_id)
        return describe_dmr_id(dmr_id, alias, self._radioid_lookups[dmr_id])

    def _refresh_identity_editors(self, dev) -> None:  # noqa: ANN001
        editable = len(self._project.image) >= DMR_ID_OFFSET + DMR_ID_SIZE
        for edit, text in ((self._alias_edit, dev.alias),
                           (self._dmr_edit, "" if dev.dmr_id is None else str(dev.dmr_id))):
            edit.setEnabled(editable)
            if not edit.hasFocus() or edit.text().strip() == text:
                edit.setText(text)
        self._dmr_info.setText(self._describe_dmr_id(dev.dmr_id, dev.alias) if editable else "—")
        user = self._radioid_lookups.get(dev.dmr_id) if dev.dmr_id is not None else None
        self._alias_from_radioid_btn.setVisible(
            editable and user is not None and bool(user.callsign)
            and user.callsign.casefold() != dev.alias.casefold())
