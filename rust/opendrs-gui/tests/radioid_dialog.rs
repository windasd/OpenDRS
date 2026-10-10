//! 從 RadioID 匯入聯絡人, headless: loading on the background thread, filters and ticks, the
//! capacity question, the import's undo step, and cancelling a download by closing the window.
//! Nothing goes to the network: the cache folder is pre-filled or the HTTP client is a closure.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use opendrs_contacts::contact_import::existing_private_ids;
use opendrs_contacts::radioid::http::{HttpError, HttpRequest, HttpResponse};
use opendrs_gui::app::{Dialog, OpenDrsApp};
use opendrs_gui::dialogs::radioid::{RadioIdAction, RadioIdDialog, SharedClient};

const NEW_TW: u32 = 9_997_001; // the fixture's contacts are 9990001–9990344
const NEW_TW_2: u32 = 9_997_002;
const CANADA: u32 = 1_023_007;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dh98vu_20250811.F136")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opendrs-gui-radioid-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch folder");
        Scratch(dir)
    }

    fn cache(&self) -> PathBuf {
        self.0.join("radioid")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app(scratch: &Scratch) -> OpenDrsApp {
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    assert!(app.open_path(&fixture()));
    app
}

/// A private-call ID the fixture already has.
fn existing_id(app: &OpenDrsApp) -> u32 {
    existing_private_ids(app.project())
        .into_iter()
        .filter(|&id| id <= 16_776_415)
        .min()
        .expect("the fixture has private-call contacts")
}

/// Three Taiwan users (one already a contact) and one in Canada.
fn csv(existing: u32) -> String {
    format!(
        "RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n\
         {existing},QA0EX,Old,,Taipei,,Taiwan\r\n\
         {NEW_TW},QA0BD,James,,New Taipei,Northern Taiwan,Taiwan\r\n\
         {NEW_TW_2},QA0ADN,Chen,Wei,Taichung,Central Taiwan,Taiwan\r\n\
         {CANADA},QA0AKM,Hans Peter,,Cornwall,Ontario,Canada\r\n"
    )
}

fn fill_cache(scratch: &Scratch, existing: u32) {
    std::fs::create_dir_all(scratch.cache()).expect("cache folder");
    std::fs::write(scratch.cache().join("user.csv"), csv(existing)).expect("cache file");
}

/// A client that must not be used; counts the calls.
fn offline(calls: &Arc<AtomicUsize>) -> SharedClient {
    let calls = Arc::clone(calls);
    Arc::new(move |_: &HttpRequest| -> Result<HttpResponse, HttpError> {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(HttpError::Connect("offline".into()))
    })
}

fn dialog(app: &mut OpenDrsApp) -> &mut RadioIdDialog {
    app.windows.radioid.as_mut().expect("the dialog is open")
}

/// Start the first load and wait for the thread to finish.
fn load(app: &mut OpenDrsApp) {
    dialog(app).start(None);
    let deadline = Instant::now() + Duration::from_secs(30);
    while dialog(app).is_loading() {
        app.radioid_poll();
        assert!(Instant::now() < deadline, "loading timed out");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn message(app: &OpenDrsApp) -> (String, String) {
    match &app.dialog {
        Some(Dialog::Message { title, text }) => (title.clone(), text.clone()),
        other => panic!("expected a message, got {other:?}"),
    }
}

fn shown_ids(app: &mut OpenDrsApp) -> Vec<u32> {
    dialog(app).shown().iter().map(|u| u.radio_id).collect()
}

#[test]
fn needs_a_project() {
    let scratch = Scratch::new("no-project");
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.import_from_radioid();
    assert!(app.windows.radioid.is_none());
    assert_eq!(
        message(&app),
        (
            "匯入聯絡人".into(),
            "請先開啟專案檔，或從對講機讀取。".into()
        )
    );
}

#[test]
fn imports_ticked_users_from_the_cache() {
    let scratch = Scratch::new("import");
    let mut app = app(&scratch);
    let existing = existing_id(&app);
    let contacts = app.project().contacts.len();
    fill_cache(&scratch, existing);
    app.import_from_radioid(); // the real client: a cache exists, so it only parses
    load(&mut app);
    assert!(app.dialog.is_none(), "{:?}", app.dialog);
    let status = dialog(&mut app).status().to_string();
    assert!(status.starts_with("本機資料 4 筆（下載於 20"), "{status}");

    let labels: Vec<String> = dialog(&mut app)
        .regions()
        .iter()
        .map(|(label, _)| label.clone())
        .collect();
    assert_eq!(labels, ["全部地區", "Taiwan（3）", "Canada（1）"]);
    assert_eq!(dialog(&mut app).region(), 1, "Taiwan by default");
    assert_eq!(shown_ids(&mut app), [existing, NEW_TW, NEW_TW_2]);
    assert!(dialog(&mut app).is_existing(existing));

    app.radioid_action(RadioIdAction::Toggle {
        radio_id: NEW_TW,
        on: true,
    });
    assert!(dialog(&mut app).set_region("Canada"));
    dialog(&mut app).apply_filter();
    assert_eq!(shown_ids(&mut app), [CANADA]);
    assert!(dialog(&mut app).is_checked(NEW_TW), "ticks survive filters");
    app.radioid_action(RadioIdAction::SelectAll);
    assert_eq!(dialog(&mut app).tally().chosen, 2);
    app.radioid_action(RadioIdAction::SelectNone);
    assert!(!dialog(&mut app).is_checked(CANADA));
    assert!(
        dialog(&mut app).is_checked(NEW_TW),
        "only the shown users are unticked"
    );

    dialog(&mut app).set_region("Taiwan");
    dialog(&mut app).apply_filter();
    app.radioid_action(RadioIdAction::Toggle {
        radio_id: existing,
        on: true,
    });
    assert_eq!(
        dialog(&mut app).tally().text(),
        format!(
            "符合 3 筆（1 筆已存在）｜已勾選 2 筆（新增 1、已存在 1）｜匯入後 {} / 1,000",
            contacts + 1
        )
    );

    app.radioid_action(RadioIdAction::Accept);
    assert!(app.windows.radioid.is_none(), "the dialog closes");
    assert_eq!(app.project().contacts.len(), contacts + 1);
    assert_eq!(app.doc.undo_text(), "復原 匯入聯絡人");
    let (title, text) = message(&app);
    assert_eq!(title, "匯入完成");
    assert!(text.starts_with("新增 1 筆聯絡人。"), "{text}");
}

#[test]
fn asks_before_importing_beyond_the_capacity() {
    let scratch = Scratch::new("capacity");
    let mut app = app(&scratch);
    let existing = existing_id(&app);
    fill_cache(&scratch, existing);
    let calls = Arc::new(AtomicUsize::new(0));
    let mut fresh = RadioIdDialog::new(scratch.cache(), offline(&calls));
    fresh.sync_project(app.project(), app.doc.revision());
    fresh.capacity = app.project().contacts.len(); // one new contact is one too many
    app.windows.radioid = Some(fresh);
    load(&mut app);
    app.radioid_action(RadioIdAction::Toggle {
        radio_id: NEW_TW,
        on: true,
    });
    let text = dialog(&mut app).tally().text();
    assert!(text.ends_with("　⚠ 超過容量"), "{text}");

    app.radioid_action(RadioIdAction::Accept);
    assert!(dialog(&mut app).asking());
    app.radioid_action(RadioIdAction::Answer(false));
    assert!(!dialog(&mut app).asking(), "否 goes back to the list");
    assert!(!app.doc.can_undo(), "nothing imported");

    app.radioid_action(RadioIdAction::Accept);
    app.radioid_action(RadioIdAction::Answer(true));
    assert!(app.windows.radioid.is_none());
    assert_eq!(app.doc.undo_text(), "復原 匯入聯絡人");
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no network");
}

#[test]
fn downloads_when_there_is_no_cache() {
    let scratch = Scratch::new("download");
    let mut app = app(&scratch);
    let body = csv(existing_id(&app));
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let client: SharedClient = Arc::new(move |_: &HttpRequest| {
        counter.fetch_add(1, Ordering::SeqCst);
        let headers = vec![("Content-Length".to_string(), body.len().to_string())];
        Ok(HttpResponse::new(
            headers,
            Box::new(std::io::Cursor::new(body.clone().into_bytes())),
        ))
    });
    let mut fresh = RadioIdDialog::new(scratch.cache(), client);
    fresh.sync_project(app.project(), app.doc.revision());
    app.windows.radioid = Some(fresh);
    load(&mut app);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(scratch.cache().join("user.csv").is_file());
    let status = dialog(&mut app).status().to_string();
    assert!(status.starts_with("本機資料 4 筆（下載於 "), "{status}");
    assert_eq!(dialog(&mut app).shown().len(), 3);
}

#[test]
fn a_failed_download_is_reported() {
    let scratch = Scratch::new("failure");
    let mut app = app(&scratch);
    let client: SharedClient = Arc::new(|_: &HttpRequest| -> Result<HttpResponse, HttpError> {
        Err(HttpError::Status(500))
    });
    let mut fresh = RadioIdDialog::new(scratch.cache(), client);
    fresh.sync_project(app.project(), app.doc.revision());
    app.windows.radioid = Some(fresh);
    load(&mut app);
    assert_eq!(dialog(&mut app).status(), "失敗：radioid.net 回應 HTTP 500");
    assert_eq!(
        message(&app),
        ("RadioID".into(), "radioid.net 回應 HTTP 500".into())
    );
    assert!(dialog(&mut app).shown().is_empty());
}

/// A body that never ends; the guard tells the test when the download dropped it.
struct Endless {
    _guard: mpsc::Sender<()>,
}

impl Read for Endless {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        std::thread::sleep(Duration::from_millis(1));
        let n = buf.len().min(16 * 1024);
        buf[..n].fill(b'x');
        Ok(n)
    }
}

#[test]
fn closing_the_window_cancels_the_download() {
    let scratch = Scratch::new("cancel");
    let mut app = app(&scratch);
    let (guard, dropped) = mpsc::channel::<()>();
    let client: SharedClient = Arc::new(move |_: &HttpRequest| {
        let body = Endless {
            _guard: guard.clone(),
        };
        Ok(HttpResponse::new(Vec::new(), Box::new(body)))
    });
    let mut fresh = RadioIdDialog::new(scratch.cache(), client);
    fresh.sync_project(app.project(), app.doc.revision());
    app.windows.radioid = Some(fresh);
    dialog(&mut app).start(None);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !dialog(&mut app).status().starts_with("下載中… ") {
        app.radioid_poll();
        assert!(Instant::now() < deadline, "no progress");
        std::thread::sleep(Duration::from_millis(5));
    }

    app.radioid_action(RadioIdAction::Cancel);
    assert!(app.windows.radioid.is_none());
    // Every sender is gone once the thread has stopped and dropped the client and the body.
    assert_eq!(
        dropped.recv_timeout(Duration::from_secs(30)),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
    let left: Vec<_> = std::fs::read_dir(scratch.cache())
        .expect("cache folder")
        .collect();
    assert!(left.is_empty(), "no cache and no part file: {left:?}");
}
