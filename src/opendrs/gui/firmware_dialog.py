"""「韌體升級」dialog: pick a firmware file, check it against the radio, then either
flash it through the IAP bootloader (built in, :mod:`opendrs.iap`) or start the vendor
IAP tool with the file path on the clipboard."""

from __future__ import annotations

import hashlib
from collections.abc import Callable
from pathlib import Path

from PySide6.QtCore import QProcess, Qt
from PySide6.QtGui import QCloseEvent, QGuiApplication
from PySide6.QtWidgets import (
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QLabel,
    QLineEdit,
    QListView,
    QListWidget,
    QListWidgetItem,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QVBoxLayout,
    QWidget,
)

from .. import fwpatch
from ..firmware_check import Finding, FirmwareFile, Level, check, inspect, verdict
from ..iap import IapImage, IapResult
from ..radio_link import Transport
from . import theme
from .iap_worker import IapWorker

STEPS = ("內建升級：依原廠說明讓對講機進入升級（IAP）模式，勾選上方確認後按「開始升級」；"
         "完成前不可拔線或關機。\n原廠工具：選擇 COM 埠並按「Open」→ 按「Open APP File」，"
         "在檔名欄按 Ctrl+V 貼上韌體路徑 → 讓對講機進入升級模式後按「IAP」。")
VERIFIED = ("內建升級已在實機驗證：2026-10-09 於 HORA DH-98VU（Kydera DR-8500UV 貼牌，BK4819）刷入原廠韌體與錄音讀取修補版各一次，"
            "皆成功；送出的位元組與原廠 IAP 工具逐一相同")

Launcher = Callable[[Path, Path], bool]  # (exe, working dir) -> started
TransportFactory = Callable[[], Transport]


def start_detached(exe: Path, workdir: Path) -> bool:
    ok, _pid = QProcess.startDetached(str(exe), [], str(workdir))
    return bool(ok)


class FirmwareDialog(QDialog):
    def __init__(self, radio_firmware: str, radio_date: str, has_recent_backup: bool, iap_exe: Path,
                 port_busy: bool, launcher: Launcher = start_detached,
                 parent: QWidget | None = None, port_label: str = "",
                 transport: TransportFactory | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("韌體升級")
        self.resize(660, 600)
        self._radio_firmware = radio_firmware
        self._radio_date = radio_date
        self._has_backup = has_recent_backup
        self._port_busy = port_busy
        self._launcher = launcher
        self._transport = transport
        self._worker: IapWorker | None = None
        self.image: FirmwareFile | None = None
        self._patch_status = ""  # fwpatch.record_read_status() of the chosen file
        self.launched = False
        self.upgrade_result: IapResult | None = None
        self._stopping = False
        self.error = ""

        self.exe = QLineEdit(str(iap_exe))
        browse_exe = QPushButton("選擇…")
        browse_exe.setAutoDefault(False)
        browse_exe.clicked.connect(self._pick_exe)
        exe_row = QHBoxLayout()
        exe_row.addWidget(self.exe, 1)
        exe_row.addWidget(browse_exe)
        self.file = QLineEdit()
        self.file.setPlaceholderText("選擇要刷入的韌體檔（*.bin）")
        self.file.setReadOnly(True)
        self.browse_file = QPushButton("選擇…")
        self.browse_file.setAutoDefault(False)
        self.browse_file.clicked.connect(self._pick_file)
        self.make_patch = QPushButton("產生錄音讀取版…")
        self.make_patch.setAutoDefault(False)
        self.make_patch.setToolTip("把原廠韌體改 30 bytes，讓對講機能從電腦讀取錄音（已在實機驗證；刷回原廠檔即可復原）")
        self.make_patch.clicked.connect(lambda: self.make_record_read())
        file_row = QHBoxLayout()
        file_row.addWidget(self.file, 1)
        file_row.addWidget(self.browse_file)
        file_row.addWidget(self.make_patch)
        self.file_info = QLabel("—")
        self.file_info.setWordWrap(True)
        radio = f"{radio_firmware}（{radio_date}）" if radio_firmware and radio_date else (radio_firmware or "未知（尚未讀取對講機）")
        form = QFormLayout()
        form.addRow("對講機目前韌體", QLabel(radio))
        form.addRow("連接埠", QLabel(port_label or "未選擇（請在工具列選擇寫頻線）"))
        form.addRow("韌體檔", file_row)
        form.addRow("檔案資訊", self.file_info)
        form.addRow("原廠 IAP 工具", exe_row)

        self.findings = QListWidget()
        self.findings.setWordWrap(True)
        self.findings.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self.findings.setResizeMode(QListView.ResizeMode.Adjust)  # re-wrap long findings on resize
        self.confirm = QCheckBox("我了解刷錯韌體可能使對講機無法開機")
        self.confirm.toggled.connect(self._update)
        self.iap_mode = QCheckBox("對講機已依原廠說明進入升級（IAP）模式，並以寫頻線連接到上面的連接埠")
        self.iap_mode.toggled.connect(self._update)
        steps = QLabel(STEPS)
        steps.setWordWrap(True)
        steps.setObjectName("muted")
        self.progress = QProgressBar()
        self.progress.setVisible(False)
        self.status = QLabel("")
        self.status.setWordWrap(True)
        self.buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        self.flash = self.buttons.addButton("開始升級", QDialogButtonBox.ButtonRole.ActionRole)
        self.flash.clicked.connect(self.start_flash)
        self.stop = self.buttons.addButton("停止", QDialogButtonBox.ButtonRole.ActionRole)
        self.stop.clicked.connect(self._cancel)
        self.stop.setVisible(False)
        self.launch = self.buttons.addButton("改用原廠 IAP 工具", QDialogButtonBox.ButtonRole.ActionRole)
        self.launch.clicked.connect(self.start_tool)
        self.buttons.rejected.connect(self.reject)
        self.exe.textChanged.connect(self._update)

        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addWidget(QLabel("檢查結果"))
        layout.addWidget(self.findings, 1)
        layout.addWidget(self.confirm)
        layout.addWidget(self.iap_mode)
        layout.addWidget(steps)
        layout.addWidget(self.progress)
        layout.addWidget(self.status)
        layout.addWidget(self.buttons)
        self._update()

    # ------------------------------------------------------------- inputs
    def _pick_exe(self) -> None:
        chosen, _ = QFileDialog.getOpenFileName(self, "選擇原廠 IAP 程式", self.exe.text(), "程式 (*.exe)")
        if chosen:
            self.exe.setText(chosen)

    def _pick_file(self) -> None:
        chosen, _ = QFileDialog.getOpenFileName(self, "選擇韌體檔", "", "韌體 (*.bin);;所有檔案 (*)")
        if chosen:
            self.set_file(Path(chosen))

    def set_file(self, path: Path) -> None:
        try:
            self.image = inspect(path)
        except (OSError, ValueError) as exc:
            self.image = None
            self.file.setText(str(path))
            self.file_info.setText(f"無法讀取：{exc}")
            self._update()
            return
        self.file.setText(str(path))
        self.confirm.setChecked(False)  # a tick only covers the warnings of the file it was given for
        img = self.image
        try:
            self._patch_status = fwpatch.record_read_status(Path(path).read_bytes())
        except OSError:
            self._patch_status = ""
        note = {"patched": "\n錄音讀取版（可從電腦讀取錄音）",
                "patchable": "\n可按「產生錄音讀取版…」做出能從電腦讀取錄音的版本"}.get(self._patch_status, "")
        self.file_info.setText(f"{img.firmware_id or '（找不到韌體 ID）'}　{img.build_date}　{img.size:,} bytes"
                               f"\nSHA-256 {img.sha256[:16]}…{note}")
        self._update()

    def make_record_read(self, out: Path | None = None) -> Path | None:
        """Write the record-read version of the chosen vendor file and select it."""
        if self.image is None or self._patch_status != "patchable" or self._running():
            return None
        source = self.image.path
        try:
            result = fwpatch.patch_record_read(source.read_bytes())
        except (OSError, fwpatch.PatchError) as exc:
            self.status.setText(f"無法產生錄音讀取版：{exc}")
            return None
        if out is None:
            default = source.with_name(f"{source.stem}_record_read.bin")
            chosen, _ = QFileDialog.getSaveFileName(self, "儲存錄音讀取版韌體", str(default), "韌體 (*.bin)")
            if not chosen:
                return None
            out = Path(chosen)
        if out.resolve() == source.resolve():
            self.status.setText("請另存新檔，不要覆蓋原廠韌體（復原時需要它）。")
            return None
        try:
            out.write_bytes(result.data)
        except OSError as exc:
            self.status.setText(f"無法寫入 {out}：{exc}")
            return None
        self.set_file(out)
        self.status.setText(f"已產生錄音讀取版：{out.name}（改 {result.changed} bytes，SHA-256 {result.sha256[:16]}…）。"
                            f"要復原時再刷一次原廠檔 {source.name}。")
        return out

    # ------------------------------------------------------------- state
    def current_findings(self) -> list[Finding]:
        """Checks on the file and the radio; BLOCK stops both ways of flashing."""
        findings = [] if self.image is None else check(self.image, self._radio_firmware, self._has_backup,
                                                       self._radio_date)
        if self._port_busy:
            findings.append(Finding(Level.BLOCK, "本程式正在與對講機傳輸，請等完成後再升級"))
        return findings

    def _builtin_findings(self) -> list[Finding]:
        if self._transport is None:
            return [Finding(Level.BLOCK, "內建升級：請先在主視窗工具列選擇寫頻線的連接埠")]
        return [Finding(Level.OK, VERIFIED)]

    def _tool_findings(self) -> list[Finding]:
        if not Path(self.exe.text()).is_file():
            return [Finding(Level.BLOCK, "原廠工具：找不到 IAP+Flash.exe，請按「選擇…」指定")]
        return []

    def _allowed(self, findings: list[Finding]) -> bool:
        level = verdict(findings) if findings else Level.BLOCK
        return self.image is not None and (level is Level.OK or (level is Level.WARN and self.confirm.isChecked()))

    def _running(self) -> bool:
        return self._worker is not None and self._worker.isRunning()

    def _update(self, *_args: object) -> None:
        common = self.current_findings()
        builtin, tool = self._builtin_findings(), self._tool_findings()
        t = theme.tokens()
        self.findings.clear()
        icons = {Level.OK: ("check", t.ok), Level.WARN: ("warning", "#d97706"), Level.BLOCK: ("warning", t.danger)}
        for f in common + builtin + tool:
            name, colour = icons[f.level]
            self.findings.addItem(QListWidgetItem(theme.icon(name, colour, 16), f.text))
        if self.image is None:
            self.findings.addItem("請先選擇韌體檔")
        running = self._running()
        needs_confirm = Level.WARN in {f.level for f in common + builtin + tool}
        self.confirm.setEnabled(not running and needs_confirm
                                and Level.BLOCK not in {f.level for f in common})
        self.iap_mode.setEnabled(not running)
        self.flash.setEnabled(not running and self.iap_mode.isChecked() and self._allowed(common + builtin))
        self.launch.setEnabled(not running and self._allowed(common + tool))
        self.browse_file.setEnabled(not running)
        self.make_patch.setEnabled(not running and self.image is not None and self._patch_status == "patchable")
        self.stop.setVisible(running)

    # ------------------------------------------------------------- built-in flashing
    def start_flash(self) -> bool:
        if not self.flash.isEnabled() or self.image is None or self._transport is None:
            return False
        try:
            data = self.image.path.read_bytes()
            image = IapImage.from_bytes(data)
        except (OSError, ValueError) as exc:
            self.status.setText(f"無法讀取韌體檔：{exc}")
            return False
        if hashlib.sha256(data).hexdigest() != self.image.sha256:
            self.set_file(self.image.path)
            self.status.setText("韌體檔在檢查後被修改過，已重新檢查，請確認結果後再開始。")
            return False
        answer = QMessageBox.warning(
            self, "開始升級",
            f"即將清除對講機的韌體並寫入 {self.image.path.name}（{len(image.pages)} 頁）。\n"
            "開始後不可拔線或關機；若中途失敗，對講機會停在升級模式，可重新執行升級。\n\n要開始嗎？",
            QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No, QMessageBox.StandardButton.No)
        if answer != QMessageBox.StandardButton.Yes:
            return False
        self.upgrade_result, self.error, self._stopping = None, "", False
        self._worker = IapWorker(self._transport, image)
        self._worker.progress.connect(self._on_progress)
        self._worker.succeeded.connect(self._on_success)
        self._worker.failed.connect(self._on_failure)
        self._worker.finished.connect(self._on_finished)
        self.progress.setRange(0, len(image.pages))
        self.progress.setValue(0)
        self.progress.setVisible(True)
        self.status.setText("連線中…")
        self._worker.start()
        self._update()
        return True

    def wait(self, ms: int = 20_000) -> None:
        """Block until the upgrade thread ends (tests)."""
        if self._worker is not None:
            self._worker.wait(ms)

    def _cancel(self) -> None:
        if self._worker is not None:
            self._worker.cancel()
            self._stopping = True
            self.stop.setEnabled(False)
            self.status.setText("停止中（送完目前這一頁或停止等待回應）…")

    def _on_progress(self, done: int, total: int, text: str) -> None:
        self.progress.setRange(0, max(total, 1))
        self.progress.setValue(done)
        if not self._stopping:
            self.status.setText(text)

    def _on_success(self, result: IapResult) -> None:
        self.upgrade_result = result
        reply = " ".join(result.erase_reply.split())
        self.status.setText(f"升級完成：{result.pages} 頁，對講機確認校驗和 0x{result.checksum:08X}"
                            f"（清除回覆：{reply}）。對講機應會自行重新啟動，之後請重新「讀取對講機」"
                            "確認韌體版本。")

    def _on_failure(self, message: str, erase_sent: bool) -> None:
        self.error = message
        self.status.setText(("升級未完成，" if erase_sent else "升級未開始，") + message)

    def _on_finished(self) -> None:
        self.progress.setVisible(self.upgrade_result is not None)
        self.iap_mode.setChecked(False)  # the radio has left (or may have left) IAP mode
        self.stop.setEnabled(True)
        self._stopping = False
        self._update()

    def closeEvent(self, event: QCloseEvent) -> None:  # noqa: N802 - Qt override
        if self._running():
            event.ignore()
            self.status.setText("升級進行中，請等完成或按「停止」。")
            return
        super().closeEvent(event)

    def reject(self) -> None:
        if self._running():
            self.status.setText("升級進行中，請等完成或按「停止」。")
            return
        super().reject()

    # ------------------------------------------------------------- vendor tool
    def start_tool(self) -> bool:
        if not self.launch.isEnabled() or self.image is None:
            return False
        QGuiApplication.clipboard().setText(str(self.image.path))
        exe = Path(self.exe.text())
        self.launched = self._launcher(exe, exe.parent)
        self.file_info.setText(self.file_info.text() + ("\n已啟動 IAP 工具；韌體路徑已複製到剪貼簿。"
                                                        if self.launched else "\n無法啟動 IAP 工具。"))
        return self.launched
