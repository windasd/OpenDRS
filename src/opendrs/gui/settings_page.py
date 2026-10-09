"""Radio settings tab: a page list (like the legacy CPS navigation) and generated forms."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtCore import Qt
from PySide6.QtWidgets import (
    QComboBox,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QListWidget,
    QScrollArea,
    QSpinBox,
    QStackedWidget,
    QWidget,
)

from ..radio_settings import Setting, TextSetting, by_key, by_page, pages

Apply = Callable[[Setting, int], None]  # (setting, raw value)


class SettingsPage(QWidget):
    def __init__(self, apply: Apply, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self._apply = apply
        self._image = b""
        self._editors: dict[str, QWidget] = {}
        self._updating = False
        self.page_list = QListWidget()
        self.page_list.setMaximumWidth(180)
        self.stack = QStackedWidget()
        for page in pages():
            self.page_list.addItem(page)
            self.stack.addWidget(self._build(page))
        self.page_list.currentRowChanged.connect(self.stack.setCurrentIndex)
        self.page_list.setCurrentRow(0)
        layout = QHBoxLayout(self)
        layout.setContentsMargins(4, 4, 4, 4)
        layout.addWidget(self.page_list)
        layout.addWidget(self.stack, 1)

    def _build(self, page: str) -> QWidget:
        form_host = QWidget()
        form = QFormLayout(form_host)
        form.setFieldGrowthPolicy(QFormLayout.FieldGrowthPolicy.FieldsStayAtSizeHint)
        for setting in by_page(page):
            editor = self._editor(setting)
            self._editors[setting.key] = editor
            label = QLabel(setting.label + "：")
            if setting.note:
                label.setToolTip(setting.note)
                editor.setToolTip(setting.note)
            form.addRow(label, editor)
        scroll = QScrollArea()
        scroll.setWidgetResizable(True)
        scroll.setWidget(form_host)
        return scroll

    def _editor(self, setting: Setting | TextSetting) -> QWidget:
        if isinstance(setting, TextSetting):
            line = QLineEdit()
            line.setMaxLength(setting.size)
            line.setMaximumWidth(200)
            line.editingFinished.connect(lambda s=setting, w=line: self._text_changed(s, w.text()))
            return line
        if setting.is_choice:
            box = QComboBox()
            for label, raw in setting.choices:
                box.addItem(label, raw)
            box.currentIndexChanged.connect(lambda _i, s=setting, b=box: self._changed(s, b.currentData()))
            return box
        spin = QSpinBox()
        spin.setRange(setting.minimum, setting.maximum)
        spin.setSingleStep(setting.scale)
        spin.setSuffix(setting.unit)
        spin.editingFinished.connect(lambda s=setting, w=spin: self._changed(s, w.value() // s.scale))
        return spin

    def _text_changed(self, setting: TextSetting, text: str) -> None:
        if not self._updating and setting.text(self._image) != text.strip():
            self._apply(setting, text)

    def _changed(self, setting: Setting, raw: int | None) -> None:
        if self._updating or raw is None or setting.raw(self._image) == raw:
            return
        self._apply(setting, raw)

    def editor(self, key: str) -> QWidget:
        return self._editors[key]

    def set_image(self, image: bytes) -> None:
        """Show the values held in ``image`` without triggering edits."""
        self._image = image
        self._updating = True
        try:
            for page in pages():
                for setting in by_page(page):
                    self._show(setting, self._editors[setting.key])
        finally:
            self._updating = False

    def _show(self, setting: Setting | TextSetting, editor: QWidget) -> None:
        if isinstance(setting, TextSetting):
            text = setting.text(self._image)
            editor.setEnabled(text is not None)
            editor.setText(text or "")
            return
        raw = setting.raw(self._image)
        parents_on = all(by_key(k).raw(self._image) for k in setting.depends)
        editor.setEnabled(raw is not None and parents_on)
        if raw is None:
            return
        if isinstance(editor, QComboBox):
            index = editor.findData(raw)
            if index < 0:  # a value the legacy CPS never writes: show it, keep it
                editor.addItem(f"未知值 {raw}", raw)
                index = editor.count() - 1
            editor.setCurrentIndex(index)
        elif isinstance(editor, QSpinBox):
            editor.setValue(raw * setting.scale)

    def select_page(self, page: str) -> None:
        items = self.page_list.findItems(page, Qt.MatchFlag.MatchExactly)
        if items:
            self.page_list.setCurrentItem(items[0])
