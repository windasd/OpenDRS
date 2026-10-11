"""工具 › 檢查更新…: compare with the latest GitHub release; download and install it (.exe build)."""

from __future__ import annotations

import logging
import shutil
from collections.abc import Callable
from pathlib import Path

from PySide6.QtCore import QThread, QUrl, Signal
from PySide6.QtGui import QDesktopServices
from PySide6.QtWidgets import (
    QCheckBox,
    QDialog,
    QDialogButtonBox,
    QLabel,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QTextBrowser,
    QVBoxLayout,
    QWidget,
)

from .. import __version__, update
from ..update import PreparedUpdate, Release, UpdateError

log = logging.getLogger(__name__)
Fetch = Callable[[], Release]
Prepare = Callable[..., PreparedUpdate]


class CheckThread(QThread):
    found = Signal(object)  # Release
    failed = Signal(str)

    def __init__(self, fetch: Fetch) -> None:
        super().__init__()
        self._fetch = fetch

    def run(self) -> None:
        try:
            self.found.emit(self._fetch())
        except UpdateError as exc:
            self.failed.emit(str(exc))
        except Exception as exc:
            log.exception("update check failed")
            self.failed.emit(f"未預期的錯誤：{exc}")


class PrepareThread(QThread):
    progress = Signal(int, int)
    ready = Signal(object)  # PreparedUpdate
    failed = Signal(str)

    def __init__(self, prepare: Prepare, release: Release, target: Path) -> None:
        super().__init__()
        self._prepare, self._release, self._target = prepare, release, target
        self._cancelled = False

    def cancel(self) -> None:
        self._cancelled = True

    def run(self) -> None:
        try:
            self.ready.emit(self._prepare(self._release, self._target, progress=self.progress.emit,
                                          cancelled=lambda: self._cancelled))
        except UpdateError as exc:
            self.failed.emit(str(exc))
        except Exception as exc:
            log.exception("update download failed")
            self.failed.emit(f"未預期的錯誤：{exc}")


class UpdateDialog(QDialog):
    """Shows the latest release; for the .exe build it can download, verify and install it."""

    def __init__(self, parent: QWidget | None = None, *, current: str = __version__,
                 fetch: Fetch = update.fetch_latest, prepare: Prepare = update.prepare,
                 target: Path | None = None, on_install: Callable[[PreparedUpdate], bool] | None = None,
                 auto_check: bool = True, on_auto_check_changed: Callable[[bool], None] | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("檢查更新")
        self._current, self._fetch, self._prepare = current, fetch, prepare
        self._target, self._on_install = target, on_install
        self._release: Release | None = None
        self._check: CheckThread | None = None
        self._download: PrepareThread | None = None

        self.versions = QLabel(f"目前版本：{current}")
        self.status = QLabel("")
        self.status.setWordWrap(True)
        self.notes = QTextBrowser()
        self.notes.setOpenExternalLinks(True)
        self.notes.setVisible(False)
        self.progress = QProgressBar()
        self.progress.setVisible(False)
        self.auto = QCheckBox("啟動時自動檢查更新（每天最多一次）")
        self.auto.setChecked(auto_check)
        if on_auto_check_changed:
            self.auto.toggled.connect(on_auto_check_changed)

        self.check_button = QPushButton("重新檢查")
        self.check_button.clicked.connect(self.check)
        self.install_button = QPushButton("下載並安裝")
        self.install_button.clicked.connect(self.install)
        self.install_button.setEnabled(False)
        self.page_button = QPushButton("開啟發佈頁面")
        self.page_button.clicked.connect(self.open_page)
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        for b in (self.install_button, self.page_button, self.check_button):
            b.setAutoDefault(False)
            buttons.addButton(b, QDialogButtonBox.ButtonRole.ActionRole)
        buttons.rejected.connect(self.reject)
        self.install_button.setVisible(target is not None)  # after addButton, which shows it again

        layout = QVBoxLayout(self)
        for w in (self.versions, self.status, self.notes, self.progress, self.auto, buttons):
            layout.addWidget(w)
        self.resize(560, 420)

    # ------------------------------------------------------------- check
    def check(self) -> None:
        if self._check is not None and self._check.isRunning():
            return
        self.status.setText("正在查詢 GitHub 上的最新版本…")
        self.check_button.setEnabled(False)
        self._check = CheckThread(self._fetch)
        self._check.found.connect(self.show_release)
        self._check.failed.connect(self._check_failed)
        self._check.finished.connect(lambda: self.check_button.setEnabled(True))
        self._check.start()

    def _check_failed(self, message: str) -> None:
        self.status.setText(f"無法檢查更新：{message}")

    def show_release(self, release: Release) -> None:
        self._release = release
        published = release.published[:10]
        self.versions.setText(f"目前版本：{self._current}　·　最新版本：{release.version}"
                              + (f"（{published} 發佈）" if published else ""))
        newer = release.is_newer_than(self._current)
        if not newer:
            self.status.setText("已是最新版本。")
        elif self._target is None:
            self.status.setText(f"有新版本 {release.tag}。目前是從原始碼執行，請用 git pull 更新，"
                                "或到發佈頁面下載 exe 版。")
        elif release.package is None:
            self.status.setText(f"有新版本 {release.tag}，但沒有可自動安裝的 Windows 版，請到發佈頁面下載。")
        else:
            size = release.package.size / 1e6
            self.status.setText(f"有新版本 {release.tag}（下載約 {size:.0f} MB）。按「下載並安裝」，"
                                "下載並核對 SHA-256 後會關閉程式、換上新版並自動重新開啟；"
                                "設定、備份與錄音不受影響。")
        self.install_button.setEnabled(newer and self._target is not None and release.package is not None)
        self.notes.setMarkdown(f"### {release.title}\n\n{release.notes or '（沒有更新說明）'}")
        self.notes.setVisible(True)

    # ------------------------------------------------------------- install
    def install(self) -> None:
        if self._download is not None and self._download.isRunning():
            self._download.cancel()
            return
        if self._release is None or self._target is None:
            return
        self.install_button.setText("取消下載")
        self.check_button.setEnabled(False)
        self.progress.setRange(0, 0)
        self.progress.setVisible(True)
        self.status.setText(f"正在下載 {self._release.tag}…")
        self._download = PrepareThread(self._prepare, self._release, self._target)
        self._download.progress.connect(self._on_progress)
        self._download.ready.connect(self._ready)
        self._download.failed.connect(self._download_failed)
        self._download.finished.connect(self._download_finished)
        self._download.start()

    def _on_progress(self, done: int, total: int) -> None:
        if total:
            self.progress.setRange(0, total)
            self.progress.setValue(done)

    def _download_finished(self) -> None:
        self.install_button.setText("下載並安裝")
        self.check_button.setEnabled(True)
        self.progress.setVisible(False)

    def _download_failed(self, message: str) -> None:
        self.status.setText(f"更新未完成：{message}")

    def _ready(self, prepared: PreparedUpdate) -> None:
        self.status.setText(f"{prepared.version} 已下載並通過 SHA-256 核對。")
        answer = QMessageBox.question(
            self, "安裝更新",
            f"要關閉 OpenDRS 並安裝 {prepared.version} 嗎？\n\n"
            "安裝完成後會自動重新開啟；有未儲存的修改時會先詢問。設定、備份與錄音不受影響。")
        if answer == QMessageBox.StandardButton.Yes and self._on_install and self._on_install(prepared):
            return
        shutil.rmtree(prepared.staging, ignore_errors=True)
        self.status.setText("已取消安裝。")

    def open_page(self) -> None:
        url = self._release.page_url if self._release else update.RELEASES_URL
        QDesktopServices.openUrl(QUrl(url))

    def done(self, result: int) -> None:
        for thread in (self._download, self._check):
            if thread is not None and thread.isRunning():
                if isinstance(thread, PrepareThread):
                    thread.cancel()
                thread.wait(30_000)
        super().done(result)
