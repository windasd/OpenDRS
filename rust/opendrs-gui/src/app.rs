//! The main window (port of `main_window.py`): menus, toolbar, the pages, the problem list, the
//! status bar, and the file operations with their confirmations.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Key, KeyboardShortcut, Modifiers, ThemePreference, ViewportCommand};
use opendrs_core::cleanup;
use opendrs_core::f136::Project;
use opendrs_core::paths;
use opendrs_core::radio_session::Mode;
use opendrs_core::settings::{self, Settings};
use opendrs_core::textcodec::{NameCodec, suggest_codec};

use crate::dialogs::Windows;
use crate::dialogs::add_contact::AddContactDialog;
use crate::dialogs::update::Updates;
use crate::document::Document;
use crate::fonts::{self, FontFile};
use crate::icons::{self, Icon};
use crate::pages::{self, PageState, Tab};
use crate::radio::RadioPanel;
use crate::table::model::Table;

pub(crate) const OPEN: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
pub(crate) const SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
pub(crate) const SAVE_AS: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::S);
pub(crate) const UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
pub(crate) const REDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
pub(crate) const PRINT: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::P);
pub(crate) const FIND: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::F);
pub(crate) const DELETE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::NONE, Key::Delete);
pub(crate) const MOVE_UP: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::ArrowUp);
pub(crate) const MOVE_DOWN: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND, Key::ArrowDown);

/// What to do once unsaved changes have been saved or discarded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    /// Open this file, or ask for one.
    Open(Option<PathBuf>),
    Close,
    /// 讀取對講機 (the result replaces the project).
    ReadRadio,
    /// Hand over to the updater, then close (`Updates::pending` holds the update).
    InstallUpdate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    Save,
    Discard,
    Cancel,
}

/// What follows a confirmed edit: a row to select, a status line, an information box.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct After {
    pub select: Option<(Table, usize)>,
    pub status: Option<String>,
    pub info: Option<(String, String)>,
}

/// The one modal dialog shown at a time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dialog {
    Message {
        title: String,
        text: String,
    },
    /// 「目前的修改尚未儲存，要先儲存嗎？」 before `Pending`.
    ConfirmDiscard(Pending),
    /// A yes/no question; 是 records `project` under `label`, then does `after` — unless the
    /// project changed after the question was asked (`revision`).
    Confirm {
        title: String,
        text: String,
        project: Box<Project>,
        label: String,
        after: After,
        revision: u64,
    },
    /// 新增分區: the name of the new zone.
    AddZone {
        name: String,
    },
    /// 新增聯絡人: type, name and ID of the new contact.
    AddContact(AddContactDialog),
    /// 寫入對講機 / 校正對講機時鐘: 是 starts the transfer, provided the project is still the
    /// one the question described (`revision`).
    Radio {
        mode: Mode,
        text: String,
        revision: u64,
    },
}

/// 外觀: follow the system, or force light / dark (stored as "auto" / "light" / "dark").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeMode {
    Auto,
    Light,
    Dark,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 3] = [ThemeMode::Auto, ThemeMode::Light, ThemeMode::Dark];

    pub fn from_setting(value: &str) -> Self {
        match value {
            "light" => ThemeMode::Light,
            "dark" => ThemeMode::Dark,
            _ => ThemeMode::Auto,
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            ThemeMode::Auto => "auto",
            ThemeMode::Light => "light",
            ThemeMode::Dark => "dark",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Auto => "跟隨系統",
            ThemeMode::Light => "淺色",
            ThemeMode::Dark => "深色",
        }
    }

    fn preference(self) -> ThemePreference {
        match self {
            ThemeMode::Auto => ThemePreference::System,
            ThemeMode::Light => ThemePreference::Light,
            ThemeMode::Dark => ThemePreference::Dark,
        }
    }
}

pub struct OpenDrsApp {
    pub(crate) settings: Settings,
    /// Settings, backups, recordings and the RadioID cache live here.
    data_dir: PathBuf,
    pub doc: Document,
    pub codec: NameCodec,
    pub tab: Tab,
    pub pages: PageState,
    pub windows: Windows,
    /// 檢查更新: the window, what the automatic check found, the accepted update.
    pub updates: Updates,
    pub status: String,
    pub dialog: Option<Dialog>,
    /// The cable's port picker and the transfer running on it.
    pub radio: RadioPanel,
    /// Repainted by background transfers (none in window-less tests).
    pub(crate) ctx: Option<egui::Context>,
    fonts: Vec<FontFile>,
    /// The user agreed to close despite unsaved changes.
    close_confirmed: bool,
    title: String,
}

impl OpenDrsApp {
    pub fn new(cc: &eframe::CreationContext<'_>, open: Option<PathBuf>) -> Self {
        let migrated = settings::migrate_legacy_dir();
        let mut app = Self::with_context(&cc.egui_ctx, paths::data_dir(), open);
        if !migrated.is_empty() {
            app.status = migrated;
        }
        app
    }

    /// Install the fonts and theme in `ctx`, read the settings in `data_dir`, open `open`.
    pub fn with_context(ctx: &egui::Context, data_dir: PathBuf, open: Option<PathBuf>) -> Self {
        let fonts = fonts::install(ctx);
        icons::install(ctx);
        let mut app = Self::with_data_dir(data_dir);
        app.ctx = Some(ctx.clone());
        ctx.set_theme(app.theme().preference());
        app.fonts = fonts;
        if app.fonts.is_empty() {
            app.status =
                "找不到中文字型：中文會顯示成方塊，可用環境變數 OPENDRS_FONT 指定字型檔".into();
        }
        if let Some(path) = open {
            app.open_path(&path);
        }
        app
    }

    /// The app without a window (tests use their own data folder).
    pub fn with_data_dir(data_dir: PathBuf) -> Self {
        let (settings, problem) = settings::load_settings(&data_dir);
        let radio = RadioPanel::new(&settings.radio_port);
        OpenDrsApp {
            codec: settings.name_codec,
            settings,
            data_dir,
            doc: Document::default(),
            tab: Tab::default(),
            pages: PageState::default(),
            windows: Windows::default(),
            updates: Updates::default(),
            status: problem.unwrap_or_default(),
            dialog: None,
            radio,
            ctx: None,
            fonts: Vec::new(),
            close_confirmed: false,
            title: String::new(),
        }
    }

    pub fn project(&self) -> &Project {
        self.doc.project()
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    pub fn fonts(&self) -> &[FontFile] {
        &self.fonts
    }

    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Change a setting (e.g. a dialog's folder) and save the file.
    pub fn update_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        change(&mut self.settings);
        self.save_settings();
    }

    /// The cable's port chosen in the toolbar (`__sim__` for the simulated radio), if any.
    pub fn radio_port(&self) -> Option<String> {
        Some(self.radio.port()).filter(|p| !p.is_empty())
    }

    /// A read, write or clock sync with the radio is running (other transfers must wait).
    pub fn transfer_running(&self) -> bool {
        self.radio.is_running()
    }

    /// The main window is locked: a transfer runs here or in the firmware or recording dialog
    /// (Python's modal dialogs blocked the window the same way).
    pub fn busy(&self) -> bool {
        self.transfer_running() || self.hardware_dialog_running()
    }

    /// Ask `text` under `title`; 是 records `project` (undo label `label`) and does `after`.
    pub fn confirm(
        &mut self,
        title: &str,
        text: String,
        project: Project,
        label: String,
        after: After,
    ) {
        self.dialog = Some(Dialog::Confirm {
            title: title.into(),
            text,
            project: Box::new(project),
            label,
            after,
            revision: self.doc.revision(),
        });
    }

    /// Answer the open yes/no question (是 = true): 是 records its edit.
    pub fn answer(&mut self, yes: bool) {
        if let Some(Dialog::Confirm {
            project,
            label,
            after,
            revision,
            ..
        }) = self.dialog.take()
        {
            if yes && revision == self.doc.revision() {
                self.finish(*project, label, after);
            }
        }
    }

    fn finish(&mut self, project: Project, label: String, after: After) {
        self.commit(project, label);
        if let Some((table, row)) = after.select {
            self.select_row(table, row);
        }
        if let Some(status) = after.status {
            self.status = status;
        }
        if let Some((title, text)) = after.info {
            self.warn(&title, text);
        }
    }

    pub fn theme(&self) -> ThemeMode {
        ThemeMode::from_setting(&self.settings.theme)
    }

    pub(crate) fn save_settings(&mut self) {
        if let Err(e) = settings::save_settings(&self.settings, &self.data_dir) {
            self.status = format!("無法儲存設定：{e}");
        }
    }

    /// A message box (警告／資訊), closed with 確定.
    pub fn warn(&mut self, title: &str, text: impl Into<String>) {
        self.dialog = Some(Dialog::Message {
            title: title.into(),
            text: text.into(),
        });
    }

    /// Record an edit made by a page; `label` names it in the undo menu.
    pub fn commit(&mut self, project: Project, label: impl Into<String>) {
        self.doc.commit(project, label);
    }

    // ---------------------------------------------------------------- file operations
    pub fn open_path(&mut self, path: &Path) -> bool {
        let doc = match Document::open(path) {
            Ok(doc) => doc,
            Err(e) => {
                self.warn("開啟失敗", format!("{}\n\n{e}", path.display()));
                return false;
            }
        };
        if let Some(guessed) = suggest_codec(&doc.project().name_fields()) {
            self.codec = guessed;
            self.settings.name_codec = guessed;
        }
        self.doc = doc;
        self.pages = PageState::default();
        self.settings = self.settings.with_recent(&path.to_string_lossy());
        self.save_settings();
        self.status = format!("已開啟 {}", path.display());
        true
    }

    fn file_dialog(&self) -> rfd::FileDialog {
        let dialog = rfd::FileDialog::new()
            .add_filter("DRS 專案", &["F136", "f136"])
            .add_filter("舊版備份", &["ini"])
            .add_filter("所有檔案", &["*"]);
        if self.settings.last_dir.is_empty() {
            dialog
        } else {
            dialog.set_directory(&self.settings.last_dir)
        }
    }

    /// Open `path`, or ask for a file; first offers to save unsaved changes.
    pub fn request_open(&mut self, path: Option<PathBuf>) {
        if self.doc.is_dirty() {
            self.dialog = Some(Dialog::ConfirmDiscard(Pending::Open(path)));
        } else {
            self.run_pending(Pending::Open(path));
        }
    }

    pub(crate) fn run_pending(&mut self, pending: Pending) {
        match pending {
            Pending::Open(Some(path)) => {
                self.open_path(&path);
            }
            Pending::Open(None) => {
                if let Some(path) = self.file_dialog().set_title("開啟專案").pick_file() {
                    self.open_path(&path);
                }
            }
            Pending::Close => self.close_confirmed = true,
            Pending::ReadRadio => {
                self.start_radio(Mode::Read);
            }
            Pending::InstallUpdate => {
                if self.install_pending_update() {
                    self.close_confirmed = true;
                }
            }
        }
    }

    /// Load a backup as the current project, detached from its file (saving asks for a new
    /// name, and it is not added to the recent files).
    pub fn open_backup(&mut self, path: &Path) -> bool {
        let project = match opendrs_core::f136::load(path) {
            Ok(project) => project,
            Err(e) => {
                self.warn(
                    "開啟失敗",
                    format!(
                        "{}

{e}",
                        path.display()
                    ),
                );
                return false;
            }
        };
        if let Some(guessed) = suggest_codec(&project.name_fields()) {
            self.codec = guessed;
        }
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        self.doc = Document::detached(project, format!("備份 {name}（存檔時另存新檔）"));
        self.pages = PageState::default();
        true
    }

    pub fn save(&mut self) -> bool {
        match self.doc.path().map(Path::to_path_buf) {
            Some(path) => self.write(&path),
            None => self.save_as(),
        }
    }

    pub fn save_as(&mut self) -> bool {
        let mut dialog = self.file_dialog().set_title("另存新檔");
        if let Some(name) = self.doc.path().and_then(Path::file_name) {
            dialog = dialog.set_file_name(name.to_string_lossy());
        }
        match dialog.save_file() {
            Some(path) => self.write(&path),
            None => false,
        }
    }

    pub fn write(&mut self, path: &Path) -> bool {
        match self.doc.save_to(path) {
            Ok(()) => {
                self.status = format!("已儲存 {}", path.display());
                true
            }
            Err(e) => {
                self.warn("儲存失敗", e);
                false
            }
        }
    }

    pub fn undo(&mut self) {
        self.doc.undo();
    }

    pub fn redo(&mut self) {
        self.doc.redo();
    }

    pub(crate) fn set_theme(&mut self, ctx: &egui::Context, mode: ThemeMode) {
        ctx.set_theme(mode.preference());
        self.settings.theme = mode.setting().into();
        self.save_settings();
    }

    /// The problem list: undecodable names, names with line breaks, problems that block saving.
    pub fn problems(&self) -> Vec<String> {
        let project = self.project();
        let mut issues = project.problems();
        let dirty_names = cleanup::scan(project);
        if dirty_names.total() > 0 {
            issues.insert(
                0,
                format!(
                    "{} 名稱含換行字元（顯示為 ␍␊），可按工具列「清除名稱中的換行字元」修正",
                    dirty_names.summary()
                ),
            );
        }
        if !project.names_decodable(self.codec) {
            issues.insert(
                0,
                format!(
                    "部分名稱無法以 {} 解碼（以 ‹十六進位› 顯示），請確認編碼",
                    self.codec.label()
                ),
            );
        }
        issues
    }

    // ---------------------------------------------------------------- frame
    fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.dialog.is_some() || self.busy() {
            return;
        }
        if ctx.input_mut(|i| i.consume_shortcut(&SAVE_AS)) {
            self.save_as();
        } else if ctx.input_mut(|i| i.consume_shortcut(&SAVE)) {
            self.save();
        } else if ctx.input_mut(|i| i.consume_shortcut(&OPEN)) {
            self.request_open(None);
        } else if ctx.input_mut(|i| i.consume_shortcut(&UNDO)) {
            self.undo();
        } else if ctx.input_mut(|i| i.consume_shortcut(&REDO)) {
            self.redo();
        } else if ctx.input_mut(|i| i.consume_shortcut(&FIND)) {
            self.focus_search(ctx);
        } else if ctx.input_mut(|i| i.consume_shortcut(&PRINT)) {
            self.print_project();
        }
        if ctx.egui_wants_keyboard_input() || !self.doc.has_project() {
            return; // Delete and Ctrl+↑↓ belong to a text field that has focus
        }
        let key = |shortcut: &KeyboardShortcut| ctx.input_mut(|i| i.consume_shortcut(shortcut));
        match self.tab {
            Tab::Channels if key(&DELETE) => self.delete_channel(),
            Tab::Channels if key(&MOVE_UP) => self.shift_channel(-1),
            Tab::Channels if key(&MOVE_DOWN) => self.shift_channel(1),
            Tab::Contacts if key(&DELETE) => self.delete_contacts(),
            Tab::Contacts if key(&MOVE_UP) => self.shift_contact(-1),
            Tab::Contacts if key(&MOVE_DOWN) => self.shift_contact(1),
            _ => {}
        }
    }

    /// 搜尋: focus the search box of the channel, zone or contact table.
    pub(crate) fn focus_search(&mut self, ctx: &egui::Context) {
        let table = match self.tab {
            Tab::Zones => Table::Zones,
            Tab::Contacts => Table::Contacts,
            Tab::Channels => Table::Channels,
            _ => return,
        };
        ctx.memory_mut(|m| m.request_focus(egui::Id::new(("search", table))));
    }

    /// A file dropped on the window opens (not while a question is open or a transfer runs).
    fn handle_drops_and_title(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if let [path] = dropped.as_slice() {
            if self.dialog.is_none() && !self.busy() {
                self.request_open(Some(path.clone()));
            }
        }
        let title = self.doc.window_title();
        if title != self.title {
            ctx.send_viewport_cmd(ViewportCommand::Title(title.clone()));
            self.title = title;
        }
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.radio_status(ui);
                let before = self.codec;
                egui::ComboBox::from_id_salt("codec")
                    .selected_text(self.codec.label())
                    .show_ui(ui, |ui| {
                        for codec in NameCodec::ALL {
                            ui.selectable_value(&mut self.codec, codec, codec.label());
                        }
                    })
                    .response
                    .on_hover_text("名稱的字元編碼（舊檔多為 Big5）");
                ui.label("名稱編碼");
                if self.codec != before {
                    self.settings.name_codec = self.codec;
                    self.save_settings();
                }
            });
        });
    }

    fn problem_list(&self, ui: &mut egui::Ui) {
        let issues = self.problems();
        let heading = if issues.is_empty() {
            "問題清單".to_string()
        } else {
            format!("問題清單（{}）", issues.len())
        };
        ui.strong(heading);
        egui::ScrollArea::vertical()
            .auto_shrink(false)
            .show(ui, |ui| {
                let (danger, ok) = icons::status_colours(ui.visuals().dark_mode);
                if issues.is_empty() {
                    ui.horizontal(|ui| {
                        ui.add(Icon::Check.image(icons::LIST, ok));
                        ui.label("沒有問題");
                    });
                }
                for issue in issues {
                    ui.horizontal(|ui| {
                        ui.add(Icon::Warning.image(icons::LIST, danger));
                        ui.add(egui::Label::new(issue).wrap());
                    });
                }
            });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.clone() else {
            return;
        };
        match dialog {
            Dialog::Message { title, text } => {
                let response = egui::Modal::new(egui::Id::new("message")).show(ctx, |ui| {
                    ui.set_max_width(520.0);
                    ui.heading(&title);
                    ui.add_space(4.0);
                    ui.label(&text);
                    ui.add_space(8.0);
                    ui.button("確定").clicked()
                });
                if response.inner || response.should_close() {
                    self.dialog = None;
                }
            }
            Dialog::ConfirmDiscard(pending) => self.confirm_discard(ctx, pending),
            Dialog::Confirm { title, text, .. } => {
                let response = egui::Modal::new(egui::Id::new("confirm")).show(ctx, |ui| {
                    ui.set_max_width(560.0);
                    ui.heading(&title);
                    ui.add_space(4.0);
                    ui.label(&text);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui.button("是").clicked() {
                            Some(true)
                        } else if ui.button("否").clicked() {
                            Some(false)
                        } else {
                            None
                        }
                    })
                    .inner
                });
                let answer = response
                    .inner
                    .or_else(|| response.should_close().then_some(false));
                if let Some(yes) = answer {
                    self.answer(yes);
                }
            }
            Dialog::Radio {
                mode,
                text,
                revision,
            } => self.radio_question(ctx, mode, &text, revision),
            Dialog::AddZone { name } => self.add_zone_dialog(ctx, name),
            Dialog::AddContact(state) => self.add_contact_dialog(ctx, state),
        }
    }

    fn confirm_discard(&mut self, ctx: &egui::Context, pending: Pending) {
        let response = egui::Modal::new(egui::Id::new("discard")).show(ctx, |ui| {
            ui.heading("尚未儲存");
            ui.label("目前的修改尚未儲存，要先儲存嗎？");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("儲存").clicked() {
                    Some(Choice::Save)
                } else if ui.button("不儲存").clicked() {
                    Some(Choice::Discard)
                } else if ui.button("取消").clicked() {
                    Some(Choice::Cancel)
                } else {
                    None
                }
            })
            .inner
        });
        let choice = response
            .inner
            .or_else(|| response.should_close().then_some(Choice::Cancel));
        match choice {
            Some(Choice::Save) => {
                self.dialog = None;
                if self.save() {
                    self.run_pending(pending);
                } else if pending == Pending::InstallUpdate {
                    self.cancel_pending_update();
                }
            }
            Some(Choice::Discard) => {
                self.dialog = None;
                self.run_pending(pending);
            }
            Some(Choice::Cancel) => {
                self.dialog = None;
                if pending == Pending::InstallUpdate {
                    self.cancel_pending_update();
                }
            }
            None => {}
        }
    }
}

impl OpenDrsApp {
    /// Closing waits for a running transfer (the process would kill it mid-write) and offers
    /// to save unsaved changes.
    fn guard_close(&mut self, ctx: &egui::Context) {
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if self.busy() {
            if close_requested {
                ctx.send_viewport_cmd(ViewportCommand::CancelClose);
                if self.transfer_running() {
                    self.warn("傳輸中", "與對講機的傳輸進行中，請等待完成。");
                } // the firmware and recording dialogs say so themselves
            }
            self.close_confirmed = false;
            return;
        }
        if close_requested && !self.close_confirmed && self.doc.is_dirty() {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.dialog = Some(Dialog::ConfirmDiscard(Pending::Close));
        }
        if self.close_confirmed {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}

impl OpenDrsApp {
    /// What runs every frame, shown or not: the close guard, then the transfer's news. In that
    /// order, a close request is refused while a transfer has not been collected yet, even if
    /// its thread finished a moment ago.
    pub(crate) fn tick(&mut self, ctx: &egui::Context) {
        self.guard_close(ctx);
        self.poll_radio();
        self.poll_updates();
    }

    /// The whole window (the self-test draws it without eframe).
    pub(crate) fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        self.handle_drops_and_title(&ctx);
        let idle = !self.busy();
        egui::Panel::top("menu").show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| self.menu_bar(ui));
        });
        egui::Panel::top("toolbar").show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| self.toolbar(ui));
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::bottom("problems")
            .resizable(true)
            .default_size(130.0)
            .show(ui, |ui| self.problem_list(ui));
        egui::CentralPanel::default().show(ui, |ui| {
            ui.add_enabled_ui(idle, |ui| pages::show(self, ui));
        });
        self.show_windows(&ctx);
        self.dialogs(&ctx);
    }
}

impl eframe::App for OpenDrsApp {
    /// Also runs while the window is minimized or hidden, so the close guard lives here.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }
}
