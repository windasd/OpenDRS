//! The RadioID and backup dialogs drawn headlessly (egui_kittest): a scratch data folder, a
//! pre-filled RadioID cache (so nothing is downloaded) and real backup files.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use opendrs_gui::app::OpenDrsApp;

const CSV: &str = concat!(
    "RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n",
    "9997001,QA0BD,James,,New Taipei,Northern Taiwan,Taiwan\r\n",
    "9997002,QA0ADN,Chen,Wei,Taichung,Central Taiwan,Taiwan\r\n",
    "1023007,QA0AKM,Hans Peter,,Cornwall,Ontario,Canada\r\n",
);

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(name)
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opendrs-gui-dialogs-{name}-{}", std::process::id()));
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

fn harness(data: &Path) -> Harness<'static, OpenDrsApp> {
    let data = data.to_path_buf();
    Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| {
            OpenDrsApp::with_context(&cc.egui_ctx, data, Some(fixture("dh98vu_20250811.F136")))
        })
}

fn fill_cache(data: &Path) {
    let cache = data.join("radioid");
    std::fs::create_dir_all(&cache).expect("cache folder");
    std::fs::write(cache.join("user.csv"), CSV).expect("cache file");
}

fn fill_backups(data: &Path) {
    let folder = data.join("backups");
    std::fs::create_dir_all(&folder).expect("backup folder");
    for stem in [
        "20261001_090000_read_DR-8500UV",
        "20261002_090000_before_write_DR-8500UV",
    ] {
        std::fs::copy(
            fixture("mmdvm_20241127.F136"),
            folder.join(format!("{stem}.F136")),
        )
        .expect("copy");
        std::fs::write(folder.join(format!("{stem}.bin")), b"raw").expect("bin");
    }
}

/// Open the RadioID dialog and draw frames until the background thread has parsed the cache.
fn open_radioid(harness: &mut Harness<'static, OpenDrsApp>) {
    harness.state_mut().import_from_radioid();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        harness.step();
        let dialog = harness.state().windows.radioid.as_ref().expect("open");
        if dialog.status().starts_with("本機資料") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "still loading: {}",
            dialog.status()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.run();
}

#[test]
fn radioid_dialog_draws_the_cached_list() {
    let scratch = Scratch::new("radioid");
    fill_cache(&scratch.0);
    let mut harness = harness(&scratch.0);
    harness.run();
    open_radioid(&mut harness);
    harness.get_by_label("下載／更新資料");
    harness.get_by_label_contains("本機資料 3 筆（下載於 ");
    harness.get_by_label("匯入 0 筆");
    harness.get_by_label("QA0BD");
    assert!(
        harness.query_by_label("QA0AKM").is_none(),
        "Canada is filtered out"
    );

    harness.get_by_label("全選符合結果").click();
    harness.run();
    harness.get_by_label("匯入 2 筆");
    harness.get_by_label_contains("已勾選 2 筆（新增 2、已存在 0）");

    harness.get_by_label("取消").click();
    harness.run();
    assert!(harness.state().windows.radioid.is_none());
    assert!(!harness.state().doc.can_undo(), "cancel imports nothing");
}

#[test]
fn backup_manager_draws_and_deleting_defaults_to_no() {
    let scratch = Scratch::new("backups");
    fill_backups(&scratch.0);
    let mut harness = harness(&scratch.0);
    harness.run();
    harness.state_mut().backup_manager();
    harness.run();
    harness.get_by_label_contains("共 2 份備份，合計 ");
    harness.get_by_label("與目前專案比較");
    harness.get_by_label("最近一次寫入前的備份（清理時保留）");

    harness.get_by_label("寫入前").click();
    harness.run();
    let selected = harness
        .state()
        .windows
        .backups
        .as_ref()
        .and_then(|d| d.selected().map(|b| b.kind.clone()));
    assert_eq!(selected.as_deref(), Some("before_write"));

    harness.get_by_label("刪除…").click();
    harness.run();
    harness.get_by_label_contains("這是最近一次寫入對講機之前的備份");
    harness.key_press(egui::Key::Enter); // the focused default button is 否
    harness.run();
    let dialog = harness.state().windows.backups.as_ref().expect("open");
    assert!(dialog.question().is_none());
    assert_eq!(dialog.backups().len(), 2, "nothing deleted");

    harness.get_by_label("關閉").click();
    harness.run();
    assert!(harness.state().windows.backups.is_none());
}

/// Renders both dialogs to `target/gui-screens/*.png` for a look (needs a GPU or a software
/// adapter): `cargo test -p opendrs-gui --test dialogs_ui -- --ignored`.
#[test]
#[ignore = "renders screenshots for a manual look"]
fn screenshots() {
    let scratch = Scratch::new("shots");
    fill_cache(&scratch.0);
    fill_backups(&scratch.0);
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gui-screens");
    std::fs::create_dir_all(&out).expect("output folder");
    let mut harness = harness(&scratch.0);
    harness.run();
    open_radioid(&mut harness);
    let image = harness.render().expect("render");
    image.save(out.join("radioid.png")).expect("save");
    harness.state_mut().windows.radioid = None;
    harness.state_mut().backup_manager();
    harness.run();
    let image = harness.render().expect("render");
    image.save(out.join("backups.png")).expect("save");
}
