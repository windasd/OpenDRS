//! 頻道、分區、聯絡人: the record tables, with cell edits recorded in the undo history.

use eframe::egui;

use crate::app::OpenDrsApp;
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

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui, table: Table) {
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
