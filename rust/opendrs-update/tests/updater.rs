//! The updater script (test_update.py: "the updater script (PowerShell)").

mod common;

use std::path::{Path, PathBuf};

use common::{TempDir, program_zip};
use opendrs_update::{
    PROGRAM_ITEMS, PreparedUpdate, extract_program, updater_command, updater_script,
};

const BOM: &[u8] = b"\xef\xbb\xbf";

/// A folder with the old version installed, plus a user file that must never be touched.
fn installed(target: &Path, version: &str) {
    std::fs::create_dir_all(target.join("licenses")).expect("licenses");
    for file in [
        "OpenDRS.exe",
        "opendrs-rs.exe",
        "README.txt",
        "licenses/THIRD_PARTY_LICENSES.txt",
    ] {
        std::fs::write(target.join(file), version).expect("program file");
    }
    std::fs::write(target.join("我的專案.F136"), b"user file").expect("user file");
}

fn prepared(tmp: &Path) -> PreparedUpdate {
    let target = tmp.join("程式資料夾");
    installed(&target, "old");
    let staging = target.join(".opendrs-update-0.2.0");
    let archive = tmp.join("new.zip");
    std::fs::write(&archive, program_zip(&[])).expect("zip");
    let source = extract_program(&archive, &staging.join("unpacked")).expect("unpacked");
    PreparedUpdate {
        version: "0.2.0".into(),
        source,
        target,
        staging,
    }
}

#[test]
fn updater_script_swaps_every_program_item_and_is_ascii() {
    let script = updater_script();
    assert!(script.is_ascii());
    assert!(script.starts_with("param(") && script.ends_with("exit 1 }\n"));
    assert!(
        script.contains("$items = @('OpenDRS.exe', 'opendrs-rs.exe', 'licenses', 'README.txt')")
    );
    for item in PROGRAM_ITEMS {
        assert!(script.contains(&format!("'{item}'")), "{item}");
    }
    assert!(!script.contains("ITEMS"));
    assert!(script.contains("$exe = Join-Path $Target 'OpenDRS.exe'"));
}

#[test]
fn updater_command_writes_the_script_and_passes_the_folders() {
    let tmp = TempDir::new("command");
    let prepared = prepared(tmp.path());
    let log = tmp.path().join("logs").join("update.log");
    let command = updater_command(&prepared, 4242, &log, true).expect("command");
    let script = prepared.staging.join("apply-update.ps1");
    let text = |p: &PathBuf| p.display().to_string();
    let powershell = text(&opendrs_update::powershell_exe());
    let expected: Vec<String> = [
        powershell.as_str(),
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-WindowStyle",
        "Hidden",
        "-File",
        &text(&script),
        "-AppPid",
        "4242",
        "-Source",
        &text(&prepared.source),
        "-Target",
        &text(&prepared.target),
        "-Staging",
        &text(&prepared.staging),
        "-Log",
        &text(&log),
    ]
    .iter()
    .map(|s| (*s).to_string())
    .collect();
    assert_eq!(command, expected);

    let written = std::fs::read(&script).expect("script");
    let body = written.strip_prefix(BOM).expect("UTF-8 BOM");
    let newline = if cfg!(windows) { "\r\n" } else { "\n" };
    assert_eq!(body, updater_script().replace('\n', newline).as_bytes());

    let no_restart = updater_command(&prepared, 4242, &log, false).expect("command");
    assert_eq!(no_restart.last().map(String::as_str), Some("-NoRestart"));
    assert_eq!(no_restart[..no_restart.len() - 1], expected[..]);
}

#[test]
fn updater_command_needs_the_staging_folder() {
    let tmp = TempDir::new("command-missing");
    let prepared = PreparedUpdate {
        version: "0.2.0".into(),
        source: tmp.path().join("gone/unpacked/OpenDRS"),
        target: tmp.path().to_path_buf(),
        staging: tmp.path().join("gone"),
    };
    let err =
        updater_command(&prepared, 1, &tmp.path().join("update.log"), true).expect_err("no folder");
    assert!(err.to_string().contains("apply-update.ps1"));
}

#[cfg(not(windows))]
#[test]
fn installing_is_only_supported_on_windows() {
    let tmp = TempDir::new("launch");
    let prepared = prepared(tmp.path());
    let err =
        opendrs_update::launch(&prepared, &tmp.path().join("update.log")).expect_err("not here");
    assert_eq!(err.to_string(), opendrs_update::INSTALL_UNSUPPORTED);
    let release = opendrs_update::parse_release(
        &common::release_json("0.2.0", b"zip", None, true),
        common::PLATFORM,
    )
    .expect("release");
    let err = opendrs_update::prepare(&release, tmp.path(), &mut |_, _| {}, &|| false)
        .expect_err("not here");
    assert_eq!(err.to_string(), opendrs_update::INSTALL_UNSUPPORTED);
}

// ------------------------------------------------- running it (Windows PowerShell 5.1)
#[cfg(windows)]
mod windows {
    use std::process::{Command, Output, Stdio};
    use std::time::Instant;

    use super::*;

    fn run_updater(prepared: &PreparedUpdate, pid: u32, log: &Path) -> Output {
        let command = updater_command(prepared, pid, log, false).expect("command");
        Command::new(&command[0])
            .args(&command[1..])
            .stdin(Stdio::null())
            .output()
            .expect("powershell runs")
    }

    fn read_log(log: &Path) -> String {
        let bytes = std::fs::read(log).expect("log");
        String::from_utf8_lossy(bytes.strip_prefix(BOM).unwrap_or(&bytes)).into_owned()
    }

    #[test]
    fn updater_waits_for_the_app_then_swaps_only_program_files() {
        let tmp = TempDir::new("run");
        let prepared = prepared(tmp.path());
        let started = Instant::now();
        let mut app = Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Start-Sleep -Seconds 2",
            ])
            .stdin(Stdio::null())
            .spawn()
            .expect("the app");
        let log = tmp.path().join("update.log");
        let result = run_updater(&prepared, app.id(), &log);
        app.wait().expect("app exits");
        let text = read_log(&log);
        assert_eq!(result.status.code(), Some(0), "{text}");
        assert!(
            started.elapsed().as_secs_f64() >= 1.5,
            "it waited for the app to exit"
        );
        let target = &prepared.target;
        assert_eq!(
            std::fs::read(target.join("OpenDRS.exe")).expect("exe"),
            b"new"
        );
        assert_eq!(
            std::fs::read(target.join("opendrs-rs.exe")).expect("cli"),
            b"cli"
        );
        assert_eq!(
            std::fs::read(target.join("README.txt")).expect("readme"),
            b"readme"
        );
        let licenses = target.join("licenses").join("THIRD_PARTY_LICENSES.txt");
        assert_eq!(std::fs::read(licenses).expect("licenses"), b"licenses");
        assert_eq!(
            std::fs::read(target.join("我的專案.F136")).expect("user file"),
            b"user file"
        );
        assert!(
            !prepared.staging.exists() && text.contains("installed"),
            "{text}"
        );
    }

    #[test]
    fn updater_restores_the_old_version_when_a_file_is_locked() {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 1;
        const FILE_SHARE_WRITE: u32 = 2;

        let tmp = TempDir::new("run-locked");
        let prepared = prepared(tmp.path());
        let target = prepared.target.clone();
        let log = tmp.path().join("update.log");
        let result = {
            // Held open without FILE_SHARE_DELETE (as Python's open() does): it cannot be moved.
            let _held = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
                .open(target.join("README.txt"))
                .expect("hold README.txt");
            run_updater(&prepared, 999_999, &log)
        };
        let text = read_log(&log);
        assert_eq!(result.status.code(), Some(1), "{text}");
        assert!(text.contains("restoring the previous version"), "{text}");
        for file in [
            "OpenDRS.exe",
            "opendrs-rs.exe",
            "README.txt",
            "licenses/THIRD_PARTY_LICENSES.txt",
        ] {
            assert_eq!(
                std::fs::read(target.join(file)).expect(file),
                b"old",
                "{file}"
            );
        }
        // The download is kept for a retry.
        assert_eq!(
            std::fs::read(prepared.source.join("OpenDRS.exe")).expect("new"),
            b"new"
        );
    }
}
