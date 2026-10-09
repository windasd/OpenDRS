"""Reading from and writing to the radio from the main window."""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import replace
from datetime import datetime
from pathlib import Path

from PySide6.QtWidgets import QMessageBox

from ..codeplug import check_writable
from ..f136 import F136FormatError, Project
from ..firmware_check import DEFAULT_IAP_EXE
from ..iap_sim import SimulatedBootloader
from ..radio_link import SerialTransport, Transport
from ..recording import DEFAULT_AMBEP2
from ..recording_sim import SimulatedRecorder
from ..settings import default_dir, save_settings
from .backup_dialog import BackupDialog
from .diff_dialog import DiffDialog
from .firmware_dialog import FirmwareDialog
from .radio_panel import SIMULATED, RadioOutcome, write_summary
from .recording_dialog import RadioIdNames, RecordingDialog


class RadioOpsMixin:
    """Starts transfers on the RadioPanel and applies their results for MainWindow."""

    def _backup_dir(self) -> Path:
        return default_dir() / "backups"

    def _remember_port(self, _index: int = 0) -> None:
        port = self._radio.port()
        if port and port != self._settings.radio_port:
            self._settings = replace(self._settings, radio_port=port)
            save_settings(self._settings)

    def _radio_ready(self, title: str) -> bool:
        if self._radio.is_running():
            return False
        if not self._radio.port():
            QMessageBox.information(self, title, "請先在工具列選擇寫頻線的連接埠。")
            return False
        return True

    def read_from_radio(self) -> bool:
        """Start reading in the background; the result loads when it finishes."""
        if not self._radio_ready("讀取對講機") or not self._confirm_discard():
            return False
        self._remember_port()
        return self._radio.start("read")

    def _on_radio_busy(self, busy: bool) -> None:
        for widget in (self._a_read, self._a_write, self._a_clock, self._radio.port_box,
                       self._radio.refresh_button, self.centralWidget()):
            widget.setEnabled(not busy)

    def _on_radio_finished(self, outcome: RadioOutcome | None, error: str) -> None:
        if outcome is None:
            QMessageBox.warning(self, "對講機", error or "傳輸未完成。")
            return
        if outcome.mode == "read" and outcome.read is not None:
            self.load_from_radio(outcome.read)
            self.statusBar().showMessage(f"已讀取 {outcome.read.identity.summary()}，備份：{outcome.backup}", 15000)
            return
        if outcome.write is None:
            return
        clock = f"對講機時鐘已設為 {outcome.date_text}（開始寫入時的電腦時間，秒數維持對講機原值）。"
        if outcome.mode == "clock":
            lines = [clock, "對講機的設定原樣寫回，沒有其他變更；目前編輯中的專案不受影響。",
                     f"寫入前的設定已備份到：{outcome.backup}",
                     "對講機正在重新啟動；之後可在對講機選單的「日期／時間」確認。"]
            QMessageBox.information(self, "校正對講機時鐘", "\n".join(lines))
            return
        lines = [f"寫入完成，對講機確認校驗和 0x{outcome.write.checksum:08X}。", clock,
                 f"寫入前的設定已備份到：{outcome.backup}",
                 "對講機正在套用設定，請等它重新啟動後再操作。"]
        QMessageBox.information(self, "寫入對講機", "\n".join(lines))

    def load_from_radio(self, result) -> None:  # noqa: ANN001
        self._history.reset(result.project)
        self._saved_project = Project()  # nothing on disk yet: prompts to save
        self._path = None
        self._source = f"從對講機讀取（{result.identity.model or '?'}，尚未儲存）"
        for page in self._pages.values():
            page.reset()
        self._refresh()

    def write_to_radio(self) -> None:
        if not self._require_project("寫入對講機"):
            return
        reasons = check_writable(self._project)
        if reasons:
            QMessageBox.warning(self, "無法寫入", "\n".join(reasons))
            return
        if not self._radio_ready("寫入對講機"):
            return
        answer = QMessageBox.question(
            self, "寫入對講機",
            f"要寫入到 {self._radio.port_label()} 嗎？\n\n{write_summary(self._project)}\n\n"
            f"寫入前會先讀取對講機並備份到 {self._backup_dir()}；機型或頻段不符會中止。")
        if answer != QMessageBox.StandardButton.Yes:
            return
        self._remember_port()
        self._radio.start("write", self._project)

    def sync_radio_clock(self) -> bool:
        """Set the radio's clock to this PC's time: read it, then write the same data back
        with a fresh date stamp (the radio sets its clock from that stamp)."""
        if not self._radio_ready("校正對講機時鐘"):
            return False
        answer = QMessageBox.question(
            self, "校正對講機時鐘",
            f"要把 {self._radio.port_label()} 上的對講機時鐘設為電腦目前的時間嗎？\n\n"
            "做法：先讀取對講機目前的設定並備份，再把同樣的資料原樣寫回，只更新「寫入時間」；"
            "對講機在寫入完成時依此設定時鐘（精確到分鐘，秒數不變），然後重新啟動。\n\n"
            "讀回的資料若無法逐位元組重建就中止，不寫入任何資料。目前編輯中的專案不受影響。")
        if answer != QMessageBox.StandardButton.Yes:
            return False
        self._remember_port()
        return self._radio.start("clock")

    def backup_manager(self) -> BackupDialog | None:
        """Browse the backups every read and write keeps: open, compare, delete."""
        if self._radio.is_running():  # a read in progress would replace whatever is opened
            QMessageBox.information(self, "備份管理", "請等目前的讀寫完成。")
            return None
        dialog = BackupDialog(self._backup_dir(), self.open_backup, self.compare_backup, parent=self)
        dialog.open()
        return dialog

    def open_backup(self, path: Path) -> bool:
        """Load a backup as the current project, detached from the file: saving asks for a
        new name, so the backup itself is never overwritten, and it stays out of 最近開啟."""
        if not self._confirm_discard():
            return False
        before = self._settings
        if not self.open_path(path):
            return False
        self._settings = replace(self._settings, recent_files=before.recent_files, last_dir=before.last_dir)
        save_settings(self._settings)
        self._rebuild_recent_menu()
        self._path = None
        self._source = f"備份 {path.name}（存檔時另存新檔）"
        self._refresh_status()
        return True

    def compare_backup(self, path: Path) -> DiffDialog | None:
        """The current project (old) against the backup (new)."""
        if not self._require_project("比較專案"):
            return None
        try:
            return self.compare_with(path)
        except (OSError, F136FormatError):  # compare_with has already shown the error
            return None

    def _has_recent_backup(self) -> bool:
        """A backup read from the radio today (before_write counts too)."""
        today = datetime.now().strftime("%Y%m%d")
        folder = self._backup_dir()
        return folder.is_dir() and any(p.name.startswith(today) for p in folder.glob("*.F136"))

    def _iap_transport(self) -> Callable[[], Transport] | None:
        port = self._radio.port()
        if not port:
            return None
        if port == SIMULATED:
            return SimulatedBootloader
        return lambda: SerialTransport(port)

    def firmware_update(self) -> FirmwareDialog:
        """Pre-flight checks, then the built-in IAP upgrade or the vendor IAP tool."""
        dev = self._project.device
        exe = Path(self._settings.iap_path) if self._settings.iap_path else DEFAULT_IAP_EXE
        dialog = FirmwareDialog(dev.firmware, dev.firmware_date, self._has_recent_backup(), exe,
                                self._radio.is_running(), parent=self, port_label=self._radio.port_label(),
                                transport=self._iap_transport())
        dialog.finished.connect(lambda _r, d=dialog: self._remember_iap(d))
        dialog.open()
        return dialog

    def _remember_iap(self, dialog: FirmwareDialog) -> None:
        chosen = dialog.exe.text().strip()
        if chosen and chosen != str(DEFAULT_IAP_EXE) and chosen != self._settings.iap_path:
            self._settings = replace(self._settings, iap_path=chosen)
            save_settings(self._settings)

    def recordings(self) -> RecordingDialog | None:
        """List and download the radio's call recordings (the vendor Wav.exe's job)."""
        if self._radio.is_running():
            QMessageBox.information(self, "讀取錄音", "請等目前的讀寫完成。")
            return None
        port = self._radio.port()
        transport: Callable[[], Transport] | None = None
        if port == SIMULATED:
            radio = SimulatedRecorder()
            transport = lambda: radio  # noqa: E731
        elif port:
            transport = lambda: SerialTransport(port)  # noqa: E731
        codec = self._settings.name_codec
        names = {c.call_id: c.name(codec) for c in self._project.contacts}
        out = Path(self._settings.recordings_dir) if self._settings.recordings_dir else default_dir() / "recordings"
        exe = Path(self._settings.ambep2_path) if self._settings.ambep2_path else DEFAULT_AMBEP2
        lookup = RadioIdNames.from_cache(default_dir() / "radioid")  # None until RadioID is downloaded
        dialog = RecordingDialog(transport, self._radio.port_label(), out, exe, names, lookup,
                                 pause=0.0 if port == SIMULATED else 0.5, parent=self)
        dialog.finished.connect(lambda _r, d=dialog: self._remember_recording_paths(d))
        dialog.open()
        return dialog

    def _remember_recording_paths(self, dialog: RecordingDialog) -> None:
        out, exe = dialog.out_dir.text().strip(), dialog.ambep2.text().strip()
        changed = replace(self._settings,
                          recordings_dir=out if out != str(default_dir() / "recordings") else "",
                          ambep2_path=exe if exe != str(DEFAULT_AMBEP2) else "")
        if changed != self._settings:
            self._settings = changed
            save_settings(self._settings)

