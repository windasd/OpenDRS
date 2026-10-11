//! Run the packaged editor's `--self-test` and the command-line tool's `--version`, and refuse
//! the release unless every check passed in the release build of this version (as
//! packaging/build.py does with the PyInstaller .exe).

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::Result;

const TIMEOUT: Duration = Duration::from_secs(180);

/// The checks `opendrs-gui/src/selftest.rs` runs (`CHECK_NAMES` there); a report missing one, or
/// with one more, comes from a different build.
pub const EXPECTED: [&str; 11] = [
    "fonts", "icon", "codecs", "settings", "radio", "window", "gpu", "serial", "tls", "update",
    "pdf",
];

/// Why a self-test report does not vouch for this build, if it does not. `strict` (on CI, where
/// every runner image has a browser) also refuses a skipped check.
pub fn problems(report: &Value, version: &str, strict: bool) -> Vec<String> {
    let mut out = Vec::new();
    if report["ok"] != true {
        out.push("有檢查沒有通過".to_owned());
    }
    if report["release"] != true {
        out.push("不是 release 建置".to_owned());
    }
    if report["version"] != version {
        out.push(format!("版本是 {}，應為 {version}", report["version"]));
    }
    let Some(checks) = report["checks"].as_object() else {
        out.push("報告沒有 checks".to_owned());
        return out;
    };
    let mut names: Vec<&str> = checks.keys().map(String::as_str).collect();
    let mut expected = EXPECTED.to_vec();
    names.sort_unstable();
    expected.sort_unstable();
    if names != expected {
        out.push(format!("檢查項目是 {names:?}，應為 {expected:?}"));
    }
    for (name, line) in checks {
        let text = line.as_str().unwrap_or_default();
        if text.starts_with("FAIL") || (strict && !text.starts_with("ok")) {
            out.push(format!("{name}：{text}"));
        }
    }
    out
}

fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).map_or(true, |mut entries| entries.next().is_none())
}

/// `program --self-test report`, with its data folder pointed at the empty folder `guard` (which
/// must still be empty afterwards: the self-test must never touch a user's data); the report
/// must say the build is complete.
pub fn run(program: &Path, report: &Path, guard: &Path, version: &str) -> Result<Value> {
    let _ = std::fs::remove_file(report);
    let _ = std::fs::remove_dir_all(guard);
    std::fs::create_dir_all(guard).map_err(|e| format!("{}：{e}", guard.display()))?;
    let mut child = Command::new(program)
        .arg("--self-test")
        .arg(report)
        .env("OPENDRS_DATA_DIR", guard)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| format!("無法執行 {}：{e}", program.display()))?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None if started.elapsed() > TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                let partial = std::fs::read_to_string(report).unwrap_or_default();
                return Err(format!(
                    "自我測試超過 {} 秒沒有結束；目前的報告：\n{partial}",
                    TIMEOUT.as_secs()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    };
    let text = std::fs::read_to_string(report)
        .map_err(|e| format!("自我測試沒有產生報告（{status}）：{e}"))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("自我測試報告：{e}"))?;
    println!("--- self-test\n{text}");
    let mut found = problems(&json, version, std::env::var_os("CI").is_some());
    if !is_empty_dir(guard) {
        found.push(format!("自我測試寫入了資料夾 {}", guard.display()));
    }
    if !status.success() || !found.is_empty() {
        return Err(format!(
            "打包後的程式自我測試失敗（{status}）：{}",
            found.join("；")
        ));
    }
    Ok(json)
}

/// `opendrs-rs --version` names this version.
pub fn check_cli(program: &Path, version: &str) -> Result<()> {
    let out = Command::new(program)
        .arg("--version")
        .output()
        .map_err(|e| format!("無法執行 {}：{e}", program.display()))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if out.status.success() && text.split_whitespace().any(|w| w == version) {
        println!("--- {}", text.trim());
        Ok(())
    } else {
        Err(format!("opendrs-rs --version 回報「{}」", text.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(pdf: &str) -> Value {
        let mut checks = serde_json::Map::new();
        for name in EXPECTED {
            checks.insert(name.into(), "ok: fine".into());
        }
        checks.insert("pdf".into(), pdf.into());
        serde_json::json!({"version": "0.1.0", "release": true, "ok": true, "checks": checks})
    }

    #[test]
    fn a_report_vouches_only_for_a_passing_release_of_this_version() {
        let good = report("skip: 找不到瀏覽器");
        assert!(problems(&good, "0.1.0", false).is_empty());
        assert_eq!(
            problems(&good, "0.1.0", true),
            ["pdf：skip: 找不到瀏覽器"],
            "CI runners have a browser"
        );
        let mut debug = good.clone();
        debug["release"] = false.into();
        assert_eq!(problems(&debug, "0.1.0", false), ["不是 release 建置"]);
        assert_eq!(problems(&good, "0.2.0", false).len(), 1);
        let mut failed = report("ok: msedge");
        failed["ok"] = false.into();
        failed["checks"]["fonts"] = "FAIL: 找不到中文字型".into();
        assert_eq!(
            problems(&failed, "0.1.0", false),
            ["有檢查沒有通過", "fonts：FAIL: 找不到中文字型"]
        );
    }

    #[test]
    fn a_missing_or_extra_check_is_refused() {
        let mut fewer = report("ok: x");
        fewer["checks"]
            .as_object_mut()
            .expect("checks")
            .remove("gpu");
        assert_eq!(problems(&fewer, "0.1.0", false).len(), 1);
        let mut more = report("ok: x");
        more["checks"]["extra"] = "ok: y".into();
        assert_eq!(problems(&more, "0.1.0", false).len(), 1);
    }

    #[test]
    fn the_guard_folder_must_stay_empty() {
        let dir = std::env::temp_dir().join(format!("xtask-guard-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        assert!(is_empty_dir(&dir));
        std::fs::write(dir.join("settings.json"), b"{}").expect("write");
        assert!(!is_empty_dir(&dir));
        std::fs::remove_dir_all(&dir).expect("cleanup");
        assert!(is_empty_dir(&dir), "a missing folder holds nothing");
    }
}
