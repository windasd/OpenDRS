//! 常用短信 page (port of `quickmsg_page.py` and `ListOpsMixin.apply_quick_message`): 16 rows,
//! edited in place on double-click.

use eframe::egui::{self, Layout};
use egui_extras::{Column, TableBuilder};
use opendrs_core::f136::Project;
use opendrs_core::quickmsg::{
    QUICK_MSG_COUNT, QUICK_MSG_SIZE, has_quick_messages, quick_messages, with_quick_message,
};
use opendrs_core::textcodec::NameCodec;

use super::form::cell_label;
use crate::app::OpenDrsApp;

pub const NOTE: &str = "雙擊一列可編輯；對講機的「常用短信」選單會列出這些內容。";
/// Error title of a refused edit.
pub const ERROR_TITLE: &str = "常用短信";
const ROW_HEIGHT: f32 = 24.0;

/// The column title.
pub fn header() -> String {
    format!("常用短信內容（最多 {QUICK_MSG_SIZE} bytes，中文每字 2 bytes）")
}

/// The 16 messages, or 16 empty rows when the image has no such area.
pub fn texts(image: &[u8], codec: NameCodec) -> Vec<String> {
    let messages = quick_messages(image, codec);
    if messages.is_empty() {
        vec![String::new(); QUICK_MSG_COUNT]
    } else {
        messages
    }
}

/// A row being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Editing {
    row: usize,
    text: String,
    focused: bool,
}

/// What this page keeps between frames.
#[derive(Default)]
pub struct State {
    /// Selected row (0-based).
    pub selected: Option<usize>,
    editing: Option<Editing>,
}

/// `常用短信 {n}`.
pub fn undo_label(number: usize) -> String {
    format!("常用短信 {number}")
}

/// Store message `number` (1-16) as typed; the error text is shown under [`ERROR_TITLE`].
pub fn apply(
    project: &Project,
    number: usize,
    text: &str,
    codec: NameCodec,
) -> Result<Project, String> {
    let image =
        with_quick_message(&project.image, number, text, codec).map_err(|e| e.to_string())?;
    Ok(Project {
        image,
        ..project.clone()
    })
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let image = &app.doc.project().image;
    let enabled = has_quick_messages(image);
    let texts = texts(image, app.codec);
    let state = &mut app.pages.quick_messages;
    let typed = ui
        .add_enabled_ui(enabled, |ui| {
            ui.label(NOTE);
            ui.add_space(4.0);
            table(ui, state, &texts)
        })
        .inner;
    let Some((row, text)) = typed else {
        return;
    };
    if text == texts[row] {
        return; // Qt reports an edit only when the text changed
    }
    match apply(app.project(), row + 1, &text, app.codec) {
        Ok(project) => app.commit(project, undo_label(row + 1)),
        Err(message) => app.warn(ERROR_TITLE, message),
    }
}

fn table(ui: &mut egui::Ui, state: &mut State, texts: &[String]) -> Option<(usize, String)> {
    let mut typed = None;
    TableBuilder::new(ui)
        .id_salt("quick-messages")
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(Layout::left_to_right(egui::Align::Center))
        .column(Column::exact(32.0))
        .column(Column::remainder())
        .header(ROW_HEIGHT, |mut head| {
            head.col(|_| {});
            head.col(|ui| {
                ui.strong(header());
            });
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, texts.len(), |mut table_row| {
                let row = table_row.index();
                table_row.set_selected(state.selected == Some(row));
                table_row.col(|ui| cell_label(ui, &(row + 1).to_string(), true));
                table_row.col(|ui| {
                    if state.editing.as_ref().is_some_and(|e| e.row == row) {
                        typed = editor(ui, state);
                    } else {
                        cell_label(ui, &texts[row], false);
                    }
                });
                let response = table_row.response();
                if response.clicked() {
                    state.selected = Some(row);
                }
                if response.double_clicked() {
                    state.selected = Some(row);
                    state.editing = Some(Editing {
                        row,
                        text: texts[row].clone(),
                        focused: false,
                    });
                }
            });
        });
    typed
}

/// The in-place editor: Enter or leaving the cell applies, Escape cancels.
fn editor(ui: &mut egui::Ui, state: &mut State) -> Option<(usize, String)> {
    let editing = state.editing.as_mut()?;
    let response = ui.add(
        egui::TextEdit::singleline(&mut editing.text)
            .desired_width(f32::INFINITY)
            .id_salt("quick-message-editor"),
    );
    if !editing.focused {
        response.request_focus();
        editing.focused = true;
        return None;
    }
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.editing = None;
        return None;
    }
    if !response.has_focus() {
        let done = state.editing.take()?;
        return Some((done.row, done.text));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_fill_sixteen_rows_even_without_the_area() {
        assert_eq!(texts(&[0u8; 16], NameCodec::Big5), vec![String::new(); 16]);
        let mut image = vec![0u8; 0x1300];
        image[0xFAE..0xFB2].copy_from_slice(b"Test");
        assert_eq!(texts(&image, NameCodec::Big5)[0], "Test");
        assert_eq!(header(), "常用短信內容（最多 40 bytes，中文每字 2 bytes）");
    }

    #[test]
    fn apply_reports_overlong_messages() {
        let project = Project {
            image: vec![0u8; 0x1300],
            ..Project::default()
        };
        let edited = apply(&project, 2, "測試訊息", NameCodec::Big5).expect("edit");
        assert_eq!(
            quick_messages(&edited.image, NameCodec::Big5)[1],
            "測試訊息"
        );
        let err = apply(&project, 3, &"x".repeat(41), NameCodec::Big5).unwrap_err();
        assert!(err.contains("41 bytes"), "{err}");
        assert_eq!(undo_label(16), "常用短信 16");
    }
}
