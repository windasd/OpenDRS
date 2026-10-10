//! 備份管理, headless: the list with its markers, opening (after the unsaved-changes question),
//! comparing, deleting and cleaning up with the Python version's confirmations. Every test uses a
//! scratch data folder with real backup pairs.

use std::fs;
use std::path::{Path, PathBuf};

use opendrs_core::codeplug::Counts;
use opendrs_core::f136;
use opendrs_core::radio_link::{RadioIdentity, ReadResult, save_backup};
use opendrs_gui::app::{Dialog, OpenDrsApp};
use opendrs_gui::dialogs::backups::{
    BackupAction, BackupDialog, EMPTY, LAST_WRITE_NOTE, Reply, files_text, newest_before_write,
    row_text, summary_text,
};

const MODEL: &str = "DR-8500UV";
const OLDEST: &str = "20261001_090000_read_DR-8500UV";
const WRITE: &str = "20261002_090000_before_write_DR-8500UV";
const READ: &str = "20261003_090000_read_DR-8500UV";
const BROKEN: &str = "20261004_090000_read_DR-8500UV";

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opendrs-gui-backups-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch folder");
        Scratch(dir)
    }

    fn backups(&self) -> PathBuf {
        self.0.join("backups")
    }

    fn file(&self, stem: &str, ext: &str) -> PathBuf {
        self.backups().join(format!("{stem}.{ext}"))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Four backups, newest first: unreadable, read, before_write (written by `save_backup`), read.
fn fill(scratch: &Scratch) {
    let folder = scratch.backups();
    fs::create_dir_all(&folder).expect("backup folder");
    for stem in [OLDEST, READ] {
        fs::copy(fixture("mmdvm_20241127.F136"), scratch.file(stem, "F136")).expect("copy");
        fs::write(scratch.file(stem, "bin"), b"raw stream").expect("bin");
    }
    let project = f136::load(fixture("dh98vu_20250811.F136")).expect("fixture");
    let read = ReadResult {
        counts: Counts::of(&project),
        project,
        stream: vec![0x5A; 64],
        identity: RadioIdentity {
            firmware: "308E.TARSCB.003".into(),
            model: MODEL.into(),
            band: "400-480".into(),
            raw: Vec::new(),
        },
    };
    let path = save_backup(&read, &folder, "before_write", "20261002_090000").expect("save");
    assert_eq!(path, scratch.file(WRITE, "F136"));
    fs::write(scratch.file(BROKEN, "F136"), b"x\r\n\r\n").expect("broken backup");
}

fn manager(app: &mut OpenDrsApp) -> &mut BackupDialog {
    app.windows.backups.as_mut().expect("the dialog is open")
}

fn times(app: &mut OpenDrsApp) -> Vec<String> {
    manager(app)
        .backups()
        .iter()
        .map(|b| b.time_text())
        .collect()
}

fn select(app: &mut OpenDrsApp, stem: &str) {
    let row = manager(app)
        .backups()
        .iter()
        .position(|b| b.path.file_stem().is_some_and(|s| s == stem))
        .expect("listed");
    app.backup_action(BackupAction::Select(row));
}

fn question(app: &mut OpenDrsApp) -> (String, String) {
    let (title, text) = manager(app).question().expect("a question");
    (title.to_string(), text.to_string())
}

fn message(app: &OpenDrsApp) -> (String, String) {
    match &app.dialog {
        Some(Dialog::Message { title, text }) => (title.clone(), text.clone()),
        other => panic!("expected a message, got {other:?}"),
    }
}

#[test]
fn lists_backups_newest_first_with_markers() {
    let scratch = Scratch::new("list");
    fill(&scratch);
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.backup_manager();
    assert!(app.dialog.is_none());
    assert_eq!(
        times(&mut app),
        [
            "2026-10-04 09:00:00",
            "2026-10-03 09:00:00",
            "2026-10-02 09:00:00",
            "2026-10-01 09:00:00"
        ]
    );
    let backups = manager(&mut app).backups().to_vec();
    assert_eq!(newest_before_write(&backups), Some(2));
    let broken = row_text(&backups[0], false);
    assert!(broken.unreadable && broken.note.starts_with("無法讀取："));
    assert_eq!(&broken.cells[3..6], ["—", "—", "—"]);
    let write = row_text(&backups[2], true);
    assert_eq!(write.cells[1], "寫入前");
    assert_eq!(write.cells[4], "24"); // the fixture's channels
    assert_eq!(write.note, LAST_WRITE_NOTE);
    assert_eq!(files_text(&backups[1]), format!("{READ}.F136、{READ}.bin"));
    assert!(summary_text(&backups).starts_with("共 4 份備份，合計 "));
}

#[test]
fn a_missing_folder_is_explained() {
    let scratch = Scratch::new("missing");
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.backup_manager();
    assert!(manager(&mut app).backups().is_empty());
    assert_eq!(summary_text(manager(&mut app).backups()), EMPTY);
    app.backup_action(BackupAction::OpenFolder);
    assert_eq!(
        message(&app),
        (
            "備份管理".into(),
            format!(
                "備份資料夾還不存在：\n{}\n\n讀取對講機後會自動建立。",
                scratch.backups().display()
            )
        )
    );
}

#[test]
fn delete_asks_first_and_warns_about_the_last_write_backup() {
    let scratch = Scratch::new("delete");
    fill(&scratch);
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.backup_manager();
    select(&mut app, WRITE);
    app.backup_action(BackupAction::Delete);
    let (title, text) = question(&mut app);
    assert_eq!(title, "刪除備份");
    assert_eq!(
        text,
        format!(
            "要刪除 2026-10-02 09:00:00 的「寫入前」備份嗎？\n\n{WRITE}.F136\n{WRITE}.bin\n\n\
             這是最近一次寫入對講機之前的備份；寫入後若有問題，就是用它寫回。\n\n刪除後無法復原。"
        )
    );
    app.backup_action(BackupAction::Reply(Reply::No));
    assert!(manager(&mut app).question().is_none());
    assert!(scratch.file(WRITE, "F136").is_file() && scratch.file(WRITE, "bin").is_file());

    app.backup_action(BackupAction::Delete);
    app.backup_action(BackupAction::Reply(Reply::Yes));
    assert!(!scratch.file(WRITE, "F136").exists() && !scratch.file(WRITE, "bin").exists());
    assert_eq!(manager(&mut app).backups().len(), 3);
    assert!(manager(&mut app).selected().is_none());
    assert!(app.dialog.is_none());

    select(&mut app, READ);
    app.backup_action(BackupAction::Delete);
    let (_, text) = question(&mut app);
    assert!(!text.contains("這是最近一次寫入"), "{text}");
}

#[test]
fn cleanup_keeps_the_newest_and_the_last_write_backup() {
    let scratch = Scratch::new("cleanup");
    fill(&scratch);
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.backup_manager();
    manager(&mut app).keep = 1;
    app.backup_action(BackupAction::Cleanup);
    let (title, text) = question(&mut app);
    assert_eq!(title, "清理備份");
    assert!(
        text.starts_with("將刪除 2 份較舊的備份（2026-10-01 09:00:00 ～ 2026-10-03 09:00:00，共 "),
        "{text}"
    );
    assert!(
        text.ends_with(
            "），保留最近 1 份。\n另外保留最近一次寫入前的備份（2026-10-02 09:00:00）。\n\n刪除後無法復原。"
        ),
        "{text}"
    );
    app.backup_action(BackupAction::Reply(Reply::Yes));
    assert_eq!(
        times(&mut app),
        ["2026-10-04 09:00:00", "2026-10-02 09:00:00"]
    );
    for stem in [OLDEST, READ] {
        assert!(!scratch.file(stem, "F136").exists() && !scratch.file(stem, "bin").exists());
    }
    app.backup_action(BackupAction::Cleanup);
    assert!(
        manager(&mut app).question().is_none(),
        "nothing left to clean"
    );
}

#[test]
fn opening_asks_about_unsaved_changes_and_detaches_the_backup() {
    let scratch = Scratch::new("open");
    fill(&scratch);
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    assert!(app.open_path(&fixture("dh98vu_20250811.F136")));
    let mut edited = app.project().clone();
    edited.contacts.pop();
    app.commit(edited, "編輯");
    assert!(app.doc.is_dirty());
    app.backup_manager();

    select(&mut app, READ);
    app.backup_action(BackupAction::Open);
    assert_eq!(
        question(&mut app),
        ("尚未儲存".into(), "目前的修改尚未儲存，要先儲存嗎？".into())
    );
    app.backup_action(BackupAction::Reply(Reply::Cancel));
    assert!(app.windows.backups.is_some() && app.doc.is_dirty());

    app.backup_action(BackupAction::Open);
    app.backup_action(BackupAction::Reply(Reply::Discard));
    assert!(app.windows.backups.is_none(), "the dialog closes");
    assert!(!app.doc.is_dirty() && app.doc.path().is_none());
    let backup = f136::load(scratch.file(READ, "F136")).expect("backup");
    assert!(app.project() == &backup, "the backup is the project now");
    let title = app.doc.window_title();
    assert!(
        title.contains(&format!("備份 {READ}.F136（存檔時另存新檔）")),
        "{title}"
    );
    let (settings, _) = opendrs_core::settings::load_settings(&scratch.0);
    assert!(
        settings.recent_files.iter().all(|p| !p.contains(READ)),
        "a backup is not a recent file"
    );
}

#[test]
fn an_unreadable_backup_does_not_open() {
    let scratch = Scratch::new("broken");
    fill(&scratch);
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.backup_manager();
    select(&mut app, BROKEN);
    app.backup_action(BackupAction::Open);
    assert_eq!(message(&app).0, "開啟失敗");
    assert!(app.windows.backups.is_some(), "the dialog stays open");
    app.dialog = None;
    select(&mut app, OLDEST);
    app.backup_action(BackupAction::Open);
    assert!(app.windows.backups.is_none());
    assert!(app.doc.has_project() && app.doc.path().is_none());
}

#[test]
fn compare_needs_a_project_then_shows_the_report() {
    let scratch = Scratch::new("compare");
    fill(&scratch);
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.backup_manager();
    select(&mut app, READ);
    app.backup_action(BackupAction::Compare);
    assert_eq!(
        message(&app),
        ("比較專案".into(), "請先開啟專案檔，或從對講機讀取。".into())
    );
    app.dialog = None;
    assert!(app.open_path(&fixture("dh98vu_20250811.F136")));
    select(&mut app, READ);
    app.backup_action(BackupAction::Compare);
    let diff = app.windows.diff.as_ref().expect("the comparison");
    assert!(
        diff.title.ends_with(&format!("新：{READ}.F136")),
        "{}",
        diff.title
    );
    assert!(app.windows.backups.is_some(), "the manager stays open");
}

/// Hold `path` open without sharing: until the handle is dropped, Windows cannot delete it.
#[cfg(windows)]
fn lock(path: &Path) -> fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .share_mode(0)
        .open(path)
        .expect("exclusive handle")
}

/// A file another program holds open makes a deletion fail half-way.
#[cfg(windows)]
#[test]
fn failed_deletions_are_reported_and_the_folder_reread() {
    let scratch = Scratch::new("fail");
    fill(&scratch);
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.backup_manager();
    let stuck = lock(&scratch.file(READ, "bin"));
    select(&mut app, READ);
    app.backup_action(BackupAction::Delete);
    app.backup_action(BackupAction::Reply(Reply::Yes));
    let (title, text) = message(&app);
    assert_eq!(title, "刪除失敗");
    assert!(
        text.starts_with(&format!(
            "{READ}.F136

"
        )),
        "{text}"
    );
    // The .F136 went first, so the half-deleted pair is no longer a backup.
    assert_eq!(manager(&mut app).backups().len(), 3);
    app.dialog = None;

    let stuck_too = lock(&scratch.file(OLDEST, "bin"));
    manager(&mut app).keep = 1;
    app.backup_action(BackupAction::Cleanup);
    app.backup_action(BackupAction::Reply(Reply::Yes));
    let (title, text) = message(&app);
    assert_eq!(title, "部分備份未刪除");
    assert!(text.starts_with(&format!("{OLDEST}.F136：")), "{text}");
    assert_eq!(
        times(&mut app),
        ["2026-10-04 09:00:00", "2026-10-02 09:00:00"]
    );
    drop((stuck, stuck_too));
}
