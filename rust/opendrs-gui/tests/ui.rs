//! The editor driven headlessly (egui_kittest): no window, a scratch settings folder.

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_kittest::Harness;
use opendrs_gui::app::OpenDrsApp;
use opendrs_gui::pages::Tab;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("opendrs-gui-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch folder");
    dir
}

fn harness(settings: &Path, open: Option<PathBuf>) -> Harness<'static, OpenDrsApp> {
    let settings = settings.to_path_buf();
    Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| OpenDrsApp::with_context(&cc.egui_ctx, settings, open))
}

/// Two presses in one frame: kittest runs a frame per queued event (0.25 s apart), so two
/// `click()`s never make a double click.
fn double_click(harness: &mut Harness<'static, OpenDrsApp>, label: &str) {
    use egui_kittest::kittest::Queryable;
    let pos = harness.get_by_label(label).rect().center();
    let input = harness.input_mut();
    input.events.push(egui::Event::PointerMoved(pos));
    for pressed in [true, false, true, false] {
        input.events.push(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        });
    }
    harness.step();
}

#[test]
fn opens_a_project_and_remembers_it() {
    let dir = scratch("open");
    let mut harness = harness(&dir, Some(fixture("dh98vu_20250811.F136")));
    harness.run();
    let app = harness.state();
    assert!(app.doc.has_project() && !app.doc.is_dirty());
    assert_eq!(app.project().channels.len(), 24);
    let (settings, _) = opendrs_core::settings::load_settings(&dir);
    assert!(settings.recent_files[0].ends_with("dh98vu_20250811.F136"));
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_broken_file_is_reported_not_opened() {
    let dir = scratch("broken");
    let bad = dir.join("bad.F136");
    std::fs::write(&bad, b"x\r\n\r\n").expect("write");
    let mut harness = harness(&dir, Some(bad));
    harness.run();
    let app = harness.state();
    assert!(!app.doc.has_project());
    assert!(
        matches!(&app.dialog, Some(opendrs_gui::app::Dialog::Message { title, .. }) if title == "開啟失敗")
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

/// Renders the window to `target/gui-screens/*.png` for a look (needs a GPU or a software
/// adapter): `cargo test -p opendrs-gui --test ui -- --ignored`.
#[test]
#[ignore = "renders screenshots for a manual look"]
fn screenshots() {
    let dir = scratch("shots");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gui-screens");
    std::fs::create_dir_all(&out).expect("output folder");
    let mut harness = harness(&dir, Some(fixture("dh98vu_20250811.F136")));
    for (tab, name) in [
        (Tab::Device, "device"),
        (Tab::Channels, "channels"),
        (Tab::Zones, "zones"),
        (Tab::Contacts, "contacts"),
    ] {
        harness.state_mut().tab = tab;
        harness.run();
        let image = harness.render().expect("render");
        image.save(out.join(format!("{name}.png"))).expect("save");
    }
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn alias_is_applied_on_enter() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let dir = scratch("alias");
    let mut harness = harness(&dir, Some(fixture("dh98vu_20250811.F136")));
    harness.run();
    let alias_field = |h: &Harness<'static, OpenDrsApp>| {
        h.get_all_by_role(eframe::egui::accesskit::Role::TextInput)
            .find(|n| n.accesskit_node().value().as_deref() == Some("QA0AFK"))
            .expect("alias field")
            .focus();
    };
    alias_field(&harness);
    harness.run();
    harness.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::A);
    harness
        .get_all_by_role(eframe::egui::accesskit::Role::TextInput)
        .find(|n| n.is_focused())
        .expect("focused field")
        .type_text("BX1ABC");
    harness.run();
    harness.key_press(eframe::egui::Key::Enter);
    harness.run();
    assert_eq!(harness.state().project().device().alias, "BX1ABC");
    assert_eq!(harness.state().doc.undo_text(), "復原 修改別名");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn a_channel_name_is_edited_in_the_table() {
    use egui_kittest::kittest::Queryable;
    let dir = scratch("cell");
    let mut harness = harness(&dir, Some(fixture("dh98vu_20250811.F136")));
    harness.state_mut().tab = Tab::Channels;
    harness.run();
    double_click(&mut harness, "TG46600");
    harness.run();
    let editors = harness
        .get_all_by_role(eframe::egui::accesskit::Role::TextInput)
        .count();
    assert!(
        editors >= 2,
        "the cell editor opens next to the search box ({editors})"
    );
    harness.key_press_modifiers(eframe::egui::Modifiers::COMMAND, eframe::egui::Key::A);
    harness
        .get_all_by_role(eframe::egui::accesskit::Role::TextInput)
        .find(|n| n.is_focused())
        .expect("cell editor")
        .type_text("RENAMED");
    harness.run();
    harness.key_press(eframe::egui::Key::Enter);
    harness.run();
    let app = harness.state();
    assert_eq!(app.project().channels[0].name(app.codec), "RENAMED");
    assert_eq!(app.doc.undo_text(), "復原 編輯");
    std::fs::remove_dir_all(&dir).expect("cleanup");
}
