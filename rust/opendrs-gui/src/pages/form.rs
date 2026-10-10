//! Editors shared by the settings-area pages: a drop-down that reports a new choice, a spin box
//! that reports its value once the user is done with it (Qt's `editingFinished`), and one-line
//! text editors that apply on Enter or when they lose focus.

use std::collections::HashMap;
use std::hash::Hash;
use std::ops::RangeInclusive;

use eframe::egui::{self, Color32};

/// Drop-down lists show about this many rows before scrolling.
const POPUP_ROWS: f32 = 16.0;

/// Width of `text` in the button font.
pub fn text_width(ui: &egui::Ui, text: &str) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    ui.fonts_mut(|f| {
        f.layout_no_wrap(text.to_owned(), font, Color32::WHITE)
            .size()
            .x
    })
}

/// Room for the widest label plus the drop-down arrow (Qt's size hint), within limits.
pub fn combo_width<'a>(ui: &egui::Ui, labels: impl IntoIterator<Item = &'a str>) -> f32 {
    let widest = labels
        .into_iter()
        .map(|label| text_width(ui, label))
        .fold(0.0, f32::max);
    (widest + 32.0).clamp(64.0, 420.0)
}

/// A drop-down of `items` showing entry `current`; returns the index the user picked when it
/// differs from `current` (Qt's `currentIndexChanged`).
pub fn choice_combo<V>(
    ui: &mut egui::Ui,
    id_salt: impl Hash + std::fmt::Debug,
    items: &[(String, V)],
    current: usize,
    width: f32,
) -> (Option<usize>, egui::Response) {
    let mut picked = None;
    let selected = items.get(current).map_or("", |(label, _)| label.as_str());
    let height = POPUP_ROWS * (ui.text_style_height(&egui::TextStyle::Body) + 4.0);
    let response = egui::ComboBox::from_id_salt(id_salt)
        .selected_text(selected)
        .width(width)
        .height(height)
        .show_ui(ui, |ui| {
            for (index, (label, _)) in items.iter().enumerate() {
                if ui.selectable_label(index == current, label).clicked() && index != current {
                    picked = Some(index);
                }
            }
        })
        .response;
    (picked, response)
}

/// A spin box (Qt `QSpinBox`): `shown` within `range`, arrow keys step by `step`, `suffix` after
/// the number. Returns the value once the user has finished with it (drag released, Enter, or
/// focus lost) if they changed it; the caller decides whether that differs from the stored value.
pub fn spin(
    ui: &mut egui::Ui,
    id: egui::Id,
    shown: i64,
    range: RangeInclusive<i64>,
    step: i64,
    suffix: &str,
) -> (Option<i64>, egui::Response) {
    let pending_id = id.with("pending");
    let mut value = ui.data(|d| d.get_temp::<i64>(pending_id)).unwrap_or(shown);
    let response = ui
        .push_id(id, |ui| {
            ui.add(
                egui::DragValue::new(&mut value)
                    .range(range)
                    .clamp_existing_to_range(false)
                    .speed(step.max(1) as f64)
                    .suffix(suffix)
                    .update_while_editing(false),
            )
        })
        .inner;
    if response.changed() {
        ui.data_mut(|d| d.insert_temp(pending_id, value));
    }
    if response.dragged() || response.has_focus() {
        return (None, response);
    }
    let finished = ui.data_mut(|d| d.remove_temp::<i64>(pending_id));
    (finished, response)
}

/// One line editor's text, and whether it had keyboard focus in the frame before.
struct LineBuffer {
    text: String,
    focused: bool,
}

/// The text of one-line editors between frames, keyed by a name of the caller's choosing.
#[derive(Default)]
pub struct TextBuffers(HashMap<String, LineBuffer>);

impl TextBuffers {
    /// A one-line editor showing `current`. Returns the typed text once, when Enter is pressed or
    /// the editor loses focus and the text differs from `current` (Escape puts `current` back).
    /// While not being edited the editor follows `current` (undo, another edit, a failed edit).
    ///
    /// The end of an edit is detected from the focus seen in the previous frame rather than
    /// `Response::lost_focus`, which stays true for two frames and would apply an edit twice.
    pub fn edit(
        &mut self,
        ui: &mut egui::Ui,
        key: &str,
        current: &str,
        width: f32,
        char_limit: Option<usize>,
    ) -> (Option<String>, egui::Response) {
        let buffer = self.0.entry(key.to_owned()).or_insert_with(|| LineBuffer {
            text: current.to_owned(),
            focused: false,
        });
        let mut editor = egui::TextEdit::singleline(&mut buffer.text)
            .desired_width(width)
            .id_salt(("line-editor", key));
        if let Some(limit) = char_limit {
            editor = editor.char_limit(limit);
        }
        // Inside a Grid the editor would shrink to the column width of the frame before.
        let response = ui
            .scope(|ui| {
                ui.set_min_width(width);
                ui.add(editor)
            })
            .inner;
        let focused = response.has_focus();
        let finished = buffer.focused && !focused;
        buffer.focused = focused;
        let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if finished && !escaped && buffer.text != current {
            // Show the stored text until the edit lands (or is refused).
            let typed = std::mem::replace(&mut buffer.text, current.to_owned());
            return (Some(typed), response);
        }
        if !focused && buffer.text != current {
            buffer.text = current.to_owned();
        }
        (None, response)
    }
}

/// A cell of a read-only list: greyed for unused slots, cut with "…" when too long (full text
/// on hover), and not catching clicks so the row gets them.
pub fn cell_label(ui: &mut egui::Ui, text: &str, grey: bool) {
    let rich = if grey {
        egui::RichText::new(text).color(Color32::GRAY)
    } else {
        egui::RichText::new(text)
    };
    ui.add(egui::Label::new(rich).selectable(false).truncate());
}
