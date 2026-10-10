//! `opendrs --self-test REPORT.json` (port of `selftest.py`): proves that a release build starts
//! and has every part it needs, without a window or a radio.
//!
//! A Rust build cannot lose modules the way a PyInstaller build can, but it still depends on
//! things it only finds at run time: the system's CJK fonts, a GPU (or software) renderer, the
//! TLS stack and certificate store, serial-port enumeration (libudev on Linux), a browser for
//! PDF export. It is also built with optimisations and without overflow checks, so the radio
//! protocol and every page are exercised again in the release binary itself. The report is a
//! JSON file because the Windows release has no console; the exit code is 0 only when every
//! check passed ("skip" counts as passed: e.g. no browser installed).

use std::future::Future;
use std::io::Write as _;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use eframe::egui;
use eframe::wgpu;
use opendrs_core::codeplug::with_synced_image;
use opendrs_core::f136::{self, Project};
use opendrs_core::layout::check_layout;
use opendrs_core::radio_link::{RadioClient, list_ports};
use opendrs_core::radio_session::RadioSession;
use opendrs_core::radio_sim::SimulatedRadio;
use opendrs_core::records::{Channel, Contact, NAME_SIZE, Zone};
use opendrs_core::settings::{self, Settings};
use opendrs_core::textcodec::{NameCodec, encode_name};

use crate::app::OpenDrsApp;
use crate::pages::Tab;
use crate::{fonts, print, report};

/// Where the report goes when no path is given.
pub const DEFAULT_REPORT: &str = "opendrs_selftest.json";
/// Every check, in the order they run (`cargo xtask dist` expects exactly these).
pub const CHECK_NAMES: [&str; 10] = [
    "fonts", "icon", "codecs", "settings", "radio", "window", "gpu", "serial", "tls", "pdf",
];
const CODEC: NameCodec = NameCodec::Big5;
const GPU_TIMEOUT: Duration = Duration::from_secs(30);
const SAMPLE_CHANNELS: u16 = 3;
const SAMPLE_DATE: &str = "2026/10/10 12:00";

/// A check that passed, or one that could not run here and does not count against the build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pass {
    Ok(String),
    Skip(String),
}

/// One check's line in the report: `ok: …`, `skip: …` or `FAIL: …`.
pub fn line(result: &Result<Pass, String>) -> String {
    match result {
        Ok(Pass::Ok(text)) => format!("ok: {text}"),
        Ok(Pass::Skip(text)) => format!("skip: {text}"),
        Err(text) => format!("FAIL: {text}"),
    }
}

fn document(results: &[(&str, Result<Pass, String>)], running: Option<&str>) -> String {
    let checks: serde_json::Map<String, serde_json::Value> = results
        .iter()
        .map(|(name, result)| ((*name).to_string(), line(result).into()))
        .collect();
    let mut document = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "release": !cfg!(debug_assertions),
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "ok": running.is_none() && results.iter().all(|(_, r)| r.is_ok()),
        "checks": checks,
    });
    if let Some(name) = running {
        // A partial report: if the process hangs or dies here, this says where.
        document["running"] = name.into();
    }
    format!(
        "{}\n",
        serde_json::to_string_pretty(&document).unwrap_or_default()
    )
}

/// Run every check in a scratch folder (never the user's data folder), write `report`, and
/// return the exit code: 0 when every check passed. The report is rewritten before each check
/// and each result is printed as soon as it is known, so a hang still leaves a trace.
pub fn run(report: &Path) -> i32 {
    let scratch = scratch_dir();
    let mut written: std::io::Result<()> = Ok(());
    let mut printed = 0;
    let results = run_checks_with(&scratch, |done, running| {
        // Seen in a terminal on macOS and Linux, or when redirected on Windows.
        for (name, result) in &done[printed..] {
            println!("{name}: {}", line(result));
        }
        let _ = std::io::stdout().flush();
        printed = done.len();
        written = std::fs::write(report, document(done, running));
    });
    let _ = std::fs::remove_dir_all(&scratch); // only scratch files; a leftover is harmless
    let ok = results.iter().all(|(_, r)| r.is_ok());
    match written {
        Ok(()) if ok => 0,
        Ok(()) => 1,
        Err(e) => {
            eprintln!("無法寫入 {}：{e}", report.display());
            2
        }
    }
}

fn scratch_dir() -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    std::env::temp_dir().join(format!("opendrs_selftest_{}_{stamp}", std::process::id()))
}

type Check<'a> = Box<dyn FnOnce() -> Result<Pass, String> + 'a>;

/// Every check, in order; a panic is reported as a failure of its check.
pub fn run_checks(scratch: &Path) -> Vec<(&'static str, Result<Pass, String>)> {
    run_checks_with(scratch, |_, _| {})
}

/// [`run_checks`], calling `progress(done, Some(next))` before each check and
/// `progress(all, None)` at the end.
pub fn run_checks_with(
    scratch: &Path,
    mut progress: impl FnMut(&[(&'static str, Result<Pass, String>)], Option<&str>),
) -> Vec<(&'static str, Result<Pass, String>)> {
    if let Err(e) = std::fs::create_dir_all(scratch) {
        let failed = vec![("scratch", Err(format!("無法建立暫存資料夾：{e}")))];
        progress(&failed, None);
        return failed;
    }
    let sample = scratch.join("selftest.F136");
    let checks: Vec<(&'static str, Check<'_>)> = vec![
        ("fonts", Box::new(check_fonts)),
        ("icon", Box::new(check_icon)),
        ("codecs", Box::new(check_codecs)),
        ("settings", Box::new(|| check_settings(scratch))),
        ("radio", Box::new(|| check_radio(scratch, &sample))),
        ("window", Box::new(|| check_window(scratch, &sample))),
        ("gpu", Box::new(check_gpu)),
        ("serial", Box::new(check_serial)),
        ("tls", Box::new(check_tls)),
        ("pdf", Box::new(|| check_pdf(scratch, &sample))),
    ];
    let mut results = Vec::with_capacity(checks.len());
    for (name, check) in checks {
        progress(&results, Some(name));
        results.push((name, guarded(check)));
    }
    progress(&results, None);
    results
}

fn guarded(check: Check<'_>) -> Result<Pass, String> {
    panic::catch_unwind(AssertUnwindSafe(check)).unwrap_or_else(|payload| {
        let text = payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        Err(format!("程式錯誤（panic）：{text}"))
    })
}

fn check_fonts() -> Result<Pass, String> {
    let installed = fonts::install(&egui::Context::default());
    if installed.is_empty() {
        return Err("找不到中文字型（可用環境變數 OPENDRS_FONT 指定字型檔）".into());
    }
    Ok(Pass::Ok(fonts::describe(&installed)))
}

/// The window icon embedded in the program decodes.
fn check_icon() -> Result<Pass, String> {
    let icon = eframe::icon_data::from_png_bytes(crate::ICON_PNG)
        .map_err(|e| format!("圖示無法解碼：{e}"))?;
    if icon.width == 0 || icon.rgba.len() != (icon.width * icon.height * 4) as usize {
        return Err("圖示大小不對".into());
    }
    Ok(Pass::Ok(format!("{}×{}", icon.width, icon.height)))
}

fn check_codecs() -> Result<Pass, String> {
    let text = "測試頻道";
    for codec in [NameCodec::Big5, NameCodec::Gbk] {
        let field = encode_name(text, NAME_SIZE, codec).map_err(|e| e.to_string())?;
        let back = opendrs_core::textcodec::decode_name(&field, codec);
        if back.as_deref() != Some(text) {
            return Err(format!("{} 編碼後讀回 {back:?}", codec.label()));
        }
    }
    Ok(Pass::Ok("Big5、GBK".into()))
}

fn check_settings(scratch: &Path) -> Result<Pass, String> {
    let dir = scratch.join("settings");
    let saved = Settings::default().with_recent("selftest.F136");
    settings::save_settings(&saved, &dir).map_err(|e| format!("無法儲存設定：{e}"))?;
    let (loaded, problem) = settings::load_settings(&dir);
    if let Some(problem) = problem {
        return Err(problem);
    }
    if loaded.recent_files != saved.recent_files {
        return Err("讀回的設定不同".into());
    }
    Ok(Pass::Ok("儲存並讀回".into()))
}

/// Three channels in one zone and a group-call contact, on the settings area `base` read from
/// the simulated radio.
fn sample_project(base: &Project) -> Result<Project, String> {
    let name = |text: &str| encode_name(text, NAME_SIZE, CODEC).map_err(|e| e.to_string());
    let zone = Zone([0; Zone::SIZE])
        .with_index(1)
        .with_layout(2, SAMPLE_CHANNELS)
        .with_name("自我測試", CODEC)
        .map_err(|e| e.to_string())?;
    let channels = (1..=SAMPLE_CHANNELS)
        .map(|n| {
            let hz = 145_000_000 + u32::from(n) * 12_500;
            Channel([0; Channel::SIZE])
                .with_index(n)
                .with_name(&format!("頻道{n}"), CODEC)
                .map(|ch| ch.with_rx(hz).with_tx(hz))
                .map_err(|e| e.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let contact = Contact::build(1, Contact::GROUP_CALL, &name("通話群組")?, 46_601)
        .map_err(|e| e.to_string())?;
    let project = with_synced_image(&Project {
        zones: vec![zone],
        channels,
        contacts: vec![contact],
        ..base.clone()
    });
    check_layout(&project).map_err(|e| e.to_string())?;
    Ok(project)
}

/// Read the simulated radio (with its backup), write a sample project to it, read it back.
fn check_radio(scratch: &Path, sample: &Path) -> Result<Pass, String> {
    let mut radio = SimulatedRadio::default();
    let backups = scratch.join("backups");
    let first = RadioSession::new(&mut radio, &backups)
        .with_pause(Duration::ZERO)
        .read()
        .map_err(|e| e.message())?;
    if !first.backup.is_file() {
        return Err("讀取後沒有留下備份".into());
    }
    let project = sample_project(&first.read.project)?;
    let written = RadioClient::new(&mut radio)
        .write_codeplug(&project, Some(SAMPLE_DATE), None)
        .map_err(|e| format!("寫入：{e}"))?;
    let back = RadioClient::new(&mut radio)
        .read_codeplug()
        .map_err(|e| format!("讀回：{e}"))?;
    let same = back.project.channels == project.channels
        && back.project.zones == project.zones
        && back.project.contacts == project.contacts;
    if !same {
        return Err("讀回的頻道、分區或聯絡人與寫入的不同".into());
    }
    f136::save(&back.project, sample, false).map_err(|e| format!("無法存檔：{e}"))?;
    Ok(Pass::Ok(format!(
        "模擬對講機讀取、寫入（校驗和 0x{:08X}）、讀回一致",
        written.checksum
    )))
}

/// Open the sample project and draw every page of the main window, as eframe would.
fn check_window(scratch: &Path, sample: &Path) -> Result<Pass, String> {
    if !sample.is_file() {
        return Err("沒有範例專案（radio 檢查失敗）".into());
    }
    let ctx = egui::Context::default();
    let mut app = OpenDrsApp::with_context(&ctx, scratch.join("data"), Some(sample.to_path_buf()));
    if !app.doc.has_project() {
        return Err(format!("無法開啟範例專案：{:?}", app.dialog));
    }
    let input = || egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1280.0, 780.0),
        )),
        ..Default::default()
    };
    for tab in Tab::ALL {
        app.tab = tab;
        for _ in 0..2 {
            let mut output = ctx.run_ui(input(), |ui| {
                app.tick(ui.ctx());
                app.draw(ui);
            });
            output.textures_delta.clear(); // nothing renders them here
        }
    }
    if app.dialog.is_some() {
        return Err(format!("畫面出現訊息：{:?}", app.dialog));
    }
    Ok(Pass::Ok(format!("{} 個頁面", Tab::ALL.len())))
}

/// Poll a future that is ready at once on native platforms (wgpu's adapter request).
fn block_on<F: Future>(future: F, timeout: Duration) -> Option<F::Output> {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    let started = Instant::now();
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
            return Some(value);
        }
        if started.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The adapter eframe would draw with (same backends and power preference, no window).
fn check_gpu() -> Result<Pass, String> {
    let setup = eframe::egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    let instance = wgpu::Instance::new(setup.instance_descriptor);
    let options = wgpu::RequestAdapterOptions {
        power_preference: setup.power_preference,
        compatible_surface: None,
        ..Default::default()
    };
    let adapter = block_on(instance.request_adapter(&options), GPU_TIMEOUT)
        .ok_or("等待繪圖介面逾時")?
        .map_err(|e| format!("找不到可用的繪圖介面（{e}）"))?;
    let info = adapter.get_info();
    // The device eframe would ask for (its limits and features), not just an adapter.
    let descriptor = (setup.device_descriptor)(&adapter);
    block_on(adapter.request_device(&descriptor), GPU_TIMEOUT)
        .ok_or("等待繪圖裝置逾時")?
        .map_err(|e| format!("{} 無法建立繪圖裝置（{e}）", info.name))?;
    Ok(Pass::Ok(format!("{}（{:?}）", info.name, info.backend)))
}

fn check_serial() -> Result<Pass, String> {
    let ports = list_ports().map_err(|e| e.to_string())?;
    Ok(Pass::Ok(format!("{} 個連接埠", ports.len())))
}

fn check_tls() -> Result<Pass, String> {
    opendrs_contacts::radioid::http::tls_ready().map(Pass::Ok)
}

/// The printed report through the browser 匯出 PDF uses.
fn check_pdf(scratch: &Path, sample: &Path) -> Result<Pass, String> {
    let Some(browser) = print::find_browser() else {
        return Ok(Pass::Skip(
            "找不到 Edge 或 Chrome；匯出 PDF 時會提示改用預覽列印".into(),
        ));
    };
    let project = f136::load(sample).map_err(|e| format!("沒有範例專案：{e}"))?;
    let html = report::report_html(&project, CODEC, "自我測試", "2026-10-10 12:00");
    let dir = scratch.join("print");
    let file =
        print::write_report_file(&dir, &html, false, "selftest").map_err(|e| e.to_string())?;
    let pdf = dir.join("selftest.pdf");
    print::html_to_pdf(&browser, &file, &pdf, &dir.join("pdf-profile"))?;
    let bytes = std::fs::read(&pdf).map_err(|e| e.to_string())?;
    if !bytes.starts_with(b"%PDF-") {
        return Err("產生的檔案不是 PDF".into());
    }
    let name = browser
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    Ok(Pass::Ok(format!("{name}，{} bytes", bytes.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = scratch_dir().with_extension(name);
        std::fs::create_dir_all(&dir).expect("scratch");
        Scratch(dir)
    }

    #[test]
    fn the_icon_decodes() {
        assert_eq!(check_icon(), Ok(Pass::Ok("256×256".into())));
    }

    #[test]
    fn checks_run_in_the_announced_order_with_progress() {
        let dir = scratch("order");
        let mut announced = Vec::new();
        let results = run_checks_with(&dir.0, |done, running| {
            if let Some(name) = running {
                assert_eq!(
                    done.len(),
                    announced.len(),
                    "one report per check, before it"
                );
                announced.push(name.to_owned());
            }
        });
        let names: Vec<&str> = results.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, CHECK_NAMES);
        assert_eq!(announced, CHECK_NAMES);
        let partial = document(&results[..3], Some("settings"));
        assert!(partial.contains("\"running\": \"settings\"") && partial.contains("\"ok\": false"));
    }

    #[test]
    fn report_lines_say_what_happened() {
        assert_eq!(line(&Ok(Pass::Ok("x".into()))), "ok: x");
        assert_eq!(line(&Ok(Pass::Skip("y".into()))), "skip: y");
        assert_eq!(line(&Err("z".into())), "FAIL: z");
    }

    #[test]
    fn the_radio_round_trip_and_every_page_pass() {
        let dir = scratch("radio");
        let sample = dir.0.join("selftest.F136");
        let radio = check_radio(&dir.0, &sample);
        assert!(matches!(radio, Ok(Pass::Ok(_))), "{radio:?}");
        let window = check_window(&dir.0, &sample);
        assert_eq!(window, Ok(Pass::Ok("9 個頁面".into())));
    }

    #[test]
    fn codecs_settings_and_tls_pass() {
        let dir = scratch("misc");
        assert!(check_codecs().is_ok());
        assert!(check_settings(&dir.0).is_ok());
        assert!(check_tls().is_ok(), "{:?}", check_tls());
    }

    #[test]
    fn a_panicking_check_is_a_failure() {
        let result = guarded(Box::new(|| panic!("boom")));
        assert_eq!(result, Err("程式錯誤（panic）：boom".into()));
    }

    #[test]
    fn the_window_check_needs_the_sample() {
        let dir = scratch("nosample");
        let result = check_window(&dir.0, &dir.0.join("missing.F136"));
        assert!(result.is_err());
    }
}
