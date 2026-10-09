"""「批次新增頻道」dialog (the legacy CPS's ADD++): N copies of a channel with stepped
frequencies and numbered names."""

from __future__ import annotations

import re
from dataclasses import dataclass

from PySide6.QtWidgets import (
    QDialog,
    QDialogButtonBox,
    QFormLayout,
    QLabel,
    QLineEdit,
    QSpinBox,
    QVBoxLayout,
    QWidget,
)

from ..device import Band, frequency_problem
from ..layout import MAX_BATCH
from ..records import Channel, hz_to_mhz, mhz_to_hz
from ..textcodec import NameCodec


@dataclass(frozen=True)
class BatchPlan:
    count: int
    start_rx_hz: int
    step_hz: int
    tx_offset_hz: int
    name_prefix: str
    first_number: int

    def rx(self, k: int) -> int:
        return self.start_rx_hz + k * self.step_hz

    def problems(self, bands: tuple[Band, ...] | None) -> list[str]:
        out = []
        for k in range(self.count):
            for label, hz in (("接收", self.rx(k)), ("發射", self.rx(k) + self.tx_offset_hz)):
                reason = frequency_problem(hz, bands)
                if reason:
                    out.append(f"第 {k + 1} 台{label}：{reason}")
        return out


def _split_name(name: str) -> tuple[str, int]:
    """"FRS_TW_14" -> ("FRS_TW_", 15): continue the numbering after the template."""
    match = re.match(r"^(.*?)(\d+)$", name)
    return (match[1], int(match[2]) + 1) if match else (name, 1)


class BatchAddDialog(QDialog):
    def __init__(self, template: Channel, codec: NameCodec, bands: tuple[Band, ...] | None,
                 zone_name: str, parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("批次新增頻道")
        self._bands = bands
        prefix, number = _split_name(template.name(codec))
        self.count = QSpinBox()
        self.count.setRange(1, MAX_BATCH)
        self.count.setValue(4)
        self.start = QLineEdit(hz_to_mhz(template.freq_a_hz + 12_500))
        self.step = QLineEdit("12.5")
        self.offset = QLineEdit(hz_to_mhz(template.freq_b_hz - template.freq_a_hz).rstrip("0").rstrip(".") or "0")
        self.prefix = QLineEdit(prefix)
        self.first = QSpinBox()
        self.first.setRange(0, 99999)
        self.first.setValue(number)
        self.preview = QLabel()
        self.preview.setWordWrap(True)
        form = QFormLayout()
        form.addRow("加入分區", QLabel(zone_name))
        form.addRow("範本頻道", QLabel(f"{template.name(codec)}（其餘設定照抄）"))
        form.addRow("數量", self.count)
        form.addRow("起始接收頻率 (MHz)", self.start)
        form.addRow("頻率間隔 (kHz)", self.step)
        form.addRow("發射差頻 (MHz)", self.offset)
        form.addRow("名稱前綴", self.prefix)
        form.addRow("起始編號", self.first)
        self.buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok | QDialogButtonBox.StandardButton.Cancel)
        self.buttons.accepted.connect(self.accept)
        self.buttons.rejected.connect(self.reject)
        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(self.preview)
        layout.addWidget(self.buttons)
        for widget in (self.start, self.step, self.offset, self.prefix):
            widget.textChanged.connect(self._update)
        for spin in (self.count, self.first):
            spin.valueChanged.connect(self._update)
        self._update()

    def plan(self) -> BatchPlan:
        """The settings entered; raises ValueError on an unreadable number."""
        step_khz = self.step.text().strip() or "0"
        return BatchPlan(self.count.value(), mhz_to_hz(self.start.text()),
                         mhz_to_hz(f"{float(step_khz) / 1000:.6f}"),
                         mhz_to_hz(self.offset.text() or "0"), self.prefix.text(), self.first.value())

    def _update(self, *_args: object) -> None:
        ok = self.buttons.button(QDialogButtonBox.StandardButton.Ok)
        try:
            plan = self.plan()
        except ValueError as exc:
            self.preview.setText(f"⚠ {exc}")
            ok.setEnabled(False)
            return
        last = plan.count - 1
        text = (f"將新增 {plan.count} 台：{plan.name_prefix}{plan.first_number}"
                f"（接收 {hz_to_mhz(plan.rx(0))}）… {plan.name_prefix}{plan.first_number + last}"
                f"（接收 {hz_to_mhz(plan.rx(last))}，發射 {hz_to_mhz(plan.rx(last) + plan.tx_offset_hz)}）")
        problems = plan.problems(self._bands)
        if problems:
            text += "\n⚠ " + "；".join(problems[:3]) + ("…" if len(problems) > 3 else "")
        self.preview.setText(text)
        ok.setEnabled(not problems)
