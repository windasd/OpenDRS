//! 韌體升級 dialog (port of the dialog tests in `tests/test_firmware_check.py` and
//! `tests/test_fwpatch.py`). Every transfer runs against `SimulatedBootloader`; no test opens a
//! serial port.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eframe::egui;
use opendrs_core::radio_link::{RadioError, Transport};
use opendrs_firmware::firmware_check::{Finding, Level, verdict};
use opendrs_firmware::fwpatch;
use opendrs_firmware::iap::{IapResult, PAGE_SIZE};
use opendrs_firmware::iap_sim::SimulatedBootloader;
use opendrs_gui::dialogs::firmware::{
    BoxedTransport, CHANGED, FirmwareDialog, FirmwareSetup, GateInput, KEEP_VENDOR, PortChoice,
    RUNNING, STOPPING, SharedTransport, TransportFactory, VERIFIED, allowed, confirm_question,
    failure_text, gate, has_backup_from, radio_text, remembered_iap, same_file, shared_factory,
    success_text, thousands,
};

const RADIO: &str = "308E.BBKTARSCB.003";
const WAIT: Duration = Duration::from_secs(20);

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opendrs-fw-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

/// A small image holding a firmware ID and a build date, like `_image` in the Python tests.
fn image(dir: &Path, firmware_id: &[u8], name: &str) -> PathBuf {
    let path = dir.join(name);
    let data = [
        &[0u8; 64][..],
        firmware_id,
        &[0u8; 32],
        b"Mar  9 2024",
        b"\x0017:02:38",
        &[0xFFu8; 64],
    ]
    .concat();
    std::fs::write(&path, data).expect("write image");
    path
}

fn setup(exe: &Path, port_busy: bool, transport: Option<TransportFactory>) -> FirmwareSetup {
    FirmwareSetup {
        radio_firmware: RADIO.into(),
        radio_date: "Mar 9 2024".into(),
        has_recent_backup: true,
        iap_exe: exe.display().to_string(),
        port_busy,
        port_label: "COM3".into(),
        transport,
    }
}

fn simulated(boot: &Arc<Mutex<SimulatedBootloader>>) -> TransportFactory {
    shared_factory(Arc::clone(boot))
}

/// The dialog ready for 開始升級: a file of the radio's build, both ticks given.
fn builtin_dialog(dir: &Path, transport: TransportFactory) -> (FirmwareDialog, PathBuf) {
    let mut dialog = FirmwareDialog::new(setup(&dir.join("none.exe"), false, Some(transport)));
    let path = image(dir, b"308E.BBKTARSCB.003.", "same.bin");
    dialog.set_file(&path);
    dialog.confirm = true;
    dialog.iap_mode = true;
    (dialog, path)
}

fn texts(dialog: &FirmwareDialog) -> Vec<String> {
    let (common, builtin, tool) = dialog.findings();
    common
        .iter()
        .chain(&builtin)
        .chain(&tool)
        .map(|f| f.text.clone())
        .collect()
}

fn copied(ctx: &egui::Context) -> Vec<String> {
    ctx.output(|o| {
        o.commands
            .iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    })
}

/// The simulated bootloader with its replies held back until `hold` is cleared (a radio that
/// is slow to answer), so Stop can be pressed mid-run.
struct HeldBootloader {
    boot: SharedTransport<SimulatedBootloader>,
    hold: Arc<AtomicBool>,
}

impl Transport for HeldBootloader {
    fn write(&mut self, data: &[u8]) -> Result<(), RadioError> {
        self.boot.write(data)
    }

    fn read_exact(&mut self, n: usize, timeout: Duration) -> Result<Vec<u8>, RadioError> {
        self.boot.read_exact(n, timeout)
    }

    fn read_idle(&mut self, idle: Duration, max_wait: Duration) -> Result<Vec<u8>, RadioError> {
        if self.hold.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
            return Ok(Vec::new());
        }
        self.boot.read_idle(idle, max_wait)
    }

    fn reset_input(&mut self) -> Result<(), RadioError> {
        self.boot.reset_input()
    }
}

#[test]
fn no_findings_count_as_block() {
    assert_eq!(verdict(&[]), Level::Ok); // the library's view
    assert!(!allowed(true, &[], true)); // the dialog's: nothing checked, nothing flashed
    let ok = [Finding {
        level: Level::Ok,
        text: "x".into(),
    }];
    assert!(allowed(true, &ok, false) && !allowed(false, &ok, true));
    let warn = [Finding {
        level: Level::Warn,
        text: "w".into(),
    }];
    assert!(!allowed(true, &warn, false) && allowed(true, &warn, true));
}

#[test]
fn gate_needs_both_ticks_and_no_common_block() {
    let f = |level| Finding {
        level,
        text: String::new(),
    };
    let (ok, warn, block) = ([f(Level::Ok)], [f(Level::Warn)], [f(Level::Block)]);
    let input = GateInput {
        has_image: true,
        common: &warn,
        builtin: &ok,
        tool: &block,
        confirmed: false,
        iap_mode: true,
        running: false,
        patchable: false,
    };
    let g = gate(&input);
    assert!(g.confirm_enabled && !g.flash_enabled && !g.launch_enabled);
    let g = gate(&GateInput {
        confirmed: true,
        ..input
    });
    assert!(g.flash_enabled && !g.launch_enabled); // the tool is missing
    let g = gate(&GateInput {
        confirmed: true,
        iap_mode: false,
        ..input
    });
    assert!(!g.flash_enabled); // the radio must be in IAP mode
    let both = [f(Level::Warn), f(Level::Block)];
    let g = gate(&GateInput {
        common: &both,
        confirmed: true,
        ..input
    });
    assert!(!g.confirm_enabled && !g.flash_enabled); // a common BLOCK cannot be ticked away
    let empty: [Finding; 0] = [];
    let g = gate(&GateInput {
        common: &empty,
        builtin: &empty,
        tool: &empty,
        confirmed: true,
        ..input
    });
    assert!(!g.flash_enabled && !g.launch_enabled); // no findings = BLOCK
    let g = gate(&GateInput {
        confirmed: true,
        running: true,
        patchable: true,
        ..input
    });
    assert!(!g.flash_enabled && !g.iap_mode_enabled && !g.browse_enabled && g.stop_visible);
    assert!(!g.make_patch_enabled);
}

#[test]
fn firmware_dialog_gates_the_launch() {
    let dir = scratch("gates");
    let exe = dir.join("IAP+Flash.exe");
    std::fs::write(&exe, b"MZ").expect("exe");
    let launched: Rc<RefCell<Vec<(PathBuf, PathBuf)>>> = Rc::default();
    let log = Rc::clone(&launched);
    let mut dialog = FirmwareDialog::new(setup(&exe, false, None)).with_launcher(move |e, d| {
        log.borrow_mut().push((e.to_path_buf(), d.to_path_buf()));
        true
    });
    assert!(!dialog.gate().launch_enabled); // no file yet
    dialog.set_file(&image(&dir, b"308E.B6.TARSCB.004.", "b6-004.bin"));
    let g = dialog.gate();
    assert!(!g.launch_enabled && g.confirm_enabled); // another version: needs the tick
    dialog.confirm = true;
    let ctx = egui::Context::default();
    assert!(dialog.gate().launch_enabled && dialog.start_tool(&ctx));
    assert_eq!(*launched.borrow(), vec![(exe.clone(), dir.clone())]);
    assert!(
        copied(&ctx)
            .last()
            .is_some_and(|t| t.ends_with("b6-004.bin"))
    );
    assert!(dialog.launched() && dialog.file_info().ends_with("韌體路徑已複製到剪貼簿。"));
    dialog.set_file(&image(&dir, b"412A.XX.TARSCB.001.", "other.bin"));
    let g = dialog.gate();
    assert!(!g.launch_enabled && !g.confirm_enabled); // blocked
    dialog.set_file(&image(&dir, b"308E.B6.TARSCB.003.", "b6.bin"));
    assert!(dialog.gate().launch_enabled && !dialog.confirm); // same build as the BBK radio
    dialog.set_file(&image(&dir, b"308E.BBKTARSCB.003.", "same.bin"));
    assert!(dialog.gate().launch_enabled);
    dialog.exe = dir.join("missing.exe").display().to_string();
    assert!(!dialog.gate().launch_enabled);
    let mut busy = FirmwareDialog::new(setup(&exe, true, None));
    busy.set_file(&image(&dir, b"308E.BBKTARSCB.003.", "same2.bin"));
    assert!(!busy.gate().launch_enabled);
    assert!(!dialog.gate().flash_enabled); // no port selected: built-in upgrade unavailable
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_failed_launch_is_reported() {
    let dir = scratch("launch-fail");
    let exe = dir.join("IAP+Flash.exe");
    std::fs::write(&exe, b"MZ").expect("exe");
    let mut dialog = FirmwareDialog::new(setup(&exe, false, None)).with_launcher(|_, _| false);
    dialog.set_file(&image(&dir, b"308E.BBKTARSCB.003.", "same.bin"));
    assert!(!dialog.start_tool(&egui::Context::default()));
    assert!(dialog.file_info().ends_with("\n無法啟動 IAP 工具。"));
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn builtin_upgrade_runs_against_simulated_bootloader() {
    let dir = scratch("builtin");
    let boot = Arc::new(Mutex::new(SimulatedBootloader::default()));
    let (mut dialog, path) = builtin_dialog(&dir, simulated(&boot));
    dialog.set_file(&path); // choosing a file clears the risk tick
    // same build, backup present, built-in upgrade verified on hardware: nothing to confirm
    let g = dialog.gate();
    assert!(!dialog.confirm && !g.confirm_enabled && g.flash_enabled);
    assert!(texts(&dialog).iter().any(|t| t == VERIFIED));
    dialog.iap_mode = false;
    assert!(!dialog.gate().flash_enabled); // the radio must be in IAP mode
    dialog.iap_mode = true;
    assert!(dialog.gate().flash_enabled && !dialog.gate().launch_enabled); // vendor exe missing
    assert!(dialog.start_flash());
    assert_eq!(
        dialog.asking(),
        Some(confirm_question("same.bin", 1).as_str())
    );
    assert!(boot.lock().expect("sim").sent.is_empty()); // nothing sent before 是
    let ctx = egui::Context::default();
    assert!(dialog.answer(true, &ctx));
    dialog.wait(WAIT);
    let data = std::fs::read(&path).expect("image");
    let boot = boot.lock().expect("sim");
    assert!(dialog.upgrade_result().is_some() && dialog.error().is_empty() && boot.finished);
    assert_eq!(&boot.flash[..data.len()], &data[..]);
    assert_eq!(boot.flash.len(), PAGE_SIZE);
    assert!(dialog.status().contains("升級完成") && dialog.status().contains("IAP Erase ok"));
    // re-confirm before another run
    assert!(!dialog.iap_mode && !dialog.gate().flash_enabled && !dialog.running());
    assert_eq!(dialog.progress(), Some((1, 1)));
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn builtin_upgrade_reports_a_silent_radio() {
    let dir = scratch("silent");
    let mut silent = SimulatedBootloader::default();
    silent.silent = true;
    let (mut dialog, _) = builtin_dialog(&dir, simulated(&Arc::new(Mutex::new(silent))));
    assert!(dialog.start_flash() && dialog.answer(true, &egui::Context::default()));
    dialog.wait(WAIT);
    assert!(dialog.upgrade_result().is_none());
    assert!(dialog.status().starts_with("升級未完成，清除指令已送出"));
    assert_eq!(dialog.progress(), None);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn builtin_upgrade_port_failure_and_declined_confirmation() {
    let dir = scratch("port-failure");
    let busy_port: TransportFactory =
        Arc::new(|| Err(RadioError::Failed("無法開啟 COM3：拒絕存取".into())));
    let (mut dialog, _) = builtin_dialog(&dir, Arc::clone(&busy_port));
    assert!(dialog.start_flash() && dialog.answer(true, &egui::Context::default()));
    dialog.wait(WAIT);
    assert_eq!(dialog.status(), "升級未開始，無法開啟 COM3：拒絕存取");
    let (mut declined, _) = builtin_dialog(&dir, busy_port);
    assert!(declined.start_flash());
    assert!(!declined.answer(false, &egui::Context::default()));
    assert!(!declined.running() && declined.asking().is_none());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn builtin_upgrade_rechecks_a_file_changed_after_the_check() {
    let dir = scratch("tampered");
    let boot = Arc::new(Mutex::new(SimulatedBootloader::default()));
    let (mut dialog, path) = builtin_dialog(&dir, simulated(&boot));
    let mut data = std::fs::read(&path).expect("image");
    data.extend_from_slice(b"tampered");
    std::fs::write(&path, data).expect("tamper");
    assert!(!dialog.start_flash() && boot.lock().expect("sim").sent.is_empty());
    assert_eq!(dialog.status(), CHANGED);
    assert!(!dialog.confirm && dialog.asking().is_none());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn the_question_is_void_when_the_file_or_the_checks_change() {
    let dir = scratch("void");
    let boot = Arc::new(Mutex::new(SimulatedBootloader::default()));
    let (mut dialog, _) = builtin_dialog(&dir, simulated(&boot));
    assert!(dialog.start_flash());
    dialog.set_file(&image(&dir, b"308E.B6.TARSCB.003.", "b6.bin")); // another file meanwhile
    assert!(!dialog.answer(true, &egui::Context::default()) && !dialog.running());
    dialog.iap_mode = true;
    assert!(dialog.start_flash());
    dialog.port_busy = true; // another transfer started meanwhile
    assert!(!dialog.answer(true, &egui::Context::default()) && !dialog.running());
    assert!(boot.lock().expect("sim").sent.is_empty());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn builtin_upgrade_cannot_be_closed_while_running_but_can_be_stopped() {
    let dir = scratch("stop");
    let boot = Arc::new(Mutex::new(SimulatedBootloader::default()));
    let hold = Arc::new(AtomicBool::new(true));
    let (sim, held) = (Arc::clone(&boot), Arc::clone(&hold));
    let slow: TransportFactory = Arc::new(move || {
        Ok(Box::new(HeldBootloader {
            boot: SharedTransport(Arc::clone(&sim)),
            hold: Arc::clone(&held),
        }) as BoxedTransport)
    });
    let (mut dialog, _) = builtin_dialog(&dir, slow);
    assert!(dialog.start_flash() && dialog.answer(true, &egui::Context::default()));
    let g = dialog.gate();
    assert!(!g.flash_enabled && !g.launch_enabled && g.stop_visible);
    assert!(!dialog.request_close() && dialog.status() == RUNNING);
    let deadline = Instant::now() + Duration::from_secs(5);
    while boot.lock().expect("sim").sent.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5)); // stop while waiting for the erase reply
    }
    dialog.stop();
    assert!(dialog.stopping() && dialog.status() == STOPPING);
    dialog.wait(Duration::from_secs(5));
    assert!(dialog.status().starts_with("升級未完成，已停止"));
    assert!(!dialog.running() && !dialog.gate().stop_visible && !dialog.stopping());
    assert!(dialog.request_close());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn another_port_voids_the_iap_mode_tick() {
    let dir = scratch("port-change");
    let boot = Arc::new(Mutex::new(SimulatedBootloader::default()));
    let (mut dialog, _) = builtin_dialog(&dir, simulated(&boot));
    dialog.set_port(Some("__sim__".into()));
    assert!(!dialog.iap_mode && !dialog.gate().flash_enabled);
    dialog.set_port(None);
    assert!(
        texts(&dialog)
            .iter()
            .any(|t| t.starts_with("內建升級：請先"))
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn another_radio_voids_both_ticks() {
    let dir = scratch("radio-change");
    let boot = Arc::new(Mutex::new(SimulatedBootloader::default()));
    let (mut dialog, _) = builtin_dialog(&dir, simulated(&boot));
    assert!(dialog.gate().flash_enabled);
    dialog.set_radio(RADIO, "Mar 9 2024", true);
    assert!(
        dialog.confirm && dialog.iap_mode,
        "the same radio keeps the ticks"
    );
    dialog.set_radio("308E.TARSCB.002.", "Jan 1 2023", true);
    assert!(!dialog.confirm && !dialog.iap_mode && !dialog.gate().flash_enabled);
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn the_simulated_port_flashes_a_fresh_bootloader() {
    let dir = scratch("sim-port");
    let mut dialog = FirmwareDialog::new(setup(&dir.join("none.exe"), false, None));
    dialog.set_port(Some("sim".into()));
    dialog.set_file(&image(&dir, b"308E.BBKTARSCB.003.", "same.bin"));
    dialog.iap_mode = true;
    assert!(dialog.start_flash() && dialog.answer(true, &egui::Context::default()));
    dialog.wait(WAIT);
    assert!(dialog.upgrade_result().is_some(), "{}", dialog.status());
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

const SIZE: usize = 841_904;

/// An image that only has the original bytes at the patch sites (everything else zero).
fn synthetic() -> Vec<u8> {
    let mut img = vec![0u8; SIZE];
    for site in fwpatch::RECORD_READ {
        let o = (site.va - fwpatch::A_BASE) as usize;
        img[o..o + site.original.len()].copy_from_slice(site.original);
    }
    img
}

#[test]
fn dialog_makes_and_selects_the_record_read_version() {
    let dir = scratch("patch");
    let src = dir.join("vendor.bin");
    std::fs::write(&src, synthetic()).expect("vendor");
    let mut dialog = FirmwareDialog::new(setup(&dir.join("none.exe"), false, None));
    assert!(!dialog.gate().make_patch_enabled); // no file yet
    dialog.set_file(&src);
    assert!(dialog.gate().make_patch_enabled && dialog.file_info().contains("產生錄音讀取版"));
    assert!(dialog.make_record_read(Some(src.clone())).is_none());
    assert_eq!(dialog.status(), KEEP_VENDOR);
    let sneaky = dir.join(".").join("vendor.bin"); // the same file under another spelling
    assert!(dialog.make_record_read(Some(sneaky)).is_none());
    assert_eq!(dialog.status(), KEEP_VENDOR);
    let out = dialog
        .make_record_read(Some(dir.join("vendor_record_read.bin")))
        .expect("patched copy");
    assert!(fwpatch::is_record_read_patched(
        &std::fs::read(&out).expect("out")
    ));
    assert_eq!(dialog.file(), out.display().to_string());
    assert!(dialog.file_info().contains("錄音讀取版"));
    assert!(!dialog.gate().make_patch_enabled && dialog.status().contains("改 30 bytes"));
    assert!(
        dialog
            .status()
            .ends_with("要復原時再刷一次原廠檔 vendor.bin。")
    );
    assert_eq!(std::fs::read(&src).expect("src"), synthetic()); // the vendor file is untouched
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn an_unreadable_file_clears_the_tick_and_blocks() {
    let dir = scratch("unreadable");
    let mut dialog = FirmwareDialog::new(setup(&dir.join("none.exe"), false, None));
    dialog.confirm = true;
    dialog.set_file(&dir.join("missing.bin"));
    assert!(dialog.image().is_none() && !dialog.confirm);
    assert!(dialog.file_info().starts_with("無法讀取："));
    assert!(
        texts(&dialog)
            .iter()
            .any(|t| t.starts_with("原廠工具：找不到"))
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn texts_match_the_python_dialog() {
    assert_eq!(
        radio_text("308E.BBKTARSCB.003", "Mar 9 2024"),
        "308E.BBKTARSCB.003（Mar 9 2024）"
    );
    assert_eq!(radio_text("X", ""), "X");
    assert_eq!(radio_text("", "Mar 9 2024"), "未知（尚未讀取對講機）");
    let result = IapResult {
        checksum: 0x00AB_12CD,
        pages: 412,
        erase_reply: " IAP\r\nErase  ok ".into(),
    };
    assert_eq!(
        success_text(&result),
        "升級完成：412 頁，對講機確認校驗和 0x00AB12CD（清除回覆：IAP Erase ok）。\
         對講機應會自行重新啟動，之後請重新「讀取對講機」確認韌體版本。"
    );
    assert_eq!(failure_text("x", true), "升級未完成，x");
    assert_eq!(failure_text("x", false), "升級未開始，x");
    assert_eq!(
        confirm_question("a.bin", 3),
        "即將清除對講機的韌體並寫入 a.bin（3 頁）。\n開始後不可拔線或關機；若中途失敗，\
         對講機會停在升級模式，可重新執行升級。\n\n要開始嗎？"
    );
    assert_eq!(
        (thousands(0), thousands(999), thousands(841_904)),
        ("0".into(), "999".into(), "841,904".into())
    );
}

#[test]
fn settings_ports_and_backups() {
    assert_eq!(remembered_iap("  ", "D", "x"), None); // nothing typed: leave it
    assert_eq!(remembered_iap("D", "D", "x"), Some(String::new()));
    assert_eq!(remembered_iap("D", "D", ""), None);
    assert_eq!(remembered_iap(" E ", "D", ""), Some("E".into()));
    assert_eq!(remembered_iap("E", "D", "E"), None);
    assert_eq!(PortChoice::from_setting(None), PortChoice::None);
    assert_eq!(PortChoice::from_setting(Some("")), PortChoice::None);
    assert_eq!(
        PortChoice::from_setting(Some("__sim__")),
        PortChoice::Simulated
    );
    assert_eq!(PortChoice::from_setting(Some("sim")), PortChoice::Simulated);
    // only parsed here; nothing opens it
    assert_eq!(
        PortChoice::from_setting(Some("COM9")),
        PortChoice::Serial("COM9".into())
    );
    assert_eq!(PortChoice::Simulated.label(), "模擬對講機（試用）");
    let dir = scratch("backups");
    assert!(!has_backup_from(&dir.join("none"), "20261010"));
    std::fs::write(dir.join("20261009_101010_read_DR8500.F136"), b"").expect("old");
    std::fs::write(dir.join("20261010_101010_read_DR8500.bin"), b"").expect("raw");
    assert!(!has_backup_from(&dir, "20261010"));
    std::fs::write(dir.join("20261010_101010_before_write_DR8500.F136"), b"").expect("new");
    assert!(has_backup_from(&dir, "20261010"));
    assert!(same_file(&dir.join("a.bin"), &dir.join(".").join("a.bin")));
    assert!(!same_file(&dir.join("a.bin"), &dir.join("b.bin")));
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
