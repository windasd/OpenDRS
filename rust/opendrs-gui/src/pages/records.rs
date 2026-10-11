//! 頻道、分區、聯絡人: the record tables, with cell edits recorded in the undo history.

use eframe::egui;

use crate::actions::page_actions;
use crate::app::OpenDrsApp;
use crate::icons::tool_button;
use crate::table::model::{self, Table};
use crate::table::view::{self, TableAction, TableState};

/// The three tables' filters, sorts and selections.
#[derive(Default)]
pub struct RecordTables {
    pub channels: TableState,
    pub zones: TableState,
    pub contacts: TableState,
}

impl RecordTables {
    pub fn get_mut(&mut self, table: Table) -> &mut TableState {
        match table {
            Table::Channels => &mut self.channels,
            Table::Zones => &mut self.zones,
            Table::Contacts => &mut self.contacts,
        }
    }
}

/// The page's commands as buttons above the table (the menu items, with their short text).
fn actions_bar(app: &mut OpenDrsApp, ui: &mut egui::Ui, table: Table) {
    let mut chosen = None;
    ui.horizontal(|ui| {
        for action in page_actions(table) {
            match action {
                None => {
                    ui.separator();
                }
                Some(command) => {
                    if tool_button(ui, command.icon, command.short, true)
                        .on_hover_text(command.tip)
                        .clicked()
                    {
                        chosen = Some(command.run);
                    }
                }
            }
        }
    });
    if let Some(run) = chosen {
        run(app);
    }
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui, table: Table) {
    actions_bar(app, ui, table);
    let revision = app.doc.revision();
    let codec = app.codec;
    let state = app.pages.tables.get_mut(table);
    let actions = egui::ScrollArea::horizontal()
        .auto_shrink(false)
        .show(ui, |ui| {
            view::show(ui, state, app.doc.project(), revision, table, codec)
        })
        .inner;
    for action in actions {
        let TableAction::Apply { row, col, input } = action;
        match model::apply(app.doc.project(), table, row, col, &input, codec) {
            Ok(Some((project, label))) => app.commit(project, label),
            Ok(None) => {}
            Err(message) => app.warn("無法套用", message),
        }
    }
}
