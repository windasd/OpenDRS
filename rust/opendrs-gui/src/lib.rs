//! OpenDRS desktop editor for the Kydera DR-8500UV / HORA DH-98VU (egui), a port of the PySide6
//! GUI in `src/opendrs/gui`. All editing logic lives in `opendrs-core`, `opendrs-firmware` and
//! `opendrs-contacts`; this crate only draws it.

pub mod app;
pub mod dialogs;
pub mod document;
pub mod fonts;
pub mod menus;
pub mod ops;
pub mod pages;
pub mod print;
pub mod radio;
pub mod report;
pub mod table;

use std::path::PathBuf;

use eframe::egui;

/// Start the editor; `open` is a project to open at once (e.g. a file the OS was asked to open).
pub fn run(open: Option<PathBuf>) -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("OpenDRS {}", env!("CARGO_PKG_VERSION")))
            .with_inner_size([1280.0, 780.0])
            .with_min_inner_size([800.0, 480.0])
            .with_drag_and_drop(true),
        ..Default::default()
    };
    eframe::run_native(
        "OpenDRS",
        options,
        Box::new(move |cc| Ok(Box::new(app::OpenDrsApp::new(cc, open)))),
    )
}
