//! 工具 › 檢查更新… (port of `update_dialog.py` and `update_ops.py`): the latest GitHub release,
//! the automatic check at start-up (at most daily), and — on Windows, running from an unpacked
//! release — downloading, verifying and installing it.
//!
//! A release without this platform's Rust archive (one that only carries the Python build, for
//! example) is shown with its notes but never offered for installing, and the check at start-up
//! stays quiet about it.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};

use chrono::{DateTime, Utc};
use eframe::egui;
use opendrs_update::{self as update, PreparedUpdate, Release, UpdateError};

use crate::app::{Dialog, OpenDrsApp, Pending};

pub const TITLE: &str = "檢查更新";
pub const MENU_TEXT: &str = "檢查更新…";
pub const MENU_TIP: &str = "查詢 GitHub 上的最新版本；Windows 版可直接下載並安裝";
pub const AUTO_CHECK: &str = "啟動時自動檢查更新（每天最多一次）";
/// The updater's log, under the data folder.
pub const UPDATE_LOG: &str = "logs/update.log";
/// This build's version.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Staging folders older than this are left by an update that was never applied.
pub const STALE_AFTER: std::time::Duration = std::time::Duration::from_secs(600);
/// Installing closes the window, which waits for these (the updater would time out meanwhile).
pub const BUSY: &str = "與對講機的傳輸、韌體升級或錄音讀取進行中，請等完成後再安裝更新。";

pub type FetchFn = dyn Fn() -> Result<Release, UpdateError> + Send + Sync;
pub type PrepareFn = dyn Fn(
        &Release,
        &Path,
        &mut dyn FnMut(u64, u64),
        &dyn Fn() -> bool,
    ) -> Result<PreparedUpdate, UpdateError>
    + Send
    + Sync;
pub type LaunchFn = dyn Fn(&PreparedUpdate, &Path) -> Result<(), UpdateError> + Send + Sync;

/// How the feature reaches GitHub and the installer (tests put fakes here).
#[derive(Clone)]
pub struct Services {
    pub fetch: Arc<FetchFn>,
    pub prepare: Arc<PrepareFn>,
    pub launch: Arc<LaunchFn>,
    /// Where an update is installed: the unpacked release this runs from; `None` where updates
    /// cannot be installed (not Windows, or not an unpacked release, e.g. `cargo run`).
    pub target: Option<PathBuf>,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            fetch: Arc::new(|| update::fetch_latest(update::CHECK_TIMEOUT)),
            prepare: Arc::new(|release, target, progress, cancelled| {
                update::prepare(release, target, progress, cancelled)
            }),
            launch: Arc::new(update::launch),
            target: if update::INSTALL_SUPPORTED {
                update::install_dir()
            } else {
                None
            },
        }
    }
}

/// `目前版本：…`, with the latest version and its date once known.
pub fn versions_text(release: Option<&Release>) -> String {
    let Some(release) = release else {
        return format!("目前版本：{CURRENT}");
    };
    let published: String = release.published.chars().take(10).collect();
    let date = if published.is_empty() {
        String::new()
    } else {
        format!("（{published} 發佈）")
    };
    format!(
        "目前版本：{CURRENT}　·　最新版本：{}{date}",
        release.version
    )
}

/// Whether 下載並安裝 applies: a newer release with this platform's archive, installable here.
pub fn can_install(release: &Release, target: Option<&Path>) -> bool {
    release.is_newer_than(CURRENT) && target.is_some() && release.package.is_some()
}

/// What the dialog says about `release`.
pub fn release_status(release: &Release, target: Option<&Path>) -> String {
    let tag = &release.tag;
    if !release.is_newer_than(CURRENT) {
        return "已是最新版本。".into();
    }
    match (&release.package, target) {
        (None, _) => format!(
            "有新版本 {tag}，但沒有本平台（{}）的 Rust 版檔案，請到發佈頁面查看。",
            update::current_platform()
        ),
        (Some(_), None) if update::INSTALL_SUPPORTED => format!(
            "有新版本 {tag}。目前不是從解壓縮的發佈檔執行（例如開發版），請到發佈頁面下載，或用 git pull 更新。"
        ),
        (Some(_), None) => format!("有新版本 {tag}，請到發佈頁面下載並解壓縮。"),
        (Some(package), Some(_)) => format!(
            "有新版本 {tag}（下載約 {:.0} MB）。按「下載並安裝」，下載並核對 SHA-256 後會關閉程式、\
             換上新版並自動重新開啟；設定、備份與錄音不受影響。",
            package.size as f64 / 1e6
        ),
    }
}

/// A release query running in the background.
struct Check {
    events: Receiver<Result<Release, String>>,
}

impl Check {
    fn start(fetch: Arc<FetchFn>, ctx: Option<egui::Context>) -> Self {
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch().map_err(|e| e.to_string()));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        Check { events }
    }

    /// The answer once it came (`None` while still waiting).
    fn poll(&self) -> Option<Result<Release, String>> {
        match self.events.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err("未預期的錯誤：查詢中斷".into())),
        }
    }
}

enum DownloadEvent {
    Progress(u64, u64),
    Done(Result<PreparedUpdate, UpdateError>),
}

/// Download, verification and unpacking running in the background.
struct Download {
    events: Receiver<DownloadEvent>,
    cancel: Arc<AtomicBool>,
}

impl Download {
    fn start(
        prepare: Arc<PrepareFn>,
        release: Release,
        target: PathBuf,
        ctx: Option<egui::Context>,
    ) -> Self {
        let (tx, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancel);
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let progress_ctx = ctx.clone();
            let mut progress = move |done: u64, total: u64| {
                let _ = progress_tx.send(DownloadEvent::Progress(done, total));
                if let Some(ctx) = &progress_ctx {
                    ctx.request_repaint();
                }
            };
            let cancelled = || stop.load(Ordering::Relaxed);
            let result = match prepare(&release, &target, &mut progress, &cancelled) {
                // Cancelled while it was verifying or unpacking: nobody wants it any more.
                Ok(prepared) if stop.load(Ordering::Relaxed) => {
                    let _ = std::fs::remove_dir_all(&prepared.staging);
                    Err(UpdateError::Cancelled)
                }
                other => other,
            };
            // No window to take it (closed or replaced): the folder is not wanted either.
            if let Err(mpsc::SendError(DownloadEvent::Done(Ok(prepared)))) =
                tx.send(DownloadEvent::Done(result))
            {
                let _ = std::fs::remove_dir_all(&prepared.staging);
            }
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
        Download { events, cancel }
    }
}

impl Drop for Download {
    /// A download whose window is gone stops at its next read.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The 檢查更新 window.
#[derive(Default)]
pub struct UpdateDialog {
    pub release: Option<Release>,
    pub status: String,
    /// (bytes done, total) while downloading; total 0 until it is known.
    pub progress: Option<(u64, u64)>,
    check: Option<Check>,
    download: Option<Download>,
    /// Downloaded and verified, waiting for 「安裝更新」 to be answered.
    pub ready: Option<PreparedUpdate>,
}

impl UpdateDialog {
    pub fn is_checking(&self) -> bool {
        self.check.is_some()
    }

    pub fn is_downloading(&self) -> bool {
        self.download.is_some()
    }

    pub fn start_check(&mut self, services: &Services, ctx: Option<egui::Context>) {
        if self.check.is_some() {
            return;
        }
        self.status = "正在查詢 GitHub 上的最新版本…".into();
        self.check = Some(Check::start(Arc::clone(&services.fetch), ctx));
    }

    fn show_release(&mut self, release: Release, target: Option<&Path>) {
        self.status = release_status(&release, target);
        self.release = Some(release);
    }

    pub fn start_download(&mut self, services: &Services, ctx: Option<egui::Context>) {
        let (Some(release), Some(target)) = (self.release.clone(), services.target.clone()) else {
            return;
        };
        self.status = format!("正在下載 {}…", release.tag);
        self.progress = Some((0, 0));
        self.download = Some(Download::start(
            Arc::clone(&services.prepare),
            release,
            target,
            ctx,
        ));
    }

    pub fn cancel_download(&self) {
        if let Some(download) = &self.download {
            download.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Collect what the background work sent since the last frame.
    fn poll(&mut self, target: Option<&Path>) {
        if let Some(result) = self.check.as_ref().and_then(Check::poll) {
            self.check = None;
            match result {
                Ok(release) => self.show_release(release, target),
                Err(message) => self.status = format!("無法檢查更新：{message}"),
            }
        }
        let mut finished = None;
        if let Some(download) = &self.download {
            loop {
                match download.events.try_recv() {
                    Ok(DownloadEvent::Progress(done, total)) => self.progress = Some((done, total)),
                    Ok(DownloadEvent::Done(result)) => {
                        finished = Some(result.map_err(|e| e.to_string()));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = Some(Err("未預期的錯誤：下載中斷".into()));
                        break;
                    }
                }
            }
        }
        if let Some(result) = finished {
            self.download = None;
            self.progress = None;
            match result {
                Ok(prepared) => {
                    self.status = format!("{} 已下載並通過 SHA-256 核對。", prepared.version);
                    self.ready = Some(prepared);
                }
                Err(message) => self.status = format!("更新未完成：{message}"),
            }
        }
    }
}

/// The update feature's state in the app.
#[derive(Default)]
pub struct Updates {
    pub services: Services,
    pub dialog: Option<UpdateDialog>,
    /// What the automatic check found (the dialog shows it without asking again).
    pub known: Option<Release>,
    /// The automatic check at start-up, and when it started.
    auto: Option<(Check, DateTime<Utc>)>,
    /// Verified and accepted, installed once the window may close (unsaved changes asked about).
    pub(crate) pending: Option<PreparedUpdate>,
}

/// What the user did in the window this frame.
enum Action {
    Check,
    Install,
    CancelDownload,
    OpenPage,
    AutoCheck(bool),
}

fn remove_staging(prepared: &PreparedUpdate) {
    let _ = std::fs::remove_dir_all(&prepared.staging);
}

impl OpenDrsApp {
    /// 工具 › 檢查更新…: show what the automatic check found, else ask GitHub now. An open
    /// window is kept as it is (with its download, if one runs).
    pub fn check_for_updates(&mut self) {
        if self.updates.dialog.is_some() {
            return;
        }
        let mut dialog = UpdateDialog::default();
        let target = self.updates.services.target.clone();
        match self.updates.known.clone() {
            Some(release) => dialog.show_release(release, target.as_deref()),
            None => dialog.start_check(&self.updates.services, self.ctx.clone()),
        }
        self.updates.dialog = Some(dialog);
    }

    /// The automatic check at start-up: at most daily (see `update::check_due`), and silent unless
    /// a newer release has this platform's archive. Returns whether a check started.
    pub fn start_update_check(&mut self, now: DateTime<Utc>) -> bool {
        if !self.settings.check_updates || !update::check_due(&self.settings.last_update_check, now)
        {
            return false;
        }
        let check = Check::start(Arc::clone(&self.updates.services.fetch), self.ctx.clone());
        self.updates.auto = Some((check, now));
        true
    }

    /// 工具 › 檢查更新… — mentions the new version once the automatic check found one.
    pub fn update_menu_text(&self) -> String {
        match &self.updates.known {
            Some(release) if release.is_newer_than(CURRENT) && release.package.is_some() => {
                format!("{MENU_TEXT}（有新版本 {}）", release.tag)
            }
            _ => MENU_TEXT.into(),
        }
    }

    /// Every frame: the automatic check's answer and the open window's background work.
    pub fn poll_updates(&mut self) {
        let answer = self
            .updates
            .auto
            .as_ref()
            .and_then(|(check, when)| check.poll().map(|result| (result, *when)));
        if let Some((result, when)) = answer {
            self.updates.auto = None;
            // A failed automatic check stays quiet; the next start tries again.
            if let Ok(release) = result {
                self.settings.last_update_check = update::now_stamp(when);
                self.save_settings();
                if release.is_newer_than(CURRENT) && release.package.is_some() {
                    self.status = format!("有新版本 {} 可以更新：工具 › 檢查更新…", release.tag);
                }
                self.updates.known = Some(release);
            }
        }
        let target = self.updates.services.target.clone();
        if let Some(dialog) = self.updates.dialog.as_mut() {
            dialog.poll(target.as_deref());
        }
    }

    /// Remove staging folders left by an update that was never applied (only older ones: an
    /// update being prepared or applied right now is left alone); returns how many went.
    pub fn clean_stale_updates(&self) -> usize {
        self.updates.services.target.as_deref().map_or(0, |target| {
            update::remove_stale_staging(target, STALE_AFTER)
        })
    }

    /// Start the updater for the accepted update; true when the window should now close.
    pub(crate) fn install_pending_update(&mut self) -> bool {
        if self.busy() {
            self.cancel_pending_update();
            self.warn("安裝更新", BUSY);
            return false;
        }
        let Some(prepared) = self.updates.pending.take() else {
            return false;
        };
        let log = self.data_dir().join(UPDATE_LOG);
        if let Some(folder) = log.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        match (self.updates.services.launch)(&prepared, &log) {
            Ok(()) => true,
            Err(e) => {
                remove_staging(&prepared);
                self.warn("安裝更新", e.to_string());
                false
            }
        }
    }

    /// The install was not confirmed after all (unsaved changes: 取消, or saving failed).
    pub(crate) fn cancel_pending_update(&mut self) {
        if let Some(prepared) = self.updates.pending.take() {
            remove_staging(&prepared);
        }
        if let Some(dialog) = self.updates.dialog.as_mut() {
            dialog.status = "已取消安裝。".into();
        }
    }

    /// 「安裝更新」 answered: 是 hands over to the updater once unsaved changes are dealt with.
    pub fn answer_install(&mut self, yes: bool) {
        let Some(prepared) = self
            .updates
            .dialog
            .as_mut()
            .and_then(|dialog| dialog.ready.take())
        else {
            return;
        };
        if !yes {
            remove_staging(&prepared);
            if let Some(dialog) = self.updates.dialog.as_mut() {
                dialog.status = "已取消安裝。".into();
            }
            return;
        }
        if self.busy() {
            remove_staging(&prepared);
            if let Some(dialog) = self.updates.dialog.as_mut() {
                dialog.status = BUSY.into();
            }
            self.warn("安裝更新", BUSY);
            return;
        }
        self.updates.pending = Some(prepared);
        if self.doc.is_dirty() {
            self.dialog = Some(Dialog::ConfirmDiscard(Pending::InstallUpdate));
        } else {
            self.run_pending(Pending::InstallUpdate);
        }
    }

    /// The 檢查更新 window and its 「安裝更新」 question.
    pub(crate) fn update_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.updates.dialog.take() else {
            return;
        };
        let target = self.updates.services.target.clone();
        let mut open = true;
        let auto = self.settings.check_updates;
        let action = egui::Window::new(TITLE)
            .open(&mut open)
            .collapsible(false)
            .default_size([560.0, 420.0])
            .show(ctx, |ui| {
                window_contents(ui, &dialog, target.as_deref(), auto)
            })
            .and_then(|response| response.inner)
            .flatten();
        let answer = dialog
            .ready
            .as_ref()
            .and_then(|prepared| install_question(ctx, &prepared.version));
        if let Some(action) = action {
            self.run_update_action(&mut dialog, action, ctx);
        }
        if open {
            self.updates.dialog = Some(dialog);
        } else {
            // Closed: dropping the download stops it (and it removes its folder); an unanswered
            // update goes too.
            if let Some(prepared) = dialog.ready.take() {
                remove_staging(&prepared);
            }
        }
        if let Some(yes) = answer {
            self.answer_install(yes);
        }
    }

    fn run_update_action(
        &mut self,
        dialog: &mut UpdateDialog,
        action: Action,
        ctx: &egui::Context,
    ) {
        match action {
            Action::Check => dialog.start_check(&self.updates.services, self.ctx.clone()),
            Action::Install => dialog.start_download(&self.updates.services, self.ctx.clone()),
            Action::CancelDownload => dialog.cancel_download(),
            Action::OpenPage => {
                let url = dialog
                    .release
                    .as_ref()
                    .map_or(update::RELEASES_URL, |r| r.page_url.as_str());
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
            Action::AutoCheck(enabled) => {
                self.settings.check_updates = enabled;
                self.save_settings();
            }
        }
    }
}

/// The window's versions, status, notes, progress, the automatic-check box and the buttons.
fn window_contents(
    ui: &mut egui::Ui,
    dialog: &UpdateDialog,
    target: Option<&Path>,
    auto: bool,
) -> Option<Action> {
    ui.label(versions_text(dialog.release.as_ref()));
    if !dialog.status.is_empty() {
        ui.add(egui::Label::new(&dialog.status).wrap());
    }
    if let Some(release) = &dialog.release {
        ui.separator();
        ui.strong(&release.title);
        let notes = if release.notes.is_empty() {
            "（沒有更新說明）"
        } else {
            release.notes.as_str()
        };
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.add(egui::Label::new(notes).wrap().selectable(true));
            });
    }
    if let Some((done, total)) = dialog.progress {
        let bar = if total > 0 {
            egui::ProgressBar::new(done as f32 / total as f32).show_percentage()
        } else {
            egui::ProgressBar::new(0.0).animate(true)
        };
        ui.add(bar);
    }
    let mut action = None;
    let mut auto = auto;
    if ui.checkbox(&mut auto, AUTO_CHECK).changed() {
        action = Some(Action::AutoCheck(auto));
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if target.is_some() {
            let downloading = dialog.is_downloading();
            let enabled = downloading
                || dialog
                    .release
                    .as_ref()
                    .is_some_and(|r| can_install(r, target));
            let text = if downloading {
                "取消下載"
            } else {
                "下載並安裝"
            };
            if ui.add_enabled(enabled, egui::Button::new(text)).clicked() {
                action = Some(if downloading {
                    Action::CancelDownload
                } else {
                    Action::Install
                });
            }
        }
        if ui.button("開啟發佈頁面").clicked() {
            action = Some(Action::OpenPage);
        }
        let busy = dialog.is_checking() || dialog.is_downloading();
        if ui
            .add_enabled(!busy, egui::Button::new("重新檢查"))
            .clicked()
        {
            action = Some(Action::Check);
        }
    });
    action
}

/// 「安裝更新」: 是 / 否 once answered (closing the question counts as 否).
fn install_question(ctx: &egui::Context, version: &str) -> Option<bool> {
    let response = egui::Modal::new(egui::Id::new("install-update")).show(ctx, |ui| {
        ui.heading("安裝更新");
        ui.label(format!(
            "要關閉 OpenDRS 並安裝 {version} 嗎？\n\n安裝完成後會自動重新開啟；有未儲存的修改時會先詢問。\
             設定、備份與錄音不受影響。"
        ));
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
    response
        .inner
        .or_else(|| response.should_close().then_some(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendrs_update::Asset;

    fn release(version: &str, package: bool) -> Release {
        Release {
            tag: format!("v{version}"),
            version: version.into(),
            title: format!("OpenDRS v{version}"),
            notes: String::new(),
            page_url: update::RELEASES_URL.into(),
            published: "2030-01-02T03:04:05Z".into(),
            package: package.then(|| Asset {
                name: format!("OpenDRS-{version}-windows-x64.zip"),
                url: String::new(),
                size: 12_400_000,
                sha256: String::new(),
            }),
            checksum: None,
        }
    }

    #[test]
    fn status_says_why_an_update_cannot_be_installed() {
        let here = Path::new("C:/OpenDRS");
        assert_eq!(
            release_status(&release(CURRENT, true), Some(here)),
            "已是最新版本。"
        );
        let newer = release("999.0.0", true);
        assert!(can_install(&newer, Some(here)) && !can_install(&newer, None));
        assert!(
            release_status(&newer, Some(here)).starts_with("有新版本 v999.0.0（下載約 12 MB）。")
        );
        let python_only = release("999.0.0", false);
        assert!(!can_install(&python_only, Some(here)));
        assert!(release_status(&python_only, Some(here)).contains("沒有本平台（"));
        assert!(release_status(&newer, None).contains("請到發佈頁面下載"));
    }

    #[test]
    fn versions_line_adds_the_release_date() {
        assert_eq!(versions_text(None), format!("目前版本：{CURRENT}"));
        assert_eq!(
            versions_text(Some(&release("9.9.9", true))),
            format!("目前版本：{CURRENT}　·　最新版本：9.9.9（2030-01-02 發佈）")
        );
    }
}
