"""「備份管理」dialog: browse the backups every radio read and write keeps; open one as the
current project, compare it with the current project, or delete it."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path

from PySide6.QtCore import Qt, QUrl
from PySide6.QtGui import QBrush, QColor, QDesktopServices
from PySide6.QtWidgets import (
    QAbstractItemView,
    QApplication,
    QDialog,
    QDialogButtonBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QMessageBox,
    QPushButton,
    QSpinBox,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from ..backups import (
    BEFORE_WRITE,
    Backup,
    cleanup_candidates,
    delete_backup,
    list_backups,
    size_text,
)
from . import theme

HEADERS = ("時間", "類型", "機型", "分區", "頻道", "聯絡人", "大小", "備註")
COUNT_COL = 3  # 分區, 頻道, 聯絡人
SIZE_COL = 6
NOTE_COL = 7
LAST_WRITE_NOTE = "最近一次寫入前的備份（清理時保留）"
DEFAULT_KEEP = 10
MAX_KEEP = 999
NOTE = ("每次讀取對講機，以及每次寫入之前，都會自動保留一份備份（.F136 專案＋.bin 原始傳輸資料）。"
        "要把備份寫回對講機：開啟後按「寫入對講機」。開啟的備份存檔時會另存新檔，備份本身不會被修改。")
OpenBackup = Callable[[Path], bool]  # load as the current project; False when cancelled / failed
CompareBackup = Callable[[Path], QDialog | None]  # a report against the current project


def _cell(text: str, align_right: bool = False) -> QTableWidgetItem:
    item = QTableWidgetItem(text)
    if align_right:
        item.setTextAlignment(Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter)
    return item


def _row_cells(backup: Backup, last_write: bool) -> list[QTableWidgetItem]:
    """One table row; ``last_write`` marks the newest before_write backup (cleanup keeps it)."""
    files = "、".join(p.name for p in backup.files())
    counts = ((backup.counts.zones, backup.counts.channels, backup.counts.contacts)
              if backup.counts is not None else ("—", "—", "—"))
    cells = [_cell(backup.time_text), _cell(backup.kind_label), _cell(backup.model),
             *(_cell(str(value), align_right=True) for value in counts),
             _cell(size_text(backup.size), align_right=True)]
    for item in cells:
        item.setToolTip(files)
    if backup.counts is None:
        note = _cell("無法讀取：" + backup.error.replace("\n", " "))
        note.setToolTip(backup.error)
        note.setForeground(QBrush(QColor(theme.tokens().danger)))
    else:
        note = _cell(LAST_WRITE_NOTE if last_write else "")
        note.setToolTip(files)
    return [*cells, note]


class BackupDialog(QDialog):
    def __init__(self, folder: Path, open_backup: OpenBackup, compare_backup: CompareBackup,
                 parent: QWidget | None = None) -> None:
        super().__init__(parent)
        self.setWindowTitle("備份管理")
        self.resize(820, 520)
        self._folder = folder
        self._open_backup, self._compare_backup = open_backup, compare_backup
        self.backups: list[Backup] = []

        where = QLabel(str(folder))
        where.setTextInteractionFlags(Qt.TextInteractionFlag.TextSelectableByMouse)
        self.folder_btn = QPushButton("開啟資料夾")
        self.folder_btn.clicked.connect(self.open_folder)
        top = QHBoxLayout()
        top.addWidget(QLabel("備份資料夾："))
        top.addWidget(where, 1)
        top.addWidget(self.folder_btn)

        self.table = QTableWidget(0, len(HEADERS))
        self.table.setHorizontalHeaderLabels(HEADERS)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.setSelectionMode(QAbstractItemView.SelectionMode.SingleSelection)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.verticalHeader().setVisible(False)
        header = self.table.horizontalHeader()
        header.setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        header.setSectionResizeMode(NOTE_COL, QHeaderView.ResizeMode.Stretch)
        self.table.itemSelectionChanged.connect(self._update)
        self.table.cellDoubleClicked.connect(lambda _row, _col: self.open_selected())

        self.open_btn = QPushButton("開啟")
        self.open_btn.setToolTip("載入為目前的專案（目前的修改會先詢問是否儲存）")
        self.open_btn.clicked.connect(self.open_selected)
        self.compare_btn = QPushButton("與目前專案比較")
        self.compare_btn.setToolTip("逐位元組比較目前的專案內容與這份備份")
        self.compare_btn.clicked.connect(self.compare_selected)
        self.delete_btn = QPushButton("刪除…")
        self.delete_btn.setToolTip("刪除這份備份的 .F136 與 .bin")
        self.delete_btn.clicked.connect(self.delete_selected)
        self.keep = QSpinBox()
        self.keep.setRange(1, MAX_KEEP)
        self.keep.setValue(DEFAULT_KEEP)
        self.keep.setSuffix(" 份")
        self.keep.valueChanged.connect(self._update)
        self.cleanup_btn = QPushButton("清理…")
        self.cleanup_btn.setToolTip("刪除較舊的備份；最近一次寫入前的備份一律保留")
        self.cleanup_btn.clicked.connect(self.cleanup)
        actions = QHBoxLayout()
        for button in (self.open_btn, self.compare_btn, self.delete_btn):
            actions.addWidget(button)
        actions.addStretch(1)
        actions.addWidget(QLabel("只保留最近"))
        actions.addWidget(self.keep)
        actions.addWidget(self.cleanup_btn)

        self.summary = QLabel("")
        note = QLabel(NOTE)
        note.setWordWrap(True)
        note.setObjectName("muted")
        buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        buttons.rejected.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addLayout(top)
        layout.addWidget(self.table, 1)
        layout.addWidget(self.summary)
        layout.addLayout(actions)
        layout.addWidget(note)
        layout.addWidget(buttons)
        for button in self.findChildren(QPushButton):
            button.setAutoDefault(False)  # Enter must not delete or open anything
        self.reload()

    # ------------------------------------------------------------- list
    def reload(self) -> None:
        """Read every backup's summary (a few ms each) and refill the table."""
        QApplication.setOverrideCursor(Qt.CursorShape.WaitCursor)
        try:
            self.backups = list_backups(self._folder)
        finally:
            QApplication.restoreOverrideCursor()
        self._fill()

    def _fill(self) -> None:
        self.table.setRowCount(0)  # drop the selection too: row N would now be another backup
        self.table.setRowCount(len(self.backups))
        last_write = self._newest_before_write()
        for row, backup in enumerate(self.backups):
            for col, item in enumerate(_row_cells(backup, backup is last_write)):
                self.table.setItem(row, col, item)
        total = sum(b.size for b in self.backups)
        self.summary.setText(f"共 {len(self.backups)} 份備份，合計 {size_text(total)}" if self.backups
                             else "目前沒有備份；讀取對講機或寫入對講機時會自動建立。")
        self._update()

    def select(self, row: int) -> None:
        self.table.selectRow(row)

    def selected(self) -> Backup | None:
        rows = self.table.selectionModel().selectedRows()
        return self.backups[rows[0].row()] if len(rows) == 1 else None

    def _newest_before_write(self) -> Backup | None:
        return next((b for b in self.backups if b.kind == BEFORE_WRITE), None)

    def _update(self) -> None:
        chosen = self.selected() is not None
        for button in (self.open_btn, self.compare_btn, self.delete_btn):
            button.setEnabled(chosen)
        self.cleanup_btn.setEnabled(bool(cleanup_candidates(self.backups, self.keep.value())))

    # ------------------------------------------------------------- actions
    def open_selected(self) -> bool:
        backup = self.selected()
        if backup is None or not self._open_backup(backup.path):
            return False
        self.accept()
        return True

    def compare_selected(self) -> QDialog | None:
        backup = self.selected()
        dialog = self._compare_backup(backup.path) if backup is not None else None
        if dialog is not None:
            dialog.open()
        return dialog

    def open_folder(self) -> bool:
        if not self._folder.is_dir():
            QMessageBox.information(self, "備份管理", f"備份資料夾還不存在：\n{self._folder}\n\n"
                                                      "讀取對講機後會自動建立。")
            return False
        return QDesktopServices.openUrl(QUrl.fromLocalFile(str(self._folder)))

    def _confirm(self, title: str, lines: list[str]) -> bool:
        answer = QMessageBox.question(self, title, "\n".join([*lines, "", "刪除後無法復原。"]),
                                      QMessageBox.StandardButton.Yes | QMessageBox.StandardButton.No,
                                      QMessageBox.StandardButton.No)
        return answer == QMessageBox.StandardButton.Yes

    def delete_selected(self) -> bool:
        backup = self.selected()
        if backup is None:
            return False
        lines = [f"要刪除 {backup.time_text} 的「{backup.kind_label}」備份嗎？", "",
                 *(p.name for p in backup.files())]
        if backup is self._newest_before_write():
            lines += ["", "這是最近一次寫入對講機之前的備份；寫入後若有問題，就是用它寫回。"]
        if not self._confirm("刪除備份", lines):
            return False
        try:
            delete_backup(backup)
        except (OSError, ValueError) as exc:
            QMessageBox.warning(self, "刪除失敗", f"{backup.path.name}\n\n{exc}")
            self.reload()  # the .F136 may be gone even if the .bin is not
            return False
        self.backups = [b for b in self.backups if b != backup]
        self._fill()
        return True

    def cleanup(self) -> int:
        """Delete all but the newest N backups (keeping the newest before_write one too)."""
        keep = self.keep.value()
        doomed = cleanup_candidates(self.backups, keep)
        if not doomed:
            return 0
        span = doomed[-1].time_text if len(doomed) == 1 else f"{doomed[-1].time_text} ～ {doomed[0].time_text}"
        freed = size_text(sum(b.size for b in doomed))
        lines = [f"將刪除 {len(doomed)} 份較舊的備份（{span}，共 {freed}），保留最近 {keep} 份。"]
        last_write = self._newest_before_write()
        if last_write is not None and self.backups.index(last_write) >= keep:
            lines.append(f"另外保留最近一次寫入前的備份（{last_write.time_text}）。")
        if not self._confirm("清理備份", lines):
            return 0
        deleted: set[Backup] = set()
        failed: list[str] = []
        for backup in doomed:
            try:
                delete_backup(backup)
                deleted.add(backup)
            except (OSError, ValueError) as exc:
                failed.append(f"{backup.path.name}：{exc}")
        if failed:  # a half-deleted pair: re-read the folder rather than guess
            self.reload()
            QMessageBox.warning(self, "部分備份未刪除", "\n".join(failed))
        else:
            self.backups = [b for b in self.backups if b not in deleted]
            self._fill()
        return len(deleted)
