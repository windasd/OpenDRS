//! 讀取／寫入對講機 and 校正時鐘 from the main window, against the simulated radio (port of
//! test_radio_panel.py's window tests). No real serial port is opened.

use std::path::{Path, PathBuf};
use std::time::Duration;

use egui_kittest::Harness;
use opendrs_core::codeplug::Counts;
use opendrs_core::f136;
use opendrs_core::radio_session::{Mode, clock_confirmation, write_confirmation};
use opendrs_core::radio_sim::SimulatedRadio;
use opendrs_gui::app::{Dialog, OpenDrsApp, Pending};
use opendrs_gui::document::Document;
use opendrs_gui::radio::{SIMULATED, SIMULATED_LABEL};

const WAIT: Duration = Duration::from_secs(30);

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures")
}

/// The radio of the 2026-10-08 capture.
fn radio() -> SimulatedRadio {
    let stream = std::fs::read(fixtures().join("capture/read_stream.bin")).expect("capture");
    SimulatedRadio::with_stream(&stream, Counts::new(4, 27, 360))
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("opendrs-radio-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch folder");
        Scratch(dir)
    }

    fn backups(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.0.join("backups"))
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

/// The window without a frame, the simulated radio picked in the toolbar.
fn app(dir: &Scratch) -> OpenDrsApp {
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    app.radio.set_ports(Vec::new(), SIMULATED);
    app.radio.simulated = Some(radio());
    app
}

fn finish(app: &mut OpenDrsApp) {
    let (mode, result) = app.radio.wait(WAIT).expect("the transfer ends");
    app.radio_finished(mode, result);
}

fn message(app: &OpenDrsApp) -> (String, String) {
    match &app.dialog {
        Some(Dialog::Message { title, text }) => (title.clone(), text.clone()),
        other => panic!("expected a message box, got {other:?}"),
    }
}

#[test]
fn a_read_loads_the_radio_as_an_unsaved_project() {
    let dir = Scratch::new("read");
    let mut app = app(&dir);
    app.read_from_radio();
    assert!(app.transfer_running());
    finish(&mut app);
    assert!(!app.transfer_running() && app.dialog.is_none());
    assert!(app.doc.has_project() && app.doc.is_dirty() && app.doc.path().is_none());
    assert_eq!(app.project().zones.len(), 4);
    assert!(app.status.starts_with("已讀取 DRS-8500"), "{}", app.status);
    let backups = dir.backups();
    assert_eq!(
        backups.len(),
        2,
        "the project and the raw stream: {backups:?}"
    );
    assert!(backups[0].ends_with("_read_DRS-8500.F136"), "{backups:?}");
    assert_eq!(app.radio.log[0], format!("讀取：連線到 {SIMULATED_LABEL}…"));
    assert!(app.radio.log[1].starts_with("已讀取：DRS-8500"));
    assert_eq!(app.radio.status, "讀取完成");
    let (settings, _) = opendrs_core::settings::load_settings(&dir.0);
    assert_eq!(settings.radio_port, SIMULATED, "the port is remembered");
}

#[test]
fn unsaved_changes_are_offered_for_saving_before_a_read() {
    let dir = Scratch::new("dirty");
    let mut app = app(&dir);
    let project = f136::load(fixtures().join("dh98vu_20250811.F136")).expect("fixture");
    app.doc = Document::unsaved(project, "test");
    app.read_from_radio();
    assert_eq!(app.dialog, Some(Dialog::ConfirmDiscard(Pending::ReadRadio)));
    assert!(!app.transfer_running(), "nothing starts before the answer");
}

#[test]
fn a_write_asks_first_and_reports_the_checksum() {
    let dir = Scratch::new("write");
    let mut app = app(&dir);
    assert!(app.open_path(&fixtures().join("capture/written_project.F136")));
    app.write_to_radio();
    let expected = write_confirmation(SIMULATED_LABEL, app.project(), &dir.0.join("backups"));
    assert_eq!(
        app.dialog,
        Some(Dialog::Radio {
            mode: Mode::Write,
            text: expected,
            revision: app.doc.revision(),
        })
    );
    app.dialog = None; // 是
    assert!(app.start_radio(Mode::Write));
    finish(&mut app);
    let (title, text) = message(&app);
    assert_eq!(title, "寫入對講機");
    assert!(text.starts_with("寫入完成，對講機確認校驗和 0x"), "{text}");
    assert!(text.ends_with("對講機正在套用設定，請等它重新啟動後再操作。"));
    assert!(dir.backups()[0].ends_with("_before_write_DRS-8500.F136"));
    assert!(!app.doc.is_dirty(), "the project itself is untouched");
}

#[test]
fn a_clock_sync_writes_the_radio_back() {
    let dir = Scratch::new("clock");
    let mut app = app(&dir);
    app.sync_radio_clock();
    assert_eq!(
        app.dialog,
        Some(Dialog::Radio {
            mode: Mode::Clock,
            text: clock_confirmation(SIMULATED_LABEL),
            revision: app.doc.revision(),
        })
    );
    app.dialog = None;
    assert!(app.start_radio(Mode::Clock));
    finish(&mut app);
    let (title, text) = message(&app);
    assert_eq!(title, "校正對講機時鐘");
    assert!(text.starts_with("對講機時鐘已設為 "), "{text}");
    assert!(!app.doc.has_project(), "the editor is not affected");
}

#[test]
fn a_failed_write_is_reported_under_the_radio_title() {
    let dir = Scratch::new("fail");
    let mut app = app(&dir);
    let mut faulty = radio();
    faulty.corrupt_write_checksum = true;
    app.radio.simulated = Some(faulty);
    assert!(app.open_path(&fixtures().join("capture/written_project.F136")));
    assert!(app.start_radio(Mode::Write));
    finish(&mut app);
    let (title, text) = message(&app);
    assert_eq!(title, "對講機");
    assert!(!text.is_empty());
    assert_eq!(app.radio.log.last(), Some(&format!("錯誤：{text}")));
}

#[test]
fn a_project_without_settings_cannot_be_written() {
    let dir = Scratch::new("unwritable");
    let mut app = app(&dir);
    let mut project = f136::load(fixtures().join("dh98vu_20250811.F136")).expect("fixture");
    project.image.truncate(64);
    app.doc = Document::detached(project, "test");
    app.write_to_radio();
    let (title, text) = message(&app);
    assert_eq!(title, "無法寫入");
    assert!(text.starts_with("專案沒有設定區"), "{text}");
    assert!(!app.transfer_running());
}

#[test]
fn a_port_that_cannot_be_opened_is_reported() {
    let dir = Scratch::new("noport");
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    let missing = "OPENDRS-NO-SUCH-PORT";
    app.radio.set_ports(
        vec![opendrs_gui::radio::PortChoice {
            device: missing.into(),
            label: format!("{missing} — test"),
        }],
        missing,
    );
    assert!(app.start_radio(Mode::Read));
    finish(&mut app);
    let (title, _) = message(&app);
    assert_eq!(title, "對講機");
    assert_eq!(app.radio.log.len(), 2);
    assert_eq!(app.radio.log[0], format!("讀取：連線到 {missing} — test…"));
    assert!(app.radio.log[1].starts_with("錯誤："));
}

#[test]
fn the_toolbar_reads_the_simulated_radio() {
    use egui_kittest::kittest::Queryable;
    let dir = Scratch::new("toolbar");
    let settings = dir.0.clone();
    let mut harness = Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| {
            let mut app = OpenDrsApp::with_context(&cc.egui_ctx, settings, None);
            app.radio.set_ports(Vec::new(), SIMULATED);
            app.radio.simulated = Some(radio());
            app
        });
    harness.run();
    assert_eq!(harness.state().radio.port_label(), SIMULATED_LABEL);
    harness.get_by_label("讀取").click();
    for _ in 0..3000 {
        harness.step();
        if !harness.state().transfer_running() && harness.state().doc.has_project() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.run();
    let app = harness.state();
    assert!(app.doc.has_project() && app.doc.is_dirty());
    assert!(app.doc.window_title().starts_with('*'));
    assert_eq!(dir.backups().len(), 2);
}

#[test]
fn a_write_question_about_an_older_project_is_refused() {
    use egui_kittest::kittest::Queryable;
    let dir = Scratch::new("stale");
    let settings = dir.0.clone();
    let written = fixtures().join("capture/written_project.F136");
    let mut harness = Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| {
            let mut app = OpenDrsApp::with_context(&cc.egui_ctx, settings, Some(written));
            app.radio.set_ports(Vec::new(), SIMULATED);
            app.radio.simulated = Some(radio());
            app
        });
    harness.run();
    let stale = harness.state().doc.revision();
    {
        let app = harness.state_mut();
        app.doc = Document::unsaved(app.project().clone(), "changed meanwhile");
        app.dialog = Some(Dialog::Radio {
            mode: Mode::Write,
            text: "寫入？".into(),
            revision: stale,
        });
    }
    harness.run();
    harness.get_by_label("是").click();
    harness.run();
    let app = harness.state();
    assert!(!app.transfer_running(), "nothing is written");
    let refused = matches!(
        &app.dialog,
        Some(Dialog::Message { title, text }) if title == "寫入對講機" && text.starts_with("詢問之後專案已經改變")
    );
    assert!(refused, "{:?}", app.dialog);
}

#[test]
fn only_one_transfer_runs_at_a_time() {
    let dir = Scratch::new("one");
    let mut app = app(&dir);
    assert!(app.start_radio(Mode::Read));
    assert!(app.busy() && !app.start_radio(Mode::Clock));
    let (title, text) = message(&app);
    assert_eq!(
        (title.as_str(), text.as_str()),
        ("校正對講機時鐘", "請等目前的讀寫完成。")
    );
    app.dialog = None;
    finish(&mut app);
    assert!(!app.busy());
}

#[test]
fn the_window_cannot_close_during_a_transfer() {
    use egui_kittest::kittest::Queryable;
    let dir = Scratch::new("close");
    let settings = dir.0.clone();
    let mut harness = Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| {
            let mut app = OpenDrsApp::with_context(&cc.egui_ctx, settings, None);
            app.radio.set_ports(Vec::new(), SIMULATED);
            app.radio.simulated = Some(radio());
            app
        });
    harness.run();
    harness.get_by_label("讀取").click();
    harness.step();
    assert!(harness.state().transfer_running());
    harness
        .input_mut()
        .viewports
        .entry(eframe::egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(eframe::egui::ViewportEvent::Close);
    harness.step();
    let refused = matches!(
        &harness.state().dialog,
        Some(Dialog::Message { title, .. }) if title == "傳輸中"
    );
    assert!(refused, "{:?}", harness.state().dialog);
    while harness.state().transfer_running() {
        harness.step();
        std::thread::sleep(Duration::from_millis(10));
    }
}
