//! 讀取錄音 dialog (port of `tests/test_recording_dialog.py`). Every transfer runs against
//! `SimulatedRecorder`; no test opens a serial port, and ambep2.exe is never found (a missing
//! path), so it is not run either.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use opendrs_core::radio_link::{RadioError, Transport};
use opendrs_firmware::recording::RecordInfo;
use opendrs_firmware::recording_sim::SimulatedRecorder;
use opendrs_gui::dialogs::firmware::shared_factory;
use opendrs_gui::dialogs::firmware::{BoxedTransport, SharedTransport, TransportFactory};
use opendrs_gui::dialogs::recording::{
    BUSY, NOT_WAV, NameLookup, RUNNING, RadioIdNames, RecordingDialog, RecordingSetup, STOPPED,
    STOPPING, all_records_plan, duration_text, remembered_paths,
};

const WAIT: Duration = Duration::from_secs(30);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opendrs-rec-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

fn radio() -> Arc<Mutex<SimulatedRecorder>> {
    Arc::new(Mutex::new(SimulatedRecorder::default()))
}

fn dialog_with(
    transport: Option<TransportFactory>,
    out: &Path,
    names: &[(u64, &str)],
    lookup: Option<NameLookup>,
) -> RecordingDialog {
    RecordingDialog::new(RecordingSetup {
        transport,
        port_label: "模擬對講機".into(),
        out_dir: out.display().to_string(),
        ambep2: out.join("missing.exe").display().to_string(),
        names: names
            .iter()
            .map(|&(id, name)| (id, name.to_owned()))
            .collect::<HashMap<_, _>>(),
        lookup,
        pause: Duration::ZERO,
    })
}

fn dialog(radio: &Arc<Mutex<SimulatedRecorder>>, out: &Path) -> RecordingDialog {
    dialog_with(Some(shared_factory(Arc::clone(radio))), out, &[], None)
}

fn run(
    dialog: &mut RecordingDialog,
    start: impl FnOnce(&mut RecordingDialog, &egui::Context) -> bool,
) {
    assert!(
        start(dialog, &egui::Context::default()),
        "{}",
        dialog.status()
    );
    dialog.wait(WAIT);
    assert!(!dialog.running());
}

fn numbers(infos: &[RecordInfo]) -> Vec<u64> {
    infos.iter().map(|i| i.number).collect()
}

/// Record numbers as the rows currently show them.
fn order(dialog: &RecordingDialog) -> Vec<String> {
    dialog.rows().iter().map(|r| r.cells()[0].clone()).collect()
}

fn saved_numbers(dialog: &RecordingDialog) -> Vec<u64> {
    dialog.saved().iter().map(|s| s.info.number).collect()
}

#[test]
fn list_then_download_checked() {
    let dir = scratch("list");
    let radio = radio();
    let factory = shared_factory(Arc::clone(&radio));
    let mut dialog = dialog_with(
        Some(factory),
        &dir.join("rec"),
        &[(9_998_765, "QA0AJS")],
        None,
    );
    run(&mut dialog, RecordingDialog::read_list);
    assert_eq!(dialog.rows().len(), 3);
    assert_eq!(dialog.error(), "");
    assert_eq!(dialog.rows()[0].cells()[3], "QA0AJS");
    assert_eq!(dialog.rows()[1].cells()[1], "發射");
    assert_eq!(dialog.rows()[0].cells()[4], "18.7 秒");
    assert!(dialog.rows().iter().all(|r| r.checked)); // new rows are ticked
    dialog.set_checked(2, false);
    assert_eq!(numbers(&dialog.checked()), [1, 3]);
    run(&mut dialog, RecordingDialog::download_checked);
    assert_eq!(saved_numbers(&dialog), [1, 3]);
    let raw = dir.join("rec").join("20250607_1234_1_R09998765.ambe");
    assert_eq!(
        std::fs::read(raw).expect("raw"),
        radio.lock().expect("sim").records[0].1
    );
    // no decoder: the raw data is kept
    assert!(dialog.rows()[0].cells()[6].contains("找不到 ambep2"));
    assert!(!dialog.play(Some(1)) && dialog.status() == NOT_WAV);
    assert!(!dialog.play(None) && dialog.status() == NOT_WAV);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn download_all_without_list_support() {
    let dir = scratch("all");
    let mut no_list = SimulatedRecorder::default();
    no_list.supports_list = false;
    let radio = Arc::new(Mutex::new(no_list));
    let mut dialog = dialog(&radio, &dir);
    run(&mut dialog, RecordingDialog::read_list);
    assert!(dialog.status().contains("不支援從電腦讀取錄音") && dialog.rows().is_empty());
    run(&mut dialog, RecordingDialog::download_all);
    assert_eq!(saved_numbers(&dialog), [1, 2, 3]);
    assert_eq!(dialog.rows().len(), 3);
    assert!(dialog.status().contains("已下載 3 筆"));
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn no_port_and_nothing_checked() {
    let dir = scratch("no-port");
    let mut none = dialog_with(None, &dir, &[], None);
    assert!(!none.ready() && !none.read_list(&egui::Context::default()));
    assert!(none.status().contains("選擇寫頻線"));
    let mut other = dialog(&radio(), &dir);
    assert!(!other.download_checked(&egui::Context::default()));
    assert!(other.status().contains("讀取清單"));
    other.port_busy = true; // the main window or the firmware dialog is transferring
    assert!(!other.read_list(&egui::Context::default()) && other.status() == BUSY);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn status_counts_only_this_run_and_names_do_not_collide() {
    let dir = scratch("collide");
    let radio = radio();
    let mut dialog = dialog(&radio, &dir);
    run(&mut dialog, RecordingDialog::read_list);
    dialog.set_checked(2, false);
    dialog.set_checked(3, false);
    run(&mut dialog, RecordingDialog::download_checked);
    run(&mut dialog, RecordingDialog::read_list);
    // not overwritten by the earlier download count
    assert!(dialog.status().contains("對講機回報 3 筆"));
    dialog.set_checked(2, false);
    dialog.set_checked(3, false);
    // the radio now holds a different call under record 1 (same time, direction and ID)
    {
        let mut sim = radio.lock().expect("sim");
        sim.records[0].1.reverse();
    }
    run(&mut dialog, RecordingDialog::download_checked);
    assert!(dialog.status().contains("已下載 1 筆"));
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("dir")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".ambe"))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "20250607_1234_1_R09998765.ambe",
            "20250607_1234_1_R09998765_2.ambe"
        ]
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// The simulated recorder with its replies held back until `hold` is cleared.
struct HeldRecorder {
    radio: SharedTransport<SimulatedRecorder>,
    hold: Arc<AtomicBool>,
}

impl Transport for HeldRecorder {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.radio.write(data)
    }

    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError> {
        self.radio.read_exact(n, timeout)
    }

    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        if self.hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
            return Ok(Vec::new());
        }
        self.radio.read_idle(idle, max_wait)
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.radio.reset_input()
    }
}

fn held(radio: &Arc<Mutex<SimulatedRecorder>>, hold: &Arc<AtomicBool>) -> TransportFactory {
    let (radio, hold) = (Arc::clone(radio), Arc::clone(hold));
    Arc::new(move || {
        Ok(Box::new(HeldRecorder {
            radio: SharedTransport(Arc::clone(&radio)),
            hold: Arc::clone(&hold),
        }) as BoxedTransport)
    })
}

fn wait_for_command(radio: &Arc<Mutex<SimulatedRecorder>>, sent_before: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while radio.lock().expect("sim").sent.len() == sent_before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn stop_and_close_while_downloading() {
    let dir = scratch("stop");
    let radio = radio();
    let hold = Arc::new(AtomicBool::new(false));
    let mut dialog = dialog_with(Some(held(&radio, &hold)), &dir, &[], None);
    run(&mut dialog, RecordingDialog::read_list);
    hold.store(true, Ordering::SeqCst);
    let before = radio.lock().expect("sim").sent.len();
    let ctx = egui::Context::default();
    assert!(dialog.download_checked(&ctx) && !dialog.read_list(&ctx)); // one transfer at a time
    assert!(!dialog.request_close() && dialog.status() == RUNNING);
    wait_for_command(&radio, before);
    dialog.stop();
    assert!(dialog.stopping() && dialog.status() == STOPPING);
    hold.store(false, Ordering::SeqCst);
    dialog.wait(WAIT);
    assert_eq!(dialog.status(), STOPPED);
    assert!(!dialog.running() && !dialog.stopping() && dialog.saved().is_empty());
    dialog.out_dir = "  ".into();
    assert!(!dialog.read_list(&ctx) && dialog.status().contains("錄音資料夾"));
    assert!(dialog.request_close());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_stopped_list_says_it_may_be_incomplete() {
    let dir = scratch("stop-list");
    let radio = radio();
    let hold = Arc::new(AtomicBool::new(true));
    let mut dialog = dialog_with(Some(held(&radio, &hold)), &dir, &[], None);
    assert!(dialog.read_list(&egui::Context::default()));
    wait_for_command(&radio, 0);
    dialog.stop();
    hold.store(false, Ordering::SeqCst);
    dialog.wait(WAIT);
    assert_eq!(
        dialog.status(),
        "對講機回報 1 筆錄音（共 3 筆）。已停止，清單可能不完整。勾選後按「下載勾選的錄音」。"
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_save_failure_is_reported_and_the_next_record_is_still_read() {
    let dir = scratch("save-fail");
    let blocker = dir.join("not-a-folder");
    std::fs::write(&blocker, b"x").expect("file");
    let radio = radio();
    let mut dialog = dialog(&radio, &blocker);
    run(&mut dialog, RecordingDialog::download_all);
    let commands = radio
        .lock()
        .expect("sim")
        .sent
        .windows(11)
        .filter(|w| w == b"Record Read")
        .count();
    assert_eq!(commands, 3); // every record was still read
    assert!(dialog.saved().is_empty());
    assert!(
        dialog.status().starts_with("第 3 筆無法儲存到 "),
        "{}",
        dialog.status()
    );
    assert_eq!(dialog.error(), dialog.status());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn lookup_names_ids_missing_from_project_contacts() {
    let dir = scratch("lookup");
    let calls: Rc<RefCell<Vec<u64>>> = Rc::default();
    let log = Rc::clone(&calls);
    let lookup: NameLookup = Box::new(move |id| {
        log.borrow_mut().push(id);
        match id {
            9_998_765 => "QA0AX 不該出現".into(),
            12345 => "QA0ABT Zhi Ming".into(),
            _ => String::new(),
        }
    });
    let factory = shared_factory(radio());
    let mut dialog = dialog_with(Some(factory), &dir, &[(9_998_765, "QA0AJS")], Some(lookup));
    run(&mut dialog, RecordingDialog::read_list);
    let partners: Vec<&str> = dialog.rows().iter().map(|r| r.partner.as_str()).collect();
    assert_eq!(partners, ["QA0AJS", "QA0ABT Zhi Ming", ""]);
    assert_eq!(dialog.rows()[0].partner_tip, "");
    assert!(dialog.rows()[1].partner_tip.contains("RadioID"));
    run(&mut dialog, RecordingDialog::read_list);
    // project contacts win; each other ID is looked up once
    assert_eq!(*calls.borrow(), [12345, 9]);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn radioid_names_from_downloaded_cache() {
    let dir = scratch("radioid");
    assert!(RadioIdNames::from_cache(&dir).is_none()); // not downloaded: no names, no error
    let csv = [
        &b"RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n"[..],
        b"19990186,QA0AKN,Wrong,,,,\r\n",
        b"9990186,QA0ABT,Zhi Ming,,New Taipei City,Taipei,Taiwan\r\n",
        "9991032,QA0AU,小美,林,New Taipei City,Taipei,Taiwan".as_bytes(), // last line, no newline
    ]
    .concat();
    std::fs::write(dir.join("user.csv"), csv).expect("csv");
    let mut names = RadioIdNames::from_cache(&dir).expect("cache");
    assert_eq!(names.name(9_990_186), "QA0ABT Zhi Ming");
    std::fs::remove_file(dir.join("user.csv")).expect("remove"); // read once, then searched in memory
    assert_eq!(names.name(9_991_032), "QA0AU 小美 林");
    assert_eq!(names.name(46621), ""); // a talkgroup
    assert_eq!(names.name(1), ""); // the all-call ID
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn select_all_none_and_sorting_keep_checked_records() {
    let dir = scratch("sort");
    let radio = radio();
    let mut dialog = dialog(&radio, &dir);
    run(&mut dialog, RecordingDialog::read_list);
    assert_eq!(dialog.sort(), None);
    assert_eq!(order(&dialog), ["1", "2", "3"]); // list order at first
    dialog.set_all_checked(false);
    assert!(dialog.checked().is_empty());
    dialog.set_all_checked(true);
    assert_eq!(numbers(&dialog.checked()), [1, 2, 3]);
    dialog.sort_by(4, true); // what clicking 長度 does
    // 6.7 s, 9.0 s, 18.7 s (as text, "18.7" would come first)
    assert_eq!(order(&dialog), ["2", "3", "1"]);
    dialog.set_checked(2, false); // record 2, now the top row
    assert_eq!(numbers(&dialog.checked()), [1, 3]);
    assert_eq!(dialog.row_of(1), Some(2));
    run(&mut dialog, RecordingDialog::download_checked);
    assert_eq!(saved_numbers(&dialog), [1, 3]);
    assert_eq!(order(&dialog), ["2", "3", "1"]);
    assert!(dialog.rows()[2].cells()[6].starts_with("20250607_1234_1_R09998765.ambe"));
    assert!(dialog.rows()[0].file.is_none() && !dialog.rows()[0].checked);
    dialog.sort_by(6, true); // 檔案: the row without a file goes last
    assert_eq!(order(&dialog), ["1", "3", "2"]);
    dialog.sort_by(6, false);
    assert_eq!(order(&dialog), ["3", "1", "2"]);
    dialog.sort_by(2, true); // 對方 ID as numbers: 9, 12345, 9998765
    assert_eq!(order(&dialog), ["3", "2", "1"]);
    dialog.sort_by(4, false);
    run(&mut dialog, RecordingDialog::read_list); // a new list keeps the chosen sort
    assert_eq!(order(&dialog), ["1", "3", "2"]);
    assert_eq!(dialog.row_of(9), None);
    dialog.sort_by(1, false); // a text column
    let directions: Vec<String> = dialog.rows().iter().map(|r| r.cells()[1].clone()).collect();
    assert_eq!(directions, ["發射", "接收", "接收"]);
    dialog.click_header(0);
    assert_eq!(dialog.sort(), Some((0, true)));
    dialog.click_header(0);
    assert_eq!(
        (dialog.sort(), order(&dialog)),
        (Some((0, false)), vec!["3".into(), "2".into(), "1".into()])
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn total_line() {
    assert_eq!(
        (
            duration_text(34.4),
            duration_text(192.0),
            duration_text(3725.0)
        ),
        (
            "34 秒".into(),
            "3 分 12 秒".into(),
            "1 小時 2 分 5 秒".into()
        )
    );
    // Python's round(): ties go to the even number
    assert_eq!(
        (duration_text(0.5), duration_text(1.5), duration_text(2.5)),
        ("0 秒".into(), "2 秒".into(), "2 秒".into())
    );
    let dir = scratch("total");
    let radio = radio();
    let mut first = dialog(&radio, &dir);
    assert_eq!(first.summary(), "");
    run(&mut first, RecordingDialog::read_list);
    assert_eq!(first.summary(), "共 3 筆，合計 34 秒"); // 18.7 + 6.7 + 9.0 s
    let mut other = dialog(&radio, &dir);
    run(&mut other, RecordingDialog::download_all);
    assert_eq!(other.summary(), "共 3 筆，合計 34 秒");
    assert!(other.status().contains("合計 34 秒"));
    let call = |number, direction, call_id, length, time: &str| RecordInfo {
        number,
        total: 2,
        direction,
        call_id,
        length,
        time: time.into(),
    };
    let long_calls = Arc::new(Mutex::new(SimulatedRecorder::with_records(vec![
        (call(1, 'R', 46621, 27000, "2026/10/09 10:00"), Vec::new()),
        (call(2, 'S', 1, 26550, "2026/10/09 10:05"), Vec::new()),
    ])));
    let mut third = dialog(&long_calls, &dir);
    run(&mut third, RecordingDialog::read_list);
    assert_eq!(third.summary(), "共 2 筆，合計 1 分 59 秒"); // 60 s + 59 s
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn all_records_plan_reads_up_to_999() {
    assert_eq!(all_records_plan(3), 2..=3);
    assert!(all_records_plan(1).is_empty() && all_records_plan(0).is_empty());
    assert_eq!(all_records_plan(5000), 2..=999);
}

#[test]
fn remembered_paths_are_empty_for_the_defaults() {
    assert_eq!(
        remembered_paths(
            " D:/rec ",
            "C:\\wav\\ambep2.exe",
            "R",
            "C:\\wav\\ambep2.exe"
        ),
        ("D:/rec".into(), String::new())
    );
    assert_eq!(
        remembered_paths("R", "D:/tools/ambep2.exe", "R", "C:\\wav\\ambep2.exe"),
        (String::new(), "D:/tools/ambep2.exe".into())
    );
}

#[test]
fn the_simulated_port_shares_one_recorder_per_dialog() {
    let dir = scratch("sim-port");
    let mut dialog = dialog_with(None, &dir, &[], None);
    dialog.set_port(Some("__sim__".into()));
    run(&mut dialog, RecordingDialog::read_list);
    run(&mut dialog, RecordingDialog::download_all);
    assert_eq!(saved_numbers(&dialog), [1, 2, 3]);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
