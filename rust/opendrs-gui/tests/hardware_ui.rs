//! The 韌體升級 and 讀取錄音 windows drawn headlessly (egui_kittest), in a scratch data folder
//! and on the simulated port (`__sim__`): no real APPDATA, no serial port.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use opendrs_gui::app::OpenDrsApp;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opendrs-hwui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

fn harness(data_dir: &Path) -> Harness<'static, OpenDrsApp> {
    let data_dir = data_dir.to_path_buf();
    let mut harness = Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| OpenDrsApp::with_context(&cc.egui_ctx, data_dir, None));
    harness
        .state_mut()
        .update_settings(|s| s.radio_port = "__sim__".into());
    harness
}

fn disabled(harness: &Harness<'_, OpenDrsApp>, label: &str) -> bool {
    harness.get_by_label(label).accesskit_node().is_disabled()
}

#[test]
fn firmware_window_renders_with_the_simulated_port() {
    let dir = scratch("firmware");
    let mut harness = harness(&dir);
    harness.state_mut().firmware_update();
    harness.run();
    assert!(harness.state().windows.firmware.is_some());
    harness.get_by_label("模擬對講機（試用）");
    harness.get_by_label_contains("內建升級已在實機驗證");
    harness.get_by_label("請先選擇韌體檔");
    assert!(disabled(&harness, "開始升級") && disabled(&harness, "改用原廠 IAP 工具"));
    assert!(disabled(&harness, "我了解刷錯韌體可能使對講機無法開機"));
    harness.get_by_label("關閉").click();
    harness.run();
    assert!(harness.state().windows.firmware.is_none());
    assert!(!dir.join("recordings").exists());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn recording_window_lists_the_simulated_recordings() {
    let dir = scratch("recording");
    let mut harness = harness(&dir);
    harness.state_mut().recordings();
    harness.run();
    assert!(!disabled(&harness, "讀取清單"));
    harness.get_by_label("讀取清單").click();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        harness.step();
        let running = harness
            .state()
            .windows
            .recording
            .as_ref()
            .is_some_and(|d| d.running());
        if !running || Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    harness.run();
    let dialog = harness.state().windows.recording.as_ref().expect("open");
    assert_eq!(dialog.rows().len(), 3, "{}", dialog.status());
    harness.get_by_label("共 3 筆，合計 34 秒");
    harness.get_by_label("關閉").click();
    harness.run();
    assert!(harness.state().windows.recording.is_none());
    let (settings, _) = opendrs_core::settings::load_settings(&dir);
    assert_eq!(settings.recordings_dir, ""); // the default folder is not stored
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
