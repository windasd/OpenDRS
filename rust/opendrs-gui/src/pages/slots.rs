//! Fixed-slot lists (scan lists, RX group lists), port of `slot_page.py`: a read-only table of
//! the slots, and the editor that renames one slot and ticks its members.

use eframe::egui::{self, Layout};
use egui_extras::{Column, TableBuilder};
use opendrs_core::pystr::{casefold, split};

use super::form::cell_label;

pub const SLOT_COLUMNS: [&str; 4] = ["#", "名稱", "數量", "成員"];
pub const UNUSED: &str = "（未使用）";
pub const EDIT_BUTTON: &str = "編輯選取的清單…";
pub const ADD_BUTTON: &str = "新增清單…";
pub const ADD_TIP: &str = "編輯第一個未使用的清單";
pub const ADD_TITLE: &str = "新增清單";
pub const OK: &str = "確定";
pub const CANCEL: &str = "取消";
/// Members are joined with this in the 成員 column.
pub const MEMBER_SEPARATOR: &str = "、";

const ROW_HEIGHT: f32 = 22.0;
const EDITOR_WIDTH: f32 = 480.0;
const EDITOR_HEIGHT: f32 = 600.0;

/// One slot as the table lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotRow {
    /// 1-based slot number.
    pub number: usize,
    pub name: String,
    pub count: usize,
    pub members: String,
    pub in_use: bool,
}

impl SlotRow {
    /// The four cells: `#`, `名稱` (`（未使用）` for an unused slot), `數量`, `成員`.
    pub fn cells(&self) -> [String; 4] {
        let name = if self.in_use {
            self.name.clone()
        } else {
            UNUSED.to_string()
        };
        [
            self.number.to_string(),
            name,
            self.count.to_string(),
            self.members.clone(),
        ]
    }
}

/// Slots in use (the tab title's count).
pub fn in_use(rows: &[SlotRow]) -> usize {
    rows.iter().filter(|r| r.in_use).count()
}

/// Number (1-based) of the first slot not in use.
pub fn first_unused(rows: &[SlotRow]) -> Option<usize> {
    rows.iter().find(|r| !r.in_use).map(|r| r.number)
}

/// 新增清單… when every slot is taken.
pub fn all_in_use_text(total: usize) -> String {
    format!("{total} 個清單都已使用；請選一個編輯或清空後再用。")
}

/// What the slot table asks the page to do this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotRequest {
    /// Open the editor for this slot (1-based).
    Edit(usize),
    /// 新增清單… with all `total` slots in use.
    AllInUse(usize),
}

/// `已勾選 {n} / {limit} 個{unit}`, with `　⚠ 超過上限` past the limit.
pub fn count_text(checked: usize, limit: usize, unit: &str) -> String {
    let over = if checked > limit {
        "　⚠ 超過上限"
    } else {
        ""
    };
    format!("已勾選 {checked} / {limit} 個{unit}{over}")
}

/// The filter box: every case-folded, whitespace-separated term occurs in the label.
pub fn matches(label: &str, filter: &str) -> bool {
    let label = casefold(label);
    let filter = casefold(filter);
    split(&filter).all(|term| label.contains(term))
}

/// The slot table with its note and the 新增清單… and 編輯選取的清單… buttons. Returns the slot to
/// edit (double-clicked row, the edit button with a row selected, or the first unused slot).
pub fn slot_table(
    ui: &mut egui::Ui,
    id: &str,
    note: &str,
    rows: &[SlotRow],
    selected: &mut Option<usize>,
) -> Option<SlotRequest> {
    let mut open = None;
    ui.with_layout(Layout::right_to_left(egui::Align::TOP), |ui| {
        if ui.button(EDIT_BUTTON).clicked() {
            open = selected.map(SlotRequest::Edit);
        }
        if ui.button(ADD_BUTTON).on_hover_text(ADD_TIP).clicked() {
            open = Some(match first_unused(rows) {
                Some(number) => {
                    *selected = Some(number);
                    SlotRequest::Edit(number)
                }
                None => SlotRequest::AllInUse(rows.len()),
            });
        }
        ui.with_layout(Layout::left_to_right(egui::Align::TOP), |ui| {
            ui.add(egui::Label::new(note).wrap());
        });
    });
    ui.add_space(4.0);
    TableBuilder::new(ui)
        .id_salt(id)
        .striped(true)
        .sense(egui::Sense::click())
        .cell_layout(Layout::left_to_right(egui::Align::Center))
        .resizable(true)
        .column(Column::initial(40.0).at_least(28.0))
        .column(Column::initial(150.0).at_least(60.0).clip(true))
        .column(Column::initial(56.0).at_least(40.0))
        .column(Column::remainder().at_least(120.0).clip(true))
        .header(ROW_HEIGHT, |mut header| {
            for title in SLOT_COLUMNS {
                header.col(|ui| {
                    ui.strong(title);
                });
            }
        })
        .body(|body| {
            body.rows(ROW_HEIGHT, rows.len(), |mut table_row| {
                let row = &rows[table_row.index()];
                table_row.set_selected(*selected == Some(row.number));
                for text in row.cells() {
                    table_row.col(|ui| cell_label(ui, &text, !row.in_use));
                }
                let response = table_row.response();
                if response.clicked() {
                    *selected = Some(row.number);
                }
                if response.double_clicked() {
                    *selected = Some(row.number);
                    open = Some(SlotRequest::Edit(row.number));
                }
            });
        });
    open
}

/// The member editor's state: a slot's name and which choices are ticked. Members are returned
/// in list order, not in the order they were ticked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MemberPicker<T> {
    pub number: usize,
    /// `掃描清單 {n}` / `接收群組 {n}`.
    pub title: String,
    pub name: String,
    /// `(label, value)` in list order.
    pub choices: Vec<(String, T)>,
    pub checked: Vec<bool>,
    pub filter: String,
    pub limit: usize,
    /// `頻道` / `聯絡人`.
    pub unit: &'static str,
    /// The project revision the editor was opened on; it closes if the project changes.
    pub revision: u64,
}

/// What the user did with the editor this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Open,
    Accept,
    Cancel,
}

impl<T: Clone + PartialEq> MemberPicker<T> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        number: usize,
        title: String,
        name: String,
        choices: Vec<(String, T)>,
        current: &[T],
        limit: usize,
        unit: &'static str,
        revision: u64,
    ) -> Self {
        let checked = choices.iter().map(|(_, v)| current.contains(v)).collect();
        MemberPicker {
            number,
            title,
            name,
            choices,
            checked,
            filter: String::new(),
            limit,
            unit,
            revision,
        }
    }

    /// The ticked values, in list order (hidden ones included).
    pub fn values(&self) -> Vec<T> {
        self.choices
            .iter()
            .zip(&self.checked)
            .filter(|(_, on)| **on)
            .map(|((_, v), _)| v.clone())
            .collect()
    }

    pub fn checked_count(&self) -> usize {
        self.checked.iter().filter(|on| **on).count()
    }

    pub fn count_text(&self) -> String {
        count_text(self.checked_count(), self.limit, self.unit)
    }

    /// 確定 is disabled past the limit.
    pub fn can_accept(&self) -> bool {
        self.checked_count() <= self.limit
    }

    /// Rows the filter box leaves visible.
    pub fn visible(&self) -> Vec<usize> {
        self.choices
            .iter()
            .enumerate()
            .filter(|(_, (label, _))| matches(label, &self.filter))
            .map(|(i, _)| i)
            .collect()
    }

    /// The modal editor; `extra_rows` adds rows to the form under 名稱 (two-column grid).
    pub fn show(&mut self, ctx: &egui::Context, extra_rows: impl FnOnce(&mut egui::Ui)) -> Outcome {
        let id = egui::Id::new(("member-editor", self.title.as_str()));
        let response = egui::Modal::new(id).show(ctx, |ui| {
            ui.set_width(EDITOR_WIDTH);
            ui.heading(&self.title);
            ui.add_space(6.0);
            egui::Grid::new(id.with("form"))
                .num_columns(2)
                .spacing([8.0, 6.0])
                .show(ui, |ui| {
                    ui.label("名稱：");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.name)
                            .hint_text("名稱（最多 10 bytes）")
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();
                    extra_rows(ui);
                });
            ui.add_space(6.0);
            ui.label(format!("勾選要加入的{}：", self.unit));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .hint_text("篩選清單…")
                    .desired_width(f32::INFINITY),
            );
            self.checklist(ui);
            ui.label(self.count_text());
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ok = ui
                    .add_enabled(self.can_accept(), egui::Button::new(OK))
                    .clicked();
                if ui.button(CANCEL).clicked() {
                    Outcome::Cancel
                } else if ok {
                    Outcome::Accept
                } else {
                    Outcome::Open
                }
            })
            .inner
        });
        // Like a Qt dialog: Escape rejects, Enter presses the default button (確定) if enabled.
        let (escaped, entered) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
            )
        });
        match response.inner {
            Outcome::Open if escaped => Outcome::Cancel,
            Outcome::Open if entered && self.can_accept() => Outcome::Accept,
            outcome => outcome,
        }
    }

    fn checklist(&mut self, ui: &mut egui::Ui) {
        let visible = self.visible();
        let row_height = ui.spacing().interact_size.y;
        let height = (EDITOR_HEIGHT - 260.0).max(200.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("members")
                .auto_shrink(false)
                .min_scrolled_height(height)
                .max_height(height)
                .show_rows(ui, row_height, visible.len(), |ui, range| {
                    for &index in &visible[range] {
                        let label = self.choices[index].0.clone();
                        ui.checkbox(&mut self.checked[index], label);
                    }
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker() -> MemberPicker<u16> {
        let choices = (1..=5)
            .map(|n| (format!("#{n} TG{n}0（組呼 {n}0）"), n))
            .collect();
        MemberPicker::new(
            2,
            "接收群組 2".into(),
            "G".into(),
            choices,
            &[4, 2, 9],
            3,
            "聯絡人",
            7,
        )
    }

    #[test]
    fn rows_show_unused_slots_greyed_name() {
        let row = SlotRow {
            number: 3,
            name: "x".into(),
            count: 0,
            members: String::new(),
            in_use: false,
        };
        assert_eq!(row.cells(), ["3", "（未使用）", "0", ""].map(String::from));
        let used = SlotRow {
            in_use: true,
            ..row.clone()
        };
        assert_eq!(used.cells()[1], "x");
        assert_eq!(in_use(&[row.clone(), used.clone()]), 1);
        // 新增清單…: the first unused slot, or the message when all are taken
        let first = SlotRow {
            number: 1,
            ..used.clone()
        };
        assert_eq!(first_unused(&[first.clone(), row]), Some(3));
        assert_eq!(first_unused(&[first, used]), None);
        assert_eq!(
            all_in_use_text(32),
            "32 個清單都已使用；請選一個編輯或清空後再用。"
        );
    }

    #[test]
    fn count_text_warns_past_the_limit() {
        assert_eq!(count_text(16, 16, "頻道"), "已勾選 16 / 16 個頻道");
        assert_eq!(
            count_text(33, 32, "聯絡人"),
            "已勾選 33 / 32 個聯絡人　⚠ 超過上限"
        );
    }

    #[test]
    fn filter_needs_every_term() {
        assert!(matches("FRS_TW／FRS_TW_1", ""));
        assert!(matches("FRS_TW／FRS_TW_1", "frs  _1"));
        assert!(!matches("FRS_TW／FRS_TW_1", "frs 2"));
        assert!(matches("Straße", "STRASSE"));
    }

    #[test]
    fn picker_keeps_list_order_and_limit() {
        let mut p = picker();
        assert_eq!(p.values(), vec![2, 4]); // 9 is not in the list: dropped
        assert_eq!(p.count_text(), "已勾選 2 / 3 個聯絡人");
        p.checked = vec![true; 5];
        assert!(!p.can_accept());
        assert_eq!(p.values(), vec![1, 2, 3, 4, 5]);
        p.filter = "tg3".into();
        assert_eq!(p.visible(), vec![2]);
        assert_eq!(p.checked_count(), 5); // hidden rows still count
    }
}
