//! The egui view of a record table (port of `filter_bar.py` and the table views): the search
//! and filter bar, a sortable virtualized table, row selection, and cell editing — text cells
//! on double-click, drop-down cells on one click (searchable when the list comes from the
//! project). All decisions come from [`super::model`] and [`super::filter`].

use std::collections::BTreeSet;

use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use opendrs_core::f136::Project;
use opendrs_core::pystr::casefold;
use opendrs_core::textcodec::NameCodec;

use super::filter::{self, Filter, Order, Show};
use super::model::{self, CONTACT_KINDS, Editor, Input, Table};

const ROW_HEIGHT: f32 = 22.0;
const ERROR_BACKGROUND: Color32 = Color32::from_rgb(0xff, 0xd6, 0xd6);
/// Drop-down lists show this many rows before scrolling (Qt's `setMaxVisibleItems(16)`).
const POPUP_ROWS: f32 = 16.0;

/// A text cell being edited.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Editing {
    row: usize,
    col: usize,
    text: String,
    focused: bool,
}

/// What the user did in the table this frame; applied by the page after drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TableAction {
    Apply {
        row: usize,
        col: usize,
        input: Input,
    },
}

/// What the cached rows were computed for: project revision, codec, filter and sort.
type CacheKey = (u64, NameCodec, Filter, Option<(usize, Order)>);

/// Filter, sort, selection and the cell editor of one table, kept between frames.
#[derive(Default)]
pub struct TableState {
    pub filter: Filter,
    pub sort: Option<(usize, Order)>,
    /// Selected rows (record rows, not display positions).
    pub selected: BTreeSet<usize>,
    anchor: Option<usize>,
    editing: Option<Editing>,
    /// The text typed into the open drop-down's search box.
    picker_search: String,
    /// (revision, codec, filter, sort) the cached rows were computed for.
    cache_key: Option<CacheKey>,
    rows: Vec<usize>,
    /// Scroll to this record row on the next frame.
    reveal: Option<usize>,
}

impl TableState {
    /// The rows to show, recomputed only when the project, codec, filter or sort changed.
    fn rows(
        &mut self,
        project: &Project,
        revision: u64,
        table: Table,
        codec: NameCodec,
    ) -> &[usize] {
        let key = (revision, codec, self.filter.clone(), self.sort);
        if self.cache_key.as_ref() != Some(&key) {
            self.rows = filter::visible_rows(project, table, codec, &self.filter, self.sort);
            self.selected
                .retain(|r| *r < model::row_count(project, table));
            self.cache_key = Some(key);
        }
        &self.rows
    }

    /// Select one row and scroll to it (after add / delete / move).
    pub fn select_only(&mut self, row: usize) {
        self.selected = BTreeSet::from([row]);
        self.anchor = Some(row);
        self.reveal = Some(row);
    }

    /// The single selected row, if exactly one is selected.
    pub fn single_selection(&self) -> Option<usize> {
        (self.selected.len() == 1)
            .then(|| self.selected.first().copied())
            .flatten()
    }

    /// 清除篩選與排序.
    pub fn reset(&mut self) {
        self.filter = Filter::default();
        self.sort = None;
    }

    fn click(&mut self, row: usize, modifiers: egui::Modifiers, multi: bool) {
        if multi && modifiers.shift {
            if let Some(anchor) = self.anchor {
                let (a, b) = (
                    self.rows.iter().position(|r| *r == anchor),
                    self.rows.iter().position(|r| *r == row),
                );
                if let (Some(a), Some(b)) = (a, b) {
                    let span = &self.rows[a.min(b)..=a.max(b)];
                    self.selected.extend(span.iter().copied());
                    return;
                }
            }
        }
        if multi && modifiers.command {
            if !self.selected.remove(&row) {
                self.selected.insert(row);
            }
        } else {
            self.selected = BTreeSet::from([row]);
        }
        self.anchor = Some(row);
    }
}

/// The search box, scope, type and quick-filter controls, the reset button and the count.
fn filter_bar(ui: &mut egui::Ui, state: &mut TableState, table: Table, total: usize) {
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.filter.text)
                .hint_text("搜尋（多個關鍵字以空白分隔，全部符合才列出）")
                .desired_width(320.0)
                .id(egui::Id::new(("search", table))),
        );
        ui.label("範圍：");
        let titles = model::columns(table);
        let scope_text = state
            .filter
            .scope
            .map_or("全部欄位", |c| titles[c].title.as_str());
        egui::ComboBox::from_id_salt(("scope", table))
            .selected_text(scope_text)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut state.filter.scope, None, "全部欄位");
                for (i, column) in titles.iter().enumerate() {
                    ui.selectable_value(&mut state.filter.scope, Some(i), &column.title);
                }
            })
            .response
            .on_hover_text("搜尋範圍");
        if table == Table::Contacts {
            let kind_text = state
                .filter
                .kind
                .and_then(|k| CONTACT_KINDS.iter().find(|(v, _)| *v == k))
                .map_or("全部類型", |(_, label)| label);
            egui::ComboBox::from_id_salt(("kind", table))
                .selected_text(kind_text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut state.filter.kind, None, "全部類型");
                    for (kind, label) in CONTACT_KINDS {
                        ui.selectable_value(&mut state.filter.kind, Some(kind), label);
                    }
                })
                .response
                .on_hover_text("只顯示此類型");
        }
        ui.label("顯示：");
        egui::ComboBox::from_id_salt(("show", table))
            .selected_text(state.filter.show.label())
            .show_ui(ui, |ui| {
                for show in Show::choices(table) {
                    ui.selectable_value(&mut state.filter.show, *show, show.label());
                }
            })
            .response
            .on_hover_text("快速篩選");
        if ui.button("清除篩選與排序").clicked() {
            state.reset();
        }
        let shown = state.rows.len();
        ui.weak(filter::count_text(shown, total, state.filter.is_filtered()));
    });
}

/// One drop-down cell: the current label, and on click the list (with a search box for
/// project lists); picking an entry yields its value.
fn choice_cell(
    ui: &mut egui::Ui,
    id: egui::Id,
    (current_text, current): (&str, Option<i64>),
    (items, none, searchable): (&[(String, i64)], Option<&str>, bool),
    search: &mut String,
) -> Option<i64> {
    let mut picked = None;
    let height = POPUP_ROWS * (ui.text_style_height(&egui::TextStyle::Body) + 4.0);
    let response = egui::ComboBox::from_id_salt(id)
        .selected_text(current_text)
        .width(ui.available_width())
        .height(height)
        .show_ui(ui, |ui| {
            if searchable {
                ui.add(
                    egui::TextEdit::singleline(search)
                        .hint_text("搜尋…")
                        .desired_width(f32::INFINITY),
                );
            }
            let needle = casefold(search.trim());
            let entries = none
                .map(|n| (n.to_string(), 0))
                .into_iter()
                .chain(items.iter().cloned());
            for (label, value) in entries {
                if !needle.is_empty() && !casefold(&label).contains(&needle) {
                    continue;
                }
                if ui
                    .selectable_label(current == Some(value), &label)
                    .clicked()
                {
                    picked = Some(value);
                }
            }
        });
    if response.response.clicked() {
        search.clear();
    }
    picked
}

/// Draw `table` for `project`; edits come back as actions for the page to apply.
pub fn show(
    ui: &mut egui::Ui,
    state: &mut TableState,
    project: &Project,
    revision: u64,
    table: Table,
    codec: NameCodec,
) -> Vec<TableAction> {
    let total = model::row_count(project, table);
    state.rows(project, revision, table, codec);
    filter_bar(ui, state, table, total);
    let rows: Vec<usize> = state.rows(project, revision, table, codec).to_vec();
    let columns = model::columns(table);
    let editors: Vec<Editor> = (0..columns.len())
        .map(|c| model::editor(project, table, c, codec))
        .collect();
    let mut actions = Vec::new();
    let mut clicked: Option<(usize, egui::Modifiers)> = None;
    let mut sort_click = None;
    let multi = table == Table::Contacts;

    let widths: Vec<Column> = columns
        .iter()
        .zip(&editors)
        .map(|(column, editor)| match (editor, column.title.as_str()) {
            (_, "#") => Column::initial(44.0),
            (
                Editor::Choices {
                    searchable: true, ..
                },
                _,
            ) => Column::initial(180.0),
            (Editor::Choices { items, .. }, _) => Column::initial(choice_width(ui, items)),
            _ => Column::auto().at_least(56.0),
        })
        .collect();
    let mut builder = TableBuilder::new(ui)
        .id_salt(("table", table))
        .striped(true)
        .resizable(true)
        .sense(egui::Sense::click())
        .cell_layout(egui::Layout::left_to_right(egui::Align::Center));
    if let Some(row) = state.reveal.take() {
        if let Some(position) = rows.iter().position(|r| *r == row) {
            builder = builder.scroll_to_row(position, Some(egui::Align::Center));
        }
    }
    for width in widths {
        builder = builder.column(width.resizable(true).clip(true));
    }
    builder
        .header(ROW_HEIGHT, |mut header| {
            for (i, column) in columns.iter().enumerate() {
                header.col(|ui| {
                    let mark = match state.sort {
                        Some((c, Order::Ascending)) if c == i => " ▲",
                        Some((c, Order::Descending)) if c == i => " ▼",
                        _ => "",
                    };
                    let title = RichText::new(format!("{}{mark}", column.title)).strong();
                    if ui
                        .add(egui::Label::new(title).sense(egui::Sense::click()))
                        .clicked()
                    {
                        sort_click = Some(i);
                    }
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut table_row| {
                let row = rows[table_row.index()];
                table_row.set_selected(state.selected.contains(&row));
                for (col, editor) in editors.iter().enumerate() {
                    let (_, response) = table_row.col(|ui| {
                        cell(
                            ui,
                            state,
                            project,
                            table,
                            codec,
                            row,
                            col,
                            editor,
                            &mut actions,
                        );
                    });
                    // The text is not selectable, so a double click reaches the cell itself.
                    let editing = state
                        .editing
                        .as_ref()
                        .is_some_and(|e| e.row == row && e.col == col);
                    if *editor == Editor::Text && !editing && response.double_clicked() {
                        state.editing = Some(Editing {
                            row,
                            col,
                            text: model::text(project, table, row, col, codec),
                            focused: false,
                        });
                    }
                    if let Some(problem) = model::frequency_problem_of(project, table, row, col) {
                        response.on_hover_text(problem);
                    }
                }
                let response = table_row.response();
                if response.clicked() {
                    clicked = Some((row, ui_modifiers(&response)));
                }
            });
        });
    if let Some((row, modifiers)) = clicked {
        state.click(row, modifiers, multi);
    }
    if let Some(col) = sort_click {
        state.sort = filter::next_sort(state.sort, col);
    }
    actions
}

/// Room for the longest fixed choice plus the drop-down arrow.
fn choice_width(ui: &egui::Ui, items: &[(String, i64)]) -> f32 {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let widest = items
        .iter()
        .map(|(label, _)| {
            ui.fonts_mut(|f| {
                f.layout_no_wrap(label.clone(), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0, f32::max);
    (widest + 36.0).clamp(56.0, 220.0)
}

fn ui_modifiers(response: &egui::Response) -> egui::Modifiers {
    response.ctx.input(|i| i.modifiers)
}

#[allow(clippy::too_many_arguments)]
fn cell(
    ui: &mut egui::Ui,
    state: &mut TableState,
    project: &Project,
    table: Table,
    codec: NameCodec,
    row: usize,
    col: usize,
    editor: &Editor,
    actions: &mut Vec<TableAction>,
) {
    let text = model::text(project, table, row, col, codec);
    let problem = model::frequency_problem_of(project, table, row, col);
    if problem.is_some() {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, ERROR_BACKGROUND);
    }
    match editor {
        Editor::Choices {
            items,
            none,
            searchable,
        } => {
            let id = egui::Id::new(("cell", table, row, col));
            let current = model::value(project, table, row, col);
            if let Some(value) = choice_cell(
                ui,
                id,
                (&text, current),
                (items, *none, *searchable),
                &mut state.picker_search,
            ) {
                actions.push(TableAction::Apply {
                    row,
                    col,
                    input: Input::Value(value),
                });
            }
        }
        Editor::Text
            if state
                .editing
                .as_ref()
                .is_some_and(|e| e.row == row && e.col == col) =>
        {
            text_editor(ui, state, actions);
        }
        _ => {
            let label = if problem.is_some() {
                RichText::new(&text).color(Color32::BLACK)
            } else {
                RichText::new(&text)
            };
            ui.add(egui::Label::new(label).truncate().selectable(false));
        }
    }
}

fn text_editor(ui: &mut egui::Ui, state: &mut TableState, actions: &mut Vec<TableAction>) {
    let Some(editing) = state.editing.as_mut() else {
        return;
    };
    let response =
        ui.add(egui::TextEdit::singleline(&mut editing.text).desired_width(f32::INFINITY));
    if !editing.focused {
        response.request_focus();
        editing.focused = true;
    }
    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escape {
        state.editing = None;
    } else if response.lost_focus() {
        let done = state.editing.take().expect("editing");
        actions.push(TableAction::Apply {
            row: done.row,
            col: done.col,
            input: Input::Text(done.text),
        });
    }
}
