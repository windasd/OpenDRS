//! The read / write / clock-sync sequences of `radio_session` (ports the worker tests of
//! test_radio_panel.py). Expected texts, status lines and progress were taken from the Python GUI
//! driving the simulated radio with the clock frozen at 2026-10-08 18:00.

mod common;

use std::error::Error as _;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use chrono::{NaiveDate, NaiveDateTime};
use common::{ReplayTransport, Silent, cap, read_exchanges, write_exchanges};
use opendrs_core::codeplug::{BLOCK, Counts, DATE_OFFSET, DATE_SIZE, build_stream, encode_date};
use opendrs_core::f136::{self, Project};
use opendrs_core::radio_link::{
    CHECKSUM_WRITE, NO_REPLY, RadioClient, RadioError, Transport, WRITE_VERB, byte_sum,
};
use opendrs_core::radio_session::{
    Mode, RadioSession, SessionError, clock_confirmation, clock_success_text, read_success_text,
    write_confirmation, write_success_text, write_summary,
};
use opendrs_core::radio_sim::SimulatedRadio;

const SIM: &str = "模擬對講機（試用）";
const SUMMARY: &str = "DRS-8500　136-480 MHz　韌體 308E.BBKTARSCB.003.Mar  9 2024";
const DATE: &str = "2026/10/08 18:00";
const BEFORE_WRITE: &str = "20261008_180000_before_write_DRS-8500.F136";

fn at() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 10, 8)
        .and_then(|d| d.and_hms_opt(18, 0, 0))
        .expect("valid date")
}

fn radio() -> SimulatedRadio {
    SimulatedRadio::with_stream(&cap("read_stream.bin"), Counts::new(4, 27, 360))
}

fn written() -> Project {
    f136::load(common::fixtures().join("capture/written_project.F136")).expect("written project")
}

/// A fresh folder for backups, removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opendrs-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Scratch(dir)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn files(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.0)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The simulated radio, recording every chunk the PC sends; optionally it reports another
/// identity once a write starts (the radio was swapped between the read and the write).
struct Spy {
    radio: SimulatedRadio,
    sent: Vec<Vec<u8>>,
    identity_at_write: Option<Vec<u8>>,
}

impl Spy {
    fn new(radio: SimulatedRadio) -> Self {
        Spy {
            radio,
            sent: Vec::new(),
            identity_at_write: None,
        }
    }
    fn write_started(&self) -> bool {
        self.sent.iter().any(|chunk| chunk.starts_with(WRITE_VERB))
    }
}

impl Transport for Spy {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.sent.push(data.to_vec());
        if data.starts_with(WRITE_VERB) {
            if let Some(identity) = self.identity_at_write.take() {
                self.radio.identity = identity;
            }
        }
        self.radio.write(data)
    }
    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError> {
        self.radio.read_exact(n, timeout)
    }
    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        self.radio.read_idle(idle, max_wait)
    }
    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.radio.reset_input()
    }
}

type Steps = Vec<(usize, usize, String)>;

struct Run<T> {
    result: Result<T, SessionError>,
    statuses: Vec<String>,
    progress: Steps,
}

/// Run one operation as the GUI would on the simulated radio: no pause, the clock frozen.
fn run<T>(
    transport: &mut dyn Transport,
    dir: &Path,
    op: impl FnOnce(&mut RadioSession<'_>) -> Result<T, SessionError>,
) -> Run<T> {
    let mut statuses = Vec::new();
    let mut progress = Vec::new();
    let result = {
        let mut session = RadioSession::new(transport, dir)
            .with_port_label(SIM)
            .with_progress(|done, total, text: &str| progress.push((done, total, text.into())))
            .with_status(|status| statuses.push(status.to_string()))
            .with_pause(Duration::ZERO)
            .with_now(at);
        op(&mut session)
    };
    Run {
        result,
        statuses,
        progress,
    }
}

fn read_progress(total: usize) -> Steps {
    let mut steps = vec![(0, 1, "連線中…".to_string())];
    steps.extend((0..total).map(|i| (i, total, format!("讀取區塊 {}/{total}", i + 1))));
    steps.push((total, total, "讀取完成".into()));
    steps
}

fn write_progress(total: usize) -> Steps {
    let mut steps = vec![(0, total, "連線中…".to_string())];
    steps.extend((0..total).map(|i| (i, total, format!("寫入區塊 {}/{total}", i + 1))));
    steps.push((total, total, "等待對講機確認…".into()));
    steps.push((total, total, "寫入完成".into()));
    steps
}

fn backed_up(file: &str, summary: &str) -> String {
    format!("備份完成：{file}（{summary}）")
}

fn without_date(stream: &[u8]) -> Vec<u8> {
    [&stream[..DATE_OFFSET], &stream[DATE_OFFSET + DATE_SIZE..]].concat()
}

fn dated(stream: &[u8], date: &str) -> Vec<u8> {
    let mut out = stream.to_vec();
    out[DATE_OFFSET..DATE_OFFSET + DATE_SIZE].copy_from_slice(&encode_date(date).unwrap());
    out
}

// ------------------------------------------------------------------ read

#[test]
fn read_backs_up_and_reports() {
    let scratch = Scratch::new("read");
    let mut radio = radio();
    let run = run(&mut radio, scratch.path(), |s| s.read());
    let outcome = run.result.unwrap();
    assert_eq!(outcome.read.counts, Counts::new(4, 27, 360));
    let name = "20261008_180000_read_DRS-8500.F136";
    assert_eq!(outcome.backup, scratch.path().join(name));
    assert_eq!(
        std::fs::read(outcome.backup.with_extension("bin")).unwrap(),
        cap("read_stream.bin")
    );
    assert_eq!(
        f136::load(&outcome.backup).unwrap().channels,
        outcome.read.project.channels
    );
    assert_eq!(
        run.statuses,
        [
            format!("讀取：連線到 {SIM}…"),
            format!("已讀取：{SUMMARY}"),
            "讀取完成".into()
        ]
    );
    assert_eq!(run.progress, read_progress(7));
    assert_eq!(
        read_success_text(&outcome),
        format!(
            "已讀取 {SUMMARY}，備份：{}",
            scratch.path().join(name).display()
        )
    );
}

#[test]
fn a_silent_radio_reports_the_checklist_and_keeps_no_backup() {
    let scratch = Scratch::new("silent");
    let mut statuses = Vec::new();
    let err = RadioSession::new(&mut Silent, scratch.path())
        .with_port_label("COM9")
        .with_status(|status| statuses.push(status.to_string()))
        .read()
        .unwrap_err();
    assert!(matches!(
        &err,
        SessionError::Radio(RadioError::Timeout { received: 0, .. })
    ));
    assert_eq!(err.message(), NO_REPLY);
    assert_eq!(
        statuses,
        [
            "讀取：連線到 COM9…".to_string(),
            format!("錯誤：{NO_REPLY}")
        ]
    );
    assert!(!scratch.path().exists());
}

// ------------------------------------------------------------------ write

#[test]
fn write_backs_up_then_writes_dated_now() {
    let scratch = Scratch::new("write");
    let mut radio = radio();
    let run = run(&mut radio, scratch.path(), |s| s.write(&written()));
    let outcome = run.result.unwrap();
    assert_eq!(radio.commits, 1);
    assert_eq!(outcome.backup, scratch.path().join(BEFORE_WRITE));
    assert_eq!(
        scratch.files(),
        [
            BEFORE_WRITE.to_string(),
            BEFORE_WRITE.replace(".F136", ".bin")
        ]
    );
    assert_eq!(outcome.before.stream, cap("read_stream.bin"));
    assert_eq!(outcome.date_text, DATE);
    assert_eq!(outcome.written.checksum, 0x0006_1D50);
    assert_eq!(
        run.statuses,
        [
            format!("寫入：連線到 {SIM}…"),
            "寫入前先讀取對講機目前的設定並備份…".into(),
            backed_up(BEFORE_WRITE, SUMMARY),
            "開始寫入…".into(),
            "寫入完成".into(),
        ]
    );
    assert_eq!(run.progress, [read_progress(7), write_progress(7)].concat());
    assert_eq!(
        write_success_text(&outcome),
        format!(
            "寫入完成，對講機確認校驗和 0x00061D50。\n\
             對講機時鐘已設為 2026/10/08 18:00（開始寫入時的電腦時間，秒數維持對講機原值）。\n\
             寫入前的設定已備份到：{}\n\
             對講機正在套用設定，請等它重新啟動後再操作。",
            scratch.path().join(BEFORE_WRITE).display()
        )
    );
    let back = RadioClient::new(&mut radio).read_codeplug().unwrap();
    assert_eq!(
        &back.stream[DATE_OFFSET..DATE_OFFSET + DATE.len()],
        DATE.as_bytes()
    );
}

#[test]
fn an_edited_project_reaches_the_radio() {
    let scratch = Scratch::new("edited");
    let project = written();
    let edited = project
        .with_channel(0, project.channels[0].with_rx(145_500_000))
        .unwrap();
    assert!(write_summary(&edited).contains("頻道 27"));
    let mut radio = radio();
    let run = run(&mut radio, scratch.path(), |s| s.write(&edited));
    assert!(run.result.is_ok());
    let back = RadioClient::new(&mut radio).read_codeplug().unwrap();
    assert_eq!(back.project.channels[0].rx_hz(), 145_500_000);
    assert_eq!(back.project.contacts, project.contacts);
}

#[test]
fn a_mismatched_radio_is_refused_before_anything_is_sent() {
    let mut wrong_band = written();
    wrong_band.image[0x50..0x58].copy_from_slice(b"F400-480");
    let other_model =
        f136::load(common::fixtures().join("mmdvm_20241124.F136")).expect("DRS-8600 project");
    let cases = [
        (
            wrong_band,
            None,
            "專案頻段為 400-480 MHz，對講機是 136-480 MHz",
        ),
        (other_model, None, "專案機型為 DRS-8600，對講機是 DRS-8500"),
        (
            written(),
            Some(b"garbage".to_vec()),
            "無法辨識對講機的機型或頻段",
        ),
    ];
    for (project, identity, reason) in cases {
        let scratch = Scratch::new("mismatch");
        let mut sim = radio();
        let unknown = identity.is_some();
        if let Some(identity) = identity {
            sim.identity = identity;
        }
        let mut spy = Spy::new(sim);
        let run = run(&mut spy, scratch.path(), |s| s.write(&project));
        let err = run.result.unwrap_err();
        let message = format!("已中止，未寫入任何資料：{reason}");
        assert_eq!(err.to_string(), message);
        assert!(!spy.write_started() && spy.radio.commits == 0, "{reason}");
        let (file, summary) = if unknown {
            (
                "20261008_180000_before_write_radio.F136",
                "未知機型　　韌體 ?",
            )
        } else {
            (BEFORE_WRITE, SUMMARY)
        };
        assert!(scratch.path().join(file).exists());
        assert_eq!(
            run.statuses,
            [
                format!("寫入：連線到 {SIM}…"),
                "寫入前先讀取對講機目前的設定並備份…".into(),
                backed_up(file, summary),
                format!("錯誤：{message}"),
            ]
        );
        assert_eq!(run.progress, read_progress(7));
    }
}

#[test]
fn the_identity_is_checked_again_during_the_handshake() {
    let scratch = Scratch::new("handshake");
    let mut spy = Spy::new(radio());
    spy.identity_at_write = Some(b"308E.X\0+DRS-8600+136-480MHZ+".to_vec());
    let run = run(&mut spy, scratch.path(), |s| s.write(&written()));
    let message = "已中止，未寫入任何資料：專案機型為 DRS-8500，對講機是 DRS-8600";
    assert_eq!(run.result.unwrap_err().to_string(), message);
    assert_eq!(spy.radio.commits, 0);
    let last = spy.sent.last().unwrap();
    assert!(last.starts_with(WRITE_VERB), "no block after the command");
    assert_eq!(
        run.statuses[3..],
        ["開始寫入…".to_string(), format!("錯誤：{message}")]
    );
}

#[test]
fn nothing_is_written_without_a_backup() {
    let scratch = Scratch::new("nobackup");
    std::fs::create_dir_all(scratch.path()).unwrap();
    let blocked = scratch.path().join("a file");
    std::fs::write(&blocked, b"").unwrap();
    let mut spy = Spy::new(radio());
    let run = run(&mut spy, &blocked.join("backups"), |s| s.write(&written()));
    let err = run.result.unwrap_err();
    assert!(matches!(err, SessionError::Backup(_)));
    assert_eq!(err.to_string(), "無法保存備份");
    assert!(err.source().is_some());
    assert!(
        err.message().starts_with("未預期的錯誤：無法保存備份（") && err.message().ends_with('）'),
        "{}",
        err.message()
    );
    assert!(!spy.write_started() && spy.radio.commits == 0);
    assert_eq!(run.statuses.len(), 3);
    assert_eq!(run.statuses[2], format!("錯誤：{}", err.message()));
}

// ------------------------------------------------------------------ clock

#[test]
fn sync_clock_writes_the_radio_back_with_only_a_new_date() {
    let scratch = Scratch::new("clock");
    let original = cap("read_stream.bin");
    let mut radio = radio();
    let run = run(&mut radio, scratch.path(), |s| s.sync_clock());
    let outcome = run.result.unwrap();
    assert_eq!(radio.commits, 1);
    assert_eq!(outcome.backup, scratch.path().join(BEFORE_WRITE));
    assert_eq!(outcome.date_text, DATE);
    let back = RadioClient::new(&mut radio).read_codeplug().unwrap().stream;
    assert_eq!(without_date(&back), without_date(&original));
    assert_eq!(back, dated(&original, DATE));
    assert_ne!(back, original);
    assert_eq!(
        run.statuses,
        [
            format!("校正時鐘：連線到 {SIM}…"),
            "寫入前先讀取對講機目前的設定並備份…".into(),
            backed_up(BEFORE_WRITE, SUMMARY),
            "開始寫入…".into(),
            "校正時鐘完成".into(),
        ]
    );
    assert_eq!(run.progress, [read_progress(7), write_progress(7)].concat());
    assert_eq!(
        clock_success_text(&outcome),
        format!(
            "對講機時鐘已設為 2026/10/08 18:00（開始寫入時的電腦時間，秒數維持對講機原值）。\n\
             對講機的設定原樣寫回，沒有其他變更；目前編輯中的專案不受影響。\n\
             寫入前的設定已備份到：{}\n\
             對講機正在重新啟動；之後可在對講機選單的「日期／時間」確認。",
            scratch.path().join(BEFORE_WRITE).display()
        )
    );
}

#[test]
fn sync_clock_refuses_data_it_cannot_rebuild() {
    let scratch = Scratch::new("clock-odd");
    let mut odd = cap("read_stream.bin");
    let last = odd.len() - 1;
    odd[last] = 1; // padding the parser drops
    let mut spy = Spy::new(SimulatedRadio::with_stream(&odd, Counts::new(4, 27, 360)));
    let run = run(&mut spy, scratch.path(), |s| s.sync_clock());
    let message = "已中止，未寫入任何資料：讀回的資料無法逐位元組重建，為了安全不寫回。";
    assert_eq!(run.result.unwrap_err().to_string(), message);
    assert!(!spy.write_started() && spy.radio.commits == 0);
    assert_eq!(
        std::fs::read(scratch.path().join(BEFORE_WRITE).with_extension("bin")).unwrap(),
        odd
    );
    assert_eq!(
        run.statuses,
        [
            format!("校正時鐘：連線到 {SIM}…"),
            "寫入前先讀取對講機目前的設定並備份…".into(),
            backed_up(BEFORE_WRITE, SUMMARY),
            format!("錯誤：{message}"),
        ]
    );
}

#[test]
fn sync_clock_refuses_an_unidentified_radio() {
    let scratch = Scratch::new("clock-unknown");
    let mut sim = radio();
    sim.identity = b"garbage".to_vec();
    let mut spy = Spy::new(sim);
    let run = run(&mut spy, scratch.path(), |s| s.sync_clock());
    let message = "已中止，未寫入任何資料：無法辨識對講機的機型或頻段";
    assert_eq!(run.result.unwrap_err().to_string(), message);
    assert!(!spy.write_started());
    assert_eq!(
        run.statuses,
        [
            format!("校正時鐘：連線到 {SIM}…"),
            "寫入前先讀取對講機目前的設定並備份…".into(),
            backed_up(
                "20261008_180000_before_write_radio.F136",
                "未知機型　　韌體 ?"
            ),
            format!("錯誤：{message}"),
        ]
    );
}

// ------------------------------------------------------------------ capture replay

/// The captured write with `stream` sent instead; the radio's checksum reply is recomputed for it.
fn write_exchanges_of(stream: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut ex = write_exchanges();
    assert_eq!(stream.len(), (ex.len() - 1) * BLOCK);
    for (step, block) in ex[1..].iter_mut().zip(stream.chunks(BLOCK)) {
        step.0 = block.to_vec();
    }
    let tail = &mut ex.last_mut().unwrap().1;
    assert!(tail.starts_with(CHECKSUM_WRITE));
    let at = CHECKSUM_WRITE.len();
    tail[at..at + 4].copy_from_slice(&byte_sum(stream).to_be_bytes());
    ex
}

fn sent_by_a_session(write_stream: &[u8]) -> Vec<u8> {
    [
        cap("read_cmd.bin"),
        b"Read".repeat(7),
        cap("write_cmd.bin"),
        write_stream.to_vec(),
    ]
    .concat()
}

#[test]
fn replayed_read_through_a_session_is_byte_exact() {
    let scratch = Scratch::new("replay-read");
    let mut t = ReplayTransport::new(read_exchanges());
    let run = run(&mut t, scratch.path(), |s| s.read());
    let outcome = run.result.unwrap();
    assert!(t.finished());
    assert_eq!(t.sent, [cap("read_cmd.bin"), b"Read".repeat(7)].concat());
    assert_eq!(outcome.read.stream, cap("read_stream.bin"));
    assert_eq!(outcome.read.project.channels, written().channels);
    assert_eq!(run.progress, read_progress(7));
}

#[test]
fn replayed_write_through_a_session_sends_the_captured_bytes_with_the_new_date() {
    assert_eq!(
        write_exchanges_of(&cap("write_stream.bin")),
        write_exchanges()
    );
    let expected = dated(&cap("write_stream.bin"), DATE);
    assert_eq!(build_stream(&written(), Some(DATE)).unwrap(), expected);
    let scratch = Scratch::new("replay-write");
    let mut t = ReplayTransport::new([read_exchanges(), write_exchanges_of(&expected)].concat());
    let run = run(&mut t, scratch.path(), |s| s.write(&written()));
    let outcome = run.result.unwrap();
    assert!(t.finished());
    assert_eq!(t.sent, sent_by_a_session(&expected));
    assert_eq!(outcome.written.checksum, byte_sum(&expected));
    assert!(outcome.written.radio_log.contains("Manufacture:TC58"));
}

#[test]
fn replayed_clock_sync_writes_back_what_was_read() {
    let scratch = Scratch::new("replay-clock");
    let expected = dated(&cap("read_stream.bin"), DATE);
    let mut t = ReplayTransport::new([read_exchanges(), write_exchanges_of(&expected)].concat());
    let run = run(&mut t, scratch.path(), |s| s.sync_clock());
    assert!(run.result.is_ok());
    assert!(t.finished());
    assert_eq!(t.sent, sent_by_a_session(&expected));
}

// ------------------------------------------------------------------ threads, texts

#[derive(Debug, PartialEq)]
enum Event {
    Progress(usize, usize, String),
    Status(String),
}

#[test]
fn a_session_runs_on_a_background_thread() {
    let scratch = Scratch::new("thread");
    let dir = scratch.path().to_path_buf();
    let (tx, rx) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut radio = radio();
        let progress_tx = tx.clone();
        let outcome = RadioSession::new(&mut radio, dir)
            .with_port_label(SIM)
            .with_progress(move |done, total, text: &str| {
                let _ = progress_tx.send(Event::Progress(done, total, text.into()));
            })
            .with_status(move |status| {
                let _ = tx.send(Event::Status(status.to_string()));
            })
            .with_pause(Duration::ZERO)
            .with_now(at)
            .sync_clock();
        (outcome, radio.commits)
    });
    let (outcome, commits) = worker.join().expect("worker thread");
    assert_eq!(outcome.unwrap().date_text, DATE);
    assert_eq!(commits, 1);
    let events: Vec<Event> = rx.iter().collect();
    let (statuses, progress): (Vec<&Event>, Vec<&Event>) =
        events.iter().partition(|e| matches!(e, Event::Status(_)));
    assert_eq!(statuses.len(), 5);
    assert_eq!(events[0], Event::Status(format!("校正時鐘：連線到 {SIM}…")));
    assert_eq!(events.last(), Some(&Event::Status("校正時鐘完成".into())));
    let expected: Vec<Event> = [read_progress(7), write_progress(7)]
        .concat()
        .into_iter()
        .map(|(done, total, text)| Event::Progress(done, total, text))
        .collect();
    assert_eq!(progress, expected.iter().collect::<Vec<_>>());
}

#[test]
fn dialog_texts_match_the_python_gui() {
    let project = written();
    assert_eq!(
        write_summary(&project),
        "機型 DRS-8500　F136-480\n分區 4、頻道 27、聯絡人 360（7 個區塊）"
    );
    assert_eq!(
        write_confirmation(
            SIM,
            &project,
            Path::new(r"C:\Users\x\AppData\Roaming\OpenDRS\backups")
        ),
        "要寫入到 模擬對講機（試用） 嗎？\n\n\
         機型 DRS-8500　F136-480\n分區 4、頻道 27、聯絡人 360（7 個區塊）\n\n\
         寫入前會先讀取對講機並備份到 C:\\Users\\x\\AppData\\Roaming\\OpenDRS\\backups；\
         機型或頻段不符會中止。"
    );
    assert_eq!(
        clock_confirmation(SIM),
        "要把 模擬對講機（試用） 上的對講機時鐘設為電腦目前的時間嗎？\n\n\
         做法：先讀取對講機目前的設定並備份，再把同樣的資料原樣寫回，只更新「寫入時間」；\
         對講機在寫入完成時依此設定時鐘（精確到分鐘，秒數不變），然後重新啟動。\n\n\
         讀回的資料若無法逐位元組重建就中止，不寫入任何資料。目前編輯中的專案不受影響。"
    );
    let modes = [Mode::Read, Mode::Write, Mode::Clock];
    assert_eq!(modes.map(Mode::name), ["讀取", "寫入", "校正時鐘"]);
    assert_eq!(
        modes.map(Mode::title),
        ["讀取對講機", "寫入對講機", "校正對講機時鐘"]
    );
}
