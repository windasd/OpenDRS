//! 檢查更新 (port of tests/test_update_dialog.py): the manual check, the daily automatic check,
//! and download → 安裝更新 → updater, all with fake services — nothing reaches the network or
//! starts an installer. A scratch settings folder is used, never the real one.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{TimeDelta, Utc};
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use opendrs_gui::app::{Dialog, OpenDrsApp, Pending};
use opendrs_gui::dialogs::update::{CURRENT, MENU_TEXT, UPDATE_LOG};
use opendrs_update::{Asset, PreparedUpdate, Release, UpdateError};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/dh98vu_20250811.F136")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opendrs-gui-update-{name}-{}", std::process::id()));
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

fn release(version: &str, package: bool) -> Release {
    Release {
        tag: format!("v{version}"),
        version: version.into(),
        title: format!("OpenDRS v{version}"),
        notes: "### 新功能\n\n- 甲".into(),
        page_url: format!("https://github.com/windasd/OpenDRS/releases/tag/v{version}"),
        published: "2030-01-02T03:04:05Z".into(),
        package: package.then(|| Asset {
            name: format!("OpenDRS-{version}-windows-x64.zip"),
            url: format!(
                "https://github.com/windasd/OpenDRS/releases/download/v{version}/OpenDRS-{version}-windows-x64.zip"
            ),
            size: 9_000_000,
            sha256: "0".repeat(64),
        }),
        checksum: None,
    }
}

fn fetching(app: &mut OpenDrsApp, found: Release) {
    app.updates.services.fetch = Arc::new(move || Ok(found.clone()));
}

/// Poll the background work until `done` (the threads answer within milliseconds).
fn wait(app: &mut OpenDrsApp, done: impl Fn(&OpenDrsApp) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(app) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the update work"
        );
        app.poll_updates();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn status(app: &OpenDrsApp) -> String {
    app.updates
        .dialog
        .as_ref()
        .expect("the update window")
        .status
        .clone()
}

#[test]
fn manual_check_shows_a_release_without_this_platforms_archive() {
    let dir = Scratch::new("manual");
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    fetching(&mut app, release("999.0.0", false)); // e.g. a release with only the Python build
    app.check_for_updates();
    assert_eq!(status(&app), "正在查詢 GitHub 上的最新版本…");
    wait(&mut app, |a| {
        !a.updates.dialog.as_ref().unwrap().is_checking()
    });
    assert!(
        status(&app).starts_with("有新版本 v999.0.0，但沒有本平台（"),
        "{}",
        status(&app)
    );
    // a failed check says why
    app.updates.services.fetch = Arc::new(|| {
        Err(UpdateError::Message(
            "查詢最新版本：找不到（HTTP 404）".into(),
        ))
    });
    let services = app.updates.services.clone();
    app.updates
        .dialog
        .as_mut()
        .unwrap()
        .start_check(&services, None);
    wait(&mut app, |a| {
        !a.updates.dialog.as_ref().unwrap().is_checking()
    });
    assert_eq!(
        status(&app),
        "無法檢查更新：查詢最新版本：找不到（HTTP 404）"
    );
    // the current version is the newest
    fetching(&mut app, release(CURRENT, true));
    let services = app.updates.services.clone();
    app.updates
        .dialog
        .as_mut()
        .unwrap()
        .start_check(&services, None);
    wait(&mut app, |a| {
        !a.updates.dialog.as_ref().unwrap().is_checking()
    });
    assert_eq!(status(&app), "已是最新版本。");
}

#[test]
fn automatic_check_is_daily_and_quiet_without_an_archive() {
    let dir = Scratch::new("auto");
    std::fs::write(dir.0.join("settings.json"), r#"{"check_updates": false}"#).unwrap();
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    assert!(!app.start_update_check(Utc::now()), "turned off");

    let dir = Scratch::new("auto-on");
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    fetching(&mut app, release("999.0.0", false));
    let now = Utc::now();
    assert!(app.start_update_check(now));
    wait(&mut app, |a| a.updates.known.is_some());
    assert_eq!(app.update_menu_text(), MENU_TEXT); // nothing to install here: stay quiet
    assert!(app.status.is_empty(), "{}", app.status);
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.0.join("settings.json")).unwrap())
            .unwrap();
    assert_eq!(saved["last_update_check"], opendrs_update::now_stamp(now));
    assert!(
        !app.start_update_check(now + TimeDelta::hours(1)),
        "at most daily"
    );

    fetching(&mut app, release("999.0.0", true));
    assert!(app.start_update_check(now + TimeDelta::hours(21)));
    wait(&mut app, |a| {
        a.updates
            .known
            .as_ref()
            .is_some_and(|r| r.package.is_some())
    });
    assert_eq!(app.update_menu_text(), "檢查更新…（有新版本 v999.0.0）");
    assert_eq!(app.status, "有新版本 v999.0.0 可以更新：工具 › 檢查更新…");
    app.check_for_updates(); // shows what the automatic check found, without asking again
    assert!(app.updates.dialog.as_ref().unwrap().release.is_some());
}

/// Fake services installing into `target`: prepare makes a staging folder, launch records calls.
fn installing(app: &mut OpenDrsApp, target: &Path) -> Arc<Mutex<Vec<PathBuf>>> {
    let launched = Arc::new(Mutex::new(Vec::new()));
    let calls = Arc::clone(&launched);
    app.updates.services.target = Some(target.to_path_buf());
    app.updates.services.prepare = Arc::new(|release, target, progress, _cancelled| {
        let staging = target.join(format!(".opendrs-update-{}", release.version));
        std::fs::create_dir_all(staging.join("unpacked/OpenDRS")).unwrap();
        progress(5, 10);
        Ok(PreparedUpdate {
            version: release.version.clone(),
            source: staging.join("unpacked/OpenDRS"),
            target: target.to_path_buf(),
            staging,
        })
    });
    app.updates.services.launch = Arc::new(move |_prepared, log| {
        calls.lock().unwrap().push(log.to_path_buf());
        Ok(())
    });
    fetching(app, release("999.0.0", true));
    launched
}

fn downloaded(app: &mut OpenDrsApp) {
    app.check_for_updates();
    wait(app, |a| !a.updates.dialog.as_ref().unwrap().is_checking());
    assert!(
        status(app).starts_with("有新版本 v999.0.0（下載約 9 MB）。"),
        "{}",
        status(app)
    );
    let services = app.updates.services.clone();
    app.updates
        .dialog
        .as_mut()
        .unwrap()
        .start_download(&services, None);
    assert_eq!(status(app), "正在下載 v999.0.0…");
    wait(app, |a| a.updates.dialog.as_ref().unwrap().ready.is_some());
    assert_eq!(status(app), "999.0.0 已下載並通過 SHA-256 核對。");
}

#[test]
fn accepted_update_hands_over_to_the_updater() {
    let dir = Scratch::new("install");
    let target = dir.0.join("OpenDRS");
    std::fs::create_dir_all(&target).unwrap();
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    let launched = installing(&mut app, &target);
    downloaded(&mut app);
    app.answer_install(false); // 否: the download is thrown away
    assert_eq!(status(&app), "已取消安裝。");
    assert!(!target.join(".opendrs-update-999.0.0").exists());
    app.check_for_updates(); // the open window is kept, not replaced
    assert_eq!(status(&app), "已取消安裝。");

    app.updates.dialog = None;
    downloaded(&mut app);
    app.answer_install(true); // nothing unsaved: straight to the updater
    assert_eq!(*launched.lock().unwrap(), vec![dir.0.join(UPDATE_LOG)]);
    assert!(app.updates.dialog.as_ref().unwrap().ready.is_none());
    assert!(app.dialog.is_none());
}

#[test]
fn unsaved_changes_are_asked_about_and_cancelling_keeps_the_program() {
    let dir = Scratch::new("unsaved");
    let target = dir.0.join("OpenDRS");
    std::fs::create_dir_all(&target).unwrap();
    let settings = dir.0.clone();
    let mut harness = Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| OpenDrsApp::with_context(&cc.egui_ctx, settings, Some(fixture())));
    let launched = installing(harness.state_mut(), &target);
    {
        let app = harness.state_mut();
        let mut edited = app.project().clone();
        edited.contacts[0] = edited.contacts[0].with_name("EDITED", app.codec).unwrap();
        app.commit(edited, "unsaved edit");
        assert!(app.doc.is_dirty());
        downloaded(app);
        app.answer_install(true);
        assert_eq!(
            app.dialog,
            Some(Dialog::ConfirmDiscard(Pending::InstallUpdate))
        );
    }
    harness.run();
    harness.get_by_label("取消").click();
    harness.run();
    let app = harness.state();
    assert!(app.dialog.is_none());
    assert!(
        launched.lock().unwrap().is_empty(),
        "the updater must not start"
    );
    assert_eq!(status(app), "已取消安裝。");
    assert!(!target.join(".opendrs-update-999.0.0").exists());
}

/// A prepare that, like hashing a large archive, only notices the cancel when it is done:
/// it makes the staging folder, then waits for the cancel and returns the finished update.
fn slow_installing(app: &mut OpenDrsApp, target: &Path) {
    app.updates.services.target = Some(target.to_path_buf());
    app.updates.services.prepare = Arc::new(|release, target, _progress, cancelled| {
        let staging = target.join(format!(".opendrs-update-{}", release.version));
        std::fs::create_dir_all(staging.join("unpacked/OpenDRS")).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while !cancelled() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(PreparedUpdate {
            version: release.version.clone(),
            source: staging.join("unpacked/OpenDRS"),
            target: target.to_path_buf(),
            staging,
        })
    });
    fetching(app, release("999.0.0", true));
}

#[test]
fn a_download_cancelled_while_verifying_leaves_nothing() {
    let dir = Scratch::new("cancel");
    let target = dir.0.join("OpenDRS");
    std::fs::create_dir_all(&target).unwrap();
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    slow_installing(&mut app, &target);
    app.check_for_updates();
    wait(&mut app, |a| {
        !a.updates.dialog.as_ref().unwrap().is_checking()
    });
    let services = app.updates.services.clone();
    app.updates
        .dialog
        .as_mut()
        .unwrap()
        .start_download(&services, None);
    wait(&mut app, |_| {
        target.join(".opendrs-update-999.0.0").exists()
    });
    app.updates.dialog.as_ref().unwrap().cancel_download();
    wait(&mut app, |a| {
        !a.updates.dialog.as_ref().unwrap().is_downloading()
    });
    assert_eq!(status(&app), "更新未完成：已取消");
    assert!(
        app.updates.dialog.as_ref().unwrap().ready.is_none(),
        "no install question"
    );
    assert!(!target.join(".opendrs-update-999.0.0").exists());
}

#[test]
fn closing_the_window_mid_download_leaves_nothing() {
    let dir = Scratch::new("close");
    let target = dir.0.join("OpenDRS");
    std::fs::create_dir_all(&target).unwrap();
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    slow_installing(&mut app, &target);
    app.check_for_updates();
    wait(&mut app, |a| {
        !a.updates.dialog.as_ref().unwrap().is_checking()
    });
    let services = app.updates.services.clone();
    app.updates
        .dialog
        .as_mut()
        .unwrap()
        .start_download(&services, None);
    wait(&mut app, |_| {
        target.join(".opendrs-update-999.0.0").exists()
    });
    app.updates.dialog = None; // the window closed: its download is dropped
    wait(&mut app, |_| {
        !target.join(".opendrs-update-999.0.0").exists()
    });
}

#[test]
fn stale_staging_folders_are_removed_but_fresh_ones_kept() {
    let dir = Scratch::new("stale");
    let target = dir.0.join("OpenDRS");
    let fresh = target.join(".opendrs-update-9.9.9");
    std::fs::create_dir_all(&fresh).unwrap();
    let mut app = OpenDrsApp::with_data_dir(dir.0.clone());
    app.updates.services.target = Some(target.clone());
    assert_eq!(app.clean_stale_updates(), 0); // just made: may be an update in progress
    assert!(fresh.is_dir());
    assert_eq!(
        opendrs_update::remove_stale_staging(&target, Duration::ZERO),
        1
    );
    assert!(!fresh.exists());
}
