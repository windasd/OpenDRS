//! 備份管理 (port of `src/opendrs/gui/backup_dialog.py` and `backup_manager` / `open_backup` /
//! `compare_backup` of `radio_ops.py`): browse the backups every radio read and write keeps; open
//! one as the current project, compare it with the current project, or delete it.

use std::path::{Path, PathBuf};

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use opendrs_core::backups::{
    BEFORE_WRITE, Backup, cleanup_candidates, delete_backup, list_backups, size_text,
};

use crate::app::OpenDrsApp;

pub const TITLE: &str = "備份管理";
pub const HEADERS: [&str; 8] = [
    "時間",
    "類型",
    "機型",
    "分區",
    "頻道",
    "聯絡人",
    "大小",
    "備註",
];
pub const LAST_WRITE_NOTE: &str = "最近一次寫入前的備份（清理時保留）";
pub const DEFAULT_KEEP: usize = 10;
pub const MAX_KEEP: usize = 999;
pub const NOTE: &str = "每次讀取對講機，以及每次寫入之前，都會自動保留一份備份（.F136 專案＋.bin 原始傳輸資料）。要把備份寫回對講機：開啟後按「寫入對講機」。開啟的備份存檔時會另存新檔，備份本身不會被修改。";
pub const EMPTY: &str = "目前沒有備份；讀取對講機或寫入對講機時會自動建立。";
pub const LAST_WRITE_WARNING: &str =
    "這是最近一次寫入對講機之前的備份；寫入後若有問題，就是用它寫回。";
pub const IRREVERSIBLE: &str = "刪除後無法復原。";
pub const DELETE_TITLE: &str = "刪除備份";
pub const CLEANUP_TITLE: &str = "清理備份";
/// Shown instead of the dialog while a radio transfer runs.
pub const BUSY: &str = "請等目前的讀寫完成。";
pub const DISCARD_TITLE: &str = "尚未儲存";
pub const DISCARD_TEXT: &str = "目前的修改尚未儲存，要先儲存嗎？";
const OPEN_TIP: &str = "載入為目前的專案（目前的修改會先詢問是否儲存）";
const COMPARE_TIP: &str = "逐位元組比較目前的專案內容與這份備份";
const DELETE_TIP: &str = "刪除這份備份的 .F136 與 .bin";
const CLEANUP_TIP: &str = "刪除較舊的備份；最近一次寫入前的備份一律保留";
const ROW_HEIGHT: f32 = 22.0;
/// The columns 分區, 頻道, 聯絡人 and 大小 are right-aligned.
const RIGHT_ALIGNED: std::ops::Range<usize> = 3..7;
/// 時間 to 大小 fit their contents (備註 takes the rest).
const WIDTHS: [f32; 7] = [150.0, 56.0, 96.0, 48.0, 48.0, 56.0, 76.0];

// ---------------------------------------------------------------- texts

/// Where the newest before_write backup is (cleanup always keeps it; the list is newest first).
pub fn newest_before_write(backups: &[Backup]) -> Option<usize> {
    backups.iter().position(|b| b.kind == BEFORE_WRITE)
}

/// 「…_read_X.F136、…_read_X.bin」: the files of a backup, the tooltip of its row.
pub fn files_text(backup: &Backup) -> String {
    backup
        .files()
        .iter()
        .filter_map(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("、")
}

/// One table row: the cells, then the note with its tooltip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowText {
    /// 時間 to 大小.
    pub cells: [String; 7],
    pub tip: String,
    pub note: String,
    pub note_tip: String,
    /// The project cannot be read: the note is red.
    pub unreadable: bool,
}

/// The row of `backup`; `last_write` marks the newest before_write backup.
pub fn row_text(backup: &Backup, last_write: bool) -> RowText {
    let files = files_text(backup);
    let [zones, channels, contacts] = match &backup.counts {
        Some(c) => [c.zones, c.channels, c.contacts].map(|n| n.to_string()),
        None => ["—", "—", "—"].map(String::from),
    };
    let cells = [
        backup.time_text(),
        backup.kind_label().to_string(),
        backup.model.clone(),
        zones,
        channels,
        contacts,
        size_text(backup.size),
    ];
    let (note, note_tip) = match &backup.counts {
        None => (
            format!("無法讀取：{}", backup.error.replace('\n', " ")),
            backup.error.clone(),
        ),
        Some(_) if last_write => (LAST_WRITE_NOTE.to_string(), files.clone()),
        Some(_) => (String::new(), files.clone()),
    };
    RowText {
        cells,
        tip: files,
        note,
        note_tip,
        unreadable: backup.counts.is_none(),
    }
}

/// The line under the table.
pub fn summary_text(backups: &[Backup]) -> String {
    if backups.is_empty() {
        return EMPTY.into();
    }
    let total: u64 = backups.iter().map(|b| b.size).sum();
    format!("共 {} 份備份，合計 {}", backups.len(), size_text(total))
}

/// A deletion question: the lines, a blank line, 「刪除後無法復原。」.
pub fn confirm_text(lines: &[String]) -> String {
    let mut all = lines.to_vec();
    all.extend([String::new(), IRREVERSIBLE.to_string()]);
    all.join("\n")
}

/// 「要刪除 … 的「讀取」備份嗎？」 with the file names.
pub fn delete_text(backup: &Backup, last_write: bool) -> String {
    let mut lines = vec![
        format!(
            "要刪除 {} 的「{}」備份嗎？",
            backup.time_text(),
            backup.kind_label()
        ),
        String::new(),
    ];
    lines.extend(
        backup
            .files()
            .iter()
            .filter_map(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned()),
    );
    if last_write {
        lines.extend([String::new(), LAST_WRITE_WARNING.to_string()]);
    }
    confirm_text(&lines)
}

/// The time span of the backups to delete (newest first): 「oldest ～ newest」, or one time.
pub fn cleanup_span(doomed: &[Backup]) -> String {
    match doomed {
        [] => String::new(),
        [only] => only.time_text(),
        [newest, .., oldest] => format!("{} ～ {}", oldest.time_text(), newest.time_text()),
    }
}

/// 「將刪除 n 份較舊的備份（…，共 …），保留最近 k 份。」 and, when the newest before_write backup
/// is kept beyond those k, 「另外保留最近一次寫入前的備份（…）。」.
pub fn cleanup_text(backups: &[Backup], doomed: &[Backup], keep: usize) -> String {
    let freed = size_text(doomed.iter().map(|b| b.size).sum());
    let mut lines = vec![format!(
        "將刪除 {} 份較舊的備份（{}，共 {freed}），保留最近 {keep} 份。",
        doomed.len(),
        cleanup_span(doomed)
    )];
    if let Some(row) = newest_before_write(backups).filter(|&row| row >= keep) {
        lines.push(format!(
            "另外保留最近一次寫入前的備份（{}）。",
            backups[row].time_text()
        ));
    }
    confirm_text(&lines)
}

pub fn missing_folder_text(folder: &Path) -> String {
    format!(
        "備份資料夾還不存在：\n{}\n\n讀取對講機後會自動建立。",
        folder.display()
    )
}

// ---------------------------------------------------------------- questions

/// A modal question with its buttons left to right; Escape (or a click outside) answers
/// `escape`. The button at `default` gets the keyboard focus when the question first shows, so
/// Enter picks it (Qt's default button); `focused` remembers that it was given.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ask<T: Copy>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    text: &str,
    buttons: &[(&str, T)],
    escape: T,
    default: Option<usize>,
    focused: &mut bool,
) -> Option<T> {
    let response = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_max_width(560.0);
        ui.heading(title);
        ui.add_space(4.0);
        ui.label(text);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let mut picked = None;
            for (i, (label, value)) in buttons.iter().enumerate() {
                let button = ui.button(*label);
                if default == Some(i) && !*focused {
                    button.request_focus();
                    *focused = true;
                }
                if button.clicked() {
                    picked = Some(*value);
                }
            }
            picked
        })
        .inner
    });
    response
        .inner
        .or_else(|| response.should_close().then_some(escape))
}

/// An answer to one of the dialog's questions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Yes,
    No,
    /// 「尚未儲存」: 儲存 / 不儲存 / 取消.
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Question {
    /// Unsaved changes before opening this backup.
    Discard(PathBuf),
    Delete {
        backup: Box<Backup>,
        text: String,
    },
    Cleanup {
        doomed: Vec<Backup>,
        text: String,
    },
}

impl Question {
    fn title(&self) -> &'static str {
        match self {
            Question::Discard(_) => DISCARD_TITLE,
            Question::Delete { .. } => DELETE_TITLE,
            Question::Cleanup { .. } => CLEANUP_TITLE,
        }
    }

    fn text(&self) -> &str {
        match self {
            Question::Discard(_) => DISCARD_TEXT,
            Question::Delete { text, .. } | Question::Cleanup { text, .. } => text,
        }
    }
}

// ---------------------------------------------------------------- the dialog

/// What the user did in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackupAction {
    Select(usize),
    /// 開啟 (or a double click).
    Open,
    /// 與目前專案比較.
    Compare,
    /// 刪除….
    Delete,
    /// 清理….
    Cleanup,
    /// 開啟資料夾.
    OpenFolder,
    /// The answer to the open question.
    Reply(Reply),
    /// 關閉 or the window's close button.
    Close,
}

/// The dialog's state while it is open.
pub struct BackupDialog {
    folder: PathBuf,
    backups: Vec<Backup>,
    selected: Option<usize>,
    /// 只保留最近 N 份 (1–999).
    pub keep: usize,
    question: Option<Question>,
    /// The default button of `question` has been focused.
    focused: bool,
}

impl BackupDialog {
    /// The dialog for `folder`, empty until [`Self::reload`].
    pub fn new(folder: PathBuf) -> Self {
        BackupDialog {
            folder,
            backups: Vec::new(),
            selected: None,
            keep: DEFAULT_KEEP,
            question: None,
            focused: false,
        }
    }

    /// Read every backup's summary (a few ms each) and refill the table.
    pub fn reload(&mut self) -> std::io::Result<()> {
        match list_backups(&self.folder) {
            Ok(found) => {
                self.fill(found);
                Ok(())
            }
            Err(e) => {
                self.fill(Vec::new());
                Err(e)
            }
        }
    }

    /// New rows drop the selection (row N would now be another backup).
    fn fill(&mut self, backups: Vec<Backup>) {
        self.backups = backups;
        self.selected = None;
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    /// Newest first.
    pub fn backups(&self) -> &[Backup] {
        &self.backups
    }

    pub fn selected(&self) -> Option<&Backup> {
        self.selected.and_then(|row| self.backups.get(row))
    }

    pub fn select(&mut self, row: usize) {
        self.selected = (row < self.backups.len()).then_some(row);
    }

    /// The open question: (title, text).
    pub fn question(&self) -> Option<(&str, &str)> {
        self.question.as_ref().map(|q| (q.title(), q.text()))
    }

    fn ask(&mut self, question: Question) {
        self.question = Some(question);
        self.focused = false;
    }

    fn doomed(&self) -> Vec<Backup> {
        cleanup_candidates(&self.backups, self.keep.clamp(1, MAX_KEEP)).unwrap_or_default()
    }

    // ------------------------------------------------------------ drawing
    fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<BackupAction>) {
        egui::Panel::top("backups-top")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("備份資料夾：");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("開啟資料夾").clicked() {
                            actions.push(BackupAction::OpenFolder);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let path = self.folder.display().to_string();
                            ui.add(egui::Label::new(path).truncate());
                        });
                    });
                });
                ui.add_space(4.0);
            });
        egui::Panel::bottom("backups-bottom")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.add_space(4.0);
                ui.label(summary_text(&self.backups));
                self.action_row(ui, actions);
                ui.add_space(4.0);
                ui.add(egui::Label::new(RichText::new(NOTE).weak()).wrap());
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("關閉").clicked() {
                        actions.push(BackupAction::Close);
                    }
                });
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.table(ui, actions));
    }

    fn action_row(&mut self, ui: &mut egui::Ui, actions: &mut Vec<BackupAction>) {
        let chosen = self.selected().is_some();
        let can_clean = !self.doomed().is_empty();
        ui.horizontal(|ui| {
            for (text, tip, action) in [
                ("開啟", OPEN_TIP, BackupAction::Open),
                ("與目前專案比較", COMPARE_TIP, BackupAction::Compare),
                ("刪除…", DELETE_TIP, BackupAction::Delete),
            ] {
                if ui
                    .add_enabled(chosen, egui::Button::new(text))
                    .on_hover_text(tip)
                    .on_disabled_hover_text(tip)
                    .clicked()
                {
                    actions.push(action);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(can_clean, egui::Button::new("清理…"))
                    .on_hover_text(CLEANUP_TIP)
                    .on_disabled_hover_text(CLEANUP_TIP)
                    .clicked()
                {
                    actions.push(BackupAction::Cleanup);
                }
                ui.add(
                    egui::DragValue::new(&mut self.keep)
                        .range(1..=MAX_KEEP)
                        .suffix(" 份"),
                );
                ui.label("只保留最近");
            });
        });
    }

    fn table(&self, ui: &mut egui::Ui, actions: &mut Vec<BackupAction>) {
        let last_write = newest_before_write(&self.backups);
        let danger = ui.visuals().error_fg_color;
        let mut builder = TableBuilder::new(ui)
            .id_salt("backups-table")
            .striped(true)
            .resizable(true)
            .auto_shrink(false)
            .sense(egui::Sense::click())
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for width in WIDTHS {
            builder = builder.column(Column::initial(width).at_least(30.0).clip(true));
        }
        builder
            .column(Column::remainder().at_least(80.0).clip(true))
            .header(ROW_HEIGHT, |mut header| {
                for title in HEADERS {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.backups.len(), |mut row| {
                    let index = row.index();
                    let text = row_text(&self.backups[index], last_write == Some(index));
                    row.set_selected(self.selected == Some(index));
                    for (col, value) in text.cells.iter().enumerate() {
                        row.col(|ui| {
                            cell(ui, value, RIGHT_ALIGNED.contains(&col), &text.tip, None);
                        });
                    }
                    row.col(|ui| {
                        let color = text.unreadable.then_some(danger);
                        cell(ui, &text.note, false, &text.note_tip, color);
                    });
                    let response = row.response();
                    if response.double_clicked() {
                        actions.extend([BackupAction::Select(index), BackupAction::Open]);
                    } else if response.clicked() {
                        actions.push(BackupAction::Select(index));
                    }
                });
            });
    }
}

fn cell(ui: &mut egui::Ui, text: &str, right: bool, tip: &str, color: Option<Color32>) {
    let rich = match color {
        Some(color) => RichText::new(text).color(color),
        None => RichText::new(text),
    };
    let label = egui::Label::new(rich).selectable(false).truncate();
    let response = if right {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add(label)
        })
        .inner
    } else {
        ui.add(label)
    };
    if !tip.is_empty() {
        response.on_hover_text(tip);
    }
}

impl OpenDrsApp {
    /// 備份管理…: browse the backups every read and write keeps.
    pub fn backup_manager(&mut self) {
        if self.transfer_running() {
            // A read in progress would replace whatever is opened.
            self.warn(TITLE, BUSY);
            return;
        }
        let mut dialog = BackupDialog::new(self.data_dir().join("backups"));
        if let Err(e) = dialog.reload() {
            self.warn(
                TITLE,
                format!("無法讀取備份資料夾：\n{}\n\n{e}", dialog.folder().display()),
            );
        }
        self.windows.backups = Some(dialog);
    }

    /// Do `action` in the open backup manager (what its buttons do).
    pub fn backup_action(&mut self, action: BackupAction) {
        let Some(mut dialog) = self.windows.backups.take() else {
            return;
        };
        if self.apply_backup_action(&mut dialog, action) {
            self.windows.backups = Some(dialog);
        }
    }

    /// False when the dialog closes.
    fn apply_backup_action(&mut self, dialog: &mut BackupDialog, action: BackupAction) -> bool {
        match action {
            BackupAction::Select(row) => dialog.select(row),
            BackupAction::Open => return self.open_selected_backup(dialog),
            BackupAction::Compare => {
                if let Some(path) = dialog.selected().map(|b| b.path.clone()) {
                    if self.require_project("比較專案") {
                        self.compare_with(&path);
                    }
                }
            }
            BackupAction::Delete => {
                if let Some(row) = dialog.selected {
                    let backup = dialog.backups[row].clone();
                    let last_write = newest_before_write(&dialog.backups) == Some(row);
                    let text = delete_text(&backup, last_write);
                    dialog.ask(Question::Delete {
                        backup: Box::new(backup),
                        text,
                    });
                }
            }
            BackupAction::Cleanup => {
                let doomed = dialog.doomed();
                if !doomed.is_empty() {
                    let text = cleanup_text(&dialog.backups, &doomed, dialog.keep);
                    dialog.ask(Question::Cleanup { doomed, text });
                }
            }
            BackupAction::OpenFolder => self.open_backup_folder(dialog.folder()),
            BackupAction::Reply(reply) => return self.answer_backup_question(dialog, reply),
            BackupAction::Close => return false,
        }
        true
    }

    /// 開啟: unsaved changes are offered for saving first.
    fn open_selected_backup(&mut self, dialog: &mut BackupDialog) -> bool {
        let Some(path) = dialog.selected().map(|b| b.path.clone()) else {
            return true;
        };
        if self.doc.is_dirty() {
            dialog.ask(Question::Discard(path));
            return true;
        }
        !self.open_backup(&path)
    }

    fn open_backup_folder(&mut self, folder: &Path) {
        if !folder.is_dir() {
            self.warn(TITLE, missing_folder_text(folder));
        } else if let Err(e) = super::open_folder(folder) {
            self.status = format!("無法開啟資料夾：{e}");
        }
    }

    fn answer_backup_question(&mut self, dialog: &mut BackupDialog, reply: Reply) -> bool {
        let Some(question) = dialog.question.take() else {
            return true;
        };
        match (question, reply) {
            (Question::Discard(path), Reply::Save) => !(self.save() && self.open_backup(&path)),
            (Question::Discard(path), Reply::Discard) => !self.open_backup(&path),
            (Question::Delete { backup, .. }, Reply::Yes) => {
                self.delete_one_backup(dialog, &backup);
                true
            }
            (Question::Cleanup { doomed, .. }, Reply::Yes) => {
                self.delete_backups(dialog, &doomed);
                true
            }
            _ => true,
        }
    }

    fn delete_one_backup(&mut self, dialog: &mut BackupDialog, backup: &Backup) {
        match delete_backup(backup) {
            Ok(_) => {
                let rest = dialog
                    .backups
                    .iter()
                    .filter(|b| *b != backup)
                    .cloned()
                    .collect();
                dialog.fill(rest);
            }
            Err(e) => {
                let name = backup
                    .path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                self.warn("刪除失敗", format!("{name}\n\n{e}"));
                self.reload_backups(dialog); // the .F136 may be gone even if the .bin is not
            }
        }
    }

    fn delete_backups(&mut self, dialog: &mut BackupDialog, doomed: &[Backup]) {
        let mut deleted = Vec::new();
        let mut failed = Vec::new();
        for backup in doomed {
            match delete_backup(backup) {
                Ok(_) => deleted.push(backup),
                Err(e) => {
                    let name = backup.path.file_name().unwrap_or_default();
                    failed.push(format!("{}：{e}", name.to_string_lossy()));
                }
            }
        }
        if failed.is_empty() {
            let rest = dialog
                .backups
                .iter()
                .filter(|b| !deleted.contains(b))
                .cloned()
                .collect();
            dialog.fill(rest);
        } else {
            // A half-deleted pair: re-read the folder rather than guess.
            self.reload_backups(dialog);
            self.warn("部分備份未刪除", failed.join("\n"));
        }
    }

    fn reload_backups(&mut self, dialog: &mut BackupDialog) {
        if let Err(e) = dialog.reload() {
            self.warn(
                TITLE,
                format!("無法讀取備份資料夾：\n{}\n\n{e}", dialog.folder().display()),
            );
        }
    }
}

/// Draw the dialog if it is open.
pub fn show(app: &mut OpenDrsApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.windows.backups.take() else {
        return;
    };
    let mut open = true;
    let mut actions = Vec::new();
    egui::Window::new(TITLE)
        .id(egui::Id::new("backups-window"))
        .open(&mut open)
        .default_size([820.0, 520.0])
        .vscroll(false)
        .show(ctx, |ui| dialog.ui(ui, &mut actions));
    if let Some(question) = dialog.question.clone() {
        let mut focused = dialog.focused;
        let reply = match &question {
            Question::Discard(_) => ask(
                ctx,
                "backups-discard",
                question.title(),
                question.text(),
                &[
                    ("儲存", Reply::Save),
                    ("不儲存", Reply::Discard),
                    ("取消", Reply::Cancel),
                ],
                Reply::Cancel,
                None,
                &mut focused,
            ),
            // Deleting defaults to 否.
            _ => ask(
                ctx,
                "backups-delete",
                question.title(),
                question.text(),
                &[("是", Reply::Yes), ("否", Reply::No)],
                Reply::No,
                Some(1),
                &mut focused,
            ),
        };
        dialog.focused = focused;
        if let Some(reply) = reply {
            actions.push(BackupAction::Reply(reply));
        }
    }
    if !open {
        actions.push(BackupAction::Close);
    }
    for action in actions {
        if !app.apply_backup_action(&mut dialog, action) {
            return;
        }
    }
    app.windows.backups = Some(dialog);
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendrs_core::backups::parse_name;
    use opendrs_core::codeplug::Counts;

    fn backup(stamp: &str, kind: &str, size: u64, readable: bool) -> Backup {
        let name = format!("{stamp}_{kind}_DR-8500UV.F136");
        let (when, kind, model) = parse_name(&name).unwrap();
        Backup {
            path: PathBuf::from(name),
            when,
            kind,
            model,
            size,
            counts: readable.then(|| Counts::new(3, 24, 120)),
            error: if readable {
                String::new()
            } else {
                "第 1 行：格式錯誤\n缺少 [Main]".into()
            },
        }
    }

    /// Newest first, as `list_backups` returns them.
    fn sample() -> Vec<Backup> {
        vec![
            backup("20261004_090000", "read", 1024, false),
            backup("20261003_090000", "read", 2048, true),
            backup("20261002_090000", BEFORE_WRITE, 2048, true),
            backup("20261001_090000", "read", 512, true),
        ]
    }

    #[test]
    fn rows_mark_unreadable_and_last_write_backups() {
        let all = sample();
        assert_eq!(newest_before_write(&all), Some(2));
        let bad = row_text(&all[0], false);
        assert!(bad.unreadable);
        assert_eq!(
            bad.cells,
            [
                "2026-10-04 09:00:00",
                "讀取",
                "DR-8500UV",
                "—",
                "—",
                "—",
                "1.0 KB"
            ]
            .map(String::from)
        );
        assert_eq!(bad.note, "無法讀取：第 1 行：格式錯誤 缺少 [Main]");
        assert_eq!(bad.note_tip, "第 1 行：格式錯誤\n缺少 [Main]");
        let last = row_text(&all[2], true);
        assert_eq!(last.cells[1], "寫入前");
        assert_eq!(&last.cells[3..6], ["3", "24", "120"]);
        assert_eq!(last.note, LAST_WRITE_NOTE);
        assert_eq!(row_text(&all[1], false).note, "");
    }

    #[test]
    fn summary_counts_and_sizes() {
        assert_eq!(summary_text(&sample()), "共 4 份備份，合計 5.5 KB");
        assert_eq!(summary_text(&[]), EMPTY);
    }

    #[test]
    fn delete_question_warns_about_the_last_write_backup() {
        let all = sample();
        let text = delete_text(&all[2], true);
        // The files do not exist here, so only the header and the warning are listed.
        assert_eq!(
            text,
            "要刪除 2026-10-02 09:00:00 的「寫入前」備份嗎？\n\n\n這是最近一次寫入對講機之前的備份；寫入後若有問題，就是用它寫回。\n\n刪除後無法復原。"
        );
        assert!(delete_text(&all[1], false).ends_with("備份嗎？\n\n\n刪除後無法復原。"));
    }

    #[test]
    fn cleanup_question_names_the_span_and_the_kept_write_backup() {
        let all = sample();
        let doomed = cleanup_candidates(&all, 1).unwrap();
        assert_eq!(doomed.len(), 2);
        assert_eq!(
            cleanup_span(&doomed),
            "2026-10-01 09:00:00 ～ 2026-10-03 09:00:00"
        );
        assert_eq!(
            cleanup_text(&all, &doomed, 1),
            "將刪除 2 份較舊的備份（2026-10-01 09:00:00 ～ 2026-10-03 09:00:00，共 2.5 KB），保留最近 1 份。\n另外保留最近一次寫入前的備份（2026-10-02 09:00:00）。\n\n刪除後無法復原。"
        );
        let doomed = cleanup_candidates(&all, 3).unwrap();
        assert_eq!(cleanup_span(&doomed), "2026-10-01 09:00:00");
        assert_eq!(
            cleanup_text(&all, &doomed, 3),
            "將刪除 1 份較舊的備份（2026-10-01 09:00:00，共 512 bytes），保留最近 3 份。\n\n刪除後無法復原。"
        );
    }

    #[test]
    fn missing_folder_message() {
        assert_eq!(
            missing_folder_text(Path::new("D:/x/backups")),
            "備份資料夾還不存在：\nD:/x/backups\n\n讀取對講機後會自動建立。"
        );
    }
}
