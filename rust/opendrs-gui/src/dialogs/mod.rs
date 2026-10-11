//! Dialog windows that keep their own state (batch add, project comparison, about); the simple
//! questions and messages are `app::Dialog`.

pub mod add_contact;
pub mod backups;
pub mod batch;
pub mod firmware;
pub mod radioid;
pub mod recording;
pub mod update;

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use eframe::egui;
use opendrs_core::diff::diff_projects;
use opendrs_core::f136;
use opendrs_core::layout::add_channels;
use opendrs_firmware::hardware;

use crate::app::{Dialog, OpenDrsApp};
use crate::fonts;
use crate::table::model::Table;
use batch::{BatchDialog, BatchOutcome};

pub const APP_NAME: &str = "OpenDRS";
pub const AUTHOR: &str = "windasd";
/// The project's page (the workspace's `repository`): source, issues and releases.
pub const WEBSITE: &str = env!("CARGO_PKG_REPOSITORY");
pub const SUMMARY: &str = "Kydera DR-8500UV／HORA DH-98VU 對講機寫頻軟體：專案編輯、讀寫頻、韌體升級（IAP）、通話錄音讀取。";
pub const NOTES: &str = "聯絡人匯入使用 RadioID.net 的公開資料；錄音解碼呼叫原廠附帶的 DVSI ambep2.exe（授權軟體，本程式不附帶也不重寫）。";

/// 支援機型 as the about box shows it.
pub fn radio_text() -> String {
    format!(
        "{}；韌體 308E.TARSCB.003（BK4819 機型回報 308E.BBKTARSCB.003）",
        hardware::supported_text()
    )
}

/// 執行環境: the Rust build and the operating system.
pub fn environment() -> String {
    format!(
        "Rust 版（egui 0.36），{} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )
}

/// Plain-text version information (for the clipboard and bug reports).
pub fn about_text(data_dir: &Path) -> String {
    let exe = std::env::current_exe().map_or_else(|_| "?".into(), |p| p.display().to_string());
    [
        format!("{APP_NAME} {}", env!("CARGO_PKG_VERSION")),
        format!("作者：{AUTHOR}"),
        format!("網站：{WEBSITE}"),
        format!("支援機型：{}", radio_text()),
        format!("執行環境：{}", environment()),
        format!("程式位置：{exe}"),
        format!("資料夾：{}", data_dir.display()),
    ]
    .join("\n")
}

/// Open a file with the system's default program.
pub fn open_with_system(path: &Path) -> std::io::Result<()> {
    let mut command = system_open_command(path);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command.spawn().map(|_| ())
}

#[cfg(windows)]
fn system_open_command(path: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new("cmd");
    // quoted by hand: a Windows path cannot contain '"', and cmd must not see & or ^ unquoted
    command
        .raw_arg(format!("/C start \"\" \"{}\"", path.display()))
        .creation_flags(CREATE_NO_WINDOW);
    command
}

#[cfg(target_os = "macos")]
fn system_open_command(path: &Path) -> Command {
    let mut command = Command::new("open");
    command.arg(path);
    command
}

#[cfg(not(any(windows, target_os = "macos")))]
fn system_open_command(path: &Path) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(path);
    command
}

/// Show a folder in the system's file manager.
pub fn open_folder(path: &Path) -> std::io::Result<()> {
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .map(|_| ())
}

/// 比較專案: the report of `diff_projects`, with 儲存報告….
pub struct DiffDialog {
    pub title: String,
    pub lines: Vec<String>,
}

impl DiffDialog {
    /// The report file: title, blank line, the lines; UTF-8 with BOM and CRLF.
    pub fn report(&self) -> Vec<u8> {
        let text = format!("{}\r\n\r\n{}", self.title, self.lines.join("\r\n"));
        [b"\xef\xbb\xbf".as_slice(), text.as_bytes()].concat()
    }
}

/// 關於本軟體.
#[derive(Default)]
pub struct AboutDialog {
    copied: bool,
    /// 檢查更新… was pressed.
    check_updates: bool,
}

/// The open dialog windows.
#[derive(Default)]
pub struct Windows {
    pub batch: Option<BatchDialog>,
    pub diff: Option<DiffDialog>,
    pub about: Option<AboutDialog>,
    pub radioid: Option<radioid::RadioIdDialog>,
    pub backups: Option<backups::BackupDialog>,
    pub firmware: Option<firmware::FirmwareDialog>,
    pub recording: Option<recording::RecordingDialog>,
}

impl OpenDrsApp {
    /// 批次新增頻道: the selected channel (else the last one) is the template.
    pub fn batch_add_channels(&mut self) {
        if !self.require_project("批次新增頻道") {
            return;
        }
        let row = match self.pages.tables.channels.single_selection() {
            Some(row) => row,
            None if self.project().channels.is_empty() => {
                self.warn("批次新增頻道", "專案中沒有可作為範本的頻道。");
                return;
            }
            None => self.project().channels.len() - 1,
        };
        self.windows.batch = Some(BatchDialog::new(self.project(), row, self.codec));
    }

    fn apply_batch(&mut self, row: usize, plan: &opendrs_core::layout::BatchPlan) {
        match add_channels(self.project(), row, plan, self.codec) {
            Ok((project, rows)) => {
                self.commit(project, format!("批次新增頻道 {} 台", plan.count));
                self.select_row(Table::Channels, rows.end - 1);
            }
            Err(e) => self.warn("批次新增頻道", e.0),
        }
    }

    /// 比較專案…: pick a project and show how it differs from the current one.
    pub fn compare_dialog(&mut self) {
        if !self.require_project("比較專案") {
            return;
        }
        let mut dialog = rfd::FileDialog::new()
            .set_title("選擇要比較的專案")
            .add_filter("DRS 專案", &["F136", "f136"])
            .add_filter("所有檔案", &["*"]);
        if let Some(dir) = self.last_dir() {
            dialog = dialog.set_directory(dir);
        }
        if let Some(path) = dialog.pick_file() {
            self.compare_with(&path);
        }
    }

    pub fn compare_with(&mut self, other_path: &Path) {
        let other = match f136::load(other_path) {
            Ok(project) => project,
            Err(e) => {
                self.warn("開啟失敗", format!("{}\n\n{e}", other_path.display()));
                return;
            }
        };
        let name = self.doc.path().and_then(Path::file_name).map_or_else(
            || "目前專案".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        let other_name = other_path.file_name().map_or_else(
            || other_path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        self.windows.diff = Some(DiffDialog {
            title: format!("舊：{name}（目前內容）　→　新：{other_name}"),
            lines: diff_projects(self.project(), &other, self.codec),
        });
    }

    /// 新增分區: ask for the new zone's name (its first channel copies the selected one).
    pub(crate) fn add_zone_dialog(&mut self, ctx: &egui::Context, mut name: String) {
        let response = egui::Modal::new(egui::Id::new("add-zone")).show(ctx, |ui| {
            ui.heading("新增分區");
            ui.label("分區名稱（新分區的第一台頻道會複製目前選取的頻道）：");
            let edit = ui.text_edit_singleline(&mut name);
            edit.request_focus();
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("確定").clicked() || enter {
                    Some(true)
                } else if ui.button("取消").clicked() {
                    Some(false)
                } else {
                    None
                }
            })
            .inner
        });
        match response
            .inner
            .or_else(|| response.should_close().then_some(false))
        {
            Some(true) => {
                self.dialog = None;
                self.add_zone_named(&name);
            }
            Some(false) => self.dialog = None,
            None => self.dialog = Some(Dialog::AddZone { name }),
        }
    }

    pub fn about(&mut self) {
        self.windows.about = Some(AboutDialog::default());
    }

    /// Draw the open dialog windows.
    pub fn show_windows(&mut self, ctx: &egui::Context) {
        radioid::show(self, ctx);
        backups::show(self, ctx);
        firmware::show(self, ctx);
        recording::show(self, ctx);
        self.update_window(ctx);
        if let Some(mut batch) = self.windows.batch.take() {
            let bands = self.project().device().bands();
            match batch.show(ctx, bands.as_deref()) {
                BatchOutcome::Open => self.windows.batch = Some(batch),
                BatchOutcome::Cancelled => {}
                BatchOutcome::Accepted(plan) => self.apply_batch(batch.row, &plan),
            }
        }
        if let Some(diff) = self.windows.diff.take() {
            let mut open = true;
            let mut save = false;
            egui::Window::new("比較專案")
                .open(&mut open)
                .default_size([900.0, 600.0])
                .show(ctx, |ui| {
                    ui.label(&diff.title);
                    ui.horizontal(|ui| save = ui.button("儲存報告…").clicked());
                    egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(diff.lines.join("\n")).monospace(),
                            )
                            .selectable(true),
                        );
                    });
                });
            if save {
                self.save_diff_report(&diff);
            }
            if open {
                self.windows.diff = Some(diff);
            }
        }
        if let Some(mut about) = self.windows.about.take() {
            let mut open = true;
            egui::Window::new("關於本軟體")
                .open(&mut open)
                .collapsible(false)
                .default_width(560.0)
                .show(ctx, |ui| self.about_ui(ui, &mut about));
            if about.check_updates {
                self.check_for_updates(); // the about box closes, as in the Python version
            } else if open {
                self.windows.about = Some(about);
            }
        }
    }

    fn save_diff_report(&mut self, diff: &DiffDialog) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("儲存比較報告")
            .set_file_name("diff.txt")
            .add_filter("文字檔", &["txt"])
            .save_file()
        else {
            return;
        };
        if let Err(e) = std::fs::write(&path, diff.report()) {
            self.warn("儲存失敗", e.to_string());
        }
    }

    fn about_ui(&mut self, ui: &mut egui::Ui, about: &mut AboutDialog) {
        ui.heading(egui::RichText::new(APP_NAME).strong());
        ui.label(format!(
            "版本 {}　·　作者 {AUTHOR}",
            env!("CARGO_PKG_VERSION")
        ));
        ui.label(SUMMARY);
        ui.add_space(8.0);
        let data_dir: PathBuf = self.data_dir().to_path_buf();
        egui::Grid::new("about")
            .num_columns(2)
            .spacing([12.0, 6.0])
            .show(ui, |ui| {
                for (label, value) in [
                    ("版本", env!("CARGO_PKG_VERSION").to_string()),
                    ("作者", AUTHOR.to_string()),
                ] {
                    ui.label(label);
                    ui.add(egui::Label::new(value).wrap().selectable(true));
                    ui.end_row();
                }
                ui.label("網站");
                ui.hyperlink_to(WEBSITE, WEBSITE)
                    .on_hover_text("原始碼、問題回報與新版本下載");
                ui.end_row();
                for (label, value) in [
                    ("執行環境", environment()),
                    ("中文字型", fonts::describe(self.fonts())),
                    ("資料夾", data_dir.display().to_string()),
                ] {
                    ui.label(label);
                    ui.add(egui::Label::new(value).wrap().selectable(true));
                    ui.end_row();
                }
            });
        ui.weak(NOTES);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let copy_text = if about.copied {
                "已複製"
            } else {
                "複製版本資訊"
            };
            if ui.button(copy_text).clicked() {
                ui.ctx().copy_text(about_text(&data_dir));
                about.copied = true;
            }
            if ui.button("開啟資料夾").clicked() {
                if let Err(e) = open_folder(&data_dir) {
                    self.status = format!("無法開啟資料夾：{e}");
                }
            }
            if ui.button(update::MENU_TEXT).clicked() {
                about.check_updates = true;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_report_is_utf8_bom_crlf() {
        let dialog = DiffDialog {
            title: "舊：a（目前內容）　→　新：b".into(),
            lines: vec!["■ 頻道".into(), "  第 1 列".into()],
        };
        let report = dialog.report();
        assert!(report.starts_with(b"\xef\xbb\xbf"));
        assert_eq!(
            String::from_utf8(report[3..].to_vec()).unwrap(),
            "舊：a（目前內容）　→　新：b\r\n\r\n■ 頻道\r\n  第 1 列"
        );
        let about = about_text(Path::new("D"));
        assert!(about.contains("\n資料夾：D"));
        assert!(about.contains("\n網站：https://github.com/windasd/OpenDRS\n"));
    }
}
