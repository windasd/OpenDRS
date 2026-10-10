//! 韌體升級 (port of `src/opendrs/gui/firmware_dialog.py` and `iap_worker.py`): pick a firmware
//! file, check it against the radio, then flash it through the IAP bootloader (built in,
//! `opendrs_firmware::iap`) or start the vendor IAP tool with the file path on the clipboard.
//!
//! Hardware safety (PORTING.md 3.2; flashing the wrong image can leave the radio unbootable):
//! * no findings count as BLOCK ([`allowed`]; `verdict(&[])` alone would say OK);
//! * WARN needs the first tick, which can only be given while the common checks have no BLOCK
//!   and is cleared whenever another file is chosen ([`gate`], [`FirmwareDialog::set_file`]);
//! * 開始升級 also needs the second tick (the radio is in IAP mode on the port shown), which is
//!   cleared after every run and whenever the toolbar's port changes;
//! * the file's SHA-256 is computed again at the start and exactly the checked bytes are sent;
//! * a last question whose default answer is 否;
//! * the window (and the main window) cannot be closed while the upgrade runs;
//! * the record-read patch is saved to a new file, never over the vendor file.
//!
//! The port value `__sim__` (the toolbar's 模擬對講機（試用）) or `sim` selects the simulators.
//! This file also holds what the 讀取錄音 dialog shares with it: the port choice, the transport
//! factory and the background worker. (It is over the 800-line guideline because the port was
//! limited to this file and `recording.rs`; the shared part and the gating texts can move to
//! their own modules once `dialogs/mod.rs` may change.)
//!
//! The main window should treat [`OpenDrsApp::hardware_dialog_running`] like its own transfer:
//! no read or write while an upgrade or a recording download runs.

use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use eframe::egui;
use opendrs_core::pystr;
use opendrs_core::radio_link::{self, RadioError, SerialTransport, Transport};
use opendrs_firmware::firmware_check::{
    Finding, FirmwareFile, Level, check, default_iap_exe, inspect, verdict,
};
use opendrs_firmware::fwpatch::{self, RecordReadStatus};
use opendrs_firmware::iap::{IapClient, IapImage, IapResult};
use opendrs_firmware::iap_sim::SimulatedBootloader;
use opendrs_firmware::sha256_hex;

use super::recording::RecordingDialog;
use crate::app::OpenDrsApp;

// ---------------------------------------------------------------- shared with 讀取錄音

/// Port values that mean 「模擬對講機（試用）」 (the GUI's entry, and the CLI's `sim`).
pub const SIMULATED_PORTS: [&str; 2] = ["__sim__", "sim"];
pub const SIMULATED_LABEL: &str = "模擬對講機（試用）";
pub const NO_PORT_LABEL: &str = "未選擇（請在工具列選擇寫頻線）";
/// Python's prefix for anything unexpected in a background transfer.
pub const UNEXPECTED: &str = "未預期的錯誤：";
/// How often a dialog looks at its worker while it runs.
pub const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The cable's port as the toolbar chose it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortChoice {
    None,
    Simulated,
    Serial(String),
}

impl PortChoice {
    pub fn from_setting(port: Option<&str>) -> Self {
        match port {
            None | Some("") => PortChoice::None,
            Some(p) if SIMULATED_PORTS.contains(&p) => PortChoice::Simulated,
            Some(p) => PortChoice::Serial(p.to_owned()),
        }
    }

    pub fn is_simulated(&self) -> bool {
        *self == PortChoice::Simulated
    }

    /// What the dialogs show next to 連接埠, as the toolbar's entry reads ("" when none).
    pub fn label(&self) -> String {
        match self {
            PortChoice::None => String::new(),
            PortChoice::Simulated => SIMULATED_LABEL.into(),
            PortChoice::Serial(name) => serial_label(name),
        }
    }
}

/// 「{device} — {description}」 (listing the ports never opens one), else the bare name.
fn serial_label(name: &str) -> String {
    radio_link::list_ports()
        .ok()
        .and_then(|ports| ports.into_iter().find(|p| p.device == name))
        .map_or_else(
            || name.to_owned(),
            |p| format!("{} — {}", p.device, p.description),
        )
}

pub type BoxedTransport = Box<dyn Transport + Send>;
/// Opens the line for one run, on the worker thread (Python: the transport factory).
pub type TransportFactory = Arc<dyn Fn() -> Result<BoxedTransport, RadioError> + Send + Sync>;

/// A real serial port, opened only when a run starts.
pub fn serial_factory(name: String) -> TransportFactory {
    Arc::new(move || Ok(Box::new(SerialTransport::open(&name)?) as BoxedTransport))
}

/// One simulated device used by every run of a dialog (and inspected by tests).
pub struct SharedTransport<T>(pub Arc<Mutex<T>>);

impl<T: Transport> SharedTransport<T> {
    fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        // a simulator has no state worth refusing over after a panic elsewhere
        f(&mut self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl<T: Transport> Transport for SharedTransport<T> {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.with(|t| t.write(data))
    }

    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError> {
        self.with(|t| t.read_exact(n, timeout))
    }

    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        self.with(|t| t.read_idle(idle, max_wait))
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.with(Transport::reset_input)
    }
}

pub fn shared_factory<T: Transport + Send + 'static>(device: Arc<Mutex<T>>) -> TransportFactory {
    Arc::new(move || Ok(Box::new(SharedTransport(Arc::clone(&device))) as BoxedTransport))
}

/// The worker's end of the channel: every message also wakes the UI.
pub struct Reporter<E> {
    tx: Sender<E>,
    ctx: egui::Context,
}

impl<E> Clone for Reporter<E> {
    fn clone(&self) -> Self {
        Reporter {
            tx: self.tx.clone(),
            ctx: self.ctx.clone(),
        }
    }
}

impl<E> Reporter<E> {
    pub fn send(&self, event: E) {
        // a closed channel means the dialog is gone: there is nobody left to tell
        let _ = self.tx.send(event);
        self.ctx.request_repaint();
    }
}

/// What [`Worker::poll`] found.
pub struct Polled<E> {
    pub events: Vec<E>,
    pub finished: bool,
    /// The thread panicked outside the guarded transfer (its message).
    pub panic: Option<String>,
}

/// A transfer on its own thread, reporting through a channel; Stop sets the cancel flag that
/// the clients' `with_cancel` reads.
pub struct Worker<E> {
    events: Receiver<E>,
    cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl<E: Send + 'static> Worker<E> {
    pub fn spawn(
        ctx: &egui::Context,
        name: &str,
        job: impl FnOnce(Reporter<E>, Arc<AtomicBool>) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (tx, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let reporter = Reporter {
            tx,
            ctx: ctx.clone(),
        };
        let flag = Arc::clone(&cancel);
        let handle = std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || job(reporter, flag))?;
        Ok(Worker {
            events,
            cancel,
            handle: Some(handle),
        })
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Everything reported so far; `finished` once the thread has ended (its port closed).
    pub fn poll(&mut self) -> Polled<E> {
        // looked at before draining, so a finished thread's last messages are all collected
        let finished = self.handle.as_ref().is_none_or(JoinHandle::is_finished);
        let events = self.events.try_iter().collect();
        let panic = if finished {
            self.handle
                .take()
                .and_then(|h| h.join().err())
                .map(|payload| panic_text(&*payload))
        } else {
            None
        };
        Polled {
            events,
            finished,
            panic,
        }
    }
}

pub fn panic_text(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "背景工作意外結束".into())
}

/// Keep the main window open while a transfer runs (the process would kill the thread).
pub fn refuse_app_close(ctx: &egui::Context, running: bool) -> bool {
    let requested = running && ctx.input(|i| i.viewport().close_requested());
    if requested {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    }
    requested
}

/// `Path.resolve()` (also for a file that does not exist yet).
fn resolved(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    match (absolute.parent(), absolute.file_name()) {
        (Some(parent), Some(name)) => {
            std::fs::canonicalize(parent).map_or_else(|_| absolute.clone(), |p| p.join(name))
        }
        _ => absolute,
    }
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    resolved(a) == resolved(b)
}

/// `f"{n:,}"`.
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

// ---------------------------------------------------------------- texts and gating

pub const STEPS: &str = "內建升級：依原廠說明讓對講機進入升級（IAP）模式，勾選上方確認後按「開始升級」；\
完成前不可拔線或關機。\n原廠工具：選擇 COM 埠並按「Open」→ 按「Open APP File」，\
在檔名欄按 Ctrl+V 貼上韌體路徑 → 讓對講機進入升級模式後按「IAP」。";
pub const VERIFIED: &str = "內建升級已在實機驗證：2026-10-09 於 HORA DH-98VU（Kydera DR-8500UV 貼牌，BK4819）\
刷入原廠韌體與錄音讀取修補版各一次，皆成功；送出的位元組與原廠 IAP 工具逐一相同";
pub const BUSY: &str = "本程式正在與對講機傳輸，請等完成後再升級";
pub const NO_PORT: &str = "內建升級：請先在主視窗工具列選擇寫頻線的連接埠";
pub const NO_TOOL: &str = "原廠工具：找不到 IAP+Flash.exe，請按「選擇…」指定";
pub const NO_FILE: &str = "請先選擇韌體檔";
pub const CONFIRM: &str = "我了解刷錯韌體可能使對講機無法開機";
pub const IAP_MODE: &str = "對講機已依原廠說明進入升級（IAP）模式，並以寫頻線連接到上面的連接埠";
pub const RUNNING: &str = "升級進行中，請等完成或按「停止」。";
pub const STOPPING: &str = "停止中（送完目前這一頁或停止等待回應）…";
pub const CHANGED: &str = "韌體檔在檢查後被修改過，已重新檢查，請確認結果後再開始。";
pub const KEEP_VENDOR: &str = "請另存新檔，不要覆蓋原廠韌體（復原時需要它）。";
pub const PATCH_TIP: &str =
    "把原廠韌體改 30 bytes，讓對講機能從電腦讀取錄音（已在實機驗證；刷回原廠檔即可復原）";
pub const FILE_HINT: &str = "選擇要刷入的韌體檔（*.bin）";

fn finding(level: Level, text: &str) -> Finding {
    Finding {
        level,
        text: text.to_owned(),
    }
}

/// Checks on the file and the radio; BLOCK stops both ways of flashing.
pub fn common_findings(
    image: Option<&FirmwareFile>,
    radio_firmware: &str,
    radio_date: &str,
    has_backup: bool,
    port_busy: bool,
) -> Vec<Finding> {
    let mut findings = image.map_or_else(Vec::new, |i| {
        check(i, radio_firmware, has_backup, radio_date)
    });
    if port_busy {
        findings.push(finding(Level::Block, BUSY));
    }
    findings
}

pub fn builtin_findings(has_port: bool) -> Vec<Finding> {
    if has_port {
        vec![finding(Level::Ok, VERIFIED)]
    } else {
        vec![finding(Level::Block, NO_PORT)]
    }
}

pub fn tool_findings(exe: &str) -> Vec<Finding> {
    if Path::new(exe).is_file() {
        Vec::new()
    } else {
        vec![finding(Level::Block, NO_TOOL)]
    }
}

/// A file is chosen and the findings allow flashing: OK, or WARN with the first tick. No
/// findings at all count as BLOCK.
pub fn allowed(has_image: bool, findings: &[Finding], confirmed: bool) -> bool {
    let level = if findings.is_empty() {
        Level::Block
    } else {
        verdict(findings)
    };
    has_image && (level == Level::Ok || (level == Level::Warn && confirmed))
}

/// Which controls are usable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Gate {
    pub confirm_enabled: bool,
    pub iap_mode_enabled: bool,
    pub flash_enabled: bool,
    pub launch_enabled: bool,
    pub browse_enabled: bool,
    pub make_patch_enabled: bool,
    pub stop_visible: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct GateInput<'a> {
    pub has_image: bool,
    pub common: &'a [Finding],
    pub builtin: &'a [Finding],
    pub tool: &'a [Finding],
    pub confirmed: bool,
    pub iap_mode: bool,
    pub running: bool,
    pub patchable: bool,
}

pub fn gate(input: &GateInput<'_>) -> Gate {
    let all = || input.common.iter().chain(input.builtin).chain(input.tool);
    let needs_confirm = all().any(|f| f.level == Level::Warn);
    let common_blocked = input.common.iter().any(|f| f.level == Level::Block);
    let with = |extra: &[Finding]| {
        let findings: Vec<Finding> = input.common.iter().chain(extra).cloned().collect();
        allowed(input.has_image, &findings, input.confirmed)
    };
    let idle = !input.running;
    Gate {
        confirm_enabled: idle && needs_confirm && !common_blocked,
        iap_mode_enabled: idle,
        flash_enabled: idle && input.iap_mode && with(input.builtin),
        launch_enabled: idle && with(input.tool),
        browse_enabled: idle,
        make_patch_enabled: idle && input.has_image && input.patchable,
        stop_visible: input.running,
    }
}

pub fn radio_text(firmware: &str, date: &str) -> String {
    match (firmware.is_empty(), date.is_empty()) {
        (false, false) => format!("{firmware}（{date}）"),
        (false, true) => firmware.to_owned(),
        (true, _) => "未知（尚未讀取對講機）".into(),
    }
}

pub fn file_info_text(image: &FirmwareFile, status: Option<&RecordReadStatus>) -> String {
    let note = match status {
        Some(RecordReadStatus::Patched) => "\n錄音讀取版（可從電腦讀取錄音）",
        Some(RecordReadStatus::Patchable) => "\n可按「產生錄音讀取版…」做出能從電腦讀取錄音的版本",
        _ => "",
    };
    let id = if image.firmware_id.is_empty() {
        "（找不到韌體 ID）"
    } else {
        &image.firmware_id
    };
    format!(
        "{id}　{}　{} bytes\nSHA-256 {}…{note}",
        image.build_date,
        thousands(image.size),
        prefix16(&image.sha256)
    )
}

fn prefix16(text: &str) -> &str {
    text.get(..16).unwrap_or(text)
}

pub fn confirm_question(file_name: &str, pages: usize) -> String {
    format!(
        "即將清除對講機的韌體並寫入 {file_name}（{pages} 頁）。\n\
         開始後不可拔線或關機；若中途失敗，對講機會停在升級模式，可重新執行升級。\n\n要開始嗎？"
    )
}

pub fn success_text(result: &IapResult) -> String {
    let reply = pystr::split(&result.erase_reply)
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "升級完成：{} 頁，對講機確認校驗和 0x{:08X}（清除回覆：{reply}）。\
         對講機應會自行重新啟動，之後請重新「讀取對講機」確認韌體版本。",
        result.pages, result.checksum
    )
}

pub fn failure_text(message: &str, erase_sent: bool) -> String {
    let head = if erase_sent {
        "升級未完成，"
    } else {
        "升級未開始，"
    };
    format!("{head}{message}")
}

pub fn launch_note(launched: bool) -> &'static str {
    if launched {
        "\n已啟動 IAP 工具；韌體路徑已複製到剪貼簿。"
    } else {
        "\n無法啟動 IAP 工具。"
    }
}

/// Settings value for the IAP tool on close: `None` = leave it (nothing typed, or unchanged).
pub fn remembered_iap(chosen: &str, default: &str, current: &str) -> Option<String> {
    let chosen = pystr::strip(chosen);
    if chosen.is_empty() {
        return None;
    }
    let value = if chosen == default { "" } else { chosen };
    (value != current).then(|| value.to_owned())
}

/// The vendor tool's default location as text ("" where there is none).
pub fn default_iap_text() -> String {
    default_iap_exe().map_or_else(String::new, |p| p.display().to_string())
}

/// A backup read from the radio today (before_write counts too).
pub fn has_backup_from(folder: &Path, today: &str) -> bool {
    std::fs::read_dir(folder).is_ok_and(|entries| {
        entries.flatten().any(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            name.starts_with(today)
                && path
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("F136"))
                && path.is_file()
        })
    })
}

// ---------------------------------------------------------------- the upgrade thread

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IapEvent {
    Progress {
        done: usize,
        total: usize,
        text: String,
    },
    Succeeded(IapResult),
    Failed {
        message: String,
        erase_sent: bool,
    },
}

/// Python's `IapWorker.run`: open the line, upgrade, report; the line is closed before the
/// thread ends.
pub fn run_upgrade(
    factory: &TransportFactory,
    image: &IapImage,
    report: &Reporter<IapEvent>,
    cancel: &AtomicBool,
) {
    let mut transport = match factory() {
        Ok(transport) => transport,
        Err(e) => {
            report.send(IapEvent::Failed {
                message: e.to_string(),
                erase_sent: false,
            });
            return;
        }
    };
    let progress = report.clone();
    let mut client = IapClient::new(&mut *transport)
        .with_progress(move |done, total, text| {
            progress.send(IapEvent::Progress {
                done,
                total,
                text: text.to_owned(),
            });
        })
        .with_cancel(|| cancel.load(Ordering::SeqCst));
    let outcome = panic::catch_unwind(AssertUnwindSafe(|| client.upgrade(image)));
    let event = match outcome {
        Ok(Ok(result)) => IapEvent::Succeeded(result),
        Ok(Err(e)) => IapEvent::Failed {
            message: e.message,
            erase_sent: e.erase_sent,
        },
        Err(payload) => IapEvent::Failed {
            message: format!("{UNEXPECTED}{}", panic_text(&*payload)),
            erase_sent: client.erase_sent(),
        },
    };
    report.send(event);
}

fn iap_factory(port: &PortChoice) -> Option<TransportFactory> {
    match port {
        PortChoice::None => None,
        // a fresh bootloader for every run, like Python's `SimulatedBootloader` factory
        PortChoice::Simulated => Some(Arc::new(|| {
            Ok(Box::new(SimulatedBootloader::default()) as BoxedTransport)
        })),
        PortChoice::Serial(name) => Some(serial_factory(name.clone())),
    }
}

/// Starts the vendor tool in its own folder; true when it started.
pub type Launcher = Box<dyn Fn(&Path, &Path) -> bool>;

pub fn start_detached(exe: &Path, workdir: &Path) -> bool {
    // the child is not waited for: it keeps running on its own
    Command::new(exe)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

// ---------------------------------------------------------------- the dialog

/// What the dialog is opened with.
pub struct FirmwareSetup {
    pub radio_firmware: String,
    pub radio_date: String,
    pub has_recent_backup: bool,
    pub iap_exe: String,
    pub port_busy: bool,
    pub port_label: String,
    pub transport: Option<TransportFactory>,
}

/// The open 開始升級 question: exactly the bytes that were checked.
struct Question {
    image: IapImage,
    sha256: String,
    text: String,
    focused: bool,
}

pub struct FirmwareDialog {
    radio_firmware: String,
    radio_date: String,
    has_backup: bool,
    /// Another transfer is running (re-read every frame).
    pub port_busy: bool,
    /// The project revision the radio fields were taken from (the window is not modal, so the
    /// project can change while it is open).
    radio_revision: Option<u64>,
    port_label: String,
    /// The toolbar's port this dialog follows (None for a dialog built by a test).
    app_port: Option<Option<String>>,
    transport: Option<TransportFactory>,
    launcher: Launcher,
    pub exe: String,
    file: String,
    file_info: String,
    image: Option<FirmwareFile>,
    patch_status: Option<RecordReadStatus>,
    pub confirm: bool,
    pub iap_mode: bool,
    status: String,
    progress: Option<(usize, usize)>,
    worker: Option<Worker<IapEvent>>,
    stopping: bool,
    progress_seen: bool,
    question: Option<Question>,
    upgrade_result: Option<IapResult>,
    error: String,
    launched: bool,
}

impl FirmwareDialog {
    pub fn new(setup: FirmwareSetup) -> Self {
        FirmwareDialog {
            radio_firmware: setup.radio_firmware,
            radio_date: setup.radio_date,
            has_backup: setup.has_recent_backup,
            port_busy: setup.port_busy,
            radio_revision: None,
            port_label: setup.port_label,
            app_port: None,
            transport: setup.transport,
            launcher: Box::new(start_detached),
            exe: setup.iap_exe,
            file: String::new(),
            file_info: "—".into(),
            image: None,
            patch_status: None,
            confirm: false,
            iap_mode: false,
            status: String::new(),
            progress: None,
            worker: None,
            stopping: false,
            progress_seen: false,
            question: None,
            upgrade_result: None,
            error: String::new(),
            launched: false,
        }
    }

    pub fn with_launcher(self, launcher: impl Fn(&Path, &Path) -> bool + 'static) -> Self {
        FirmwareDialog {
            launcher: Box::new(launcher),
            ..self
        }
    }

    pub fn running(&self) -> bool {
        self.worker.is_some()
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn file(&self) -> &str {
        &self.file
    }

    pub fn file_info(&self) -> &str {
        &self.file_info
    }

    pub fn image(&self) -> Option<&FirmwareFile> {
        self.image.as_ref()
    }

    pub fn progress(&self) -> Option<(usize, usize)> {
        self.progress
    }

    pub fn upgrade_result(&self) -> Option<&IapResult> {
        self.upgrade_result.as_ref()
    }

    pub fn error(&self) -> &str {
        &self.error
    }

    pub fn launched(&self) -> bool {
        self.launched
    }

    /// The 開始升級 question is waiting for an answer.
    pub fn asking(&self) -> Option<&str> {
        self.question.as_ref().map(|q| q.text.as_str())
    }

    /// Follow the toolbar's port. A different port voids the IAP-mode tick, which was given for
    /// the radio on the old one.
    pub fn set_port(&mut self, port: Option<String>) {
        let choice = PortChoice::from_setting(port.as_deref());
        self.transport = iap_factory(&choice);
        self.port_label = choice.label();
        self.app_port = Some(port);
        self.iap_mode = false;
    }

    /// Follow the project's radio. The checks compare the file with that radio, so another
    /// radio (or a backup appearing or disappearing) voids the ticks given for the old findings.
    pub fn set_radio(&mut self, firmware: &str, date: &str, has_backup: bool) {
        if self.radio_firmware != firmware
            || self.radio_date != date
            || self.has_backup != has_backup
        {
            self.radio_firmware = firmware.to_owned();
            self.radio_date = date.to_owned();
            self.has_backup = has_backup;
            self.confirm = false;
            self.iap_mode = false;
        }
    }

    fn patchable(&self) -> bool {
        self.patch_status == Some(RecordReadStatus::Patchable)
    }

    /// (common, built-in, vendor tool) findings, in the order the list shows them.
    pub fn findings(&self) -> (Vec<Finding>, Vec<Finding>, Vec<Finding>) {
        (
            common_findings(
                self.image.as_ref(),
                &self.radio_firmware,
                &self.radio_date,
                self.has_backup,
                self.port_busy,
            ),
            builtin_findings(self.transport.is_some()),
            tool_findings(&self.exe),
        )
    }

    pub fn gate(&self) -> Gate {
        let (common, builtin, tool) = self.findings();
        gate(&GateInput {
            has_image: self.image.is_some(),
            common: &common,
            builtin: &builtin,
            tool: &tool,
            confirmed: self.confirm,
            iap_mode: self.iap_mode,
            running: self.running(),
            patchable: self.patchable(),
        })
    }

    /// Choose the file to flash. The risk tick covers only the warnings of the file it was given
    /// for, so it is cleared.
    pub fn set_file(&mut self, path: &Path) {
        self.file = path.display().to_string();
        self.confirm = false;
        self.patch_status = None;
        match inspect(path) {
            Ok(image) => {
                self.patch_status = std::fs::read(path)
                    .ok()
                    .map(|data| fwpatch::record_read_status(&data));
                self.file_info = file_info_text(&image, self.patch_status.as_ref());
                self.image = Some(image);
            }
            Err(e) => {
                self.image = None;
                self.file_info = format!("無法讀取：{e}");
            }
        }
    }

    /// Write the record-read version of the chosen vendor file to `out` (asks when `None`) and
    /// select it. Never over the vendor file.
    pub fn make_record_read(&mut self, out: Option<PathBuf>) -> Option<PathBuf> {
        if !self.patchable() || self.running() {
            return None;
        }
        let source = self.image.as_ref()?.path.clone();
        let patched = std::fs::read(&source)
            .map_err(|e| e.to_string())
            .and_then(|data| fwpatch::patch_record_read(&data).map_err(|e| e.to_string()));
        let result = match patched {
            Ok(result) => result,
            Err(e) => {
                self.status = format!("無法產生錄音讀取版：{e}");
                return None;
            }
        };
        let out = match out {
            Some(out) => out,
            None => pick_patch_target(&source)?,
        };
        if same_file(&out, &source) {
            self.status = KEEP_VENDOR.into();
            return None;
        }
        if let Err(e) = std::fs::write(&out, &result.data) {
            self.status = format!("無法寫入 {}：{e}", out.display());
            return None;
        }
        self.set_file(&out);
        self.status = format!(
            "已產生錄音讀取版：{}（改 {} bytes，SHA-256 {}…）。要復原時再刷一次原廠檔 {}。",
            file_name(&out),
            result.changed,
            prefix16(&result.sha256),
            file_name(&source)
        );
        Some(out)
    }

    /// 開始升級: re-check the file, then ask (default 否). True when the question is open.
    pub fn start_flash(&mut self) -> bool {
        if !self.gate().flash_enabled || self.transport.is_none() || self.question.is_some() {
            return false;
        }
        let Some(checked) = self.image.clone() else {
            return false;
        };
        let read = std::fs::read(&checked.path).map_err(|e| e.to_string());
        let (data, image) =
            match read.and_then(|d| IapImage::from_bytes(&d).map(|i| (d, i)).map_err(|e| e.0)) {
                Ok(pair) => pair,
                Err(e) => {
                    self.status = format!("無法讀取韌體檔：{e}");
                    return false;
                }
            };
        let sha256 = sha256_hex(&data);
        if sha256 != checked.sha256 {
            self.set_file(&checked.path);
            self.status = CHANGED.into();
            return false;
        }
        self.question = Some(Question {
            text: confirm_question(&file_name(&checked.path), image.page_count()),
            image,
            sha256,
            focused: false,
        });
        true
    }

    /// The answer to 開始升級; 是 starts the upgrade thread. True when it started.
    pub fn answer(&mut self, yes: bool, ctx: &egui::Context) -> bool {
        let Some(question) = self.question.take() else {
            return false;
        };
        // nothing may have changed since the question was asked
        let same_file = self
            .image
            .as_ref()
            .is_some_and(|i| i.sha256 == question.sha256);
        if !yes || !same_file || !self.gate().flash_enabled {
            return false;
        }
        let Some(factory) = self.transport.clone() else {
            return false;
        };
        let pages = question.image.page_count();
        let image = question.image;
        let worker = Worker::spawn(ctx, "opendrs-iap", move |report, cancel| {
            run_upgrade(&factory, &image, &report, &cancel);
        });
        match worker {
            Ok(worker) => self.worker = Some(worker),
            Err(e) => {
                self.status = format!("{UNEXPECTED}{e}");
                return false;
            }
        }
        self.upgrade_result = None;
        self.error.clear();
        self.stopping = false;
        self.progress_seen = false;
        self.progress = Some((0, pages));
        self.status = "連線中…".into();
        true
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

    /// Take in what the upgrade thread reported.
    pub fn poll(&mut self) {
        let Some(worker) = self.worker.as_mut() else {
            return;
        };
        let polled = worker.poll();
        let mut reported = false;
        for event in polled.events {
            reported |= !matches!(event, IapEvent::Progress { .. });
            self.on_event(event);
        }
        if !polled.finished {
            return;
        }
        if !reported && self.upgrade_result.is_none() && self.error.is_empty() {
            let why = polled.panic.unwrap_or_else(|| "背景工作意外結束".into());
            // the erase command goes out right after the first progress message
            self.on_event(IapEvent::Failed {
                message: format!("{UNEXPECTED}{why}"),
                erase_sent: self.progress_seen,
            });
        }
        self.worker = None;
        self.progress = self
            .upgrade_result
            .as_ref()
            .and(self.progress.or(Some((1, 1))));
        self.iap_mode = false; // the radio has left (or may have left) IAP mode
        self.stopping = false;
    }

    fn on_event(&mut self, event: IapEvent) {
        match event {
            IapEvent::Progress { done, total, text } => {
                self.progress_seen = true;
                self.progress = Some((done, total.max(1)));
                if !self.stopping {
                    self.status = text;
                }
            }
            IapEvent::Succeeded(result) => {
                self.status = success_text(&result);
                self.upgrade_result = Some(result);
            }
            IapEvent::Failed {
                message,
                erase_sent,
            } => {
                self.status = failure_text(&message, erase_sent);
                self.error = message;
            }
        }
    }

    /// Block until the upgrade thread ends or `timeout` passes (tests).
    pub fn wait(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        self.poll();
        while self.running() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            self.poll();
        }
    }

    /// Closing is refused while the upgrade runs.
    pub fn request_close(&mut self) -> bool {
        if self.running() {
            self.status = RUNNING.into();
            return false;
        }
        true
    }

    /// 改用原廠 IAP 工具: the firmware path on the clipboard, the tool started in its folder.
    pub fn start_tool(&mut self, ctx: &egui::Context) -> bool {
        if !self.gate().launch_enabled {
            return false;
        }
        let Some(image) = &self.image else {
            return false;
        };
        ctx.copy_text(image.path.display().to_string());
        let exe = PathBuf::from(&self.exe);
        let workdir = exe
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        self.launched = (self.launcher)(&exe, &workdir);
        self.file_info.push_str(launch_note(self.launched));
        self.launched
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

fn pick_patch_target(source: &Path) -> Option<PathBuf> {
    let stem = source
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let mut dialog = rfd::FileDialog::new()
        .set_title("儲存錄音讀取版韌體")
        .set_file_name(format!("{stem}_record_read.bin"))
        .add_filter("韌體", &["bin"]);
    if let Some(dir) = source.parent() {
        dialog = dialog.set_directory(dir);
    }
    dialog.save_file()
}

// ---------------------------------------------------------------- the main window's side

impl OpenDrsApp {
    /// 韌體升級（IAP）…: pre-flight checks, then the built-in upgrade or the vendor tool.
    pub fn firmware_update(&mut self) {
        if self.windows.firmware.is_some() {
            return; // already open; an upgrade in progress must never be replaced
        }
        let device = self.project().device();
        let exe = match self.settings().iap_path.as_str() {
            "" => default_iap_text(),
            path => path.to_owned(),
        };
        let today = chrono::Local::now().format("%Y%m%d").to_string();
        let mut dialog = FirmwareDialog::new(FirmwareSetup {
            radio_firmware: device.firmware,
            radio_date: device.firmware_date,
            has_recent_backup: has_backup_from(&self.data_dir().join("backups"), &today),
            iap_exe: exe,
            port_busy: self.firmware_port_busy(),
            port_label: String::new(),
            transport: None,
        });
        dialog.set_port(self.radio_port());
        self.windows.firmware = Some(dialog);
    }

    /// A firmware upgrade or a recording transfer is running in one of the hardware dialogs
    /// (the main window must not start a read or write meanwhile).
    pub fn hardware_dialog_running(&self) -> bool {
        self.windows
            .firmware
            .as_ref()
            .is_some_and(FirmwareDialog::running)
            || self
                .windows
                .recording
                .as_ref()
                .is_some_and(RecordingDialog::running)
    }

    fn firmware_port_busy(&self) -> bool {
        self.transfer_running()
            || self
                .windows
                .recording
                .as_ref()
                .is_some_and(RecordingDialog::running)
    }

    fn remember_iap(&mut self, chosen: &str) {
        if let Some(value) = remembered_iap(chosen, &default_iap_text(), &self.settings().iap_path)
        {
            self.update_settings(|s| s.iap_path = value);
        }
    }
}

// ---------------------------------------------------------------- drawing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    PickFile,
    PickExe,
    MakePatch,
    Flash,
    Stop,
    Launch,
    Close,
}

/// Draw the dialog if it is open.
pub fn show(app: &mut OpenDrsApp, ctx: &egui::Context) {
    let Some(mut dialog) = app.windows.firmware.take() else {
        return;
    };
    dialog.poll();
    if !dialog.running() && dialog.question.is_none() {
        let port = app.radio_port();
        if dialog.app_port.as_ref() != Some(&port) {
            dialog.set_port(port);
        }
    }
    if !dialog.running() && dialog.radio_revision != Some(app.doc.revision()) {
        let device = app.project().device();
        let today = chrono::Local::now().format("%Y%m%d").to_string();
        let has_backup = has_backup_from(&app.data_dir().join("backups"), &today);
        dialog.set_radio(&device.firmware, &device.firmware_date, has_backup);
        dialog.radio_revision = Some(app.doc.revision());
    }
    dialog.port_busy = app.firmware_port_busy();
    if refuse_app_close(ctx, dialog.running()) {
        dialog.status = RUNNING.into();
    }
    if dialog.running() {
        ctx.request_repaint_after(POLL_INTERVAL);
    }
    let mut open = true;
    let mut action = None;
    egui::Window::new("韌體升級")
        .id(egui::Id::new("firmware-dialog"))
        .open(&mut open)
        .default_size([660.0, 600.0])
        .collapsible(false)
        .show(ctx, |ui| action = dialog_ui(ui, &mut dialog));
    if !open {
        action = Some(Action::Close);
    }
    if let Some(answer) = question_ui(ctx, &mut dialog) {
        dialog.answer(answer, ctx);
    }
    match action {
        Some(Action::Close) if dialog.request_close() => {
            app.remember_iap(&dialog.exe.clone());
            return;
        }
        Some(Action::PickFile) => {
            if let Some(path) = rfd::FileDialog::new()
                .set_title("選擇韌體檔")
                .add_filter("韌體", &["bin"])
                .add_filter("所有檔案", &["*"])
                .pick_file()
            {
                dialog.set_file(&path);
            }
        }
        Some(Action::PickExe) => {
            let mut picker = rfd::FileDialog::new()
                .set_title("選擇原廠 IAP 程式")
                .add_filter("程式", &["exe"]);
            if let Some(dir) = Path::new(&dialog.exe).parent() {
                picker = picker.set_directory(dir);
            }
            if let Some(path) = picker.pick_file() {
                dialog.exe = path.display().to_string();
            }
        }
        Some(Action::MakePatch) => {
            dialog.make_record_read(None);
        }
        Some(Action::Flash) => {
            dialog.start_flash();
        }
        Some(Action::Stop) => dialog.stop(),
        Some(Action::Launch) => {
            dialog.start_tool(ctx);
        }
        Some(Action::Close) | None => {}
    }
    app.windows.firmware = Some(dialog);
}

fn level_icon(ui: &egui::Ui, level: Level) -> egui::RichText {
    let visuals = ui.visuals();
    match level {
        Level::Ok => egui::RichText::new("✔").color(egui::Color32::from_rgb(22, 163, 74)),
        Level::Warn => egui::RichText::new("⚠").color(visuals.warn_fg_color),
        Level::Block => egui::RichText::new("⚠").color(visuals.error_fg_color),
    }
}

fn button(ui: &mut egui::Ui, enabled: bool, text: &str) -> bool {
    ui.add_enabled(enabled, egui::Button::new(text)).clicked()
}

fn dialog_ui(ui: &mut egui::Ui, d: &mut FirmwareDialog) -> Option<Action> {
    let gate = d.gate();
    let mut action = None;
    egui::Grid::new("firmware-form")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label("對講機目前韌體");
            ui.label(radio_text(&d.radio_firmware, &d.radio_date));
            ui.end_row();
            ui.label("連接埠");
            ui.label(if d.port_label.is_empty() {
                NO_PORT_LABEL
            } else {
                &d.port_label
            });
            ui.end_row();
            ui.label("韌體檔");
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut d.file.as_str())
                        .hint_text(FILE_HINT)
                        .desired_width(300.0),
                );
                if button(ui, gate.browse_enabled, "選擇…") {
                    action = Some(Action::PickFile);
                }
                let patch = ui
                    .add_enabled(
                        gate.make_patch_enabled,
                        egui::Button::new("產生錄音讀取版…"),
                    )
                    .on_hover_text(PATCH_TIP)
                    .on_disabled_hover_text(PATCH_TIP);
                if patch.clicked() {
                    action = Some(Action::MakePatch);
                }
            });
            ui.end_row();
            ui.label("檔案資訊");
            ui.add(egui::Label::new(&d.file_info).wrap());
            ui.end_row();
            ui.label("原廠 IAP 工具");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut d.exe).desired_width(360.0));
                if ui.button("選擇…").clicked() {
                    action = Some(Action::PickExe);
                }
            });
            ui.end_row();
        });
    ui.label("檢查結果");
    findings_ui(ui, d);
    ui.add_enabled(
        gate.confirm_enabled,
        egui::Checkbox::new(&mut d.confirm, CONFIRM),
    );
    ui.add_enabled(
        gate.iap_mode_enabled,
        egui::Checkbox::new(&mut d.iap_mode, IAP_MODE),
    );
    ui.add(egui::Label::new(egui::RichText::new(STEPS).weak()).wrap());
    if let Some((done, total)) = d.progress {
        let fraction = done as f32 / total.max(1) as f32;
        ui.add(egui::ProgressBar::new(fraction).show_percentage());
    }
    ui.add(egui::Label::new(&d.status).wrap());
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("關閉").clicked() {
            action = Some(Action::Close);
        }
        if button(ui, gate.flash_enabled, "開始升級") {
            action = Some(Action::Flash);
        }
        if gate.stop_visible && button(ui, !d.stopping, "停止") {
            action = Some(Action::Stop);
        }
        if button(ui, gate.launch_enabled, "改用原廠 IAP 工具") {
            action = Some(Action::Launch);
        }
    });
    action
}

fn findings_ui(ui: &mut egui::Ui, d: &FirmwareDialog) {
    let (common, builtin, tool) = d.findings();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        egui::ScrollArea::vertical()
            .id_salt("firmware-findings")
            .max_height(170.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for f in common.iter().chain(&builtin).chain(&tool) {
                    ui.horizontal_top(|ui| {
                        ui.label(level_icon(ui, f.level));
                        ui.add(egui::Label::new(&f.text).wrap());
                    });
                }
                if d.image.is_none() {
                    ui.label(NO_FILE);
                }
            });
    });
}

/// The last question before flashing; 否 has the focus, so Enter or Escape answers 否.
fn question_ui(ctx: &egui::Context, d: &mut FirmwareDialog) -> Option<bool> {
    let question = d.question.as_mut()?;
    let response = egui::Modal::new(egui::Id::new("firmware-confirm")).show(ctx, |ui| {
        ui.set_max_width(520.0);
        ui.heading("開始升級");
        ui.add_space(4.0);
        ui.label(&question.text);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let yes = ui.button("是");
            let no = ui.button("否");
            if !question.focused {
                no.request_focus();
                question.focused = true;
            }
            if yes.clicked() {
                Some(true)
            } else if no.clicked() {
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
