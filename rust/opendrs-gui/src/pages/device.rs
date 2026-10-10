//! 裝置資訊: the radio's identity fields, and the editable alias and DMR ID (port of
//! `device_editing.py`).

use std::collections::HashMap;
use std::path::Path;

use eframe::egui;
use opendrs_contacts::radioid::{self, RadioIdUser};
use opendrs_core::device::{
    Band, DMR_ID_OFFSET, DMR_ID_SIZE, MAX_ALIAS_LEN, MAX_DMR_ID, parse_dmr_id,
};
use opendrs_core::f136::Project;
use opendrs_core::pystr::{casefold, strip, thousands};
use opendrs_firmware::hardware;

use crate::app::OpenDrsApp;

/// The editors' text and the RadioID lookups already made (the cache file is ~17 MB).
#[derive(Default)]
pub struct DeviceState {
    alias: String,
    dmr_id: String,
    lookups: HashMap<u32, Option<RadioIdUser>>,
}

/// The DMR ID line: the RadioID registration when known, and a warning when the alias is not
/// the registered callsign.
pub fn describe_dmr_id(
    dmr_id: u32,
    alias: &str,
    user: Option<&RadioIdUser>,
    cache_available: bool,
) -> String {
    if !cache_available {
        return "尚未下載 RadioID 資料，無法查詢呼號".into();
    }
    let Some(user) = user else {
        return format!("RadioID 查無 {dmr_id}");
    };
    let place: Vec<&str> = [user.city.as_str(), user.country.as_str()]
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect();
    let mut text = format!("RadioID：{} {}", user.callsign, user.first_name)
        .trim_end()
        .to_string();
    if !place.is_empty() {
        text.push_str(&format!("（{}）", place.join("、")));
    }
    if !alias.is_empty() && !user.callsign.is_empty() && casefold(alias) != casefold(&user.callsign)
    {
        text.push_str(&format!(
            "\n⚠ 別名 {alias} 與 RadioID 登記的呼號 {} 不同（呼號可能已變更）",
            user.callsign
        ));
    }
    text
}

/// `f"{hz / 1e6:g}"` for a band edge.
fn mhz(hz: u32) -> String {
    let value = f64::from(hz) / 1e6;
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        value.to_string()
    }
}

/// 「136–174 MHz、400–480 MHz」, or 「未知（不檢查）」.
pub fn bands_text(bands: Option<&[Band]>) -> String {
    match bands {
        Some(bands) if !bands.is_empty() => bands
            .iter()
            .map(|(lo, hi)| format!("{}–{} MHz", mhz(*lo), mhz(*hi)))
            .collect::<Vec<_>>()
            .join("、"),
        _ => "未知（不檢查）".into(),
    }
}

fn or_dash(text: &str) -> &str {
    if text.is_empty() { "—" } else { text }
}

fn identity_editable(project: &Project) -> bool {
    project.image.len() >= DMR_ID_OFFSET + DMR_ID_SIZE
}

impl DeviceState {
    fn lookup(&mut self, radioid_dir: &Path, dmr_id: u32) -> Option<&RadioIdUser> {
        self.lookups
            .entry(dmr_id)
            .or_insert_with(|| radioid::lookup_cached(radioid_dir, dmr_id).ok().flatten())
            .as_ref()
    }
}

fn info_grid(ui: &mut egui::Ui, id: &str, rows: &[(&str, String)]) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([14.0, 10.0])
        .show(ui, |ui| {
            for (label, value) in rows {
                ui.label(*label);
                ui.add(egui::Label::new(value.as_str()).selectable(true));
                ui.end_row();
            }
        });
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let device = app.project().device();
    let also = hardware::known_as(&device.model);
    let model = if also.is_empty() {
        or_dash(&device.model).to_string()
    } else {
        format!("{}（{also}）", device.model)
    };
    ui.heading("裝置資訊");
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        ui.group(|ui| {
            ui.strong("對講機");
            info_grid(
                ui,
                "radio",
                &[
                    ("機型", model),
                    ("型號", or_dash(&device.model_number).into()),
                    ("韌體版本", or_dash(&device.firmware).into()),
                    ("版本日期", or_dash(&device.firmware_date).into()),
                    ("頻段標示", or_dash(&device.band_label).into()),
                    ("允許頻段", bands_text(device.bands().as_deref())),
                    ("寫入時間", or_dash(&device.write_date).into()),
                    ("出廠編號", or_dash(&device.factory_number).into()),
                    ("序號", or_dash(&device.serial_number).into()),
                ],
            );
        });
        ui.group(|ui| {
            ui.strong("專案");
            let size = format!(
                "{} bytes（原樣保留）",
                thousands(app.project().image.len() as u64)
            );
            info_grid(ui, "project", &[("設定區大小", size)]);
        });
    });
    ui.add_space(6.0);
    ui.group(|ui| {
        ui.strong("本機身分");
        identity(app, ui);
    });
    ui.add_space(6.0);
    ui.weak("頻道記錄 +0x0C 為接收頻率、+0x10 為發射頻率（2026-10-08 以實機讀寫擷取確認）。");
}

/// A one-line editor that applies its text when Enter is pressed or it loses focus.
fn editor(
    ui: &mut egui::Ui,
    text: &mut String,
    current: &str,
    limit: usize,
    hint: &str,
) -> Option<String> {
    let response = ui.add(
        egui::TextEdit::singleline(text)
            .char_limit(limit)
            .desired_width(160.0),
    );
    let response = response.on_hover_text(hint);
    if response.lost_focus() {
        // Enter or leaving the box applies; the caller restores the text if it fails.
        return (strip(text) != current).then(|| strip(text).to_string());
    }
    if !response.has_focus() && strip(text) != current {
        *text = current.to_string(); // the project changed (undo, another edit): show it
    }
    None
}

fn identity(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let device = app.project().device();
    let editable = identity_editable(app.project());
    let current_id = device.dmr_id.map_or_else(String::new, |id| id.to_string());
    let radioid_dir = app.data_dir().join("radioid");
    let mut change: Option<(Result<Project, String>, &str)> = None;
    egui::Grid::new("identity")
        .num_columns(2)
        .spacing([14.0, 10.0])
        .show(ui, |ui| {
            ui.label("別名");
            ui.horizontal(|ui| {
                ui.add_enabled_ui(editable, |ui| {
                    let hint = format!("最多 {MAX_ALIAS_LEN} 個英數字元");
                    let state = &mut app.pages.device;
                    if let Some(text) =
                        editor(ui, &mut state.alias, &device.alias, MAX_ALIAS_LEN, &hint)
                    {
                        change = Some((
                            app.project().with_alias(&text).map_err(|e| e.to_string()),
                            "修改別名",
                        ));
                    }
                });
                ui.weak(format!(
                    "最多 {MAX_ALIAS_LEN} 個英數字元；按 Enter 或離開欄位即套用"
                ));
            });
            ui.end_row();

            ui.label("本機 DMR ID");
            ui.horizontal(|ui| {
                ui.add_enabled_ui(editable, |ui| {
                    let state = &mut app.pages.device;
                    state.dmr_id.retain(|c| c.is_ascii_digit());
                    let hint = format!("1–{MAX_DMR_ID}");
                    if let Some(text) = editor(ui, &mut state.dmr_id, &current_id, 8, &hint) {
                        let edited =
                            parse_dmr_id(&text)
                                .map_err(|e| e.to_string())
                                .and_then(|id| {
                                    app.project().with_dmr_id(id).map_err(|e| e.to_string())
                                });
                        change = Some((edited, "修改 DMR ID"));
                    }
                });
                let cache_available = radioid::cache_info(&radioid_dir).is_some();
                let state = &mut app.pages.device;
                let (info, callsign) = match (editable, device.dmr_id) {
                    (false, _) => ("—".to_string(), None),
                    (true, None) => ("未設定".to_string(), None),
                    (true, Some(id)) => {
                        let user = if cache_available {
                            state.lookup(&radioid_dir, id).cloned()
                        } else {
                            None
                        };
                        let text =
                            describe_dmr_id(id, &device.alias, user.as_ref(), cache_available);
                        let callsign = user
                            .map(|u| u.callsign)
                            .filter(|c| !c.is_empty() && casefold(c) != casefold(&device.alias));
                        (text, callsign)
                    }
                };
                ui.label(info);
                if let Some(callsign) = callsign {
                    if ui.button("以 RadioID 呼號更新別名").clicked() {
                        change = Some((
                            app.project()
                                .with_alias(&callsign)
                                .map_err(|e| e.to_string()),
                            "別名改為 RadioID 呼號",
                        ));
                    }
                }
            });
            ui.end_row();
        });
    match change {
        Some((Ok(project), label)) => app.commit(project, label),
        Some((Err(message), label)) => {
            app.pages.device.alias = device.alias.clone();
            app.pages.device.dmr_id = current_id;
            app.warn(label, message);
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> RadioIdUser {
        RadioIdUser {
            radio_id: 9_998_765,
            callsign: "QA0AFK".into(),
            first_name: "Test".into(),
            last_name: String::new(),
            city: "Taipei".into(),
            state: String::new(),
            country: "Taiwan".into(),
        }
    }

    #[test]
    fn dmr_id_lines_like_python() {
        assert_eq!(
            describe_dmr_id(1, "", None, false),
            "尚未下載 RadioID 資料，無法查詢呼號"
        );
        assert_eq!(describe_dmr_id(1, "", None, true), "RadioID 查無 1");
        assert_eq!(
            describe_dmr_id(9_998_765, "qa0afk", Some(&user()), true),
            "RadioID：QA0AFK Test（Taipei、Taiwan）"
        );
        assert_eq!(
            describe_dmr_id(9_998_765, "OLD", Some(&user()), true),
            "RadioID：QA0AFK Test（Taipei、Taiwan）\n⚠ 別名 OLD 與 RadioID 登記的呼號 QA0AFK 不同（呼號可能已變更）"
        );
        let nameless = RadioIdUser {
            first_name: String::new(),
            city: String::new(),
            country: String::new(),
            ..user()
        };
        assert_eq!(
            describe_dmr_id(9_998_765, "", Some(&nameless), true),
            "RadioID：QA0AFK"
        );
    }

    #[test]
    fn band_text_like_python() {
        assert_eq!(
            bands_text(Some(&[
                (136_000_000, 174_000_000),
                (400_000_000, 480_000_000)
            ])),
            "136–174 MHz、400–480 MHz"
        );
        assert_eq!(
            bands_text(Some(&[(433_050_000, 434_790_000)])),
            "433.05–434.79 MHz"
        );
        assert_eq!(bands_text(None), "未知（不檢查）");
    }
}
