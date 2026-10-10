//! The structural edits from the menus, headless: confirmations, undo labels and selections
//! must be the Python version's (`channel_ops.py`, `contact_ops.py`).

use std::path::{Path, PathBuf};

use opendrs_gui::app::{Dialog, OpenDrsApp};
use opendrs_gui::pages::Tab;
use opendrs_gui::table::model::Table;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dh98vu_20250811.F136")
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("opendrs-gui-ops-{name}-{}", std::process::id()));
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

fn app(scratch: &Scratch) -> OpenDrsApp {
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    assert!(app.open_path(&fixture()));
    app
}

fn question(app: &OpenDrsApp) -> (String, String) {
    match &app.dialog {
        Some(Dialog::Confirm { title, text, .. }) => (title.clone(), text.clone()),
        other => panic!("expected a question, got {other:?}"),
    }
}

fn message(app: &OpenDrsApp) -> (String, String) {
    match &app.dialog {
        Some(Dialog::Message { title, text }) => (title.clone(), text.clone()),
        other => panic!("expected a message, got {other:?}"),
    }
}

#[test]
fn delete_channel_asks_then_records_one_undo_step() {
    let scratch = Scratch::new("delete-channel");
    let mut app = app(&scratch);
    app.delete_channel();
    assert_eq!(
        message(&app),
        (
            "刪除頻道".into(),
            "請先在「頻道」分頁選擇要刪除的頻道。".into()
        )
    );
    app.dialog = None;
    app.select_row(Table::Channels, 0);
    app.delete_channel();
    let (title, text) = question(&app);
    assert_eq!(title, "刪除頻道");
    assert!(
        text.starts_with("刪除頻道「TG46600」（分區 1「MMDVM」第 1 台）？"),
        "{text}"
    );
    assert!(text.ends_with("可用「復原」退回。"));
    app.answer(false);
    assert_eq!(app.project().channels.len(), 24);
    app.delete_channel();
    app.answer(true);
    assert_eq!(app.project().channels.len(), 23);
    assert_eq!(app.doc.undo_text(), "復原 刪除頻道 TG46600");
    assert_eq!(app.pages.tables.channels.single_selection(), Some(0));
    app.undo();
    assert_eq!(app.project().channels.len(), 24);
}

#[test]
fn add_and_move_channels() {
    let scratch = Scratch::new("add-move");
    let mut app = app(&scratch);
    app.add_channel();
    assert_eq!(app.project().channels.len(), 25);
    assert_eq!(app.doc.undo_text(), "復原 新增頻道");
    assert_eq!(
        (app.tab, app.pages.tables.channels.single_selection()),
        (Tab::Channels, Some(24))
    );
    app.select_row(Table::Channels, 1);
    let second = app.project().channels[1].name(app.codec);
    app.shift_channel(-1);
    assert_eq!(app.doc.undo_text(), "復原 上移頻道");
    assert_eq!(app.project().channels[0].name(app.codec), second);
    assert_eq!(app.pages.tables.channels.single_selection(), Some(0));
    app.shift_channel(-1);
    assert_eq!(app.status, "已經在分區的第一台");
}

#[test]
fn zones_are_added_by_name_and_deleted_with_their_channels() {
    let scratch = Scratch::new("zones");
    let mut app = app(&scratch);
    app.add_zone();
    assert_eq!(
        app.dialog,
        Some(Dialog::AddZone {
            name: "ZONE5".into()
        })
    );
    app.dialog = None;
    app.add_zone_named(" NEWZ ");
    assert_eq!(app.project().zones.len(), 5);
    assert_eq!(app.doc.undo_text(), "復原 新增分區 NEWZ");
    assert_eq!(app.tab, Tab::Zones);
    app.select_row(Table::Zones, 0);
    app.delete_zone();
    let (_, text) = question(&app);
    assert!(
        text.starts_with("刪除分區 1「MMDVM」以及其中的 7 台頻道？"),
        "{text}"
    );
    app.answer(true);
    assert_eq!(app.project().zones.len(), 4);
    assert_eq!(app.doc.undo_text(), "復原 刪除分區 MMDVM");
}

#[test]
fn contacts_in_use_cannot_be_deleted_and_moves_need_one_row() {
    let scratch = Scratch::new("contacts");
    let mut app = app(&scratch);
    app.tab = Tab::Contacts;
    app.pages.tables.contacts.selected.extend([1, 2]); // TG46600_TW is a TX contact
    app.delete_contacts();
    let (title, text) = message(&app);
    assert_eq!(title, "刪除聯絡人");
    assert!(
        text.starts_with("下列頻道的發話對象是要刪除的聯絡人，請先改掉："),
        "{text}"
    );
    app.dialog = None;
    app.pages.tables.contacts.selected.extend([15, 16]);
    app.shift_contact(1);
    assert_eq!(app.status, "請只選一筆聯絡人再移動");
    app.pages.tables.contacts.selected = [15, 16].into();
    app.delete_contacts();
    let (_, text) = question(&app);
    assert!(text.starts_with("刪除 2 筆聯絡人（"), "{text}");
    app.answer(true);
    assert_eq!(app.project().contacts.len(), 357);
    assert_eq!(app.doc.undo_text(), "復原 刪除聯絡人 2 筆");
}

#[test]
fn clean_names_shows_an_example() {
    let scratch = Scratch::new("clean");
    let mut app = app(&scratch);
    app.clean_control_chars();
    let (_, text) = question(&app);
    assert!(
        text.starts_with(
            "將清除 聯絡人 343 筆 名稱中的換行／控制字元，例如「QA0BD␍␊」→「QA0BD」。"
        ),
        "{text}"
    );
    app.answer(true);
    assert_eq!(app.doc.undo_text(), "復原 清除換行字元");
    assert!(app.problems().is_empty());
    app.clean_control_chars();
    assert_eq!(message(&app).1, "沒有名稱含換行或控制字元。");
}

#[test]
fn channel_csv_round_trip_and_import() {
    let scratch = Scratch::new("csv");
    let mut app = app(&scratch);
    let path = scratch.0.join("ch.csv");
    assert!(app.write_channels_csv(&path));
    app.read_channels_csv(&path);
    assert_eq!(
        message(&app).1,
        "沒有需要變更的頻道（更新 0 個頻道，新增 0 個，未變動 24 個）。"
    );
    app.dialog = None;
    let edited =
        std::fs::read_to_string(&path)
            .expect("csv")
            .replacen("TG46600,439", "FROM_CSV,439", 1);
    std::fs::write(&path, edited).expect("write");
    app.read_channels_csv(&path);
    let (title, text) = question(&app);
    assert_eq!(title, "從 CSV 匯入頻道");
    assert_eq!(
        text,
        "更新 1 個頻道，新增 0 個，未變動 23 個（檔案編碼 utf-8-sig）。\n\n要套用嗎？可用「復原」退回。"
    );
    app.answer(true);
    assert_eq!(app.project().channels[0].name(app.codec), "FROM_CSV");
    assert_eq!(app.doc.undo_text(), "復原 CSV 匯入頻道");
}

#[test]
fn nothing_loaded_asks_for_a_project() {
    let scratch = Scratch::new("empty");
    let mut app = OpenDrsApp::with_data_dir(scratch.0.clone());
    app.add_channel();
    assert_eq!(
        message(&app),
        ("新增頻道".into(), "請先開啟專案檔，或從對講機讀取。".into())
    );
}

#[test]
fn the_report_is_titled_after_the_file() {
    let scratch = Scratch::new("title");
    let app = app(&scratch);
    assert_eq!(app.report_title(), "dh98vu_20250811");
    assert!(app.report().contains("<h1>dh98vu_20250811</h1>"));
}

/// Needs Microsoft Edge or Google Chrome: `cargo test -p opendrs-gui --test ops -- --ignored`.
#[test]
#[ignore = "starts a headless browser"]
fn a_pdf_is_exported_with_the_installed_browser() {
    let scratch = Scratch::new("pdf");
    let mut app = app(&scratch);
    let pdf = scratch.0.join("報表.pdf");
    assert!(app.export_pdf_to(&pdf), "{:?}", app.dialog);
    let bytes = std::fs::read(&pdf).expect("pdf");
    assert!(
        bytes.starts_with(b"%PDF-") && bytes.len() > 10_000,
        "{} bytes",
        bytes.len()
    );
    assert!(app.status.starts_with("已匯出 PDF 到 "));
}
