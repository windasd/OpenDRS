//! The editor's command-line options: `--version` and `--self-test REPORT.json`.

use std::path::PathBuf;
use std::process::Command;

use opendrs_gui::selftest::CHECK_NAMES;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opendrs-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

#[test]
fn version_prints_the_package_version() {
    let out = Command::new(env!("CARGO_BIN_EXE_opendrs"))
        .arg("--version")
        .output()
        .expect("run opendrs");
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        env!("CARGO_PKG_VERSION")
    );
}

/// Every check runs and is reported; whether all pass depends on the machine (fonts, GPU), so
/// only the report's shape and its agreement with the exit code are checked here. The release
/// build is held to "all passed" by `cargo xtask dist`.
#[test]
fn self_test_writes_a_report_that_matches_the_exit_code() {
    let dir = scratch("selftest");
    let report = dir.join("報告.json");
    // Where the user's settings would go: the self-test must leave it untouched.
    let guard = dir.join("data");
    std::fs::create_dir_all(&guard).expect("guard folder");
    let status = Command::new(env!("CARGO_BIN_EXE_opendrs"))
        .arg("--self-test")
        .arg(&report)
        .env("OPENDRS_DATA_DIR", &guard)
        .output()
        .expect("run opendrs")
        .status;
    assert!(
        std::fs::read_dir(&guard).expect("guard").next().is_none(),
        "the self-test wrote into the data folder"
    );
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&report).expect("report")).expect("JSON");
    assert_eq!(json["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(json["release"], !cfg!(debug_assertions));
    let checks = json["checks"].as_object().expect("checks");
    let mut names: Vec<&str> = checks.keys().map(String::as_str).collect();
    let mut expected = CHECK_NAMES.to_vec();
    names.sort_unstable();
    expected.sort_unstable();
    assert_eq!(names, expected);
    assert!(
        json.get("running").is_none(),
        "the final report is complete"
    );
    for name in [
        "icon", "codecs", "radio", "settings", "tls", "update", "window",
    ] {
        let line = checks[name].as_str().unwrap_or_default();
        assert!(line.starts_with("ok: "), "{name}: {line}");
    }
    let ok = json["ok"] == true;
    assert_eq!(status.code(), Some(if ok { 0 } else { 1 }), "{json}");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn unknown_options_are_refused_not_opened_as_files() {
    let run = |arg: &str| {
        Command::new(env!("CARGO_BIN_EXE_opendrs"))
            .arg(arg)
            .output()
            .expect("run opendrs")
    };
    let help = run("--help");
    assert!(help.status.success());
    assert!(String::from_utf8_lossy(&help.stdout).contains("--self-test"));
    assert_eq!(run("--selftest").status.code(), Some(2));
}
