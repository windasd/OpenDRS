"""Read from / write to the radio from the main window: port picker, progress, worker.

The port picker lives on the toolbar and the progress bar in the status bar, so a
read or write needs no separate window. Every read keeps a backup; every write first
reads and backs up the radio's current settings and aborts on a model/band mismatch.
"""

from __future__ import annotations

import logging
import time
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from PySide6.QtCore import QObject, QThread, Signal
from PySide6.QtWidgets import QComboBox, QLabel, QProgressBar, QPushButton, QWidget

from ..codeplug import DATE_FORMAT, Counts
from ..f136 import Project
from ..radio_link import (
    RadioClient,
    RadioError,
    ReadResult,
    SerialTransport,
    Transport,
    WriteResult,
    clock_write_project,
    identity_mismatch,
    list_ports,
    save_backup,
)
from ..radio_sim import SimulatedRadio

log = logging.getLogger(__name__)
SIMULATED = "__sim__"
PAUSE_BETWEEN_COMMANDS = 1.5  # let the radio return to its command loop after a read
MODE_NAMES = {"read": "讀取", "write": "寫入", "clock": "校正時鐘"}


@dataclass(frozen=True)
class RadioOutcome:
    mode: str
    read: ReadResult | None
    write: WriteResult | None
    backup: Path | None
    date_text: str = ""  # the stamp written; the radio set its clock from it


class RadioWorker(QThread):
    progress = Signal(int, int, str)
    message = Signal(str)
    succeeded = Signal(object)
    failed = Signal(str)

    def __init__(self, factory: Callable[[], Transport], mode: str, backup_dir: Path,
                 project: Project | None = None, pause: float = PAUSE_BETWEEN_COMMANDS) -> None:
        super().__init__()
        self._factory, self._mode, self._backup_dir = factory, mode, backup_dir
        self._project, self._pause = project, pause

    def run(self) -> None:
        try:
            transport = self._factory()
        except RadioError as exc:
            self.failed.emit(str(exc))
            return
        try:
            client = RadioClient(transport, self.progress.emit)
            if self._mode == "read":
                result = client.read_codeplug()
                backup = save_backup(result, self._backup_dir, "read")
                self.message.emit(f"已讀取：{result.identity.summary()}")
                self.succeeded.emit(RadioOutcome("read", result, None, backup))
                return
            self.message.emit("寫入前先讀取對講機目前的設定並備份…")
            before = client.read_codeplug()
            backup = save_backup(before, self._backup_dir, "before_write")
            self.message.emit(f"備份完成：{backup.name}（{before.identity.summary()}）")
            # "clock": write the radio's own data back unchanged; only the date stamp differs
            target = clock_write_project(before) if self._mode == "clock" else self._project
            assert target is not None
            reason = identity_mismatch(target, before.identity)
            if reason:
                raise RadioError(f"已中止，未寫入任何資料：{reason}")
            time.sleep(self._pause)
            self.message.emit("開始寫入…")
            date_text = datetime.now().strftime(DATE_FORMAT)  # noqa: DTZ005 - the radio keeps local time
            written = client.write_codeplug(target, date_text,
                                            expect=lambda ident: identity_mismatch(target, ident))
            self.succeeded.emit(RadioOutcome(self._mode, before, written, backup, date_text))
        except RadioError as exc:
            self.failed.emit(str(exc))
        except Exception as exc:  # noqa: BLE001 - report anything unexpected to the user
            log.exception("radio operation failed")
            self.failed.emit(f"未預期的錯誤：{exc}")
        finally:
            transport.close()


def write_summary(project: Project) -> str:
    c = Counts.of(project)
    dev = project.device
    return (f"機型 {dev.model or '?'}　{dev.band_label or '?'}\n"
            f"分區 {c.zones}、頻道 {c.channels}、聯絡人 {c.contacts}（{c.total_blocks} 個區塊）")


class RadioPanel(QObject):
    """Toolbar port picker + status-bar progress, running one transfer at a time.

    ``finished`` carries the outcome (or None) and the error text (empty on success).
    """

    busy_changed = Signal(bool)
    finished = Signal(object, str)

    def __init__(self, backup_dir: Callable[[], Path], preferred_port: str = "",
                 parent: QWidget | None = None,
                 simulated_radio: SimulatedRadio | None = None) -> None:
        super().__init__(parent)
        self._backup_dir = backup_dir
        self._sim = simulated_radio
        self._worker: RadioWorker | None = None
        self._outcome: RadioOutcome | None = None
        self._error = ""
        self.log: list[str] = []
        self.port_box = QComboBox()
        self.port_box.setMinimumWidth(200)
        self.port_box.setMaximumWidth(280)
        self.port_box.setSizeAdjustPolicy(QComboBox.SizeAdjustPolicy.AdjustToMinimumContentsLengthWithIcon)
        self.port_box.setMinimumContentsLength(18)
        self.port_box.setToolTip("寫頻線連接埠")
        self.refresh_button = QPushButton("重新整理")
        self.refresh_button.setToolTip("重新列出連接埠（插拔寫頻線後使用）")
        self.refresh_button.clicked.connect(lambda: self.refresh_ports())
        self.progress = QProgressBar()
        self.progress.setMaximumWidth(220)
        self.progress.setVisible(False)
        self.status = QLabel("")
        self.refresh_ports(preferred_port)

    # ------------------------------------------------------------- ports
    def refresh_ports(self, preferred: str = "") -> None:
        keep = preferred or self.port()
        self.port_box.clear()
        try:
            ports = list_ports()
        except Exception as exc:  # noqa: BLE001
            ports = []
            self._note(f"無法列出連接埠：{exc}")
        for p in ports:
            self.port_box.addItem(f"{p.device} — {p.description}", p.device)
        self.port_box.addItem("模擬對講機（試用）", SIMULATED)
        found = self.port_box.findData(keep) if keep else -1
        self.port_box.setCurrentIndex(found if found >= 0 else 0)
        if not ports:
            self.status.setText("找不到序列埠：請確認寫頻線已插入並安裝驅動程式")

    def port(self) -> str:
        return str(self.port_box.currentData() or "")

    def port_label(self) -> str:
        return self.port_box.currentText()

    # ------------------------------------------------------------- transfer
    def is_running(self) -> bool:
        return self._worker is not None and self._worker.isRunning()

    def start(self, mode: str, project: Project | None = None) -> bool:
        if self.is_running() or not self.port():
            return False
        port = self.port()
        sim = port == SIMULATED
        if sim:
            radio = self._sim or SimulatedRadio()
            factory: Callable[[], Transport] = lambda: radio  # noqa: E731
        else:
            factory = lambda: SerialTransport(port)  # noqa: E731
        self._outcome, self._error, self.log = None, "", []
        self._worker = RadioWorker(factory, mode, self._backup_dir(), project,
                                   pause=0.0 if sim else PAUSE_BETWEEN_COMMANDS)
        self._worker.progress.connect(self._on_progress)
        self._worker.message.connect(self._note)
        self._worker.succeeded.connect(self._on_success)
        self._worker.failed.connect(self._on_failure)
        self._worker.finished.connect(self._on_finished)
        self.progress.setRange(0, 0)  # busy until the first block arrives
        self.progress.setVisible(True)
        self._note(f"{MODE_NAMES[mode]}：連線到 {self.port_label()}…")
        self.busy_changed.emit(True)
        self._worker.start()
        return True

    def wait(self, ms: int = 20_000) -> None:
        """Block until the transfer thread ends (tests)."""
        if self._worker is not None:
            self._worker.wait(ms)

    def _note(self, text: str) -> None:
        self.log.append(text)
        self.status.setText(text)

    def _on_progress(self, done: int, total: int, text: str) -> None:
        self.progress.setRange(0, max(total, 1))
        self.progress.setValue(done)
        self.status.setText(text)

    def _on_success(self, outcome: RadioOutcome) -> None:
        self._outcome = outcome

    def _on_failure(self, message: str) -> None:
        self._error = message
        self._note("錯誤：" + message)

    def _on_finished(self) -> None:
        self.progress.setVisible(False)
        if self._outcome is not None:
            self.status.setText(f"{MODE_NAMES[self._outcome.mode]}完成")
        self.busy_changed.emit(False)
        self.finished.emit(self._outcome, self._error)
