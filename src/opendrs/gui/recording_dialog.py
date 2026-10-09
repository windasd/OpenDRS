"""「讀取錄音」dialog: list the radio's call recordings, download them, convert to WAV."""

from __future__ import annotations

import logging
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from pathlib import Path

from PySide6.QtCore import Qt, QUrl
from PySide6.QtGui import QCloseEvent, QDesktopServices
from PySide6.QtWidgets import (
    QAbstractItemView,
    QDialog,
    QDialogButtonBox,
    QFileDialog,
    QFormLayout,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QProgressBar,
    QPushButton,
    QSpinBox,
    QTableWidget,
    QTableWidgetItem,
    QVBoxLayout,
    QWidget,
)

from .. import radioid
from ..radio_link import Transport
from ..recording import DEFAULT_LIST_MAX, MAX_RECORDS, RecordInfo
from .recording_worker import RecordingWorker, Saved

log = logging.getLogger(__name__)

HEADERS = ("編號", "收／發", "對方 ID", "聯絡人", "長度", "時間", "檔案")
FILE_COL = len(HEADERS) - 1
NUMBER_ROLE = Qt.ItemDataRole.UserRole  # column 0: the record number (rows move when sorted)
SORT_ROLE = Qt.ItemDataRole.UserRole + 1  # numeric sort key (編號, 對方 ID, 長度)
RADIOID_TIP = "RadioID.net 離線資料（不在專案聯絡人中）"
NOTE = ("對講機會把通話錄成 AMBE+2 資料；下載後以原廠的 ambep2.exe 解碼成 WAV（與原廠 Wav.exe 相同）。"
        "DR-8500UV／DH-98VU 原廠韌體（308E.BBK／B6.TARSCB.003）沒有接上這些指令；刷入「錄音讀取修補版」"
        "（DR8500UV_B6_record_read.bin）後即可讀取（已實機驗證）。"
        "下載時不要在對講機上播放或刪除錄音。")
TransportFactory = Callable[[], Transport]
NameLookup = Callable[[int], str]  # call ID -> 「呼號 姓名」, "" when unknown
_CSV_HEADER = ",".join(radioid.EXPECTED_HEADER).encode("ascii") + b"\n"


class RadioIdNames:
    """``lookup`` for :class:`RecordingDialog`: 「呼號 姓名」 from the downloaded RadioID.net
    user.csv. The file (~17 MB) is read on the first call only and each ID is then one byte
    search; parsing all of it (``radioid.load_cached``) would take seconds."""

    def __init__(self, directory: Path) -> None:
        self._path = directory / radioid.CSV_NAME
        self._data: bytes | None = None

    @classmethod
    def from_cache(cls, directory: Path) -> RadioIdNames | None:
        """``None`` when the RadioID data has not been downloaded."""
        return cls(directory) if radioid.cache_info(directory) is not None else None

    def __call__(self, radio_id: int) -> str:
        user = self.find(radio_id)
        return f"{user.callsign} {user.full_name}".strip() if user is not None else ""

    def find(self, radio_id: int) -> radioid.RadioIdUser | None:
        if self._data is None:
            try:
                self._data = self._path.read_bytes()
            except OSError as exc:  # removed or locked since the dialog opened: no names
                log.warning("cannot read RadioID cache %s: %s", self._path, exc)
                self._data = b""
        start = self._data.find(b"\n%d," % radio_id)
        if start < 0:
            return None
        end = self._data.find(b"\n", start + 1)
        row = self._data[start + 1 : end if end >= 0 else len(self._data)]
        try:  # one row, validated like the whole file
            users = radioid.parse_csv(_CSV_HEADER + row).users
        except radioid.RadioIdError:
            return None
        return users[0] if users else None


def duration_text(seconds: float) -> str:
    """「3 分 12 秒」; hours and minutes only when there are any."""
    hours, rest = divmod(round(seconds), 3600)
    minutes, secs = divmod(rest, 60)
    if hours:
        return f"{hours} 小時 {minutes} 分 {secs} 秒"
    return f"{minutes} 分 {secs} 秒" if minutes else f"{secs} 秒"


def total_text(infos: Sequence[RecordInfo]) -> str:
    """「共 25 筆，合計 3 分 12 秒」."""
    return f"共 {len(infos)} 筆，合計 {duration_text(sum(i.seconds for i in infos))}"


class _SortItem(QTableWidgetItem):
    """Sorts by SORT_ROLE when both cells have a number there, otherwise by text."""

    def __lt__(self, other: QTableWidgetItem) -> bool:
        mine, theirs = self.data(SORT_ROLE), other.data(SORT_ROLE)
        if isinstance(mine, int | float) and isinstance(theirs, int | float):
            return mine < theirs
        return self.text() < other.text()  # not super().__lt__: PySide6 calls this override again


def _cell(text: str, key: int | None = None) -> _SortItem:
    item = _SortItem(text)
    if key is not None:
        item.setData(SORT_ROLE, key)
    return item


class RecordingDialog(QDialog):
    def __init__(self, transport: TransportFactory | None, port_label: str, out_dir: Path, ambep2: Path,
                 names: dict[int, str] | None = None, lookup: NameLookup | None = None,
                 pause: float = 0.5, parent: QWidget | None = None) -> None:
        """``names``: the project's contacts (call ID -> name); ``lookup`` names the other IDs
        (e.g. :class:`RadioIdNames`), called once per ID."""
        super().__init__(parent)
        self.setWindowTitle("讀取錄音")
        self.resize(820, 600)
        self._transport, self._names, self._pause = transport, names or {}, pause
        self._lookup = lookup
        self._looked_up: dict[int, str] = {}
        self._worker: RecordingWorker | None = None
        self.records: list[RecordInfo] = []
        self.saved: list[Saved] = []
        self.error = ""
        self._mode = ""
        self._run_saved: list[Saved] = []
        self._stopping = False

        self.out_dir = QLineEdit(str(out_dir))
        pick_dir = QPushButton("選擇…")
        pick_dir.clicked.connect(self._pick_dir)
        open_dir = QPushButton("開啟資料夾")
        open_dir.clicked.connect(lambda: QDesktopServices.openUrl(QUrl.fromLocalFile(self.out_dir.text())))
        dir_row = QHBoxLayout()
        for w, stretch in ((self.out_dir, 1), (pick_dir, 0), (open_dir, 0)):
            dir_row.addWidget(w, stretch)
        self.ambep2 = QLineEdit(str(ambep2))
        pick_exe = QPushButton("選擇…")
        pick_exe.clicked.connect(self._pick_exe)
        exe_row = QHBoxLayout()
        exe_row.addWidget(self.ambep2, 1)
        exe_row.addWidget(pick_exe)
        form = QFormLayout()
        form.addRow("連接埠", QLabel(port_label or "未選擇（請在工具列選擇寫頻線）"))
        form.addRow("錄音資料夾", dir_row)
        form.addRow("ambep2.exe", exe_row)

        self.max_count = QSpinBox()
        self.max_count.setRange(1, MAX_RECORDS)
        self.max_count.setValue(DEFAULT_LIST_MAX)
        self.list_btn = QPushButton("讀取清單")
        self.list_btn.clicked.connect(self.read_list)
        self.download_btn = QPushButton("下載勾選的錄音")
        self.download_btn.clicked.connect(self.download_checked)
        self.all_btn = QPushButton("依編號全部下載")
        self.all_btn.setToolTip("不讀清單，直接從第 1 筆讀到最後一筆（對講機不支援清單指令時使用）")
        self.all_btn.clicked.connect(self.download_all)
        self.stop = QPushButton("停止")
        self.stop.clicked.connect(self._cancel)
        actions = QHBoxLayout()
        actions.addWidget(QLabel("最多"))
        actions.addWidget(self.max_count)
        for b in (self.list_btn, self.download_btn, self.all_btn, self.stop):
            actions.addWidget(b)
        actions.addStretch(1)

        self.table = QTableWidget(0, len(HEADERS))
        self.table.setHorizontalHeaderLabels(HEADERS)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.verticalHeader().setVisible(False)
        header = self.table.horizontalHeader()
        header.setSectionResizeMode(QHeaderView.ResizeMode.ResizeToContents)
        header.setSectionResizeMode(FILE_COL, QHeaderView.ResizeMode.Stretch)
        header.setSortIndicator(-1, Qt.SortOrder.AscendingOrder)  # list order until a header is clicked
        self.table.setSortingEnabled(True)
        self.table.cellDoubleClicked.connect(lambda row, _col: self.play(row))
        self.play_btn = QPushButton("播放")
        self.play_btn.clicked.connect(lambda: self.play(self.table.currentRow()))
        self.select_all = QPushButton("全選")
        self.select_all.clicked.connect(lambda: self.set_all_checked(True))
        self.select_none = QPushButton("全不選")
        self.select_none.clicked.connect(lambda: self.set_all_checked(False))
        self.summary = QLabel("")
        below = QHBoxLayout()
        below.addWidget(self.select_all)
        below.addWidget(self.select_none)
        below.addStretch(1)
        below.addWidget(self.summary)

        self.progress = QProgressBar()
        self.progress.setVisible(False)
        self.status = QLabel("")
        self.status.setWordWrap(True)
        note = QLabel(NOTE)
        note.setWordWrap(True)
        note.setObjectName("muted")
        self.buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Close)
        self.buttons.addButton(self.play_btn, QDialogButtonBox.ButtonRole.ActionRole)
        self.buttons.rejected.connect(self.reject)

        layout = QVBoxLayout(self)
        layout.addLayout(form)
        layout.addLayout(actions)
        layout.addWidget(self.table, 1)
        layout.addLayout(below)
        layout.addWidget(note)
        layout.addWidget(self.progress)
        layout.addWidget(self.status)
        layout.addWidget(self.buttons)
        for button in self.findChildren(QPushButton):
            button.setAutoDefault(False)  # Enter in a text box must not open a picker
        self._update()

    # ------------------------------------------------------------- inputs
    def _pick_dir(self) -> None:
        chosen = QFileDialog.getExistingDirectory(self, "錄音資料夾", self.out_dir.text())
        if chosen:
            self.out_dir.setText(chosen)

    def _pick_exe(self) -> None:
        chosen, _ = QFileDialog.getOpenFileName(self, "選擇 ambep2.exe", self.ambep2.text(), "程式 (*.exe)")
        if chosen:
            self.ambep2.setText(chosen)

    def checked(self) -> list[RecordInfo]:
        """The ticked records in list order, however the table is sorted."""
        ticked = set()
        for row in range(self.table.rowCount()):
            item = self.table.item(row, 0)
            if item is not None and item.checkState() == Qt.CheckState.Checked:
                ticked.add(item.data(NUMBER_ROLE))
        return [info for info in self.records if info.number in ticked]

    def set_all_checked(self, checked: bool) -> None:
        state = Qt.CheckState.Checked if checked else Qt.CheckState.Unchecked
        for row in range(self.table.rowCount()):
            item = self.table.item(row, 0)
            if item is not None:
                item.setCheckState(state)

    # ------------------------------------------------------------- table
    def row_of(self, number: int) -> int:
        """The table row showing record ``number`` (rows move when sorted), or -1."""
        for row in range(self.table.rowCount()):
            item = self.table.item(row, 0)
            if item is not None and item.data(NUMBER_ROLE) == number:
                return row
        return -1

    @contextmanager
    def _rows_pinned(self) -> Iterator[None]:
        """Rows must not move while their cells are written; resorts by the header afterwards."""
        self.table.setSortingEnabled(False)
        try:
            yield
        finally:
            self.table.setSortingEnabled(True)

    def _partner(self, call_id: int) -> tuple[str, str]:
        """(name, tooltip): the project's contact name first, else what ``lookup`` knows."""
        if self._names.get(call_id):
            return self._names[call_id], ""
        if self._lookup is None:
            return "", ""
        if call_id not in self._looked_up:
            self._looked_up[call_id] = self._lookup(call_id)
        name = self._looked_up[call_id]
        return name, RADIOID_TIP if name else ""

    def _show(self, info: RecordInfo) -> int:
        """Add or refresh the row for ``info`` (call inside ``_rows_pinned``); a refreshed row
        keeps its check box."""
        row = self.row_of(info.number)
        state = Qt.CheckState.Checked
        if row < 0:
            row = self.table.rowCount()
            self.table.insertRow(row)
        else:
            state = self.table.item(row, 0).checkState()
        first = _cell(str(info.number), info.number)
        first.setData(NUMBER_ROLE, info.number)
        first.setFlags(first.flags() | Qt.ItemFlag.ItemIsUserCheckable)
        first.setCheckState(state)
        name, tip = self._partner(info.call_id)
        partner = _cell(name)
        partner.setToolTip(tip)
        cells = (first, _cell("接收" if info.direction == "R" else "發射"), _cell(str(info.call_id), info.call_id),
                 partner, _cell(f"{info.seconds:.1f} 秒", info.length), _cell(info.time))
        for col, item in enumerate(cells):
            self.table.setItem(row, col, item)
        return row

    def _show_total(self) -> None:
        self.summary.setText(total_text(self.records) if self.records else "")

    def play(self, row: int) -> bool:
        item = self.table.item(row, FILE_COL) if row >= 0 else None
        path = item.data(Qt.ItemDataRole.UserRole) if item is not None else None
        if not path or not str(path).lower().endswith(".wav"):
            self.status.setText("這一列還沒有下載成 WAV。")
            return False
        return QDesktopServices.openUrl(QUrl.fromLocalFile(str(path)))

    # ------------------------------------------------------------- transfers
    def _running(self) -> bool:
        return self._worker is not None and self._worker.isRunning()

    def _start(self, mode: str, numbers: list[tuple[int, int]] | None = None) -> bool:
        if self._running() or self._transport is None:
            return False
        out = self.out_dir.text().strip()
        if not out:
            self.status.setText("請先指定錄音資料夾。")
            return False
        self.error, self._mode, self._run_saved, self._stopping = "", mode, [], False
        self._worker = RecordingWorker(self._transport, mode, Path(out), Path(self.ambep2.text().strip()),
                                       numbers, self.max_count.value(), self._pause)
        self._worker.progress.connect(self._on_progress)
        self._worker.listed.connect(self._on_listed)
        self._worker.saved.connect(self._on_saved)
        self._worker.failed.connect(self._on_failed)
        self._worker.finished.connect(self._on_finished)
        self.progress.setRange(0, 0)
        self.progress.setVisible(True)
        self.status.setText("連線中…")
        self._worker.start()
        self._update()
        return True

    def read_list(self) -> bool:
        return self._start("list")

    def download_checked(self) -> bool:
        chosen = self.checked()
        if not chosen:
            self.status.setText("請先「讀取清單」並勾選要下載的錄音。")
            return False
        return self._start("download", [(i.number, i.pages_field) for i in chosen])

    def download_all(self) -> bool:
        return self._start("all")

    def wait(self, ms: int = 20_000) -> None:
        if self._worker is not None:
            self._worker.wait(ms)

    def _cancel(self) -> None:
        if self._worker is not None:
            self._worker.cancel()
            self._stopping = True
            self.stop.setEnabled(False)
            self.status.setText("停止中（完成目前這一塊）…")

    def _on_progress(self, done: int, total: int, text: str) -> None:
        self.progress.setRange(0, max(total, 1))
        self.progress.setValue(done)
        if not self._stopping:
            self.status.setText(text)

    def _on_listed(self, infos: list[RecordInfo]) -> None:
        self.records = list(infos)
        with self._rows_pinned():
            self.table.setRowCount(0)
            for info in infos:
                self._show(info)
        self._show_total()
        cut = "已停止，清單可能不完整。" if self._stopping else ""
        self.status.setText(f"對講機回報 {len(infos)} 筆錄音（共 {infos[-1].total} 筆）。{cut}勾選後按「下載勾選的錄音」。")

    def _on_saved(self, saved: Saved) -> None:
        self.saved.append(saved)
        self._run_saved.append(saved)
        info = saved.info  # the radio's line at download time replaces the listed one
        if any(i.number == info.number for i in self.records):
            self.records = [info if i.number == info.number else i for i in self.records]
        else:
            self.records = [*self.records, info]
        path = saved.wav or saved.raw
        item = _cell(path.name + (f"（{saved.note}）" if saved.note else ""))
        item.setData(Qt.ItemDataRole.UserRole, str(path))
        with self._rows_pinned():
            self.table.setItem(self._show(info), FILE_COL, item)
        self._show_total()

    def _on_failed(self, message: str) -> None:
        self.error = message
        self.status.setText(message)

    def _on_finished(self) -> None:
        self.progress.setVisible(False)
        if self._mode in ("download", "all") and not self.error and self._run_saved:
            wavs = sum(1 for s in self._run_saved if s.wav)
            length = duration_text(sum(s.info.seconds for s in self._run_saved))
            self.status.setText(f"已下載 {len(self._run_saved)} 筆（{wavs} 筆轉成 WAV，合計 {length}），"
                                f"存放在 {self.out_dir.text()}")
        self.stop.setEnabled(True)
        self._stopping = False
        self._update()

    def _update(self) -> None:
        running = self._running()
        ready = self._transport is not None and not running
        for w in (self.list_btn, self.download_btn, self.all_btn, self.max_count, self.out_dir, self.ambep2):
            w.setEnabled(ready)
        self.stop.setVisible(running)
        if self._transport is None:
            self.status.setText("請先在主視窗工具列選擇寫頻線的連接埠。")

    def closeEvent(self, event: QCloseEvent) -> None:  # noqa: N802 - Qt override
        if self._running():
            event.ignore()
            self.status.setText("傳輸進行中，請等完成或按「停止」。")
            return
        super().closeEvent(event)

    def reject(self) -> None:
        if self._running():
            self.status.setText("傳輸進行中，請等完成或按「停止」。")
            return
        super().reject()
