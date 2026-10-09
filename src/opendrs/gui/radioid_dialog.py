"""Pick RadioID users to import as private-call contacts."""

from __future__ import annotations

import datetime as dt
from pathlib import Path

from PySide6.QtCore import (
    QAbstractTableModel,
    QModelIndex,
    QPersistentModelIndex,
    QRegularExpression,
    Qt,
    QThread,
    QTimer,
    Signal,
)
from PySide6.QtGui import QBrush, QColor, QRegularExpressionValidator
from PySide6.QtWidgets import (
    QComboBox,
    QDialog,
    QDialogButtonBox,
    QHBoxLayout,
    QHeaderView,
    QLabel,
    QLineEdit,
    QMessageBox,
    QProgressBar,
    QPushButton,
    QTableView,
    QVBoxLayout,
)

from .. import radioid
from ..contact_import import DEFAULT_CONTACT_CAPACITY, NameFormat, existing_private_ids
from ..f136 import Project
from ..radioid import Query, RadioIdError, RadioIdUser, UserDb

Index = QModelIndex | QPersistentModelIndex
ALL_REGIONS = "全部地區"
_MUTED = QBrush(QColor("#888888"))


class _LoadThread(QThread):
    progress = Signal(int, int)
    status = Signal(str)
    loaded = Signal(object)  # UserDb | None
    failed = Signal(str)

    def __init__(self, directory: Path, refresh: bool) -> None:
        super().__init__()
        self._dir, self._refresh = directory, refresh

    def run(self) -> None:
        try:
            if self._refresh:
                self.status.emit("下載中…")
                changed = radioid.download(self._dir, progress=self.progress.emit)
                if not changed:
                    self.status.emit("伺服器資料未變更，使用本機資料")
            self.status.emit("解析中…")
            result = radioid.load_cached(self._dir)
            self.loaded.emit(result[0] if result else None)
        except RadioIdError as exc:
            self.failed.emit(str(exc))


class UserTableModel(QAbstractTableModel):
    checked_changed = Signal()
    HEADERS = ("", "ID", "呼號", "姓名", "城市", "州／省", "國家")

    def __init__(self, existing: set[int]) -> None:
        super().__init__()
        self._rows: list[RadioIdUser] = []
        self._existing = existing
        self.checked: set[int] = set()

    def set_rows(self, rows: list[RadioIdUser]) -> None:
        self.beginResetModel()
        self._rows = rows
        self.endResetModel()

    def rows(self) -> list[RadioIdUser]:
        return self._rows

    def rowCount(self, parent: Index = QModelIndex()) -> int:  # noqa: B008, N802
        return 0 if parent.isValid() else len(self._rows)

    def columnCount(self, parent: Index = QModelIndex()) -> int:  # noqa: B008, N802
        return 0 if parent.isValid() else len(self.HEADERS)

    def headerData(self, section: int, orientation: Qt.Orientation,  # noqa: N802
                   role: int = Qt.ItemDataRole.DisplayRole) -> object:
        if role == Qt.ItemDataRole.DisplayRole and orientation == Qt.Orientation.Horizontal:
            return self.HEADERS[section]
        return None

    def _present(self, user: RadioIdUser) -> bool:
        return user.radio_id in self._existing

    def flags(self, index: Index) -> Qt.ItemFlag:
        if not index.isValid():
            return Qt.ItemFlag.NoItemFlags
        base = Qt.ItemFlag.ItemIsEnabled | Qt.ItemFlag.ItemIsSelectable
        return base | Qt.ItemFlag.ItemIsUserCheckable if index.column() == 0 else base

    def data(self, index: Index, role: int = Qt.ItemDataRole.DisplayRole) -> object:
        if not index.isValid():
            return None
        user = self._rows[index.row()]
        col = index.column()
        if role == Qt.ItemDataRole.CheckStateRole and col == 0:
            return Qt.CheckState.Checked if user.radio_id in self.checked else Qt.CheckState.Unchecked
        if role == Qt.ItemDataRole.DisplayRole:
            if col == 0:
                return "已存在" if self._present(user) else ""
            return (str(user.radio_id), user.callsign, user.full_name, user.city,
                    user.state, user.country)[col - 1]
        if role == Qt.ItemDataRole.ToolTipRole and self._present(user):
            return "已在聯絡人中：原名稱是呼號（含舊呼號）或與 RadioID 相符時更新名稱，編號不變；自訂名稱（如「老王」）保留"
        if role == Qt.ItemDataRole.ForegroundRole and self._present(user):
            return _MUTED
        return None

    def setData(self, index: Index, value: object,  # noqa: N802
                role: int = Qt.ItemDataRole.EditRole) -> bool:
        if role != Qt.ItemDataRole.CheckStateRole or index.column() != 0:
            return False
        user = self._rows[index.row()]
        state = Qt.CheckState(value) if not isinstance(value, Qt.CheckState) else value
        if state == Qt.CheckState.Checked:
            self.checked.add(user.radio_id)
        else:
            self.checked.discard(user.radio_id)
        self.dataChanged.emit(index, index, [Qt.ItemDataRole.CheckStateRole])
        self.checked_changed.emit()
        return True

    def set_all(self, on: bool) -> None:
        ids = {u.radio_id for u in self._rows}
        if on:
            self.checked |= ids
        else:
            self.checked -= ids
        if self._rows:
            self.dataChanged.emit(self.index(0, 0), self.index(len(self._rows) - 1, 0),
                                  [Qt.ItemDataRole.CheckStateRole])
        self.checked_changed.emit()


class RadioIdDialog(QDialog):
    def __init__(self, project: Project, cache_dir: Path, db: UserDb | None = None,
                 capacity: int = DEFAULT_CONTACT_CAPACITY, parent=None) -> None:
        super().__init__(parent)
        self.setWindowTitle("從 RadioID.net 匯入聯絡人")
        self.resize(980, 640)
        self._project = project
        self._cache_dir = cache_dir
        self._capacity = capacity
        self._db: UserDb | None = None
        self._thread: _LoadThread | None = None
        self._by_id: dict[int, RadioIdUser] = {}
        self._model = UserTableModel(existing_private_ids(project))
        self._model.checked_changed.connect(self._update_counts)
        self._debounce = QTimer(self, singleShot=True, interval=250)
        self._debounce.timeout.connect(self._apply_filter)
        self._build_ui()
        if db is not None:
            self._set_db(db)
        else:
            self._start_load(refresh=radioid.cache_info(cache_dir) is None)

    # ------------------------------------------------------------- layout
    def _build_ui(self) -> None:
        root = QVBoxLayout(self)

        top = QHBoxLayout()
        self._status = QLabel()
        self._refresh_btn = QPushButton("下載／更新資料")
        self._refresh_btn.clicked.connect(lambda: self._start_load(refresh=True))
        self._progress = QProgressBar()
        self._progress.setMaximumWidth(220)
        self._progress.hide()
        top.addWidget(self._status, 1)
        top.addWidget(self._progress)
        top.addWidget(self._refresh_btn)
        root.addLayout(top)

        filters = QHBoxLayout()
        self._region = QComboBox()
        self._region.setMinimumWidth(220)
        self._prefix = QLineEdit(placeholderText="ID 前綴，例如 466（台灣）")
        self._prefix.setValidator(QRegularExpressionValidator(QRegularExpression(r"\d{0,8}")))
        self._search = QLineEdit(placeholderText="搜尋 ID、呼號、姓名、城市（多個關鍵字以空白分隔）")
        for w in (self._prefix, self._search):
            w.textChanged.connect(self._debounce.start)
        self._region.currentIndexChanged.connect(self._debounce.start)
        filters.addWidget(QLabel("地區："))
        filters.addWidget(self._region)
        filters.addWidget(self._prefix)
        filters.addWidget(self._search, 1)
        root.addLayout(filters)

        self._view = QTableView()
        self._view.setModel(self._model)
        self._view.setSelectionBehavior(QTableView.SelectionBehavior.SelectRows)
        self._view.verticalHeader().setVisible(False)
        header = self._view.horizontalHeader()
        header.setSectionResizeMode(QHeaderView.ResizeMode.Interactive)
        header.setStretchLastSection(True)
        for col, width in enumerate((70, 80, 90, 180, 140, 140)):
            self._view.setColumnWidth(col, width)
        root.addWidget(self._view, 1)

        bottom = QHBoxLayout()
        sel_all = QPushButton("全選符合結果")
        sel_all.clicked.connect(lambda: self._model.set_all(True))
        sel_none = QPushButton("取消全選")
        sel_none.clicked.connect(lambda: self._model.set_all(False))
        self._fmt = QComboBox()
        for fmt in NameFormat:
            self._fmt.addItem(fmt.label, fmt.value)
        self._counts = QLabel()
        bottom.addWidget(sel_all)
        bottom.addWidget(sel_none)
        bottom.addSpacing(16)
        bottom.addWidget(QLabel("名稱格式："))
        bottom.addWidget(self._fmt)
        bottom.addWidget(self._counts, 1)
        root.addLayout(bottom)

        self._buttons = QDialogButtonBox(QDialogButtonBox.StandardButton.Ok
                                         | QDialogButtonBox.StandardButton.Cancel)
        self._buttons.accepted.connect(self._accept)
        self._buttons.rejected.connect(self.reject)
        root.addWidget(self._buttons)
        self._update_counts()

    # ------------------------------------------------------------- loading
    def _start_load(self, refresh: bool) -> None:
        if self._thread is not None and self._thread.isRunning():
            return
        self._refresh_btn.setEnabled(False)
        self._progress.setRange(0, 0)
        self._progress.show()
        self._thread = _LoadThread(self._cache_dir, refresh)
        self._thread.status.connect(self._status.setText)
        self._thread.progress.connect(self._on_progress)
        self._thread.loaded.connect(self._on_loaded)
        self._thread.failed.connect(self._on_failed)
        self._thread.finished.connect(self._on_thread_done)
        self._thread.start()

    def _on_progress(self, got: int, total: int) -> None:
        if total:
            self._progress.setRange(0, 1000)
            self._progress.setValue(int(got * 1000 / total))
        self._status.setText(f"下載中… {got / 1e6:.1f} MB" + (f" / {total / 1e6:.1f} MB" if total else ""))

    def _on_loaded(self, db: UserDb | None) -> None:
        if db is None:
            self._status.setText("尚無本機資料，請按「下載／更新資料」")
            return
        self._set_db(db)

    def _on_failed(self, message: str) -> None:
        self._status.setText("失敗：" + message)
        QMessageBox.warning(self, "RadioID", message)

    def _on_thread_done(self) -> None:
        self._progress.hide()
        self._refresh_btn.setEnabled(True)

    def _set_db(self, db: UserDb) -> None:
        self._db = db
        self._by_id = {u.radio_id: u for u in db.users}
        info = radioid.cache_info(self._cache_dir)
        when = (dt.datetime.fromtimestamp(info.downloaded_at).strftime("%Y-%m-%d %H:%M")
                if info else "—")
        self._status.setText(f"本機資料 {len(db):,} 筆（下載於 {when}）")
        self._region.blockSignals(True)
        self._region.clear()
        self._region.addItem(ALL_REGIONS, "")
        for country, count in db.countries():
            self._region.addItem(f"{country}（{count:,}）", country)
        taiwan = self._region.findData("Taiwan")
        self._region.setCurrentIndex(taiwan if taiwan >= 0 else 0)
        self._region.blockSignals(False)
        self._apply_filter()

    # ------------------------------------------------------------- filtering
    def query(self) -> Query:
        return Query(country=self._region.currentData() or "", id_prefix=self._prefix.text(),
                     text=self._search.text())

    def _apply_filter(self) -> None:
        if self._db is None:
            return
        q = self.query()
        self._model.set_rows([] if q.is_empty() else self._db.search(q))
        self._update_counts()

    def _update_counts(self) -> None:
        rows = self._model.rows()
        present = sum(1 for u in rows if u.radio_id in self._model._existing)
        chosen = len(self._model.checked)
        new = self._new_count()
        total_after = len(self._project.contacts) + new
        over = total_after > self._capacity
        self._counts.setText(
            f"符合 {len(rows):,} 筆（{present} 筆已存在）｜已勾選 {chosen:,} 筆"
            f"（新增 {new:,}、已存在 {chosen - new:,}）｜"
            f"匯入後 {total_after:,} / {self._capacity:,}" + ("　⚠ 超過容量" if over else "")
        )
        ok = self._buttons.button(QDialogButtonBox.StandardButton.Ok)
        ok.setText(f"匯入 {chosen:,} 筆")
        ok.setEnabled(chosen > 0)

    # ------------------------------------------------------------- result
    def selected_users(self) -> list[RadioIdUser]:
        return sorted((self._by_id[i] for i in self._model.checked if i in self._by_id),
                      key=lambda u: u.radio_id)

    def name_format(self) -> NameFormat:
        return NameFormat(self._fmt.currentData())

    def _new_count(self) -> int:
        """Checked users not yet in the list (existing ones are overwritten in place)."""
        return len(self._model.checked - self._model._existing)

    def _accept(self) -> None:
        if len(self._project.contacts) + self._new_count() > self._capacity:
            answer = QMessageBox.question(
                self, "超過容量",
                f"勾選數量超過容量上限 {self._capacity:,}，超出部分不會匯入。要繼續嗎？")
            if answer != QMessageBox.StandardButton.Yes:
                return
        self.accept()

    def done(self, result: int) -> None:
        if self._thread is not None and self._thread.isRunning():
            self._thread.wait()
        super().done(result)
