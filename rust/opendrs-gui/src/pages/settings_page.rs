//! 對講機設定 page (port of `settings_page.py` and `ListOpsMixin.apply_setting`): the page list
//! of the legacy CPS on the left and a generated form on the right.

use eframe::egui;
use opendrs_core::f136::Project;
use opendrs_core::pystr::strip;
use opendrs_core::radio_settings::{self, RadioSetting, Setting, TextSetting, by_key};

use super::form::{TextBuffers, choice_combo, combo_width, spin};
use crate::app::OpenDrsApp;

/// Error title of a refused edit.
pub const ERROR_TITLE: &str = "對講機設定";
const PAGE_LIST_WIDTH: f32 = 160.0;
const TEXT_WIDTH: f32 = 200.0;

/// What this page keeps between frames.
#[derive(Default)]
pub struct State {
    /// Index into [`radio_settings::pages`].
    pub page: usize,
    texts: TextBuffers,
}

impl State {
    /// Show `page` (e.g. 「加密」); unknown names are ignored.
    pub fn select_page(&mut self, page: &str) {
        if let Some(index) = radio_settings::pages().iter().position(|p| *p == page) {
            self.page = index;
        }
    }
}

/// How one editor shows the image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditorView {
    /// A drop-down; a stored value without a choice gets an extra `未知值 {raw}` entry.
    Choice {
        items: Vec<(String, u32)>,
        current: usize,
    },
    /// A spin box in display units (raw × scale), kept within the range like QSpinBox.
    Number {
        value: i64,
        min: i64,
        max: i64,
        step: i64,
        unit: &'static str,
    },
    Text {
        text: String,
        limit: usize,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingView {
    /// `{label}：`
    pub label: String,
    /// The note, shown as a tooltip ("" for none).
    pub tip: &'static str,
    pub enabled: bool,
    pub editor: EditorView,
}

/// Every setting this one depends on is switched on (non-zero).
pub fn parents_on(setting: &Setting, image: &[u8]) -> bool {
    setting.depends.iter().all(|key| match by_key(key) {
        Some(RadioSetting::Bits(parent)) => parent.raw(image).is_some_and(|raw| raw != 0),
        Some(RadioSetting::Text(parent)) => parent.text(image).is_some_and(|t| !t.is_empty()),
        None => false,
    })
}

fn choice_view(setting: &Setting, raw: Option<u32>) -> EditorView {
    let mut items = setting.choices.clone();
    let current = match raw {
        None => 0,
        Some(raw) => match items.iter().position(|(_, v)| *v == raw) {
            Some(index) => index,
            None => {
                // A value the legacy CPS never writes: show it, keep it.
                items.push((format!("未知值 {raw}"), raw));
                items.len() - 1
            }
        },
    };
    EditorView::Choice { items, current }
}

/// What the editor of `setting` shows for `image`, and whether it can be used.
pub fn view(setting: &RadioSetting, image: &[u8]) -> SettingView {
    match setting {
        RadioSetting::Text(s) => {
            let text = s.text(image);
            SettingView {
                label: format!("{}：", s.label),
                tip: s.note,
                enabled: text.is_some(),
                editor: EditorView::Text {
                    text: text.unwrap_or_default(),
                    limit: s.size,
                },
            }
        }
        RadioSetting::Bits(s) => {
            let raw = s.raw(image);
            let editor = if s.is_choice() {
                choice_view(s, raw)
            } else {
                let shown = raw.map_or(s.minimum, |r| i64::from(r) * s.scale);
                EditorView::Number {
                    value: shown.clamp(s.minimum, s.maximum.max(s.minimum)),
                    min: s.minimum,
                    max: s.maximum,
                    step: s.scale,
                    unit: s.unit,
                }
            };
            SettingView {
                label: format!("{}：", s.label),
                tip: s.note,
                enabled: raw.is_some() && parents_on(s, image),
                editor,
            }
        }
    }
}

/// The raw value a spin box value stores (`value // scale`).
pub fn spin_raw(value: i64, scale: i64) -> i64 {
    value.div_euclid(scale.max(1))
}

/// `{page}：{label}`.
pub fn undo_label(setting: &RadioSetting) -> String {
    format!("{}：{}", setting.page(), setting.label())
}

/// Store a choice or number; the error text is shown under [`ERROR_TITLE`].
pub fn apply_raw(project: &Project, setting: &Setting, raw: i64) -> Result<Project, String> {
    let image = setting
        .with_raw(&project.image, raw)
        .map_err(|e| e.to_string())?;
    Ok(Project {
        image,
        ..project.clone()
    })
}

/// Store a text setting (stripped by the setting).
pub fn apply_text(project: &Project, setting: &TextSetting, text: &str) -> Result<Project, String> {
    let image = setting
        .with_text(&project.image, text)
        .map_err(|e| e.to_string())?;
    Ok(Project {
        image,
        ..project.clone()
    })
}

/// A change the user made in the form.
enum Change {
    Raw(i64),
    Text(String),
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let pages = radio_settings::pages();
    let state = &mut app.pages.settings;
    state.page = state.page.min(pages.len().saturating_sub(1));
    egui::Panel::left("settings-pages")
        .resizable(false)
        .exact_size(PAGE_LIST_WIDTH)
        .show(ui, |ui| {
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                for (index, page) in pages.iter().enumerate() {
                    ui.selectable_value(&mut state.page, index, *page);
                }
            });
        });
    let Some(page) = pages.get(state.page).copied() else {
        return;
    };
    let mut change = None;
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| {
            egui::Grid::new(("settings-form", page))
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    for setting in radio_settings::by_page(page) {
                        if let Some(c) =
                            row(ui, &mut app.pages.settings, app.doc.project(), setting)
                        {
                            change = Some((setting, c));
                        }
                    }
                });
        });
    if let Some((setting, change)) = change {
        let project = app.project();
        let result = match (setting, change) {
            (RadioSetting::Bits(s), Change::Raw(raw)) => apply_raw(project, s, raw),
            (RadioSetting::Text(s), Change::Text(text)) => apply_text(project, s, &text),
            _ => return,
        };
        match result {
            Ok(project) => app.commit(project, undo_label(setting)),
            Err(message) => app.warn(ERROR_TITLE, message),
        }
    }
}

/// One form row: the label and its editor; returns the user's change.
fn row(
    ui: &mut egui::Ui,
    state: &mut State,
    project: &Project,
    setting: &RadioSetting,
) -> Option<Change> {
    let image = &project.image;
    let view = view(setting, image);
    let label = ui.label(&view.label);
    if !view.tip.is_empty() {
        label.on_hover_text(view.tip);
    }
    let (change, response) = ui
        .add_enabled_ui(view.enabled, |ui| editor(ui, state, setting, image, &view))
        .inner;
    if !view.tip.is_empty() {
        response
            .on_hover_text(view.tip)
            .on_disabled_hover_text(view.tip);
    }
    ui.end_row();
    change
}

fn editor(
    ui: &mut egui::Ui,
    state: &mut State,
    setting: &RadioSetting,
    image: &[u8],
    view: &SettingView,
) -> (Option<Change>, egui::Response) {
    let key = setting.key();
    match (&view.editor, setting) {
        (EditorView::Choice { items, current }, RadioSetting::Bits(s)) => {
            let width = combo_width(ui, items.iter().map(|(l, _)| l.as_str()));
            let (picked, response) = choice_combo(ui, ("setting", key), items, *current, width);
            let change = picked
                .map(|i| items[i].1)
                .filter(|raw| s.raw(image) != Some(*raw))
                .map(|raw| Change::Raw(i64::from(raw)));
            (change, response)
        }
        (
            EditorView::Number {
                value,
                min,
                max,
                step,
                unit,
            },
            RadioSetting::Bits(s),
        ) => {
            let id = egui::Id::new(("setting", key));
            let (finished, response) = spin(ui, id, *value, *min..=*max, *step, unit);
            let change = finished
                .map(|v| spin_raw(v, s.scale))
                .filter(|raw| s.raw(image).map(i64::from) != Some(*raw))
                .map(Change::Raw);
            (change, response)
        }
        (EditorView::Text { text, limit }, _) => {
            let (typed, response) = state.texts.edit(ui, key, text, TEXT_WIDTH, Some(*limit));
            let change = typed.filter(|t| strip(t) != text).map(Change::Text);
            (change, response)
        }
        _ => (None, ui.label("")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(key: &str) -> &'static Setting {
        match by_key(key) {
            Some(RadioSetting::Bits(s)) => s,
            _ => panic!("{key}"),
        }
    }

    #[test]
    fn unknown_values_get_their_own_entry() {
        let tot = bits("tot");
        let mut image = vec![0u8; 0x2000];
        image[tot.offset] = 200;
        let view = view(by_key("tot").unwrap(), &image);
        let EditorView::Choice { items, current } = view.editor else {
            panic!("choice")
        };
        assert_eq!(items.len(), tot.choices.len() + 1);
        assert_eq!(items[current].0, "未知值 200");
        assert_eq!(view.label, "限時發射：");
    }

    #[test]
    fn editors_are_disabled_without_bytes_or_with_a_parent_off() {
        let short = vec![0u8; 0x90];
        assert!(!view(by_key("language").unwrap(), &short).enabled);
        assert!(!view(by_key("dtmf_code").unwrap(), &short).enabled);
        let mut image = vec![0u8; 0x2000];
        assert!(!view(by_key("power_save_ratio").unwrap(), &image).enabled);
        let parent = bits("power_save");
        image[parent.offset] |= parent.mask as u8;
        assert!(view(by_key("power_save_ratio").unwrap(), &image).enabled);
    }

    #[test]
    fn every_dependency_names_a_setting() {
        for setting in radio_settings::SETTINGS.iter() {
            if let RadioSetting::Bits(s) = setting {
                for key in &s.depends {
                    assert!(by_key(key).is_some(), "{} depends on {key}", s.key);
                }
            }
        }
    }

    #[test]
    fn numbers_show_scaled_and_clamped_and_store_floor_division() {
        let setting = Setting::new("t", "基本設置", "時間", 0, 0xFF)
            .with_range(10, 100)
            .depending_on(&[]);
        let setting = Setting {
            scale: 10,
            unit: "S",
            ..setting
        };
        let EditorView::Number { value, step, .. } =
            view(&RadioSetting::Bits(setting.clone()), &[50]).editor
        else {
            panic!("number")
        };
        assert_eq!((value, step), (100, 10)); // 500 shown as the maximum
        let EditorView::Number { value, .. } = view(&RadioSetting::Bits(setting), &[3]).editor
        else {
            panic!("number")
        };
        assert_eq!(value, 30);
        assert_eq!(spin_raw(25, 10), 2);
        assert_eq!(spin_raw(-5, 10), -1);
    }

    #[test]
    fn undo_label_names_page_and_setting() {
        assert_eq!(undo_label(by_key("tot").unwrap()), "基本設置：限時發射");
        assert_eq!(
            undo_label(by_key("dtmf_code").unwrap()),
            "DMR服務項目：DTMF碼"
        );
    }
}
