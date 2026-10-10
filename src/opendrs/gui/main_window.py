"""Main window: open / edit / save ``.F136`` projects."""

from __future__ import annotations

import logging
from dataclasses import replace
from pathlib import Path

from PySide6.QtCore import QSize, Qt
from PySide6.QtGui import QAction, QActionGroup, QKeySequence
from PySide6.QtWidgets import (
    QApplication,
    QComboBox,
    QFileDialog,
    QGroupBox,
    QLabel,
    QLineEdit,
    QListWidget,
    QListWidgetItem,
    QMainWindow,
    QMenu,
    QMessageBox,
    QSplitter,
    QTabWidget,
    QToolBar,
    QVBoxLayout,
    QWidget,
)

from .. import __version__, f136, hardware, radioid
from ..cleanup import scan
from ..diff import diff_projects
from ..f136 import F136FormatError, Project
from ..history import History
from ..records import NAME_SIZE
from ..settings import Settings, load_settings, save_settings
from ..textcodec import NameCodec, suggest_codec
from . import theme
from .about_dialog import AboutDialog
from .alarm_page import AlarmEncryptionPage
from .channel_ops import ChannelOpsMixin
from .constants import FILE_FILTER, TABS
from .contact_ops import ContactOpsMixin
from .delegates import (
    ChoiceDelegate,
    ProjectPickerDelegate,
    alarm_list_choices,
    contact_choices,
    encryption_key_choices,
    rx_group_choices,
    scan_list_choices,
    zone_choices,
)
from .device_editing import DeviceEditingMixin
from .diff_dialog import DiffDialog
from .filter_bar import TablePage
from .group_page import RxGroupPage
from .list_ops import ListOpsMixin
from .models import (
    CHANNEL_COLUMNS,
    CHANNEL_EMERGENCY_COLUMN,
    CHANNEL_ENCRYPTION_COLUMN,
    CHANNEL_RX_GROUP_COLUMN,
    CHANNEL_SCAN_COLUMN,
    CHANNEL_TX_CONTACT_COLUMN,
    CHANNEL_ZONE_COLUMN,
    make_models,
)
from .print_ops import PrintOpsMixin
from .quickmsg_page import QuickMessagePage
from .radio_ops import RadioOpsMixin
from .radio_panel import RadioPanel
from .record_filter import RecordFilterProxy
from .scan_page import ScanListPage
from .settings_page import SettingsPage

log = logging.getLogger(__name__)


class MainWindow(DeviceEditingMixin, ChannelOpsMixin, ContactOpsMixin, ListOpsMixin, RadioOpsMixin,
                 PrintOpsMixin, QMainWindow):
    def __init__(self, settings: Settings | None = None) -> None:
        super().__init__()
        self._settings = settings or load_settings()
        self._history = History(Project())
        self._saved_project = Project()
        self._path: Path | None = None
        self._source = ""
        self._models = make_models()
        self._pages: dict[str, TablePage] = {}
        self._radioid_lookups: dict[int, radioid.RadioIdUser | None] = {}
        self._proxies: dict[str, RecordFilterProxy] = {}
        self._filters: dict[str, QLineEdit] = {}
        for model in self._models.values():
            model.edit_failed.connect(self._on_edit_failed)
            model.project_changed.connect(self._on_model_edit)
            model.project_restructured.connect(self._on_model_restructured)
        self.setAcceptDrops(True)
        self._icon_actions: list[tuple[QAction, str]] = []
        self._radio = RadioPanel(self._backup_dir, self._settings.radio_port, parent=self)
        self._radio.busy_changed.connect(self._on_radio_busy)
        self._radio.finished.connect(self._on_radio_finished)
        self._radio.port_box.activated.connect(self._remember_port)
        self._build_ui()
        self.statusBar().addPermanentWidget(QLabel("名稱編碼"))
        self.statusBar().addPermanentWidget(self._codec_box)
        self.statusBar().addPermanentWidget(self._radio.status)
        self.statusBar().addPermanentWidget(self._radio.progress)
        self._refresh()

    # ---------------------------------------------------------------- project state
    @property
    def _project(self) -> Project:
        return self._history.current

    @property
    def codec(self) -> NameCodec:
        # Qt stores the str-based enum as a plain str; convert back.
        return NameCodec(self._codec_box.currentData())

    def is_dirty(self) -> bool:
        return self._project != self._saved_project

    def _commit(self, project: Project, label: str) -> None:
        if self._history.push(project, label):
            self._refresh()

    def undo(self) -> None:
        if self._history.undo() is not None:
            self._refresh()

    def redo(self) -> None:
        if self._history.redo() is not None:
            self._refresh()

    # ---------------------------------------------------------------- layout
    def _action(self, text: str, slot, shortcut=None, tip: str = "",  # noqa: ANN001
                icon_name: str = "", short: str = "") -> QAction:
        act = QAction(text, self)
        if icon_name:
            self._icon_actions.append((act, icon_name))
            act.setIcon(theme.icon(icon_name))
        if short:  # toolbar label; the menu keeps the full text
            act.setIconText(short)
        if shortcut is not None:
            act.setShortcut(shortcut)
        if tip:
            act.setStatusTip(tip)
            act.setToolTip(tip)
        act.triggered.connect(slot)
        return act

    def _build_ui(self) -> None:
        self.resize(1280, 780)
        self.setWindowIcon(theme.app_icon())
        K = QKeySequence.StandardKey
        a_open = self._action("開啟…", self.open_dialog, K.Open, icon_name="open", short="開啟")
        a_save = self._action("儲存", self.save, K.Save, icon_name="save")
        a_save_as = self._action("另存新檔…", self.save_as, K.SaveAs)
        self._a_undo = self._action("復原", self.undo, K.Undo, icon_name="undo", short="復原")
        self._a_redo = self._action("重做", self.redo, K.Redo, icon_name="redo", short="重做")
        a_radioid = self._action("從 RadioID 匯入聯絡人…", self.import_from_radioid,
                                 icon_name="users", short="RadioID 匯入")
        a_clean = self._action("清除名稱中的換行字元", self.clean_control_chars,
                               tip="移除舊版寫頻軟體寫進名稱的換行等控制字元",
                               icon_name="clean", short="清除換行")
        a_csv_out = self._action("匯出聯絡人 CSV…", self.export_contacts_csv)
        a_csv_in = self._action("從 CSV 匯入聯絡人…", self.import_contacts_csv)
        a_diff = self._action("比較專案…", self.compare_dialog,
                              tip="與另一個專案逐位元組比較，用來找出設定存放的位置", icon_name="compare")
        a_find = self._action("搜尋", self._focus_filter, K.Find, icon_name="search")
        a_add_ch = self._action("新增頻道（複製選取的頻道）", self.add_channel,
                                tip="複製目前選取的頻道，加在同一分區的最後面；掃描清單與其他頻道的編號不受影響",
                                icon_name="plus")
        self._a_del_ch = self._action("刪除頻道", self.delete_channel, K.Delete,
                                      tip="刪除選取的頻道；同分區之後的頻道編號與掃描清單會自動調整",
                                      icon_name="trash")
        self._a_del_ch.setShortcutContext(Qt.ShortcutContext.WidgetWithChildrenShortcut)
        self._a_del_contacts = self._action("刪除選取的聯絡人", self.delete_contacts, K.Delete,
                                            tip="刪除選取的聯絡人；編號往前遞補，頻道與接收群組自動調整",
                                            icon_name="trash")
        self._a_del_contacts.setShortcutContext(Qt.ShortcutContext.WidgetWithChildrenShortcut)
        up, down = QKeySequence("Ctrl+Up"), QKeySequence("Ctrl+Down")
        self._a_ch_up = self._action("上移頻道", lambda: self.shift_channel(-1), up, icon_name="up",
                                     tip="與同分區的上一台交換位置；掃描清單等引用跟著調整")
        self._a_ch_down = self._action("下移頻道", lambda: self.shift_channel(1), down, icon_name="down",
                                       tip="與同分區的下一台交換位置；掃描清單等引用跟著調整")
        self._a_ct_up = self._action("上移聯絡人", lambda: self.shift_contact(-1), up, icon_name="up",
                                     tip="與上一筆交換位置；頻道發話對象與接收群組跟著調整")
        self._a_ct_down = self._action("下移聯絡人", lambda: self.shift_contact(1), down, icon_name="down",
                                       tip="與下一筆交換位置；頻道發話對象與接收群組跟著調整")
        for act in (self._a_ch_up, self._a_ch_down, self._a_ct_up, self._a_ct_down):
            act.setShortcutContext(Qt.ShortcutContext.WidgetWithChildrenShortcut)
        a_batch = self._action("批次新增頻道…", self.batch_add_channels, icon_name="plus",
                               tip="以選取的頻道為範本，一次新增多台（頻率依間隔遞增、名稱依序編號）")
        a_read = self._action("讀取對講機…", self.read_from_radio,
                              tip="經寫頻線讀取對講機目前的設定（自動保留備份）", icon_name="read", short="讀取")
        a_write = self._action("寫入對講機…", self.write_to_radio,
                               tip="寫入前會先讀取並備份對講機目前的設定，機型或頻段不符會中止",
                               icon_name="write", short="寫入")

        bar = self.menuBar()
        m_file = bar.addMenu("檔案(&F)")
        m_file.addActions([a_open])
        self._recent_menu = QMenu("最近開啟(&R)", self)
        m_file.addMenu(self._recent_menu)
        m_file.addSeparator()
        m_file.addActions([a_save, a_save_as])
        m_file.addSeparator()
        m_file.addAction(self._action("列印…", self.print_project, K.Print,
                                      tip="列印裝置資訊、各分區頻道、聯絡人、接收群組與掃描清單（A4 橫式）"))
        m_file.addAction(self._action("預覽列印…", self.print_preview))
        m_file.addAction(self._action("匯出 PDF…", self.export_pdf, tip="把同一份列印內容存成 PDF"))
        m_file.addSeparator()
        m_file.addAction(self._action("結束", self.close, K.Quit))
        m_edit = bar.addMenu("編輯(&E)")
        m_edit.addActions([self._a_undo, self._a_redo])
        m_edit.addSeparator()
        m_edit.addActions([a_find, a_clean])
        m_contacts = bar.addMenu("聯絡人(&C)")
        m_contacts.addActions([a_radioid, a_csv_in, a_csv_out])
        m_contacts.addSeparator()
        m_contacts.addActions([self._a_ct_up, self._a_ct_down, self._a_del_contacts])
        a_add_zone = self._action("新增分區…", self.add_zone,
                                  tip="在最後新增一個分區，第一台頻道複製目前選取的頻道")
        a_del_zone = self._action("刪除分區", self.delete_zone,
                                  tip="刪除選取的分區與其中的頻道；之後的分區與掃描清單自動調整")
        m_channels = bar.addMenu("頻道／分區(&H)")
        m_channels.addActions([a_add_ch, a_batch, self._a_del_ch])
        m_channels.addSeparator()
        m_channels.addActions([self._a_ch_up, self._a_ch_down])
        m_channels.addSeparator()
        m_channels.addActions([a_add_zone, a_del_zone])
        m_channels.addSeparator()
        m_channels.addAction(self._action("匯出頻道 CSV…", self.export_channels_csv,
                                          tip="把所有頻道匯出成 Excel 可開啟的 CSV（依分區排列）"))
        m_channels.addAction(self._action("從 CSV 匯入頻道…", self.import_channels_csv,
                                          tip="依「分區＋頻道」更新既有頻道，頻道欄空白則新增到該分區最後；不會刪除頻道"))
        m_radio = bar.addMenu("對講機(&R)")
        m_radio.addActions([a_read, a_write])
        self._a_clock = self._action("校正對講機時鐘…", self.sync_radio_clock,
                                     tip="讀取對講機後原樣寫回、只更新寫入時間，讓對講機依電腦時間設定時鐘"
                                         "（錄音的日期時間來自這個時鐘）")
        m_radio.addAction(self._a_clock)
        m_radio.addAction(self._action("備份管理…", self.backup_manager, icon_name="save",
                                       tip="瀏覽每次讀取／寫入前自動保留的備份：開啟、與目前專案比較、刪除"))
        m_radio.addSeparator()
        m_radio.addAction(self._action("韌體升級（IAP）…", self.firmware_update, icon_name="chip",
                                       tip="檢查韌體檔是否適用這台對講機，再透過 IAP 升級（內建或原廠工具）"))
        m_radio.addAction(self._action("讀取錄音…", self.recordings, icon_name="read",
                                       tip="列出並下載對講機的通話錄音，轉成 WAV（取代原廠 Wav.exe）"))
        m_tools = bar.addMenu("工具(&T)")
        m_tools.addAction(a_diff)
        m_tools.addSeparator()
        m_tools.addAction(self._action("關於本軟體…", self.about, tip="版本、作者與執行環境"))
        m_view = bar.addMenu("檢視(&V)")
        m_theme = m_view.addMenu("外觀")
        self._theme_actions = {}
        group = QActionGroup(self)
        for mode in theme.ThemeMode:
            act = QAction(mode.label, self, checkable=True)
            act.setChecked(mode.value == self._settings.theme)
            act.triggered.connect(lambda _c=False, m=mode: self.set_theme(m))
            group.addAction(act)
            m_theme.addAction(act)
            self._theme_actions[mode] = act

        toolbar = QToolBar("主要")
        toolbar.setObjectName("main_toolbar")
        toolbar.setMovable(False)
        toolbar.setIconSize(QSize(18, 18))
        toolbar.setToolButtonStyle(Qt.ToolButtonStyle.ToolButtonTextBesideIcon)
        self.addToolBar(toolbar)
        toolbar.addActions([a_open, a_save, self._a_undo, self._a_redo])
        toolbar.addSeparator()
        toolbar.addWidget(QLabel(" 連接埠 "))
        toolbar.addWidget(self._radio.port_box)
        self._radio.refresh_button.setText("")
        self._radio.refresh_button.setIcon(theme.icon("refresh"))
        self._icon_buttons = [(self._radio.refresh_button, "refresh")]
        toolbar.addWidget(self._radio.refresh_button)
        toolbar.addActions([a_read, a_write])
        self._a_read, self._a_write = a_read, a_write
        toolbar.addSeparator()
        toolbar.addActions([a_radioid, a_clean])
        self._codec_box = QComboBox()
        self._codec_box.setToolTip("名稱的字元編碼（舊檔多為 Big5）")
        for codec in NameCodec:
            self._codec_box.addItem(codec.label, codec)
        self._codec_box.setCurrentIndex(list(NameCodec).index(self._settings.name_codec))
        self._codec_box.currentIndexChanged.connect(self._on_codec_changed)

        self._tabs = QTabWidget()
        self._tabs.addTab(self._device_page(), "裝置資訊")
        for key, title in TABS:
            self._tabs.addTab(self._table_page(key), title)
        self._scan_page = ScanListPage(self.edit_scan_list)
        self._tabs.addTab(self._scan_page, "掃描清單")
        self._group_page = RxGroupPage(self.edit_rx_group)
        self._tabs.addTab(self._group_page, "接收群組")
        self._settings_page = SettingsPage(self.apply_setting)
        self._tabs.addTab(self._settings_page, "對講機設定")
        self._quickmsg_page = QuickMessagePage(self.apply_quick_message)
        self._tabs.addTab(self._quickmsg_page, "常用短信")
        self._alarm_page = AlarmEncryptionPage(self.apply_alarm_edit)
        self._tabs.addTab(self._alarm_page, "報警與加密")

        self._tabs.setDocumentMode(False)
        self._tabs.tabBar().setExpanding(False)
        self._problems = QListWidget()
        self._problems_box = QGroupBox("問題清單")
        box_layout = QVBoxLayout(self._problems_box)
        box_layout.setContentsMargins(8, 6, 8, 8)
        box_layout.addWidget(self._problems)
        split = QSplitter(Qt.Orientation.Vertical)
        split.addWidget(self._tabs)
        split.addWidget(self._problems_box)
        split.setSizes([620, 130])
        split.setChildrenCollapsible(False)
        host = QWidget()
        host_layout = QVBoxLayout(host)
        host_layout.setContentsMargins(8, 8, 8, 4)
        host_layout.addWidget(split)
        self.setCentralWidget(host)
        self._rebuild_recent_menu()

    def _table_page(self, key: str) -> QWidget:
        page = TablePage(key, self._models[key])
        if key == "channels":
            self._contact_picker = ProjectPickerDelegate(contact_choices, page.view)
            page.view.setItemDelegateForColumn(CHANNEL_TX_CONTACT_COLUMN, self._contact_picker)
            self._group_picker = ProjectPickerDelegate(rx_group_choices, page.view)
            page.view.setItemDelegateForColumn(CHANNEL_RX_GROUP_COLUMN, self._group_picker)
            self._zone_picker = ProjectPickerDelegate(zone_choices, page.view, none_label=None)
            page.view.setItemDelegateForColumn(CHANNEL_ZONE_COLUMN, self._zone_picker)
            self._key_picker = ProjectPickerDelegate(encryption_key_choices, page.view)
            page.view.setItemDelegateForColumn(CHANNEL_ENCRYPTION_COLUMN, self._key_picker)
            self._scan_picker = ProjectPickerDelegate(scan_list_choices, page.view)
            page.view.setItemDelegateForColumn(CHANNEL_SCAN_COLUMN, self._scan_picker)
            self._alarm_picker = ProjectPickerDelegate(alarm_list_choices, page.view)
            page.view.setItemDelegateForColumn(CHANNEL_EMERGENCY_COLUMN, self._alarm_picker)
            self._choice_pickers = {}
            for col, column in enumerate(CHANNEL_COLUMNS):
                if column.choices:
                    picker = ChoiceDelegate(list(column.choices), page.view)
                    page.view.setItemDelegateForColumn(col, picker)
                    self._choice_pickers[col] = picker
            page.view.addActions([self._a_del_ch, self._a_ch_up, self._a_ch_down])  # keys work in the table
        if key == "contacts":
            page.view.addActions([self._a_del_contacts, self._a_ct_up, self._a_ct_down])
        self._pages[key] = page
        self._proxies[key] = page.proxy
        self._filters[key] = page.search
        return page

    def _focus_filter(self) -> None:
        index = self._tabs.currentIndex()
        if 0 < index <= len(TABS):
            box = self._filters[TABS[index - 1][0]]
            box.setFocus()
            box.selectAll()

    # ---------------------------------------------------------------- refresh
    def _refresh(self) -> None:
        for model in self._models.values():
            model.set_project(self._project, self.codec)
        dev = self._project.device
        bands = dev.bands
        also = hardware.known_as(dev.model)
        self._dev_labels["model"].setText(f"{dev.model}（{also}）" if also else (dev.model or "—"))
        for key in ("factory_number", "serial_number", "model_number"):
            self._dev_labels[key].setText(getattr(dev, key) or "—")
        self._dev_labels["firmware"].setText(dev.firmware or "—")
        self._dev_labels["firmware_date"].setText(dev.firmware_date or "—")
        self._dev_labels["band"].setText(dev.band_label or "—")
        self._dev_labels["bands"].setText(
            "、".join(f"{lo / 1e6:g}–{hi / 1e6:g} MHz" for lo, hi in bands) if bands else "未知（不檢查）"
        )
        self._dev_labels["date"].setText(dev.write_date or "—")
        self._refresh_identity_editors(dev)
        in_use = self._scan_page.set_project(self._project, self.codec)
        self._tabs.setTabText(self._tabs.indexOf(self._scan_page), f"掃描清單（{in_use}）")
        self._settings_page.set_image(self._project.image)
        self._quickmsg_page.set_image(self._project.image, self.codec)
        self._alarm_page.set_project(self._project, self.codec)
        groups_in_use = self._group_page.set_project(self._project, self.codec)
        self._tabs.setTabText(self._tabs.indexOf(self._group_page), f"接收群組（{groups_in_use}）")
        self._dev_labels["img"].setText(f"{len(self._project.image):,} bytes（原樣保留）")
        for i, (key, title) in enumerate(TABS, start=1):
            self._tabs.setTabText(i, f"{title}（{self._models[key].rowCount()}）")
        self._refresh_status()

    def set_theme(self, mode: theme.ThemeMode) -> None:
        """Switch light / dark / system and re-tint every icon."""
        app = QApplication.instance()
        if app is not None:
            QApplication.setOverrideCursor(Qt.CursorShape.WaitCursor)  # re-styling takes ~1 s
            try:
                theme.apply_theme(app, mode)
            finally:
                QApplication.restoreOverrideCursor()
        for act, name in self._icon_actions:
            act.setIcon(theme.icon(name))
        for button, name in self._icon_buttons:
            button.setIcon(theme.icon(name))
        self._theme_actions[mode].setChecked(True)
        self._settings = replace(self._settings, theme=mode.value)
        save_settings(self._settings)
        self._refresh_problems()

    def _refresh_status(self) -> None:
        self._refresh_problems()
        self._a_undo.setEnabled(self._history.can_undo)
        self._a_redo.setEnabled(self._history.can_redo)
        self._a_undo.setText(f"復原 {self._history.undo_label}".strip())
        self._a_redo.setText(f"重做 {self._history.redo_label}".strip())
        name = self._path.name if self._path else (self._source or "未開啟檔案")
        self.setWindowTitle(f"{'*' if self.is_dirty() else ''}{name} — OpenDRS {__version__}")

    def _refresh_problems(self) -> None:
        self._problems.clear()
        issues = self._project.problems()
        dirty_names = scan(self._project)
        if dirty_names.total:
            issues.insert(0, f"{dirty_names.summary()} 名稱含換行字元（顯示為 ␍␊），"
                             "可按工具列「清除名稱中的換行字元」修正")
        if not self._project.names_decodable(self.codec):
            issues.insert(0, f"部分名稱無法以 {self.codec.label} 解碼（以 ‹十六進位› 顯示），請確認編碼")
        t = theme.tokens()
        for text in issues:
            item = QListWidgetItem(theme.icon("warning", t.danger, 16), text)
            self._problems.addItem(item)
        if not issues:
            self._problems.addItem(QListWidgetItem(theme.icon("check", t.ok, 16), "沒有問題"))
        self._problems_box.setTitle(f"問題清單（{len(issues)}）" if issues else "問題清單")

    def _on_model_edit(self, project: Project) -> None:
        # The edited table already shows the change; avoid resetting it (keeps
        # scroll position and selection) and only sync the other tables.
        if not self._history.push(project, "編輯"):
            return
        sender = self.sender()
        for model in self._models.values():
            if model is not sender:
                model.set_project(project, self.codec)
        self._refresh_status()

    def _on_model_restructured(self, project: Project, what: str) -> None:
        """An edit that reorders records (e.g. a channel moved to another zone)."""
        self._commit(project, f"變更{what}")

    def _on_codec_changed(self) -> None:
        self._settings = replace(self._settings, name_codec=self.codec)
        save_settings(self._settings)
        self._refresh()

    def _on_edit_failed(self, message: str) -> None:
        QMessageBox.warning(self, "無法套用", message)

    # ---------------------------------------------------------------- file ops
    def _rebuild_recent_menu(self) -> None:
        self._recent_menu.clear()
        paths = [p for p in self._settings.recent_files if Path(p).is_file()]
        for p in paths:
            act = self._recent_menu.addAction(p)
            act.triggered.connect(lambda _checked=False, path=p: self._open_recent(path))
        self._recent_menu.setEnabled(bool(paths))

    def _open_recent(self, path: str) -> None:
        if self._confirm_discard():
            self.open_path(Path(path))

    def open_dialog(self) -> None:
        if not self._confirm_discard():
            return
        path, _ = QFileDialog.getOpenFileName(self, "開啟專案", self._settings.last_dir, FILE_FILTER)
        if path:
            self.open_path(Path(path))

    def open_path(self, path: Path) -> bool:
        try:
            project = f136.load(path)
        except (OSError, F136FormatError) as exc:
            QMessageBox.critical(self, "開啟失敗", f"{path}\n\n{exc}")
            return False
        self._history.reset(project)
        self._saved_project = project
        self._path = path
        self._source = ""
        for page in self._pages.values():
            page.reset()
        guessed = suggest_codec([r.raw[r.NAME_OFFSET:r.NAME_OFFSET + NAME_SIZE]
                                 for r in (*project.channels, *project.zones, *project.contacts)])
        if guessed is not None and guessed is not self.codec:
            self._codec_box.setCurrentIndex(list(NameCodec).index(guessed))
        self._settings = self._settings.with_recent(str(path))
        save_settings(self._settings)
        self._rebuild_recent_menu()
        self._refresh()
        return True

    def save(self) -> bool:
        if self._path is None:
            return self.save_as()
        return self._write(self._path)

    def save_as(self) -> bool:
        path, _ = QFileDialog.getSaveFileName(self, "另存新檔", str(self._path or ""), FILE_FILTER)
        return self._write(Path(path)) if path else False

    def _write(self, path: Path) -> bool:
        try:
            f136.save(self._project, path)
        except (OSError, F136FormatError) as exc:
            QMessageBox.critical(self, "儲存失敗", str(exc))
            return False
        self._saved_project, self._path = self._project, path
        self._refresh_status()
        return True

    def _confirm_discard(self) -> bool:
        if not self.is_dirty():
            return True
        answer = QMessageBox.question(
            self, "尚未儲存", "目前的修改尚未儲存，要先儲存嗎？",
            QMessageBox.StandardButton.Save | QMessageBox.StandardButton.Discard
            | QMessageBox.StandardButton.Cancel,
        )
        if answer == QMessageBox.StandardButton.Save:
            return self.save()
        return answer == QMessageBox.StandardButton.Discard

    def closeEvent(self, event) -> None:  # noqa: N802
        if self._radio.is_running():
            QMessageBox.warning(self, "傳輸中", "與對講機的傳輸進行中，請等待完成。")
            event.ignore()
            return
        if self._confirm_discard():
            event.accept()
        else:
            event.ignore()

    def dragEnterEvent(self, event) -> None:  # noqa: N802
        urls = event.mimeData().urls()
        if len(urls) == 1 and urls[0].isLocalFile():
            event.acceptProposedAction()

    def dropEvent(self, event) -> None:  # noqa: N802
        urls = event.mimeData().urls()
        if urls and urls[0].isLocalFile() and self._confirm_discard():
            self.open_path(Path(urls[0].toLocalFile()))

    # ---------------------------------------------------------------- contacts
    def has_project(self) -> bool:
        p = self._project
        return bool(p.image or p.contacts or p.zones or p.channels)

    def _require_project(self, title: str) -> bool:
        if not self.has_project():
            QMessageBox.information(self, title, "請先開啟專案檔，或從對講機讀取。")
            return False
        return True

    # ---------------------------------------------------------------- radio
    # ---------------------------------------------------------------- tools
    def about(self) -> AboutDialog:
        dialog = AboutDialog(self)
        dialog.open()
        return dialog

    def compare_dialog(self) -> None:
        if not self._require_project("比較專案"):
            return
        chosen, _ = QFileDialog.getOpenFileName(self, "選擇要比較的專案", self._settings.last_dir,
                                                FILE_FILTER)
        if chosen:
            self.compare_with(Path(chosen)).exec()

    def compare_with(self, other_path: Path) -> DiffDialog:
        try:
            other = f136.load(other_path)
        except (OSError, F136FormatError) as exc:
            QMessageBox.critical(self, "開啟失敗", f"{other_path}\n\n{exc}")
            raise
        title = f"舊：{self._path.name if self._path else '目前專案'}（目前內容）　→　新：{other_path.name}"
        return DiffDialog(title, diff_projects(self._project, other, self.codec), parent=self)
