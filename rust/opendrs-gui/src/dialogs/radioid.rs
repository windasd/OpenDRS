//! 從 RadioID.net 匯入聯絡人 (port of `src/opendrs/gui/radioid_dialog.py` and `apply_import` of
//! `contact_ops.py`): pick RadioID users to import as private-call contacts.
//!
//! The user list is downloaded (when there is no local copy, or on 下載／更新資料) and parsed on
//! a background thread; closing the window cancels a download. Filters apply 250 ms after the
//! last change, and ticks are kept by radio ID, so they survive filter changes.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use opendrs_contacts::contact_import::{
    DEFAULT_CONTACT_CAPACITY, ImportReport, NameFormat, existing_private_ids, import_users,
};
use opendrs_contacts::radioid::http::{HttpClient, UreqClient};
use opendrs_contacts::radioid::{self, CacheInfo, DownloadOptions, Query, RadioIdUser, UserDb};
use opendrs_core::f136::Project;
use opendrs_core::pystr::thousands;

use super::backups::ask;
use crate::app::OpenDrsApp;

pub const TITLE: &str = "從 RadioID.net 匯入聯絡人";
pub const ALL_REGIONS: &str = "全部地區";
/// The region selected when the list is loaded.
pub const DEFAULT_REGION: &str = "Taiwan";
pub const EXISTING_TIP: &str = "已在聯絡人中：原名稱是呼號（含舊呼號）或與 RadioID 相符時更新名稱，編號不變；自訂名稱（如「老王」）保留";
pub const NO_CACHE: &str = "尚無本機資料，請按「下載／更新資料」";
pub const DOWNLOADING: &str = "下載中…";
pub const NOT_MODIFIED: &str = "伺服器資料未變更，使用本機資料";
pub const PARSING: &str = "解析中…";
/// The title of the download / parse failure message.
pub const FAILURE_TITLE: &str = "RadioID";
pub const IMPORT_LABEL: &str = "匯入聯絡人";
pub const OVER_CAPACITY_TITLE: &str = "超過容量";
/// Filters apply this long after the last change.
pub const DEBOUNCE: Duration = Duration::from_millis(250);
const PREFIX_HINT: &str = "ID 前綴，例如 466（台灣）";
const SEARCH_HINT: &str = "搜尋 ID、呼號、姓名、城市（多個關鍵字以空白分隔）";
/// `QRegularExpressionValidator(r"\d{0,8}")`.
const MAX_PREFIX: usize = 8;
const HEADERS: [&str; 7] = ["", "ID", "呼號", "姓名", "城市", "州／省", "國家"];
const WIDTHS: [f32; 6] = [70.0, 80.0, 90.0, 180.0, 140.0, 140.0];
const ROW_HEIGHT: f32 = 22.0;
const MUTED: Color32 = Color32::from_rgb(0x88, 0x88, 0x88);

/// The HTTP layer of the download (tests pass a closure; the application [`UreqClient`]).
pub type SharedClient = Arc<dyn HttpClient + Send + Sync>;

// ---------------------------------------------------------------- texts

/// `format(n, ",")`.
fn grouped(n: usize) -> String {
    thousands(n as u64)
}

/// 「下載中… 1.2 MB / 17.0 MB」 (MB = 10⁶ bytes; no total when the server did not send one).
pub fn progress_text(got: u64, total: i64) -> String {
    let mut text = format!("{DOWNLOADING} {:.1} MB", got as f64 / 1e6);
    if total != 0 {
        text.push_str(&format!(" / {:.1} MB", total as f64 / 1e6));
    }
    text
}

/// The progress bar's fill, once the total is known.
pub fn progress_fraction(got: u64, total: i64) -> Option<f32> {
    (total > 0).then(|| (got as f64 / total as f64).clamp(0.0, 1.0) as f32)
}

/// 「本機資料 12,345 筆（下載於 2026-10-08 13:01）」.
pub fn loaded_text(count: usize, when: &str) -> String {
    format!("本機資料 {} 筆（下載於 {when}）", grouped(count))
}

/// When the cached list was downloaded, local time to the minute; 「—」 without a cache.
pub fn downloaded_when(info: Option<&CacheInfo>) -> String {
    info.and_then(|info| local_minute(info.downloaded_at))
        .unwrap_or_else(|| "—".into())
}

/// `datetime.fromtimestamp(seconds).strftime("%Y-%m-%d %H:%M")`.
fn local_minute(seconds: f64) -> Option<String> {
    if !seconds.is_finite() {
        return None;
    }
    let micros = (seconds * 1e6).round() as i64;
    let utc = chrono::DateTime::from_timestamp_micros(micros)?;
    Some(
        utc.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
    )
}

/// The OK button: 「匯入 1,234 筆」.
pub fn import_button_text(chosen: usize) -> String {
    format!("匯入 {} 筆", grouped(chosen))
}

/// The question before importing more than fits.
pub fn over_capacity_text(capacity: usize) -> String {
    format!(
        "勾選數量超過容量上限 {}，超出部分不會匯入。要繼續嗎？",
        grouped(capacity)
    )
}

/// The ID prefix box accepts up to eight ASCII digits (the whole edit is refused otherwise).
pub fn valid_prefix(text: &str) -> bool {
    text.len() <= MAX_PREFIX && text.bytes().all(|b| b.is_ascii_digit())
}

/// The region combo: (label, country); 「全部地區」 first, then the countries, most users first.
pub fn region_items(db: &UserDb) -> Vec<(String, String)> {
    std::iter::once((ALL_REGIONS.to_string(), String::new()))
        .chain(
            db.countries()
                .into_iter()
                .map(|(country, n)| (format!("{country}（{}）", grouped(n)), country)),
        )
        .collect()
}

/// Taiwan when it is listed, else 全部地區.
pub fn default_region(items: &[(String, String)]) -> usize {
    items
        .iter()
        .position(|(_, country)| country == DEFAULT_REGION)
        .unwrap_or(0)
}

/// The numbers under the table.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    /// Users matching the filter.
    pub shown: usize,
    /// Of those, already in the contacts.
    pub shown_existing: usize,
    /// Ticked users (all filters).
    pub chosen: usize,
    /// Ticked users not yet in the contacts (the others are updated in place).
    pub new: usize,
    pub contacts: usize,
    pub capacity: usize,
}

impl Tally {
    pub fn after(&self) -> usize {
        self.contacts + self.new
    }

    pub fn over_capacity(&self) -> bool {
        self.after() > self.capacity
    }

    /// 「符合 … 筆（… 筆已存在）｜已勾選 … 筆（新增 …、已存在 …）｜匯入後 … / 1,000」.
    pub fn text(&self) -> String {
        let mut text = format!(
            "符合 {} 筆（{} 筆已存在）｜已勾選 {} 筆（新增 {}、已存在 {}）｜匯入後 {} / {}",
            grouped(self.shown),
            self.shown_existing,
            grouped(self.chosen),
            grouped(self.new),
            grouped(self.chosen - self.new),
            grouped(self.after()),
            grouped(self.capacity),
        );
        if self.over_capacity() {
            text.push_str("　⚠ 超過容量");
        }
        text
    }
}

// ---------------------------------------------------------------- background loading

/// What the loading thread reports.
#[derive(Debug)]
pub enum LoadEvent {
    Status(String),
    Progress {
        got: u64,
        total: i64,
    },
    /// The parsed list; `None` when there is no local copy.
    Loaded(Option<UserDb>),
    Failed(String),
}

/// The loading thread's work (`_LoadThread.run`): refresh the cache in `dir` when asked, then
/// parse it. Stops without parsing once `cancelled` says so.
pub fn load(
    dir: &Path,
    refresh: bool,
    client: &dyn HttpClient,
    options: &DownloadOptions,
    cancelled: &dyn Fn() -> bool,
    send: &mut dyn FnMut(LoadEvent),
) {
    if refresh {
        send(LoadEvent::Status(DOWNLOADING.into()));
        let result = {
            let mut progress = |got: u64, total: i64| send(LoadEvent::Progress { got, total });
            radioid::download_cancellable(dir, client, options, Some(&mut progress), cancelled)
        };
        match result {
            Ok(true) => {}
            Ok(false) => send(LoadEvent::Status(NOT_MODIFIED.into())),
            Err(e) => return send(LoadEvent::Failed(e.to_string())),
        }
    }
    if cancelled() {
        return;
    }
    send(LoadEvent::Status(PARSING.into()));
    match radioid::load_cached(dir) {
        Ok(found) => send(LoadEvent::Loaded(found.map(|(db, _skipped)| db))),
        Err(e) => send(LoadEvent::Failed(e.to_string())),
    }
}

/// The running loading thread; dropping it (the window closed) cancels the download.
struct Loader {
    events: Receiver<LoadEvent>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Job {
    dir: PathBuf,
    refresh: bool,
    client: SharedClient,
    options: DownloadOptions,
    repaint: Option<egui::Context>,
}

fn spawn(job: Job) -> std::io::Result<Loader> {
    let (tx, events) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("radioid".into())
        .spawn(move || {
            let cancelled = || flag.load(Ordering::Relaxed);
            let mut send = |event: LoadEvent| {
                // Nobody listens once the window is closed; the result is simply dropped.
                let _ = tx.send(event);
                if let Some(ctx) = &job.repaint {
                    ctx.request_repaint();
                }
            };
            let client: &dyn HttpClient = job.client.as_ref();
            load(
                &job.dir,
                job.refresh,
                client,
                &job.options,
                &cancelled,
                &mut send,
            );
        })?;
    Ok(Loader { events, cancel })
}

// ---------------------------------------------------------------- the dialog

/// What the user did in the window this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RadioIdAction {
    /// 下載／更新資料.
    Refresh,
    /// 全選符合結果.
    SelectAll,
    /// 取消全選 (the users matching the filter).
    SelectNone,
    Toggle {
        radio_id: u32,
        on: bool,
    },
    /// 匯入 n 筆.
    Accept,
    /// The answer to 「超過容量」.
    Answer(bool),
    /// 取消 or the window's close button.
    Cancel,
}

/// The dialog's state while it is open.
pub struct RadioIdDialog {
    cache_dir: PathBuf,
    client: SharedClient,
    options: DownloadOptions,
    /// The contact list's size limit (the radio's 1,000).
    pub capacity: usize,
    pub name_format: NameFormat,
    started: bool,
    loader: Option<Loader>,
    repaint: Option<egui::Context>,
    status: String,
    progress: Option<f32>,
    failures: Vec<String>,
    db: Option<UserDb>,
    index: HashMap<u32, usize>,
    regions: Vec<(String, String)>,
    region: usize,
    prefix: String,
    search: String,
    filter_due: Option<Instant>,
    /// Indices into the user list.
    rows: Vec<usize>,
    checked: BTreeSet<u32>,
    existing: HashSet<u32>,
    contacts: usize,
    revision: Option<u64>,
    asking: bool,
}

impl RadioIdDialog {
    /// The dialog for the cache in `cache_dir`; nothing is loaded before [`Self::start`].
    pub fn new(cache_dir: PathBuf, client: SharedClient) -> Self {
        RadioIdDialog {
            cache_dir,
            client,
            options: DownloadOptions::default(),
            capacity: DEFAULT_CONTACT_CAPACITY,
            name_format: NameFormat::Callsign,
            started: false,
            loader: None,
            repaint: None,
            status: String::new(),
            progress: None,
            failures: Vec::new(),
            db: None,
            index: HashMap::new(),
            regions: Vec::new(),
            region: 0,
            prefix: String::new(),
            search: String::new(),
            filter_due: None,
            rows: Vec::new(),
            checked: BTreeSet::new(),
            existing: HashSet::new(),
            contacts: 0,
            revision: None,
            asking: false,
        }
    }

    /// Follow the project being edited: which IDs exist, how many contacts there are.
    pub fn sync_project(&mut self, project: &Project, revision: u64) {
        if self.revision != Some(revision) {
            self.existing = existing_private_ids(project);
            self.contacts = project.contacts.len();
            self.revision = Some(revision);
        }
    }

    /// The first load: download when there is no local copy, otherwise parse it. `repaint` is
    /// woken whenever the thread reports something.
    pub fn start(&mut self, repaint: Option<egui::Context>) {
        if repaint.is_some() {
            self.repaint = repaint;
        }
        if !self.started {
            self.started = true;
            let refresh = radioid::cache_info(&self.cache_dir).is_none();
            self.start_load(refresh);
        }
    }

    fn start_load(&mut self, refresh: bool) {
        if self.loader.is_some() {
            return;
        }
        self.progress = None;
        let job = Job {
            dir: self.cache_dir.clone(),
            refresh,
            client: Arc::clone(&self.client),
            options: self.options.clone(),
            repaint: self.repaint.clone(),
        };
        match spawn(job) {
            Ok(loader) => self.loader = Some(loader),
            Err(e) => self.fail(e.to_string()),
        }
    }

    pub fn is_loading(&self) -> bool {
        self.loader.is_some()
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    /// Take in what the thread reported; returns the failures to show as messages.
    pub fn poll(&mut self) -> Vec<String> {
        let mut events = Vec::new();
        let mut finished = false;
        if let Some(loader) = &self.loader {
            loop {
                match loader.events.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = true;
                        break;
                    }
                }
            }
        }
        for event in events {
            self.handle(event);
        }
        if finished {
            self.loader = None;
        }
        std::mem::take(&mut self.failures)
    }

    fn handle(&mut self, event: LoadEvent) {
        match event {
            LoadEvent::Status(text) => self.status = text,
            LoadEvent::Progress { got, total } => {
                if let Some(fraction) = progress_fraction(got, total) {
                    self.progress = Some(fraction);
                }
                self.status = progress_text(got, total);
            }
            LoadEvent::Loaded(None) => self.status = NO_CACHE.into(),
            LoadEvent::Loaded(Some(db)) => self.set_db(db),
            LoadEvent::Failed(message) => self.fail(message),
        }
    }

    fn fail(&mut self, message: String) {
        self.status = format!("失敗：{message}");
        self.failures.push(message);
    }

    fn set_db(&mut self, db: UserDb) {
        self.index = db
            .users()
            .iter()
            .enumerate()
            .map(|(i, user)| (user.radio_id, i))
            .collect();
        let when = downloaded_when(radioid::cache_info(&self.cache_dir).as_ref());
        self.status = loaded_text(db.len(), &when);
        self.regions = region_items(&db);
        self.region = default_region(&self.regions);
        self.db = Some(db);
        self.apply_filter();
    }

    // ------------------------------------------------------------ filtering
    pub fn regions(&self) -> &[(String, String)] {
        &self.regions
    }

    pub fn region(&self) -> usize {
        self.region
    }

    /// Select the region of `country` (「」 = 全部地區); false when it is not listed.
    pub fn set_region(&mut self, country: &str) -> bool {
        match self.regions.iter().position(|(_, c)| c == country) {
            Some(i) => {
                self.region = i;
                self.schedule_filter();
                true
            }
            None => false,
        }
    }

    /// Type into the ID prefix box; a text that is not up to eight digits is refused.
    pub fn set_prefix(&mut self, text: &str) -> bool {
        if !valid_prefix(text) {
            return false;
        }
        if self.prefix != text {
            self.prefix = text.into();
            self.schedule_filter();
        }
        true
    }

    pub fn set_search(&mut self, text: &str) {
        if self.search != text {
            self.search = text.into();
            self.schedule_filter();
        }
    }

    fn schedule_filter(&mut self) {
        self.filter_due = Some(Instant::now() + DEBOUNCE);
    }

    pub fn filter_pending(&self) -> bool {
        self.filter_due.is_some()
    }

    /// Apply the filter once its delay has passed; otherwise how long is left.
    pub fn tick(&mut self, now: Instant) -> Option<Duration> {
        let due = self.filter_due?;
        if now >= due {
            self.apply_filter();
            None
        } else {
            Some(due - now)
        }
    }

    pub fn query(&self) -> Query {
        Query {
            country: self
                .regions
                .get(self.region)
                .map_or_else(String::new, |(_, country)| country.clone()),
            id_prefix: self.prefix.clone(),
            text: self.search.clone(),
        }
    }

    /// Show the users matching the filter now (nothing for an empty query).
    pub fn apply_filter(&mut self) {
        self.filter_due = None;
        let Some(db) = &self.db else {
            return;
        };
        let query = self.query();
        self.rows = if query.is_empty() {
            Vec::new()
        } else {
            db.search(&query)
                .into_iter()
                .filter_map(|user| self.index.get(&user.radio_id).copied())
                .collect()
        };
    }

    /// The users shown in the table.
    pub fn shown(&self) -> Vec<&RadioIdUser> {
        match &self.db {
            Some(db) => self.rows.iter().map(|&i| &db.users()[i]).collect(),
            None => Vec::new(),
        }
    }

    // ------------------------------------------------------------ ticks
    pub fn is_existing(&self, radio_id: u32) -> bool {
        self.existing.contains(&radio_id)
    }

    pub fn is_checked(&self, radio_id: u32) -> bool {
        self.checked.contains(&radio_id)
    }

    pub fn toggle(&mut self, radio_id: u32, on: bool) {
        if on {
            self.checked.insert(radio_id);
        } else {
            self.checked.remove(&radio_id);
        }
    }

    /// 全選符合結果／取消全選: tick or untick the users matching the filter.
    pub fn set_all(&mut self, on: bool) {
        let ids: Vec<u32> = self.shown().iter().map(|user| user.radio_id).collect();
        for id in ids {
            self.toggle(id, on);
        }
    }

    pub fn tally(&self) -> Tally {
        let shown = self.shown();
        Tally {
            shown: shown.len(),
            shown_existing: shown
                .iter()
                .filter(|u| self.is_existing(u.radio_id))
                .count(),
            chosen: self.checked.len(),
            new: self
                .checked
                .iter()
                .filter(|id| !self.existing.contains(id))
                .count(),
            contacts: self.contacts,
            capacity: self.capacity,
        }
    }

    /// The ticked users that are in the list, by radio ID.
    pub fn selected_users(&self) -> Vec<RadioIdUser> {
        let Some(db) = &self.db else {
            return Vec::new();
        };
        self.checked
            .iter()
            .filter_map(|id| self.index.get(id))
            .map(|&i| db.users()[i].clone())
            .collect()
    }

    /// The 「超過容量」 question is open.
    pub fn asking(&self) -> bool {
        self.asking
    }

    // ------------------------------------------------------------ drawing
    fn ui(&mut self, ui: &mut egui::Ui, actions: &mut Vec<RadioIdAction>) {
        egui::Panel::top("radioid-top")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .show(ui, |ui| {
                self.status_row(ui, actions);
                ui.add_space(4.0);
                self.filter_row(ui);
                ui.add_space(4.0);
            });
        egui::Panel::bottom("radioid-bottom")
            .frame(egui::Frame::NONE)
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.add_space(4.0);
                self.selection_row(ui, actions);
                ui.add_space(4.0);
                self.button_row(ui, actions);
            });
        egui::CentralPanel::no_frame().show(ui, |ui| self.table(ui, actions));
    }

    fn status_row(&self, ui: &mut egui::Ui, actions: &mut Vec<RadioIdAction>) {
        ui.horizontal(|ui| {
            ui.label(&self.status);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let busy = self.is_loading();
                if ui
                    .add_enabled(!busy, egui::Button::new("下載／更新資料"))
                    .clicked()
                {
                    actions.push(RadioIdAction::Refresh);
                }
                if busy {
                    match self.progress {
                        Some(fraction) => {
                            ui.add(egui::ProgressBar::new(fraction).desired_width(220.0));
                        }
                        None => {
                            ui.spinner();
                        }
                    }
                }
            });
        });
    }

    fn filter_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label("地區：");
            let before = self.region;
            let current = self
                .regions
                .get(self.region)
                .map_or("", |(label, _)| label.as_str())
                .to_string();
            egui::ComboBox::from_id_salt("radioid-region")
                .width(220.0)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    for (i, (label, _)) in self.regions.iter().enumerate() {
                        ui.selectable_value(&mut self.region, i, label);
                    }
                });
            if self.region != before {
                self.schedule_filter();
            }
            let mut prefix = self.prefix.clone();
            let edit = egui::TextEdit::singleline(&mut prefix)
                .hint_text(PREFIX_HINT)
                .desired_width(200.0);
            if ui.add(edit).changed() {
                self.set_prefix(&prefix);
            }
            let mut search = self.search.clone();
            let edit = egui::TextEdit::singleline(&mut search)
                .hint_text(SEARCH_HINT)
                .desired_width(ui.available_width());
            if ui.add(edit).changed() {
                self.set_search(&search);
            }
        });
    }

    fn table(&self, ui: &mut egui::Ui, actions: &mut Vec<RadioIdAction>) {
        let Some(db) = &self.db else {
            return;
        };
        let mut builder = TableBuilder::new(ui)
            .id_salt("radioid-users")
            .striped(true)
            .resizable(true)
            .auto_shrink(false)
            .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
        for width in WIDTHS {
            builder = builder.column(Column::initial(width).at_least(24.0).clip(true));
        }
        builder
            .column(Column::remainder().at_least(60.0).clip(true))
            .header(ROW_HEIGHT, |mut header| {
                for title in HEADERS {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(ROW_HEIGHT, self.rows.len(), |mut row| {
                    let user = &db.users()[self.rows[row.index()]];
                    let present = self.is_existing(user.radio_id);
                    let text = |value: &str| {
                        let rich = RichText::new(value);
                        if present { rich.color(MUTED) } else { rich }
                    };
                    let tip = |response: egui::Response| {
                        if present {
                            response.on_hover_text(EXISTING_TIP);
                        }
                    };
                    row.col(|ui| {
                        let mut on = self.is_checked(user.radio_id);
                        let response =
                            ui.checkbox(&mut on, text(if present { "已存在" } else { "" }));
                        if response.changed() {
                            actions.push(RadioIdAction::Toggle {
                                radio_id: user.radio_id,
                                on,
                            });
                        }
                        tip(response);
                    });
                    let id = user.radio_id.to_string();
                    let name = user.full_name();
                    for value in [
                        id.as_str(),
                        user.callsign.as_str(),
                        name.as_str(),
                        user.city.as_str(),
                        user.state.as_str(),
                        user.country.as_str(),
                    ] {
                        row.col(|ui| {
                            let label = egui::Label::new(text(value)).selectable(false).truncate();
                            tip(ui.add(label));
                        });
                    }
                });
            });
    }

    fn selection_row(&mut self, ui: &mut egui::Ui, actions: &mut Vec<RadioIdAction>) {
        ui.horizontal(|ui| {
            if ui.button("全選符合結果").clicked() {
                actions.push(RadioIdAction::SelectAll);
            }
            if ui.button("取消全選").clicked() {
                actions.push(RadioIdAction::SelectNone);
            }
            ui.add_space(16.0);
            ui.label("名稱格式：");
            egui::ComboBox::from_id_salt("radioid-format")
                .selected_text(self.name_format.label())
                .show_ui(ui, |ui| {
                    for format in NameFormat::ALL {
                        ui.selectable_value(&mut self.name_format, format, format.label());
                    }
                });
            ui.label(self.tally().text());
        });
    }

    fn button_row(&self, ui: &mut egui::Ui, actions: &mut Vec<RadioIdAction>) {
        let chosen = self.checked.len();
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("取消").clicked() {
                actions.push(RadioIdAction::Cancel);
            }
            let ok = egui::Button::new(import_button_text(chosen));
            if ui.add_enabled(chosen > 0, ok).clicked() {
                actions.push(RadioIdAction::Accept);
            }
        });
    }
}

impl OpenDrsApp {
    /// 從 RadioID 匯入聯絡人…
    pub fn import_from_radioid(&mut self) {
        if !self.require_project(IMPORT_LABEL) {
            return;
        }
        if self.windows.radioid.is_some() {
            return; // already open (and maybe downloading)
        }
        let client: SharedClient = Arc::new(UreqClient);
        let mut dialog = RadioIdDialog::new(self.data_dir().join("radioid"), client);
        dialog.sync_project(self.project(), self.doc.revision());
        self.windows.radioid = Some(dialog);
    }

    /// Import `users` as private-call contacts: one undo step 「匯入聯絡人」, then 「匯入完成」.
    pub fn apply_radioid_import(
        &mut self,
        users: &[RadioIdUser],
        format: NameFormat,
    ) -> Option<ImportReport> {
        match import_users(
            self.project(),
            users,
            format,
            self.codec,
            DEFAULT_CONTACT_CAPACITY,
        ) {
            Ok((project, report)) => {
                self.commit(project, IMPORT_LABEL);
                self.warn("匯入完成", report.summary());
                Some(report)
            }
            Err(e) => {
                self.warn(IMPORT_LABEL, e.to_string());
                None
            }
        }
    }

    /// Show what the loading thread reported; a failure becomes a 「RadioID」 message.
    pub fn radioid_poll(&mut self) {
        let Some(dialog) = self.windows.radioid.as_mut() else {
            return;
        };
        if let Some(message) = dialog.poll().pop() {
            self.warn(FAILURE_TITLE, message);
        }
    }

    /// Do `action` in the open RadioID dialog (what its buttons do).
    pub fn radioid_action(&mut self, action: RadioIdAction) {
        let Some(mut dialog) = self.windows.radioid.take() else {
            return;
        };
        if self.apply_radioid_action(&mut dialog, action) {
            self.windows.radioid = Some(dialog);
        }
    }

    /// False when the dialog closes.
    fn apply_radioid_action(&mut self, dialog: &mut RadioIdDialog, action: RadioIdAction) -> bool {
        match action {
            RadioIdAction::Refresh => dialog.start_load(true),
            RadioIdAction::SelectAll => dialog.set_all(true),
            RadioIdAction::SelectNone => dialog.set_all(false),
            RadioIdAction::Toggle { radio_id, on } => dialog.toggle(radio_id, on),
            RadioIdAction::Accept => {
                dialog.sync_project(self.project(), self.doc.revision());
                let tally = dialog.tally();
                if tally.chosen == 0 {
                    return true;
                }
                if !tally.over_capacity() {
                    return self.finish_radioid_import(dialog);
                }
                dialog.asking = true;
            }
            RadioIdAction::Answer(yes) => {
                dialog.asking = false;
                if yes {
                    return self.finish_radioid_import(dialog);
                }
            }
            RadioIdAction::Cancel => return false,
        }
        true
    }

    fn finish_radioid_import(&mut self, dialog: &RadioIdDialog) -> bool {
        self.apply_radioid_import(&dialog.selected_users(), dialog.name_format);
        false
    }
}

/// Draw the dialog if it is open.
pub fn show(app: &mut OpenDrsApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.windows.radioid.take() else {
        return;
    };
    dialog.sync_project(app.project(), app.doc.revision());
    dialog.start(Some(ctx.clone()));
    if let Some(message) = dialog.poll().pop() {
        app.warn(FAILURE_TITLE, message);
    }
    let mut open = true;
    let mut actions = Vec::new();
    egui::Window::new(TITLE)
        .id(egui::Id::new("radioid-window"))
        .open(&mut open)
        .default_size([980.0, 640.0])
        .vscroll(false)
        .show(ctx, |ui| dialog.ui(ui, &mut actions));
    if dialog.asking {
        let mut focused = true; // no default button: Qt's question box has none either
        let buttons = [("是", true), ("否", false)];
        if let Some(yes) = ask(
            ctx,
            "radioid-over-capacity",
            OVER_CAPACITY_TITLE,
            &over_capacity_text(dialog.capacity),
            &buttons,
            false,
            None,
            &mut focused,
        ) {
            actions.push(RadioIdAction::Answer(yes));
        }
    }
    if !open {
        actions.push(RadioIdAction::Cancel);
    }
    for action in actions {
        if !app.apply_radioid_action(&mut dialog, action) {
            return; // closed: dropping the dialog cancels a download
        }
    }
    // After drawing, so a filter typed this frame wakes the window when its delay is over.
    let pending = dialog.filter_pending();
    match dialog.tick(Instant::now()) {
        Some(wait) => ctx.request_repaint_after(wait),
        None if pending => ctx.request_repaint(), // applied: draw the new rows
        None => {}
    }
    app.windows.radioid = Some(dialog);
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendrs_contacts::radioid::http::{HttpError, HttpRequest, HttpResponse};

    const CSV: &str = concat!(
        "RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n",
        "9990001,QA0BD,James,,New Taipei,Northern Taiwan,Taiwan\r\n",
        "9990002,QA0ADN,Chen,Wei,Taichung,Central Taiwan,Taiwan\r\n",
        "1023007,QA0AKM,Hans Peter,,Cornwall,Ontario,Canada\r\n",
    );

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("opendrs-gui-radioid-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("scratch folder");
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn serve(body: &'static str) -> impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> {
        move |_: &HttpRequest| {
            let headers = vec![("Content-Length".to_string(), body.len().to_string())];
            Ok(HttpResponse::new(
                headers,
                Box::new(std::io::Cursor::new(body.as_bytes())),
            ))
        }
    }

    fn run_load(dir: &Path, refresh: bool, client: &dyn HttpClient) -> Vec<LoadEvent> {
        let mut events = Vec::new();
        load(
            dir,
            refresh,
            client,
            &DownloadOptions::default(),
            &|| false,
            &mut |event| events.push(event),
        );
        events
    }

    fn statuses(events: &[LoadEvent]) -> Vec<String> {
        events
            .iter()
            .map(|event| match event {
                LoadEvent::Status(text) => text.clone(),
                LoadEvent::Progress { got, total } => progress_text(*got, *total),
                LoadEvent::Loaded(Some(db)) => format!("loaded {}", db.len()),
                LoadEvent::Loaded(None) => "none".into(),
                LoadEvent::Failed(message) => format!("failed {message}"),
            })
            .collect()
    }

    #[test]
    fn texts_match_python() {
        assert_eq!(
            progress_text(1_234_567, 17_000_000),
            "下載中… 1.2 MB / 17.0 MB"
        );
        assert_eq!(progress_text(250_000, 0), "下載中… 0.2 MB"); // ties to even, like Python
        assert_eq!(progress_fraction(5, 10), Some(0.5));
        assert_eq!(progress_fraction(5, 0), None);
        assert_eq!(
            loaded_text(250_123, "2026-10-08 13:01"),
            "本機資料 250,123 筆（下載於 2026-10-08 13:01）"
        );
        assert_eq!(downloaded_when(None), "—");
        assert_eq!(import_button_text(1234), "匯入 1,234 筆");
        assert_eq!(
            over_capacity_text(1000),
            "勾選數量超過容量上限 1,000，超出部分不會匯入。要繼續嗎？"
        );
    }

    #[test]
    fn download_time_is_local_to_the_minute() {
        let info = CacheInfo {
            path: PathBuf::new(),
            size: 0,
            downloaded_at: 1_791_435_709.5,
            last_modified: String::new(),
        };
        let when = downloaded_when(Some(&info));
        assert_eq!(when.len(), "2026-10-08 05:01".len(), "{when}");
        assert!(when.starts_with("2026-10-0"), "{when}");
        let broken = CacheInfo {
            downloaded_at: f64::NAN,
            ..info
        };
        assert_eq!(downloaded_when(Some(&broken)), "—");
    }

    #[test]
    fn prefix_is_up_to_eight_ascii_digits() {
        assert!(valid_prefix(""));
        assert!(valid_prefix("46600001"));
        assert!(!valid_prefix("466000012"));
        assert!(!valid_prefix("46a"));
        assert!(!valid_prefix("４６６")); // full-width digits: Qt's \d is ASCII here
    }

    #[test]
    fn tally_text_and_capacity() {
        let tally = Tally {
            shown: 12_345,
            shown_existing: 1_002,
            chosen: 1_500,
            new: 1_200,
            contacts: 30,
            capacity: 1000,
        };
        assert_eq!(
            tally.text(),
            "符合 12,345 筆（1002 筆已存在）｜已勾選 1,500 筆（新增 1,200、已存在 300）｜匯入後 1,230 / 1,000　⚠ 超過容量"
        );
        let fits = Tally { new: 970, ..tally };
        assert!(!fits.over_capacity());
        assert!(fits.text().ends_with("匯入後 1,000 / 1,000"));
    }

    #[test]
    fn regions_default_to_taiwan() {
        let db = UserDb::new(radioid::parse_csv(CSV.as_bytes()).unwrap().users);
        let items = region_items(&db);
        let labels: Vec<&str> = items.iter().map(|(label, _)| label.as_str()).collect();
        assert_eq!(labels, ["全部地區", "Taiwan（2）", "Canada（1）"]);
        assert_eq!(default_region(&items), 1);
        assert_eq!(default_region(&items[..1]), 0);
    }

    #[test]
    fn loading_downloads_without_a_cache_then_parses() {
        let scratch = Scratch::new("load");
        let events = run_load(&scratch.0, true, &serve(CSV));
        assert_eq!(
            statuses(&events),
            ["下載中…", "下載中… 0.0 MB / 0.0 MB", "解析中…", "loaded 3"]
        );
        // The cache is there now: a plain load only parses.
        let offline = |_: &HttpRequest| -> Result<HttpResponse, HttpError> {
            panic!("no network when parsing the cache")
        };
        assert_eq!(
            statuses(&run_load(&scratch.0, false, &offline)),
            ["解析中…", "loaded 3"]
        );
    }

    #[test]
    fn loading_reports_not_modified_and_failures() {
        let scratch = Scratch::new("fail");
        let not_found =
            |_: &HttpRequest| -> Result<HttpResponse, HttpError> { Err(HttpError::Status(304)) };
        assert_eq!(
            statuses(&run_load(&scratch.0, true, &not_found)),
            ["下載中…", NOT_MODIFIED, "解析中…", "none"]
        );
        let broken =
            |_: &HttpRequest| -> Result<HttpResponse, HttpError> { Err(HttpError::Status(500)) };
        assert_eq!(
            statuses(&run_load(&scratch.0, true, &broken)),
            ["下載中…", "failed radioid.net 回應 HTTP 500"]
        );
    }

    #[test]
    fn a_cancelled_load_stops_before_parsing() {
        let scratch = Scratch::new("cancel");
        let mut events = Vec::new();
        load(
            &scratch.0,
            true,
            &serve(CSV),
            &DownloadOptions::default(),
            &|| true,
            &mut |event| events.push(event),
        );
        assert_eq!(statuses(&events), ["下載中…", "failed 已取消下載"]);
        assert!(radioid::cache_info(&scratch.0).is_none());
    }

    #[test]
    fn filters_wait_for_the_debounce() {
        let scratch = Scratch::new("debounce");
        let client: SharedClient = Arc::new(serve(CSV));
        let mut dialog = RadioIdDialog::new(scratch.0.clone(), client);
        dialog.handle(LoadEvent::Loaded(Some(UserDb::new(
            radioid::parse_csv(CSV.as_bytes()).unwrap().users,
        ))));
        assert_eq!(dialog.shown().len(), 2, "Taiwan at first");
        dialog.set_search("james");
        assert!(dialog.filter_pending());
        let now = Instant::now();
        assert!(dialog.tick(now).is_some());
        assert_eq!(dialog.shown().len(), 2, "not yet");
        assert_eq!(dialog.tick(now + DEBOUNCE), None);
        assert_eq!(dialog.shown().len(), 1);
        assert!(!dialog.set_prefix("9a"));
        assert!(dialog.set_region(""));
        dialog.set_search("");
        dialog.apply_filter();
        assert!(dialog.query().is_empty());
        assert!(dialog.shown().is_empty(), "an empty query shows nothing");
    }
}
