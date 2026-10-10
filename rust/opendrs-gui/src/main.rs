// No console window behind the editor in release builds on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    opendrs_gui::run(std::env::args_os().nth(1).map(Into::into))
}
