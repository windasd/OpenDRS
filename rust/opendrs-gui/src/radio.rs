//! 讀取／寫入對講機 and 校正對講機時鐘 from the main window (port of `radio_panel.py` and
//! `radio_ops.py`): the port picker on the toolbar, the progress in the status bar, and one
//! transfer at a time on a background thread. The sequences themselves (backup first, refuse a
//! model / band mismatch) are `opendrs_core::radio_session`.

use std::panic::{self, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::JoinHandle;
use std::time::Duration;

use eframe::egui;
use opendrs_core::codeplug::check_writable;
use opendrs_core::f136::Project;
use opendrs_core::radio_link::{RadioError, SerialTransport, Transport, list_ports};
use opendrs_core::radio_session::{
    Mode, PAUSE_BETWEEN_COMMANDS, RadioSession, ReadOutcome, Status, WriteOutcome,
    clock_confirmation, clock_success_text, read_success_text, write_confirmation,
    write_success_text,
};
use opendrs_core::radio_sim::SimulatedRadio;

use crate::app::{Dialog, OpenDrsApp, Pending};
use crate::document::Document;
use crate::pages::PageState;

/// The port value of 模擬對講機（試用）.
pub const SIMULATED: &str = "__sim__";
pub const SIMULATED_LABEL: &str = "模擬對講機（試用）";
pub const READ_TIP: &str = "經寫頻線讀取對講機目前的設定（自動保留備份）";
pub const WRITE_TIP: &str = "寫入前會先讀取並備份對講機目前的設定，機型或頻段不符會中止";
pub const CLOCK_TIP: &str = "讀取對講機後原樣寫回、只更新寫入時間，讓對講機依電腦時間設定時鐘（錄音的日期時間來自這個時鐘）";
const NO_PORTS: &str = "找不到序列埠：請確認寫頻線已插入並安裝驅動程式";
const PICK_PORT: &str = "請先在工具列選擇寫頻線的連接埠。";
const BUSY: &str = "請等目前的讀寫完成。";
const CHANGED: &str = "詢問之後專案已經改變，沒有寫入；請重新選擇「寫入對講機」。";

/// One entry of the port picker.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortChoice {
    pub device: String,
    pub label: String,
}

/// What a finished transfer brought back.
#[derive(Debug)]
pub enum Outcome {
    Read(ReadOutcome),
    Write(WriteOutcome),
    Clock(WriteOutcome),
}

type Finished = (Result<Outcome, String>, Option<SimulatedRadio>);

enum Event {
    Progress(usize, usize, String),
    Status(String),
    /// The result (the error text on failure) and the simulated radio, handed back.
    Finished(Box<Finished>),
}

struct Job {
    mode: Mode,
    events: Receiver<Event>,
    thread: Option<JoinHandle<()>>,
}

/// What the worker thread needs.
struct Request {
    mode: Mode,
    port: String,
    port_label: String,
    backup_dir: PathBuf,
    project: Option<Project>,
    simulated: Option<SimulatedRadio>,
}

/// The toolbar's port picker and the status bar's progress, running one transfer at a time.
#[derive(Default)]
pub struct RadioPanel {
    pub ports: Vec<PortChoice>,
    pub selected: usize,
    /// The status-bar text: the last step, progress or error.
    pub status: String,
    /// (done, total) while a transfer runs; total 0 until the first block arrives.
    pub progress: Option<(usize, usize)>,
    /// Every status line of the current transfer (tests read it).
    pub log: Vec<String>,
    /// The radio behind 模擬對講機（試用）; it keeps what was written between transfers.
    pub simulated: Option<SimulatedRadio>,
    job: Option<Job>,
}

impl RadioPanel {
    /// The picker with the system's ports, `preferred` selected when present.
    pub fn new(preferred: &str) -> Self {
        let mut panel = RadioPanel::default();
        panel.refresh_ports(preferred);
        panel
    }

    /// List the ports again (after plugging in the cable); keeps the selection when possible.
    pub fn refresh_ports(&mut self, preferred: &str) {
        let keep = if preferred.is_empty() {
            self.port()
        } else {
            preferred.to_string()
        };
        let ports = match list_ports() {
            Ok(ports) => ports,
            Err(e) => {
                self.note(e.to_string());
                Vec::new()
            }
        };
        let none_found = ports.is_empty();
        self.set_ports(
            ports
                .into_iter()
                .map(|p| PortChoice {
                    label: format!("{} — {}", p.device, p.description),
                    device: p.device,
                })
                .collect(),
            &keep,
        );
        if none_found {
            self.status = NO_PORTS.into();
        }
    }

    /// Show `ports` and then 模擬對講機（試用）; select `keep`, or the first entry.
    pub fn set_ports(&mut self, ports: Vec<PortChoice>, keep: &str) {
        self.ports = ports;
        self.ports.push(PortChoice {
            device: SIMULATED.into(),
            label: SIMULATED_LABEL.into(),
        });
        self.selected = self
            .ports
            .iter()
            .position(|p| !keep.is_empty() && p.device == keep)
            .unwrap_or(0);
    }

    /// Select the entry whose port is `device`; false when there is none.
    pub fn select(&mut self, device: &str) -> bool {
        match self.ports.iter().position(|p| p.device == device) {
            Some(index) => {
                self.selected = index;
                true
            }
            None => false,
        }
    }

    /// The selected port (`__sim__` for the simulated radio), or "".
    pub fn port(&self) -> String {
        self.ports
            .get(self.selected)
            .map_or_else(String::new, |p| p.device.clone())
    }

    /// The selected entry as shown.
    pub fn port_label(&self) -> String {
        self.ports
            .get(self.selected)
            .map_or_else(String::new, |p| p.label.clone())
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    fn note(&mut self, text: String) {
        self.status.clone_from(&text);
        self.log.push(text);
    }

    /// Start `mode` on the selected port in the background; `ctx` is repainted on every step.
    pub fn start(
        &mut self,
        mode: Mode,
        project: Option<Project>,
        backup_dir: PathBuf,
        ctx: Option<egui::Context>,
    ) -> bool {
        let port = self.port();
        if self.is_running() || port.is_empty() {
            return false;
        }
        let port_label = self.port_label();
        let simulated = (port == SIMULATED).then(|| self.simulated.take().unwrap_or_default());
        self.log.clear();
        self.status = Status::Connecting {
            mode,
            port: &port_label,
        }
        .to_string();
        self.progress = Some((0, 0));
        let request = Request {
            mode,
            port,
            port_label,
            backup_dir,
            project,
            simulated,
        };
        let (sender, events) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("radio".into())
            .spawn(move || {
                let send = |event: Event| {
                    let _ = sender.send(event); // the window was closed: nobody is listening
                    if let Some(ctx) = &ctx {
                        ctx.request_repaint();
                    }
                };
                let (result, simulated) = run_guarded(request, &send);
                send(Event::Finished(Box::new((result, simulated))));
            });
        match thread {
            Ok(thread) => {
                self.job = Some(Job {
                    mode,
                    events,
                    thread: Some(thread),
                });
                true
            }
            Err(e) => {
                self.progress = None;
                self.note(format!("錯誤：未預期的錯誤：{e}"));
                false
            }
        }
    }

    /// Take the worker's news; returns the result once the transfer has ended.
    pub fn poll(&mut self) -> Option<(Mode, Result<Outcome, String>)> {
        let mut finished = None;
        if let Some(job) = &mut self.job {
            loop {
                match job.events.try_recv() {
                    Ok(Event::Progress(done, total, text)) => {
                        self.progress = Some((done, total.max(1)));
                        self.status = text;
                    }
                    Ok(Event::Status(text)) => {
                        self.status.clone_from(&text);
                        self.log.push(text);
                    }
                    Ok(Event::Finished(done)) => {
                        let (result, simulated) = *done;
                        if simulated.is_some() {
                            self.simulated = simulated;
                        }
                        finished = Some((job.mode, result));
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        finished = Some((job.mode, Err(String::new())));
                        break;
                    }
                }
            }
        }
        if finished.is_some() {
            if let Some(thread) = self.job.take().and_then(|mut job| job.thread.take()) {
                let _ = thread.join(); // it has sent its last event
            }
            self.progress = None;
        }
        finished
    }

    /// Block until the transfer ends and return its result (tests).
    pub fn wait(&mut self, timeout: Duration) -> Option<(Mode, Result<Outcome, String>)> {
        let started = std::time::Instant::now();
        while self.is_running() && started.elapsed() < timeout {
            if let Some(done) = self.poll() {
                return Some(done);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        None
    }
}

fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_default()
}

/// Run the request; a panic is reported like Python's 未預期的錯誤.
fn run_guarded(
    request: Request,
    send: &dyn Fn(Event),
) -> (Result<Outcome, String>, Option<SimulatedRadio>) {
    match panic::catch_unwind(AssertUnwindSafe(|| run(request, send))) {
        Ok(done) => done,
        Err(payload) => {
            let message = format!("未預期的錯誤：{}", panic_text(payload.as_ref()));
            send(Event::Status(Status::Failed(&message).to_string()));
            (Err(message), None)
        }
    }
}

enum Link {
    Simulated(SimulatedRadio),
    Serial(SerialTransport),
}

impl Link {
    fn transport(&mut self) -> &mut dyn Transport {
        match self {
            Link::Simulated(radio) => radio,
            Link::Serial(port) => port,
        }
    }
}

fn run(
    request: Request,
    send: &dyn Fn(Event),
) -> (Result<Outcome, String>, Option<SimulatedRadio>) {
    let Request {
        mode,
        port,
        port_label,
        backup_dir,
        project,
        simulated,
    } = request;
    let (mut link, pause) = match simulated {
        Some(radio) => (Link::Simulated(radio), Duration::ZERO),
        None => match SerialTransport::open(&port) {
            Ok(serial) => (Link::Serial(serial), PAUSE_BETWEEN_COMMANDS),
            Err(e) => {
                // The session never started: report its first line and the failure here.
                let message = e.to_string();
                send(Event::Status(
                    Status::Connecting {
                        mode,
                        port: &port_label,
                    }
                    .to_string(),
                ));
                send(Event::Status(Status::Failed(&message).to_string()));
                return (Err(message), None);
            }
        },
    };
    let result = {
        let mut session = RadioSession::new(link.transport(), backup_dir)
            .with_port_label(port_label)
            .with_pause(pause)
            .with_progress(|done, total, text: &str| {
                send(Event::Progress(done, total, text.to_string()))
            })
            .with_status(|status: &Status<'_>| send(Event::Status(status.to_string())));
        match (mode, &project) {
            (Mode::Read, _) => session.read().map(Outcome::Read),
            (Mode::Write, Some(project)) => session.write(project).map(Outcome::Write),
            (Mode::Write, None) => Err(RadioError::Failed("沒有要寫入的專案".into()).into()),
            (Mode::Clock, _) => session.sync_clock().map(Outcome::Clock),
        }
    };
    let simulated = match link {
        Link::Simulated(radio) => Some(radio),
        Link::Serial(_) => None,
    };
    (result.map_err(|e| e.message()), simulated)
}

// ------------------------------------------------------------------ main window

impl OpenDrsApp {
    fn backup_dir(&self) -> PathBuf {
        self.data_dir().join("backups")
    }

    /// The port picked in the toolbar is kept for next time.
    pub fn remember_port(&mut self) {
        let port = self.radio.port();
        if !port.is_empty() && port != self.settings().radio_port {
            self.update_settings(|s| s.radio_port = port);
        }
    }

    /// No transfer is running and a port is picked (otherwise says so under `title`).
    fn radio_ready(&mut self, title: &str) -> bool {
        if self.busy() {
            return false;
        }
        if self.radio.port().is_empty() {
            self.warn(title, PICK_PORT);
            return false;
        }
        true
    }

    /// 讀取對講機…: after the unsaved-changes question, read in the background; the result
    /// loads when it finishes.
    pub fn read_from_radio(&mut self) {
        if !self.radio_ready(Mode::Read.title()) {
            return;
        }
        if self.doc.is_dirty() {
            self.dialog = Some(Dialog::ConfirmDiscard(Pending::ReadRadio));
        } else {
            self.start_radio(Mode::Read);
        }
    }

    /// 寫入對講機…: check the project, ask, then write in the background.
    pub fn write_to_radio(&mut self) {
        if !self.require_project(Mode::Write.title()) {
            return;
        }
        let reasons = check_writable(self.project());
        if !reasons.is_empty() {
            self.warn("無法寫入", reasons.join("\n"));
            return;
        }
        if !self.radio_ready(Mode::Write.title()) {
            return;
        }
        let text = write_confirmation(&self.radio.port_label(), self.project(), &self.backup_dir());
        self.dialog = Some(Dialog::Radio {
            mode: Mode::Write,
            text,
            revision: self.doc.revision(),
        });
    }

    /// 校正對講機時鐘…: read the radio, write the same data back with a fresh date stamp.
    pub fn sync_radio_clock(&mut self) {
        if !self.radio_ready(Mode::Clock.title()) {
            return;
        }
        self.dialog = Some(Dialog::Radio {
            mode: Mode::Clock,
            text: clock_confirmation(&self.radio.port_label()),
            revision: self.doc.revision(),
        });
    }

    /// Start `mode` on the selected port (the questions have been answered).
    pub fn start_radio(&mut self, mode: Mode) -> bool {
        if self.busy() {
            // Another transfer may hold the port (the buttons are disabled meanwhile).
            self.warn(mode.title(), BUSY);
            return false;
        }
        self.remember_port();
        let project = (mode == Mode::Write).then(|| self.project().clone());
        let ctx = self.ctx.clone();
        let backup_dir = self.backup_dir();
        self.radio.start(mode, project, backup_dir, ctx)
    }

    /// Apply a finished transfer: load what was read, or report the write.
    pub fn radio_finished(&mut self, mode: Mode, result: Result<Outcome, String>) {
        let outcome = match result {
            Ok(outcome) => outcome,
            Err(message) => {
                let text = if message.is_empty() {
                    "傳輸未完成。".to_string()
                } else {
                    message
                };
                self.warn("對講機", text);
                return;
            }
        };
        match outcome {
            Outcome::Read(read) => {
                self.status = read_success_text(&read);
                self.load_from_radio(read);
            }
            Outcome::Write(written) => self.warn(mode.title(), write_success_text(&written)),
            Outcome::Clock(written) => self.warn(mode.title(), clock_success_text(&written)),
        }
    }

    /// The radio's settings become the project: unsaved, with no file yet. A question still
    /// open about the old project is dropped.
    fn load_from_radio(&mut self, outcome: ReadOutcome) {
        if matches!(
            self.dialog,
            Some(Dialog::Confirm { .. } | Dialog::Radio { .. } | Dialog::AddZone { .. })
        ) {
            self.dialog = None;
        }
        let model = &outcome.read.identity.model;
        let source = format!(
            "從對講機讀取（{}，尚未儲存）",
            if model.is_empty() { "?" } else { model }
        );
        self.doc = Document::unsaved(outcome.read.project, source);
        self.pages = PageState::default();
    }

    /// Take the worker's news each frame.
    pub fn poll_radio(&mut self) {
        if let Some((mode, result)) = self.radio.poll() {
            self.radio_finished(mode, result);
        }
    }

    /// 連接埠, its refresh button, 讀取 and 寫入 on the toolbar.
    pub fn radio_toolbar(&mut self, ui: &mut egui::Ui) {
        let idle = !self.busy();
        ui.label(" 連接埠 ");
        let before = self.radio.selected;
        ui.add_enabled_ui(idle, |ui| {
            let radio = &mut self.radio;
            egui::ComboBox::from_id_salt("port")
                .width(220.0)
                .selected_text(radio.port_label())
                .show_ui(ui, |ui| {
                    for (index, port) in radio.ports.iter().enumerate() {
                        ui.selectable_value(&mut radio.selected, index, &port.label);
                    }
                })
                .response
                .on_hover_text("寫頻線連接埠");
            if ui
                .button("重新整理")
                .on_hover_text("重新列出連接埠（插拔寫頻線後使用）")
                .clicked()
            {
                radio.refresh_ports("");
            }
        });
        if self.radio.selected != before {
            self.remember_port();
        }
        let ready = idle;
        if ui
            .add_enabled(ready, egui::Button::new("讀取"))
            .on_hover_text(READ_TIP)
            .clicked()
        {
            self.read_from_radio();
        }
        if ui
            .add_enabled(ready, egui::Button::new("寫入"))
            .on_hover_text(WRITE_TIP)
            .clicked()
        {
            self.write_to_radio();
        }
    }

    /// The transfer's last step and its progress, at the right of the status bar.
    pub fn radio_status(&self, ui: &mut egui::Ui) {
        if let Some((done, total)) = self.radio.progress {
            let bar = if total == 0 {
                egui::ProgressBar::new(0.0).animate(true)
            } else {
                egui::ProgressBar::new(done as f32 / total as f32).show_percentage()
            };
            ui.add(bar.desired_width(220.0));
        }
        if !self.radio.status.is_empty() {
            ui.label(&self.radio.status);
        }
    }

    /// The question before a write or a clock sync; 是 starts it if the project is still the
    /// one described (`revision`).
    pub fn radio_question(&mut self, ctx: &egui::Context, mode: Mode, text: &str, revision: u64) {
        let response = egui::Modal::new(egui::Id::new("radio-question")).show(ctx, |ui| {
            ui.set_max_width(560.0);
            ui.heading(mode.title());
            ui.add_space(4.0);
            ui.label(text);
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
            self.dialog = None;
            if yes && mode == Mode::Write && revision != self.doc.revision() {
                self.warn(mode.title(), CHANGED);
            } else if yes {
                self.start_radio(mode);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_simulated_radio_is_always_offered_last() {
        let mut panel = RadioPanel::default();
        panel.set_ports(
            vec![PortChoice {
                device: "COM9".into(),
                label: "COM9 — USB".into(),
            }],
            "__sim__",
        );
        assert_eq!(panel.ports.len(), 2);
        assert_eq!(panel.port(), SIMULATED);
        assert_eq!(panel.port_label(), SIMULATED_LABEL);
        panel.set_ports(Vec::new(), "COM9");
        assert_eq!(
            panel.port(),
            SIMULATED,
            "a missing port falls back to the first entry"
        );
    }
}
