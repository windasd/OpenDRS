//! 讀取錄音 (port of `src/opendrs/gui/recording_dialog.py` and `recording_worker.py`): list the
//! radio's call recordings, download them, convert them to WAV with the vendor's ambep2.exe
//! (only ever run, never bundled), and play them with the system's player.
//!
//! The transfers are read-only. They run on a worker thread (see `firmware::Worker`); Stop
//! takes effect between blocks, and the window cannot be closed meanwhile. The port value
//! `__sim__` / `sim` selects one `SimulatedRecorder` for the whole dialog, as in Python.
//! (Over the 800-line guideline for the same reason as `firmware.rs`: the port was limited to
//! these two files.)

use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::sync::atomic::{self, AtomicBool};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_extras::{Column, TableBuilder};
use opendrs_contacts::radioid::{self, CSV_NAME, EXPECTED_HEADER, RadioIdUser};
use opendrs_core::pystr;
use opendrs_core::radio_link::Transport;
use opendrs_firmware::recording::{
    DEFAULT_LIST_MAX, MAX_RECORDS, RecordClient, RecordInfo, RecordingError, Saved, default_ambep2,
    save_recording,
};
use opendrs_firmware::recording_sim::SimulatedRecorder;

use super::firmware::{
    FirmwareDialog, NO_PORT_LABEL, POLL_INTERVAL, PortChoice, Reporter, TransportFactory,
    UNEXPECTED, Worker, panic_text, refuse_app_close, serial_factory, shared_factory,
};
use super::open_with_system;
use crate::app::OpenDrsApp;

pub const HEADERS: [&str; 7] = [
    "編號",
    "收／發",
    "對方 ID",
    "聯絡人",
    "長度",
    "時間",
    "檔案",
];
pub const FILE_COL: usize = HEADERS.len() - 1;
pub const RADIOID_TIP: &str = "RadioID.net 離線資料（不在專案聯絡人中）";
pub const NOTE: &str = "對講機會把通話錄成 AMBE+2 資料；下載後以原廠的 ambep2.exe 解碼成 WAV（與原廠 Wav.exe 相同）。\
DR-8500UV／DH-98VU 原廠韌體（308E.BBK／B6.TARSCB.003）沒有接上這些指令；刷入「錄音讀取修補版」\
（DR8500UV_B6_record_read.bin）後即可讀取（已實機驗證）。下載時不要在對講機上播放或刪除錄音。";
pub const ALL_TIP: &str = "不讀清單，直接從第 1 筆讀到最後一筆（對講機不支援清單指令時使用）";
pub const NO_PORT: &str = "請先在主視窗工具列選擇寫頻線的連接埠。";
pub const BUSY: &str = "請等目前的讀寫完成。";
pub const RUNNING: &str = "傳輸進行中，請等完成或按「停止」。";
pub const STOPPING: &str = "停止中（完成目前這一塊）…";
pub const NO_DIR: &str = "請先指定錄音資料夾。";
pub const NOTHING_CHECKED: &str = "請先「讀取清單」並勾選要下載的錄音。";
pub const NOT_WAV: &str = "這一列還沒有下載成 WAV。";
pub const STOPPED: &str = "已停止。";
/// 依編號全部下載 reads records 1 ..= min(total, this).
pub const ALL_RECORDS_LIMIT: u64 = 999;
/// Between two records on a real radio (it goes back to its command loop).
pub const PAUSE: Duration = Duration::from_millis(500);

// ---------------------------------------------------------------- texts

/// 「3 分 12 秒」; hours and minutes only when there are any (Python's `round`: ties to even).
pub fn duration_text(seconds: f64) -> String {
    let total = seconds.round_ties_even().max(0.0) as u64;
    let (hours, rest) = (total / 3600, total % 3600);
    let (minutes, secs) = (rest / 60, rest % 60);
    if hours > 0 {
        format!("{hours} 小時 {minutes} 分 {secs} 秒")
    } else if minutes > 0 {
        format!("{minutes} 分 {secs} 秒")
    } else {
        format!("{secs} 秒")
    }
}

/// 「共 25 筆，合計 3 分 12 秒」.
pub fn total_text(infos: &[RecordInfo]) -> String {
    let seconds: f64 = infos.iter().map(RecordInfo::seconds).sum();
    format!("共 {} 筆，合計 {}", infos.len(), duration_text(seconds))
}

pub fn direction_text(info: &RecordInfo) -> &'static str {
    if info.direction == 'R' {
        "接收"
    } else {
        "發射"
    }
}

pub fn length_text(info: &RecordInfo) -> String {
    format!("{:.1} 秒", info.seconds())
}

/// The 檔案 cell: the WAV (else the raw data) and why there is no WAV.
pub fn file_text(saved: &Saved) -> String {
    let path = saved.wav.as_ref().unwrap_or(&saved.raw);
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    if saved.note.is_empty() {
        name
    } else {
        format!("{name}（{}）", saved.note)
    }
}

pub fn listed_text(count: usize, total: u64, stopped: bool) -> String {
    let cut = if stopped {
        "已停止，清單可能不完整。"
    } else {
        ""
    };
    format!("對講機回報 {count} 筆錄音（共 {total} 筆）。{cut}勾選後按「下載勾選的錄音」。")
}

pub fn downloaded_text(saved: &[Saved], out_dir: &str) -> String {
    let wavs = saved.iter().filter(|s| s.wav.is_some()).count();
    let seconds: f64 = saved.iter().map(|s| s.info.seconds()).sum();
    format!(
        "已下載 {} 筆（{wavs} 筆轉成 WAV，合計 {}），存放在 {out_dir}",
        saved.len(),
        duration_text(seconds)
    )
}

/// The records 依編號全部下載 reads after record 1 (whose line gives the total).
pub fn all_records_plan(total: u64) -> RangeInclusive<u32> {
    let last = u32::try_from(total.min(ALL_RECORDS_LIMIT)).unwrap_or(u32::MAX);
    2..=last
}

/// Settings values for (recordings_dir, ambep2_path) on close: "" when equal to the default.
pub fn remembered_paths(
    out_dir: &str,
    ambep2: &str,
    default_dir: &str,
    default_ambep2: &str,
) -> (String, String) {
    let (out, exe) = (pystr::strip(out_dir), pystr::strip(ambep2));
    (
        if out == default_dir { "" } else { out }.to_owned(),
        if exe == default_ambep2 { "" } else { exe }.to_owned(),
    )
}

pub fn default_ambep2_text() -> String {
    default_ambep2().map_or_else(String::new, |p| p.display().to_string())
}

// ---------------------------------------------------------------- RadioID names

/// 「呼號 姓名」 from the downloaded RadioID.net user.csv. The file (~17 MB) is read on the first
/// lookup only and each ID is then one byte search (`radioid::lookup_cached` re-reads the file
/// on every call; parsing all of it would take seconds).
pub struct RadioIdNames {
    path: PathBuf,
    data: Option<Vec<u8>>,
}

impl RadioIdNames {
    /// `None` when the RadioID data has not been downloaded.
    pub fn from_cache(directory: &Path) -> Option<Self> {
        radioid::cache_info(directory).map(|_| RadioIdNames {
            path: directory.join(CSV_NAME),
            data: None,
        })
    }

    pub fn name(&mut self, radio_id: u64) -> String {
        self.find(radio_id).map_or_else(String::new, |user| {
            pystr::strip(&format!("{} {}", user.callsign, user.full_name())).to_owned()
        })
    }

    pub fn find(&mut self, radio_id: u64) -> Option<RadioIdUser> {
        // removed or locked since the dialog opened: no names (they are only a convenience)
        let data = self
            .data
            .get_or_insert_with(|| std::fs::read(&self.path).unwrap_or_default());
        let start = find_row(data, format!("\n{radio_id},").as_bytes())?;
        let row = &data[start + 1..];
        let row = &row[..row.iter().position(|&b| b == b'\n').unwrap_or(row.len())];
        // one row, validated like the whole file
        let text = [EXPECTED_HEADER.join(",").as_bytes(), b"\n", row].concat();
        radioid::parse_csv(&text).ok()?.users.into_iter().next()
    }
}

/// Where `needle` (which starts with a newline) first occurs, as Python's `bytes.find` finds
/// it; only line starts are compared, so the 17 MB file takes one quick pass.
fn find_row(data: &[u8], needle: &[u8]) -> Option<usize> {
    let mut from = 0;
    while let Some(offset) = data[from..].iter().position(|&b| b == b'\n') {
        let at = from + offset;
        if data[at..].starts_with(needle) {
            return Some(at);
        }
        from = at + 1;
    }
    None
}

/// Names the IDs that are not project contacts (e.g. [`RadioIdNames`]); called once per ID.
pub type NameLookup = Box<dyn FnMut(u64) -> String>;

// ---------------------------------------------------------------- the transfer thread

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordingJob {
    List {
        max: usize,
    },
    /// (record, pages) pairs, in list order.
    Download(Vec<(u32, u32)>),
    /// Records 1 ..= min(total, 999) without the list command.
    All,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RecordingEvent {
    Progress {
        done: usize,
        total: usize,
        text: String,
    },
    Listed(Vec<RecordInfo>),
    Saved(Saved),
    Failed(String),
}

struct Run<'r> {
    out_dir: &'r Path,
    ambep2: &'r Path,
    pause: Duration,
    report: &'r Reporter<RecordingEvent>,
    cancel: &'r AtomicBool,
}

impl Run<'_> {
    /// Stop if asked; give the radio a moment back in its command loop between records.
    fn between(&self, index: usize) -> Result<(), RecordingError> {
        if self.cancel.load(atomic::Ordering::SeqCst) {
            return Err(RecordingError::Failed(STOPPED.into()));
        }
        if index > 0 {
            std::thread::sleep(self.pause);
        }
        Ok(())
    }

    /// A recording that cannot be stored is reported and the run goes on with the next one.
    fn save(&self, (info, data): (RecordInfo, Vec<u8>)) {
        match save_recording(self.out_dir, &info, &data, Some(self.ambep2)) {
            Ok(saved) => self.report.send(RecordingEvent::Saved(saved)),
            Err(e) => self.report.send(RecordingEvent::Failed(e.to_string())),
        }
    }

    fn job<T: Transport + ?Sized>(
        &self,
        client: &mut RecordClient<'_, T>,
        job: &RecordingJob,
    ) -> Result<(), RecordingError> {
        match job {
            RecordingJob::List { max } => {
                let infos = client.list_records(*max)?;
                self.report.send(RecordingEvent::Listed(infos));
            }
            RecordingJob::Download(numbers) => {
                for (i, &(number, pages)) in numbers.iter().enumerate() {
                    self.between(i)?;
                    self.save(client.read_record(number, pages)?);
                }
            }
            RecordingJob::All => {
                let (first, data) = client.read_record(1, 1)?;
                let plan = all_records_plan(first.total);
                self.save((first, data));
                for number in plan {
                    self.between(number as usize)?;
                    self.save(client.read_record(number, 1)?);
                }
            }
        }
        Ok(())
    }
}

/// Python's `RecordingWorker.run`; the line is closed before the thread ends.
pub fn run_recording(
    factory: &TransportFactory,
    job: &RecordingJob,
    out_dir: &Path,
    ambep2: &Path,
    pause: Duration,
    report: &Reporter<RecordingEvent>,
    cancel: &AtomicBool,
) {
    let mut transport = match factory() {
        Ok(transport) => transport,
        Err(e) => {
            report.send(RecordingEvent::Failed(e.to_string()));
            return;
        }
    };
    let progress = report.clone();
    let mut client = RecordClient::new(&mut *transport)
        .with_progress(move |done, total, text| {
            progress.send(RecordingEvent::Progress {
                done,
                total,
                text: text.to_owned(),
            });
        })
        .with_cancel(move || cancel.load(atomic::Ordering::SeqCst));
    let run = Run {
        out_dir,
        ambep2,
        pause,
        report,
        cancel,
    };
    let outcome =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run.job(&mut client, job)));
    match outcome {
        Ok(Ok(())) => {}
        Ok(Err(e)) => report.send(RecordingEvent::Failed(e.to_string())),
        Err(payload) => report.send(RecordingEvent::Failed(format!(
            "{UNEXPECTED}{}",
            panic_text(&*payload)
        ))),
    }
}

fn recording_factory(port: &PortChoice) -> Option<TransportFactory> {
    match port {
        PortChoice::None => None,
        // one simulated radio for the whole dialog, as in Python
        PortChoice::Simulated => Some(shared_factory(Arc::new(Mutex::new(
            SimulatedRecorder::default(),
        )))),
        PortChoice::Serial(name) => Some(serial_factory(name.clone())),
    }
}

// ---------------------------------------------------------------- the table

#[derive(Clone, Debug, PartialEq)]
pub struct FileCell {
    pub text: String,
    pub path: PathBuf,
}

/// One table row; rows are kept in display order (as Qt's table does).
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub info: RecordInfo,
    pub checked: bool,
    pub partner: String,
    pub partner_tip: String,
    pub file: Option<FileCell>,
}

impl Row {
    /// The cell texts, 編號 to 檔案.
    pub fn cells(&self) -> [String; 7] {
        [
            self.info.number.to_string(),
            direction_text(&self.info).to_owned(),
            self.info.call_id.to_string(),
            self.partner.clone(),
            length_text(&self.info),
            self.info.time.clone(),
            self.file
                .as_ref()
                .map_or_else(String::new, |f| f.text.clone()),
        ]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum SortKey<'r> {
    Number(u64),
    Text(&'r str),
}

/// 編號, 對方 ID and 長度 sort as numbers; a row without a file has no key in 檔案.
fn sort_key(row: &Row, column: usize) -> Option<SortKey<'_>> {
    Some(match column {
        0 => SortKey::Number(row.info.number),
        1 => SortKey::Text(direction_text(&row.info)),
        2 => SortKey::Number(row.info.call_id),
        3 => SortKey::Text(&row.partner),
        4 => SortKey::Number(row.info.length),
        5 => SortKey::Text(&row.info.time),
        _ => SortKey::Text(&row.file.as_ref()?.text),
    })
}

/// Qt's stable table sort: rows without a key go last, in their current order.
pub fn sort_rows(rows: Vec<Row>, column: usize, ascending: bool) -> Vec<Row> {
    let (mut keyed, rest): (Vec<Row>, Vec<Row>) = rows
        .into_iter()
        .partition(|r| sort_key(r, column).is_some());
    keyed.sort_by(|a, b| {
        let order = sort_key(a, column).cmp(&sort_key(b, column));
        if ascending { order } else { order.reverse() }
    });
    keyed.extend(rest);
    keyed
}

// ---------------------------------------------------------------- the dialog

/// What the dialog is opened with.
pub struct RecordingSetup {
    pub transport: Option<TransportFactory>,
    pub port_label: String,
    pub out_dir: String,
    pub ambep2: String,
    /// The project's contacts (call ID -> name).
    pub names: HashMap<u64, String>,
    pub lookup: Option<NameLookup>,
    pub pause: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    List,
    Download,
}

pub type Player = Box<dyn Fn(&Path) -> std::io::Result<()>>;

pub struct RecordingDialog {
    transport: Option<TransportFactory>,
    port_label: String,
    /// The toolbar's port this dialog follows (None for a dialog built by a test).
    app_port: Option<Option<String>>,
    pub out_dir: String,
    pub ambep2: String,
    pub max_count: usize,
    names: HashMap<u64, String>,
    lookup: Option<NameLookup>,
    looked_up: HashMap<u64, String>,
    pause: Duration,
    /// Another transfer is running (re-read every frame).
    pub port_busy: bool,
    player: Player,
    records: Vec<RecordInfo>,
    rows: Vec<Row>,
    sort: Option<(usize, bool)>,
    selected: Option<u64>,
    saved: Vec<Saved>,
    run_saved: Vec<Saved>,
    error: String,
    status: String,
    progress: Option<(usize, usize)>,
    mode: Mode,
    stopping: bool,
    worker: Option<Worker<RecordingEvent>>,
}

impl RecordingDialog {
    pub fn new(setup: RecordingSetup) -> Self {
        RecordingDialog {
            transport: setup.transport,
            port_label: setup.port_label,
            app_port: None,
            out_dir: setup.out_dir,
            ambep2: setup.ambep2,
            max_count: DEFAULT_LIST_MAX,
            names: setup.names,
            lookup: setup.lookup,
            looked_up: HashMap::new(),
            pause: setup.pause,
            port_busy: false,
            player: Box::new(open_with_system),
            records: Vec::new(),
            rows: Vec::new(),
            sort: None,
            selected: None,
            saved: Vec::new(),
            run_saved: Vec::new(),
            error: String::new(),
            status: String::new(),
            progress: None,
            mode: Mode::List,
            stopping: false,
            worker: None,
        }
    }

    pub fn with_player(self, player: impl Fn(&Path) -> std::io::Result<()> + 'static) -> Self {
        RecordingDialog {
            player: Box::new(player),
            ..self
        }
    }

    pub fn running(&self) -> bool {
        self.worker.is_some()
    }

    /// Ready to transfer: a port, nothing running.
    pub fn ready(&self) -> bool {
        self.transport.is_some() && !self.running()
    }

    pub fn status(&self) -> &str {
        if self.transport.is_none() {
            NO_PORT
        } else {
            &self.status
        }
    }

    pub fn error(&self) -> &str {
        &self.error
    }

    pub fn records(&self) -> &[RecordInfo] {
        &self.records
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn saved(&self) -> &[Saved] {
        &self.saved
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        self.progress
    }

    pub fn summary(&self) -> String {
        if self.records.is_empty() {
            String::new()
        } else {
            total_text(&self.records)
        }
    }

    /// Follow the toolbar's port (a new simulated radio for a new choice).
    pub fn set_port(&mut self, port: Option<String>) {
        let choice = PortChoice::from_setting(port.as_deref());
        self.transport = recording_factory(&choice);
        self.port_label = choice.label();
        self.pause = if choice.is_simulated() {
            Duration::ZERO
        } else {
            PAUSE
        };
        self.app_port = Some(port);
    }

    /// The ticked records in list order, however the table is sorted.
    pub fn checked(&self) -> Vec<RecordInfo> {
        let ticked: Vec<u64> = self
            .rows
            .iter()
            .filter(|r| r.checked)
            .map(|r| r.info.number)
            .collect();
        self.records
            .iter()
            .filter(|i| ticked.contains(&i.number))
            .cloned()
            .collect()
    }

    pub fn set_all_checked(&mut self, checked: bool) {
        for row in &mut self.rows {
            row.checked = checked;
        }
    }

    pub fn set_checked(&mut self, number: u64, checked: bool) {
        if let Some(row) = self.rows.iter_mut().find(|r| r.info.number == number) {
            row.checked = checked;
        }
    }

    /// The table row showing record `number` (rows move when sorted).
    pub fn row_of(&self, number: u64) -> Option<usize> {
        self.rows.iter().position(|r| r.info.number == number)
    }

    pub fn sort(&self) -> Option<(usize, bool)> {
        self.sort
    }

    /// Sort by `column` (what setting the header's sort indicator does).
    pub fn sort_by(&mut self, column: usize, ascending: bool) {
        self.sort = Some((column, ascending));
        self.resort();
    }

    /// A header click: a new column sorts ascending, the same one flips.
    pub fn click_header(&mut self, column: usize) {
        let ascending = match self.sort {
            Some((current, ascending)) if current == column => !ascending,
            _ => true,
        };
        self.sort_by(column, ascending);
    }

    fn resort(&mut self) {
        if let Some((column, ascending)) = self.sort {
            self.rows = sort_rows(std::mem::take(&mut self.rows), column, ascending);
        }
    }

    pub fn select(&mut self, number: Option<u64>) {
        self.selected = number;
    }

    pub fn selected(&self) -> Option<u64> {
        self.selected
    }

    /// (name, tooltip): the project's contact name first, else what the lookup knows.
    fn partner(&mut self, call_id: u64) -> (String, String) {
        if let Some(name) = self.names.get(&call_id).filter(|n| !n.is_empty()) {
            return (name.clone(), String::new());
        }
        let Some(lookup) = self.lookup.as_mut() else {
            return (String::new(), String::new());
        };
        let name = self
            .looked_up
            .entry(call_id)
            .or_insert_with(|| lookup(call_id))
            .clone();
        let tip = if name.is_empty() { "" } else { RADIOID_TIP };
        (name, tip.to_owned())
    }

    /// Add or refresh the row for `info`; a refreshed row keeps its tick and its file.
    fn show_row(&mut self, info: RecordInfo) -> usize {
        let (partner, partner_tip) = self.partner(info.call_id);
        match self.row_of(info.number) {
            Some(at) => {
                let row = &mut self.rows[at];
                row.info = info;
                row.partner = partner;
                row.partner_tip = partner_tip;
                at
            }
            None => {
                self.rows.push(Row {
                    info,
                    checked: true,
                    partner,
                    partner_tip,
                    file: None,
                });
                self.rows.len() - 1
            }
        }
    }

    /// 播放 / a double click: the row's WAV with the system's player.
    pub fn play(&mut self, number: Option<u64>) -> bool {
        let wav = number
            .and_then(|n| self.rows.iter().find(|r| r.info.number == n))
            .and_then(|r| r.file.as_ref())
            .map(|f| f.path.clone())
            .filter(|p| p.to_string_lossy().to_lowercase().ends_with(".wav"));
        let Some(wav) = wav else {
            self.status = NOT_WAV.into();
            return false;
        };
        match (self.player)(&wav) {
            Ok(()) => true,
            Err(e) => {
                self.status = format!("無法開啟 {}：{e}", wav.display());
                false
            }
        }
    }

    // ------------------------------------------------------------ transfers

    fn start(&mut self, ctx: &egui::Context, job: RecordingJob) -> bool {
        if self.running() {
            return false;
        }
        let Some(factory) = self.transport.clone() else {
            return false;
        };
        if self.port_busy {
            self.status = BUSY.into();
            return false;
        }
        let out = pystr::strip(&self.out_dir).to_owned();
        if out.is_empty() {
            self.status = NO_DIR.into();
            return false;
        }
        let ambep2 = PathBuf::from(pystr::strip(&self.ambep2));
        let mode = match job {
            RecordingJob::List { .. } => Mode::List,
            _ => Mode::Download,
        };
        let pause = self.pause;
        let worker = Worker::spawn(ctx, "opendrs-recording", move |report, cancel| {
            run_recording(
                &factory,
                &job,
                Path::new(&out),
                &ambep2,
                pause,
                &report,
                &cancel,
            );
        });
        match worker {
            Ok(worker) => self.worker = Some(worker),
            Err(e) => {
                self.status = format!("{UNEXPECTED}{e}");
                return false;
            }
        }
        self.error.clear();
        self.mode = mode;
        self.run_saved.clear();
        self.stopping = false;
        self.progress = Some((0, 0));
        self.status = "連線中…".into();
        true
    }

    pub fn read_list(&mut self, ctx: &egui::Context) -> bool {
        let max = self.max_count.clamp(1, MAX_RECORDS);
        self.start(ctx, RecordingJob::List { max })
    }

    pub fn download_checked(&mut self, ctx: &egui::Context) -> bool {
        let chosen = self.checked();
        if chosen.is_empty() {
            self.status = NOTHING_CHECKED.into();
            return false;
        }
        let numbers = chosen
            .iter()
            .map(|i| (u32::try_from(i.number).unwrap_or(u32::MAX), i.pages_field()))
            .collect();
        self.start(ctx, RecordingJob::Download(numbers))
    }

    pub fn download_all(&mut self, ctx: &egui::Context) -> bool {
        self.start(ctx, RecordingJob::All)
    }

    pub fn stop(&mut self) {
        if let Some(worker) = &self.worker {
            worker.cancel();
            self.stopping = true;
            self.status = STOPPING.into();
        }
    }

    pub fn stopping(&self) -> bool {
        self.stopping
    }

    /// Take in what the transfer thread reported.
    pub fn poll(&mut self) {
        let Some(worker) = self.worker.as_mut() else {
            return;
        };
        let polled = worker.poll();
        for event in polled.events {
            self.on_event(event);
        }
        if !polled.finished {
            return;
        }
        if let Some(why) = polled.panic {
            self.on_event(RecordingEvent::Failed(format!("{UNEXPECTED}{why}")));
        }
        self.worker = None;
        self.progress = None;
        if self.mode == Mode::Download && self.error.is_empty() && !self.run_saved.is_empty() {
            self.status = downloaded_text(&self.run_saved, &self.out_dir);
        }
        self.stopping = false;
    }

    fn on_event(&mut self, event: RecordingEvent) {
        match event {
            RecordingEvent::Progress { done, total, text } => {
                self.progress = Some((done, total.max(1)));
                if !self.stopping {
                    self.status = text;
                }
            }
            RecordingEvent::Listed(infos) => {
                let total = infos.last().map_or(0, |i| i.total);
                self.rows.clear();
                for info in &infos {
                    self.show_row(info.clone());
                }
                self.records = infos;
                self.resort();
                self.status = listed_text(self.records.len(), total, self.stopping);
            }
            RecordingEvent::Saved(saved) => self.on_saved(saved),
            RecordingEvent::Failed(message) => {
                self.status = message.clone();
                self.error = message;
            }
        }
    }

    fn on_saved(&mut self, saved: Saved) {
        // the radio's line at download time replaces the listed one
        let info = saved.info.clone();
        match self.records.iter_mut().find(|i| i.number == info.number) {
            Some(listed) => *listed = info.clone(),
            None => self.records.push(info.clone()),
        }
        let at = self.show_row(info);
        self.rows[at].file = Some(FileCell {
            text: file_text(&saved),
            path: saved.wav.clone().unwrap_or_else(|| saved.raw.clone()),
        });
        self.resort();
        self.saved.push(saved.clone());
        self.run_saved.push(saved);
    }

    /// Block until the transfer thread ends or `timeout` passes (tests).
    pub fn wait(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        self.poll();
        while self.running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            self.poll();
        }
    }

    /// Closing is refused while a transfer runs.
    pub fn request_close(&mut self) -> bool {
        if self.running() {
            self.status = RUNNING.into();
            return false;
        }
        true
    }
}

// ---------------------------------------------------------------- the main window's side

impl OpenDrsApp {
    /// 讀取錄音…: list and download the radio's call recordings (the vendor Wav.exe's job).
    pub fn recordings(&mut self) {
        if self.windows.recording.is_some() {
            return; // already open; a transfer in progress must never be replaced
        }
        let flashing = self
            .windows
            .firmware
            .as_ref()
            .is_some_and(FirmwareDialog::running);
        if self.transfer_running() || flashing {
            self.warn("讀取錄音", BUSY);
            return;
        }
        let names = self
            .project()
            .contacts
            .iter()
            .map(|c| (u64::from(c.call_id()), c.name(self.codec)))
            .collect();
        let settings = self.settings();
        let out_dir = match settings.recordings_dir.as_str() {
            "" => self.default_recordings_dir(),
            dir => dir.to_owned(),
        };
        let ambep2 = match settings.ambep2_path.as_str() {
            "" => default_ambep2_text(),
            path => path.to_owned(),
        };
        // None until RadioID has been downloaded
        let lookup = RadioIdNames::from_cache(&self.data_dir().join("radioid"))
            .map(|mut names| Box::new(move |id| names.name(id)) as NameLookup);
        let mut dialog = RecordingDialog::new(RecordingSetup {
            transport: None,
            port_label: String::new(),
            out_dir,
            ambep2,
            names,
            lookup,
            pause: PAUSE,
        });
        dialog.set_port(self.radio_port());
        self.windows.recording = Some(dialog);
    }

    fn default_recordings_dir(&self) -> String {
        self.data_dir().join("recordings").display().to_string()
    }

    fn remember_recording_paths(&mut self, out_dir: &str, ambep2: &str) {
        let (dir, exe) = remembered_paths(
            out_dir,
            ambep2,
            &self.default_recordings_dir(),
            &default_ambep2_text(),
        );
        let settings = self.settings();
        if dir != settings.recordings_dir || exe != settings.ambep2_path {
            self.update_settings(|s| {
                s.recordings_dir = dir;
                s.ambep2_path = exe;
            });
        }
    }
}

// ---------------------------------------------------------------- drawing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    PickDir,
    OpenDir,
    PickExe,
    List,
    Download,
    All,
    Stop,
    Play(Option<u64>),
    Close,
}

/// Draw the dialog if it is open.
pub fn show(app: &mut OpenDrsApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.windows.recording.take() else {
        return;
    };
    dialog.poll();
    if !dialog.running() {
        let port = app.radio_port();
        if dialog.app_port.as_ref() != Some(&port) {
            dialog.set_port(port);
        }
    }
    dialog.port_busy = app.transfer_running()
        || app
            .windows
            .firmware
            .as_ref()
            .is_some_and(FirmwareDialog::running);
    if refuse_app_close(ctx, dialog.running()) {
        dialog.status = RUNNING.into();
    }
    if dialog.running() {
        ctx.request_repaint_after(POLL_INTERVAL);
    }
    let mut open = true;
    let mut action = None;
    egui::Window::new("讀取錄音")
        .id(egui::Id::new("recording-dialog"))
        .open(&mut open)
        .default_size([820.0, 600.0])
        .collapsible(false)
        .show(ctx, |ui| action = dialog_ui(ui, &mut dialog));
    if !open {
        action = Some(Action::Close);
    }
    match action {
        Some(Action::Close) if dialog.request_close() => {
            let (out, exe) = (dialog.out_dir.clone(), dialog.ambep2.clone());
            app.remember_recording_paths(&out, &exe);
            return;
        }
        Some(Action::PickDir) => {
            if let Some(dir) = rfd::FileDialog::new()
                .set_title("錄音資料夾")
                .set_directory(&dialog.out_dir)
                .pick_folder()
            {
                dialog.out_dir = dir.display().to_string();
            }
        }
        Some(Action::OpenDir) => {
            if let Err(e) = super::open_folder(Path::new(pystr::strip(&dialog.out_dir))) {
                dialog.status = format!("無法開啟資料夾：{e}");
            }
        }
        Some(Action::PickExe) => {
            let mut picker = rfd::FileDialog::new()
                .set_title("選擇 ambep2.exe")
                .add_filter("程式", &["exe"]);
            if let Some(dir) = Path::new(&dialog.ambep2).parent() {
                picker = picker.set_directory(dir);
            }
            if let Some(path) = picker.pick_file() {
                dialog.ambep2 = path.display().to_string();
            }
        }
        Some(Action::List) => {
            dialog.read_list(ctx);
        }
        Some(Action::Download) => {
            dialog.download_checked(ctx);
        }
        Some(Action::All) => {
            dialog.download_all(ctx);
        }
        Some(Action::Stop) => dialog.stop(),
        Some(Action::Play(number)) => {
            dialog.play(number);
        }
        Some(Action::Close) | None => {}
    }
    app.windows.recording = Some(dialog);
}

fn dialog_ui(ui: &mut egui::Ui, d: &mut RecordingDialog) -> Option<Action> {
    let ready = d.ready();
    let mut action = None;
    let mut set = |a: Action| action = Some(a);
    egui::Grid::new("recording-form")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("連接埠");
            ui.label(if d.port_label.is_empty() {
                NO_PORT_LABEL
            } else {
                &d.port_label
            });
            ui.end_row();
            ui.label("錄音資料夾");
            ui.horizontal(|ui| {
                ui.add_enabled(
                    ready,
                    egui::TextEdit::singleline(&mut d.out_dir).desired_width(440.0),
                );
                if ui.add_enabled(ready, egui::Button::new("選擇…")).clicked() {
                    set(Action::PickDir);
                }
                if ui.button("開啟資料夾").clicked() {
                    set(Action::OpenDir);
                }
            });
            ui.end_row();
            ui.label("ambep2.exe");
            ui.horizontal(|ui| {
                ui.add_enabled(
                    ready,
                    egui::TextEdit::singleline(&mut d.ambep2).desired_width(440.0),
                );
                if ui.add_enabled(ready, egui::Button::new("選擇…")).clicked() {
                    set(Action::PickExe);
                }
            });
            ui.end_row();
        });
    ui.horizontal(|ui| {
        ui.label("最多");
        ui.add_enabled(
            ready,
            egui::DragValue::new(&mut d.max_count).range(1..=MAX_RECORDS),
        );
        if ui
            .add_enabled(ready, egui::Button::new("讀取清單"))
            .clicked()
        {
            set(Action::List);
        }
        if ui
            .add_enabled(ready, egui::Button::new("下載勾選的錄音"))
            .clicked()
        {
            set(Action::Download);
        }
        let all = ui
            .add_enabled(ready, egui::Button::new("依編號全部下載"))
            .on_hover_text(ALL_TIP)
            .on_disabled_hover_text(ALL_TIP);
        if all.clicked() {
            set(Action::All);
        }
        if d.running()
            && ui
                .add_enabled(!d.stopping, egui::Button::new("停止"))
                .clicked()
        {
            set(Action::Stop);
        }
    });
    let reserve = 190.0;
    let height = (ui.available_height() - reserve).max(120.0);
    if let Some(a) = table_ui(ui, d, height) {
        set(a);
    }
    ui.horizontal(|ui| {
        if ui.button("全選").clicked() {
            d.set_all_checked(true);
        }
        if ui.button("全不選").clicked() {
            d.set_all_checked(false);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(d.summary());
        });
    });
    ui.add(egui::Label::new(egui::RichText::new(NOTE).weak()).wrap());
    if let Some((done, total)) = d.progress {
        let bar = if total == 0 {
            egui::ProgressBar::new(0.0).animate(true)
        } else {
            egui::ProgressBar::new(done as f32 / total as f32).show_percentage()
        };
        ui.add(bar);
    }
    ui.add(egui::Label::new(d.status()).wrap());
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("關閉").clicked() {
            set(Action::Close);
        }
        if ui.button("播放").clicked() {
            set(Action::Play(d.selected));
        }
    });
    action
}

fn header_text(column: usize, sort: Option<(usize, bool)>) -> String {
    match sort {
        Some((c, true)) if c == column => format!("{} ▲", HEADERS[column]),
        Some((c, false)) if c == column => format!("{} ▼", HEADERS[column]),
        _ => HEADERS[column].to_owned(),
    }
}

fn table_ui(ui: &mut egui::Ui, d: &mut RecordingDialog, height: f32) -> Option<Action> {
    let mut action = None;
    let mut clicked_header = None;
    let mut select = None;
    let sort = d.sort;
    let selected = d.selected;
    let rows = &mut d.rows;
    TableBuilder::new(ui)
        .id_salt("recording-table")
        .striped(true)
        .sense(egui::Sense::click())
        .min_scrolled_height(height)
        .max_scroll_height(height)
        .columns(Column::auto().resizable(true), FILE_COL)
        .column(Column::remainder())
        .header(22.0, |mut header| {
            for column in 0..HEADERS.len() {
                header.col(|ui| {
                    if ui
                        .add(egui::Button::new(header_text(column, sort)).frame(false))
                        .clicked()
                    {
                        clicked_header = Some(column);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(20.0, rows.len(), |mut table_row| {
                let row = &mut rows[table_row.index()];
                let number = row.info.number;
                table_row.set_selected(selected == Some(number));
                let cells = row.cells();
                table_row.col(|ui| {
                    ui.checkbox(&mut row.checked, &cells[0]);
                });
                for (column, text) in cells.iter().enumerate().skip(1) {
                    table_row.col(|ui| {
                        let label = ui.add(egui::Label::new(text).selectable(false));
                        if column == 3 && !row.partner_tip.is_empty() {
                            label.on_hover_text(&row.partner_tip);
                        }
                    });
                }
                let response = table_row.response();
                if response.double_clicked() {
                    select = Some(number);
                    action = Some(Action::Play(Some(number)));
                } else if response.clicked() {
                    select = Some(number);
                }
            });
        });
    if let Some(column) = clicked_header {
        d.click_header(column);
    }
    if let Some(number) = select {
        d.select(Some(number));
    }
    action
}
