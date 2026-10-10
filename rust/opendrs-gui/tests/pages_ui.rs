//! The settings-area pages driven headlessly (egui_kittest): each page renders, the member
//! editors commit or cancel, and pages without their image area are disabled. A scratch
//! settings folder is used, never the real one.

use std::path::{Path, PathBuf};

use eframe::egui;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable, by};
use opendrs_core::f136::Project;
use opendrs_core::radio_settings::{RadioSetting, by_key};
use opendrs_core::scanlist::scan_lists;
use opendrs_gui::app::{Dialog, OpenDrsApp};
use opendrs_gui::document::Document;
use opendrs_gui::pages::{Tab, alarm_page, group_page, scan_page};

const PAGES: [Tab; 5] = [
    Tab::ScanLists,
    Tab::RxGroups,
    Tab::Settings,
    Tab::QuickMessages,
    Tab::Alarms,
];

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dh98vu_20250811.F136")
}

/// A fresh scratch folder for the app's settings; removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opendrs-gui-pages-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch folder");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn harness(settings: &Path) -> Harness<'static, OpenDrsApp> {
    let settings = settings.to_path_buf();
    Harness::builder()
        .with_size([1280.0, 780.0])
        .build_eframe(move |cc| OpenDrsApp::with_context(&cc.egui_ctx, settings, Some(fixture())))
}

#[test]
fn every_page_renders() {
    let dir = Scratch::new("render");
    let mut harness = harness(&dir.0);
    for tab in PAGES {
        harness.state_mut().tab = tab;
        harness.run();
    }
    for page in 0..opendrs_core::radio_settings::pages().len() {
        harness.state_mut().tab = Tab::Settings;
        harness.state_mut().pages.settings.page = page;
        harness.run();
    }
    let app = harness.state();
    assert!(app.dialog.is_none() && !app.doc.is_dirty());
}

#[test]
fn scan_editor_adds_a_channel() {
    let dir = Scratch::new("scan");
    let mut harness = harness(&dir.0);
    harness.state_mut().tab = Tab::ScanLists;
    harness.run();
    double_click(&mut harness, "ScanFRS_TW");
    harness.run();
    assert!(harness.state().pages.scan.editor.is_some());
    harness.get_by_label("MMDVM／TG46600").click();
    harness.run();
    harness.get_by_label("已勾選 15 / 16 個頻道");
    harness.key_press(egui::Key::Enter); // the default button
    harness.run();
    let app = harness.state();
    assert!(app.pages.scan.editor.is_none() && app.dialog.is_none());
    assert_eq!(app.doc.undo_text(), "復原 編輯掃描清單 2");
    let entries = scan_lists(&app.project().image)[1].entries();
    assert_eq!(entries.len(), 15);
    assert_eq!(entries[0], (1, 1)); // list order, not the order of ticking
}

#[test]
fn group_editor_cancels_and_blocks_over_the_limit() {
    let dir = Scratch::new("group");
    let mut harness = harness(&dir.0);
    harness.state_mut().tab = Tab::RxGroups;
    harness.run();
    assert!(group_page::open_editor(harness.state_mut(), 1));
    harness
        .state_mut()
        .pages
        .groups
        .editor
        .as_mut()
        .expect("editor")
        .checked
        .fill(true);
    harness.run();
    assert!(harness.get_by_label("確定").accesskit_node().is_disabled());
    harness.get_by_label("取消").click();
    harness.run();
    let app = harness.state();
    assert!(app.pages.groups.editor.is_none() && !app.doc.can_undo());
}

#[test]
fn an_open_editor_closes_when_the_project_changes() {
    let dir = Scratch::new("undo");
    let mut harness = harness(&dir.0);
    harness.state_mut().tab = Tab::ScanLists;
    let edited = scan_page::apply(
        harness.state().project(),
        3,
        "NEW",
        &[(1, 1)],
        (false, 0, (0, 0)),
        harness.state().codec,
    )
    .expect("edit");
    harness.state_mut().commit(edited, scan_page::undo_label(3));
    harness.run();
    assert!(scan_page::open_editor(harness.state_mut(), 3));
    harness.run();
    harness.state_mut().undo();
    harness.run();
    assert!(harness.state().pages.scan.editor.is_none());
}

/// Double-click the widget labelled `label` (both clicks in one frame: kittest advances the
/// clock a quarter second per queued event, too slow for a double click).
fn double_click(harness: &mut Harness<'static, OpenDrsApp>, label: &str) {
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

/// The line editor showing `value`.
fn line_editor<'t>(
    harness: &'t Harness<'static, OpenDrsApp>,
    value: &'t str,
) -> egui_kittest::Node<'t> {
    harness.get(by().value(value).role(egui::accesskit::Role::TextInput))
}

/// Click into the line editor showing `value`, type `text` at the end, and press Enter.
fn type_into(harness: &mut Harness<'static, OpenDrsApp>, value: &str, text: &str) {
    line_editor(harness, value).click();
    harness.run();
    harness.key_press(egui::Key::End);
    line_editor(harness, value).type_text(text);
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
}

#[test]
fn text_settings_apply_on_enter() {
    let dir = Scratch::new("dtmf");
    let mut harness = harness(&dir.0);
    harness.state_mut().tab = Tab::Settings;
    harness
        .state_mut()
        .pages
        .settings
        .select_page("DMR服務項目");
    harness.run();
    type_into(&mut harness, "1234", "5");
    let app = harness.state();
    assert_eq!(app.doc.undo_text(), "復原 DMR服務項目：DTMF碼");
    let Some(RadioSetting::Text(dtmf)) = by_key("dtmf_code") else {
        panic!("dtmf_code")
    };
    assert_eq!(dtmf.text(&app.project().image).as_deref(), Some("12345"));
    line_editor(&harness, "12345"); // the editor follows the project
    harness.state_mut().undo(); // applied once: one undo step
    harness.run();
    assert!(!harness.state().doc.can_undo());
    line_editor(&harness, "1234");
}

#[test]
fn quick_messages_are_edited_in_place() {
    let dir = Scratch::new("quick");
    let mut harness = harness(&dir.0);
    harness.state_mut().tab = Tab::QuickMessages;
    harness.run();
    double_click(&mut harness, "3");
    harness.run();
    assert_eq!(harness.state().pages.quick_messages.selected, Some(2));
    let editor = harness.get_by_role(egui::accesskit::Role::TextInput);
    editor.type_text("Hi");
    harness.run();
    harness.key_press(egui::Key::Enter);
    harness.run();
    let app = harness.state();
    assert_eq!(app.doc.undo_text(), "復原 常用短信 3");
    let texts = opendrs_gui::pages::quickmsg_page::texts(&app.project().image, app.codec);
    assert_eq!(texts[2], "Hi");
}

#[test]
fn refused_text_shows_the_warning_and_the_stored_value() {
    let dir = Scratch::new("dtmf-bad");
    let mut harness = harness(&dir.0);
    harness.state_mut().tab = Tab::Settings;
    harness
        .state_mut()
        .pages
        .settings
        .select_page("DMR服務項目");
    harness.run();
    type_into(&mut harness, "1234", "E");
    let app = harness.state();
    assert!(!app.doc.can_undo());
    assert!(matches!(
        &app.dialog,
        Some(Dialog::Message { title, text }) if title == "對講機設定" && text.contains("「E」")
    ));
    line_editor(&harness, "1234");
}

#[test]
fn encryption_aliases_apply_on_enter() {
    let dir = Scratch::new("alarm");
    let mut harness = harness(&dir.0);
    harness.state_mut().tab = Tab::Alarms;
    harness.run();
    type_into(&mut harness, "windasd", "2");
    assert_eq!(harness.state().doc.undo_text(), "復原 加密金鑰 1");
    let rows = alarm_page::key_rows(&harness.state().project().image, harness.state().codec);
    assert_eq!(
        rows[0],
        ["windasd2".to_string(), "12 34 56 78 9A BC".to_string()]
    );
}

#[test]
fn pages_without_their_area_are_disabled() {
    let dir = Scratch::new("short");
    let mut harness = harness(&dir.0);
    let project = harness.state().project().clone();
    harness.state_mut().doc = Document::unsaved(
        Project {
            image: project.image[..0x200].to_vec(),
            ..project
        },
        "測試",
    );
    harness.state_mut().tab = Tab::ScanLists;
    harness.run();
    assert!(
        harness
            .get_by_label("編輯選取的清單…")
            .accesskit_node()
            .is_disabled()
    );
    for tab in PAGES {
        harness.state_mut().tab = tab;
        harness.run();
    }
    assert!(harness.state().dialog.is_none());
}

/// Renders the pages to `target/gui-screens/pages-*.png` for a look (needs a GPU or a software
/// adapter): `cargo test -p opendrs-gui --test pages_ui -- --ignored`.
#[test]
#[ignore = "renders screenshots for a manual look"]
fn screenshots() {
    let dir = Scratch::new("shots");
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../target/gui-screens");
    std::fs::create_dir_all(&out).expect("output folder");
    let mut harness = harness(&dir.0);
    let save = |harness: &mut Harness<'static, OpenDrsApp>, name: &str| {
        harness.run();
        let image = harness.render().expect("render");
        image
            .save(out.join(format!("pages-{name}.png")))
            .expect("save");
    };
    for (tab, name) in [
        (Tab::ScanLists, "scan"),
        (Tab::RxGroups, "groups"),
        (Tab::Settings, "settings"),
        (Tab::QuickMessages, "quick"),
        (Tab::Alarms, "alarms"),
    ] {
        harness.state_mut().tab = tab;
        save(&mut harness, name);
    }
    harness.state_mut().pages.settings.page = 1;
    harness.state_mut().tab = Tab::Settings;
    save(&mut harness, "settings-menu");
    harness
        .state_mut()
        .pages
        .settings
        .select_page("DMR服務項目");
    save(&mut harness, "settings-dmr");
    harness.state_mut().tab = Tab::ScanLists;
    scan_page::open_editor(harness.state_mut(), 2);
    save(&mut harness, "scan-editor");
    harness.state_mut().pages.scan.editor = None;
    harness.state_mut().tab = Tab::RxGroups;
    group_page::open_editor(harness.state_mut(), 1);
    save(&mut harness, "group-editor");
}
