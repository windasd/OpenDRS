//! 新增聯絡人 (port of `contact_dialog.py` and `ContactOpsMixin.add_contact`): type, name and ID
//! of one contact. 確定 checks it with `contact_edit::add_contact` and keeps the dialog open,
//! showing the reason, while it cannot be added (duplicate type and ID, name too long, …).

use eframe::egui;
use opendrs_core::codeplug::MAX_CONTACTS;
use opendrs_core::contact_edit::add_contact;
use opendrs_core::pystr::strip;
use opendrs_core::records::{Contact, NAME_SIZE};

use crate::app::{Dialog, OpenDrsApp};
use crate::icons;
use crate::table::model::Table;

pub const TITLE: &str = "新增聯絡人";
pub const NOTE: &str = "加在清單最後面；其他聯絡人的編號不變。";
/// The contact types in the type box (`Contact.KINDS`).
pub const KINDS: [(u8, &str); 4] = [(4, "組呼"), (5, "個呼"), (6, "全呼"), (7, "無地址呼")];
/// At most this many digits can be typed into the ID box (as the Python validator `\d{0,8}`).
const ID_DIGITS: usize = 8;

/// What the dialog holds between frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddContactDialog {
    pub kind: u8,
    pub name: String,
    pub call_id: String,
    /// Why the last 確定 was refused ("" while there is nothing to say).
    pub error: String,
}

impl AddContactDialog {
    pub fn new(kind: u8) -> Self {
        let kind = if KINDS.iter().any(|(k, _)| *k == kind) {
            kind
        } else {
            Contact::GROUP_CALL
        };
        Self {
            kind,
            name: String::new(),
            call_id: String::new(),
            error: String::new(),
        }
    }

    fn kind_label(&self) -> &'static str {
        KINDS
            .iter()
            .find(|(k, _)| *k == self.kind)
            .map_or("", |(_, label)| label)
    }
}

/// Keep only the digits of what was typed, at most [`ID_DIGITS`] of them.
pub fn id_digits(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_digit)
        .take(ID_DIGITS)
        .collect()
}

impl OpenDrsApp {
    /// 聯絡人 › 新增聯絡人…: ask for the new contact (its type defaults to the selected one's).
    pub fn add_contact(&mut self) {
        if !self.require_project(TITLE) {
            return;
        }
        if self.project().contacts.len() >= MAX_CONTACTS {
            // say so before asking for anything
            self.warn(TITLE, format!("聯絡人已達對講機上限 {MAX_CONTACTS} 筆。"));
            return;
        }
        let kind = self
            .pages
            .tables
            .contacts
            .single_selection()
            .and_then(|row| self.project().contacts.get(row))
            .map_or(Contact::GROUP_CALL, Contact::kind);
        self.dialog = Some(Dialog::AddContact(AddContactDialog::new(kind)));
    }

    /// Append the contact; returns why it cannot be added. On success the contacts tab shows the
    /// new row, with the search and filters cleared if they would hide it.
    pub fn add_contact_values(
        &mut self,
        kind: u8,
        name: &str,
        call_id: u32,
    ) -> Result<usize, String> {
        let (project, row) = add_contact(
            self.project(),
            kind,
            name,
            call_id,
            self.codec,
            MAX_CONTACTS,
        )
        .map_err(|e| e.0)?;
        self.commit(project, format!("新增聯絡人 {}", strip(name)));
        self.select_row(Table::Contacts, row);
        let revision = self.doc.revision();
        let codec = self.codec;
        let state = &mut self.pages.tables.contacts;
        if !state.shows(self.doc.project(), revision, Table::Contacts, codec, row) {
            state.reset(); // hidden by the search or a filter
            state.select_only(row);
        }
        Ok(row)
    }

    pub(crate) fn add_contact_dialog(&mut self, ctx: &egui::Context, mut dialog: AddContactDialog) {
        let response = egui::Modal::new(egui::Id::new("add-contact")).show(ctx, |ui| {
            let danger = icons::status_colours(ui.visuals().dark_mode).0;
            ui.heading(TITLE);
            egui::Grid::new("add-contact-form")
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("類型：");
                    egui::ComboBox::from_id_salt("add-contact-kind")
                        .selected_text(dialog.kind_label())
                        .show_ui(ui, |ui| {
                            for (kind, label) in KINDS {
                                ui.selectable_value(&mut dialog.kind, kind, label);
                            }
                        });
                    ui.end_row();
                    ui.label("名稱：");
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.name)
                            .hint_text(format!("最多 {NAME_SIZE} bytes（中文字每字 2 bytes）")),
                    );
                    ui.end_row();
                    ui.label("ID：");
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut dialog.call_id)
                            .hint_text(format!("1–{}", Contact::MAX_ID)),
                    );
                    if edit.changed() {
                        dialog.call_id = id_digits(&dialog.call_id);
                    }
                    ui.end_row();
                });
            ui.weak(NOTE);
            if !dialog.error.is_empty() {
                ui.colored_label(danger, &dialog.error);
            }
            ui.add_space(8.0);
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if ui.button("確定").clicked() || enter {
                    Some(true)
                } else if ui.button("取消").clicked() {
                    Some(false)
                } else {
                    None
                }
            })
            .inner
        });
        let choice = response
            .inner
            .or_else(|| response.should_close().then_some(false));
        match choice {
            Some(true) => {
                if dialog.call_id.is_empty() {
                    dialog.error = "請輸入 ID".into();
                    self.dialog = Some(Dialog::AddContact(dialog));
                    return;
                }
                let call_id = dialog.call_id.parse::<u32>().unwrap_or(0);
                self.dialog = None;
                if let Err(problem) = self.add_contact_values(dialog.kind, &dialog.name, call_id) {
                    dialog.error = problem;
                    self.dialog = Some(Dialog::AddContact(dialog));
                }
            }
            Some(false) => self.dialog = None,
            None => self.dialog = Some(Dialog::AddContact(dialog)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_box_keeps_at_most_eight_digits() {
        assert_eq!(id_digits("1,000"), "1000");
        assert_eq!(id_digits("+5-1"), "51");
        assert_eq!(id_digits("１２3"), "3"); // full-width digits are not ASCII digits
        assert_eq!(id_digits("1234567890"), "12345678");
    }

    #[test]
    fn unknown_kind_defaults_to_group_call() {
        assert_eq!(AddContactDialog::new(5).kind, 5);
        assert_eq!(AddContactDialog::new(9).kind, Contact::GROUP_CALL);
        assert_eq!(AddContactDialog::new(7).kind_label(), "無地址呼");
    }
}
