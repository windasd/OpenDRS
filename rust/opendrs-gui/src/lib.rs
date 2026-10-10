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
pub mod selftest;
pub mod table;

use std::path::PathBuf;

use eframe::egui;

/// The application icon (rendered by `tools/make_icons.py`).
pub const ICON_PNG: &[u8] = include_bytes!("../assets/opendrs.png");

/// Start the editor; `open` is a project to open at once (e.g. a file the OS was asked to open).
pub fn run(open: Option<PathBuf>) -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title(format!("OpenDRS {}", env!("CARGO_PKG_VERSION")))
        .with_app_id("opendrs") // matches opendrs.desktop on Linux
        .with_inner_size([1280.0, 780.0])
        .with_min_inner_size([800.0, 480.0])
        .with_drag_and_drop(true);
    if let Ok(icon) = eframe::icon_data::from_png_bytes(ICON_PNG) {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "OpenDRS",
        options,
        Box::new(move |cc| Ok(Box::new(app::OpenDrsApp::new(cc, open)))),
    )
}
