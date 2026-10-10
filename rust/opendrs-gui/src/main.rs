// No console window behind the editor in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! `opendrs [project.F136]`, `opendrs --version`, `opendrs --self-test [REPORT.json]`.

use std::path::PathBuf;
use std::process::ExitCode;

use opendrs_gui::selftest;

const USAGE: &str =
    "用法：opendrs [專案.F136]\n　　　opendrs --version\n　　　opendrs --self-test [報告.json]";

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    match args.first().and_then(|a| a.to_str()) {
        Some("--version") => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--self-test") => {
            let report = args
                .get(1)
                .map_or_else(|| PathBuf::from(selftest::DEFAULT_REPORT), PathBuf::from);
            ExitCode::from(u8::try_from(selftest::run(&report)).unwrap_or(1))
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(flag) if flag.starts_with("--") => {
            eprintln!("不認得的參數 {flag}\n{USAGE}");
            ExitCode::from(2)
        }
        _ => open_window(args.into_iter().next().map(PathBuf::from)),
    }
}

fn open_window(project: Option<PathBuf>) -> ExitCode {
    match opendrs_gui::run(project) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // The Windows release has no console: say it in a message box.
            let text = format!(
                "無法開啟視窗：{e}\n\n可執行「opendrs --self-test 報告.json」檢查缺少的元件。"
            );
            eprintln!("{text}");
            let _ = rfd::MessageDialog::new()
                .set_title("OpenDRS")
                .set_description(text)
                .set_level(rfd::MessageLevel::Error)
                .show();
            ExitCode::FAILURE
        }
    }
}
