//! 報警與加密 page (port of `alarm_page.py` and `ListOpsMixin.apply_alarm_edit`): the digital
//! alarm lists, the GPS report channel and the encryption keys.

use eframe::egui;
use opendrs_core::alarm_enc::{
    ALARM_MODES, ALARM_TYPES, AlarmEdit, AlarmList, ENC_KEY_BYTES, MAX_MIC_SECONDS, alarm_lists,
    encryption_keys, gps_channel, parse_key_hex, with_alarm, with_encryption_key, with_gps_channel,
};
use opendrs_core::f136::Project;
use opendrs_core::pystr::strip;
use opendrs_core::scanlist::Entry;
use opendrs_core::textcodec::NameCodec;

use super::form::{TextBuffers, choice_combo, combo_width, spin};
use super::scan_page::{channel_entries, entry_label};
use crate::app::OpenDrsApp;

/// Error title of a refused edit.
pub const ERROR_TITLE: &str = "報警與加密";
pub const ALARM_COLUMNS: [&str; 6] = [
    "名稱",
    "報警類型",
    "報警模式",
    "報警信道",
    "不禮貌嘗試次數",
    "緊急麥克風持續時間",
];
pub const KEY_COLUMNS: [&str; 2] = [
    "加密秘鑰別名（最多 10 bytes，清空即刪除）",
    "加密初始化密鑰（6 bytes 十六進位）",
];
pub const ALARM_GROUP: &str = "數字報警列表";
pub const GPS_GROUP: &str = "GPS";
pub const GPS_LABEL: &str = "GPS上報信道：";
pub const KEY_GROUP: &str = "加密列表（加密等級在「對講機設定 › 加密」）";
pub const ATTEMPTS: std::ops::RangeInclusive<i64> = 1..=15;
pub const MIC_SUFFIX: &str = "S";
const NAME_WIDTH: f32 = 130.0;
const KEY_WIDTH: f32 = 300.0;

/// What this page keeps between frames.
#[derive(Default)]
pub struct State {
    texts: TextBuffers,
}

/// A channel reference as one number (the Python combo boxes' item data).
pub fn pack(entry: Entry) -> u32 {
    u32::from(entry.0) << 16 | u32::from(entry.1)
}

pub fn unpack(value: u32) -> Entry {
    ((value >> 16) as u16, (value & 0xFFFF) as u16)
}

/// `Off` and every channel as `{zone}／{channel}`.
pub fn channel_choices(project: &Project, codec: NameCodec) -> Vec<(String, u32)> {
    std::iter::once(("Off".to_string(), 0))
        .chain(
            channel_entries(project)
                .into_iter()
                .map(|e| (entry_label(project, e, codec), pack(e))),
        )
        .collect()
}

/// `choices` with the entry holding `current`; a value without one gets `未知值 {current}`.
pub fn with_current(mut choices: Vec<(String, u32)>, current: u32) -> (Vec<(String, u32)>, usize) {
    let index = match choices.iter().position(|(_, v)| *v == current) {
        Some(index) => index,
        None => {
            choices.push((format!("未知值 {current}"), current));
            choices.len() - 1
        }
    };
    (choices, index)
}

fn fixed(choices: &[(&str, u8)]) -> Vec<(String, u32)> {
    choices
        .iter()
        .map(|(label, v)| (label.to_string(), u32::from(*v)))
        .collect()
}

/// The 報警類型 drop-down of an alarm list.
pub fn type_items(alarm: &AlarmList) -> (Vec<(String, u32)>, usize) {
    with_current(fixed(&ALARM_TYPES), u32::from(alarm.alarm_type()))
}

/// The 報警模式 drop-down of an alarm list.
pub fn mode_items(alarm: &AlarmList) -> (Vec<(String, u32)>, usize) {
    with_current(fixed(&ALARM_MODES), u32::from(alarm.mode()))
}

/// The 報警信道 drop-down: a channel that is not offered shows as `未知值 {packed}` (as Python).
pub fn alarm_channel_items(
    channels: &[(String, u32)],
    alarm: &AlarmList,
) -> (Vec<(String, u32)>, usize) {
    with_current(channels.to_vec(), pack(alarm.channel()))
}

/// The GPS上報信道 drop-down; a target that is not offered shows as
/// `分區 {z}／第 {c} 台（不存在）`.
pub fn gps_items(project: &Project, codec: NameCodec) -> (Vec<(String, u32)>, usize) {
    let mut items = channel_choices(project, codec);
    let current = gps_channel(&project.image);
    let index = match items.iter().position(|(_, v)| *v == pack(current)) {
        Some(index) => index,
        None => {
            let (zone, channel) = current;
            items.push((
                format!("分區 {zone}／第 {channel} 台（不存在）"),
                pack(current),
            ));
            items.len() - 1
        }
    };
    (items, index)
}

/// Each key slot as (alias, key as `12 34 56 78 9A BC`); empty for an unused slot.
pub fn key_rows(image: &[u8], codec: NameCodec) -> Vec<[String; 2]> {
    encryption_keys(image)
        .iter()
        .map(|k| {
            if !k.in_use() {
                return [String::new(), String::new()];
            }
            let hex: Vec<String> = k.key().iter().map(|b| format!("{b:02X}")).collect();
            [k.alias(codec), hex.join(" ")]
        })
        .collect()
}

/// A spin box shows a stored value outside its range at the nearest end.
pub fn shown(value: u8, range: &std::ops::RangeInclusive<i64>) -> i64 {
    i64::from(value).clamp(*range.start(), *range.end())
}

/// One edit made on this page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Edit {
    Alarm {
        number: usize,
        edit: AlarmEdit,
    },
    Gps(Entry),
    /// The row's alias and key text after the edit.
    Key {
        number: usize,
        alias: String,
        key: String,
    },
}

impl Edit {
    /// `報警列表 {n}`, `GPS上報信道` or `加密金鑰 {n}`.
    pub fn undo_label(&self) -> String {
        match self {
            Edit::Alarm { number, .. } => format!("報警列表 {number}"),
            Edit::Gps(_) => "GPS上報信道".into(),
            Edit::Key { number, .. } => format!("加密金鑰 {number}"),
        }
    }
}

/// Apply an edit; the error text is shown under [`ERROR_TITLE`]. A key is stored only when
/// both alias and key are filled in, otherwise six zero bytes (an empty alias deletes it).
pub fn apply(project: &Project, edit: &Edit, codec: NameCodec) -> Result<Project, String> {
    let image = &project.image;
    let image = match edit {
        Edit::Alarm { number, edit } => with_alarm(image, *number, edit, codec),
        Edit::Gps(entry) => with_gps_channel(image, *entry),
        Edit::Key { number, alias, key } => {
            let bytes = if !strip(alias).is_empty() && !strip(key).is_empty() {
                parse_key_hex(key).map_err(|e| e.to_string())?
            } else {
                vec![0; ENC_KEY_BYTES]
            };
            with_encryption_key(image, *number, alias, &bytes, codec)
        }
    }
    .map_err(|e| e.to_string())?;
    Ok(Project {
        image,
        ..project.clone()
    })
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let codec = app.codec;
    let project = app.doc.project();
    let enabled = !alarm_lists(&project.image).is_empty();
    let state = &mut app.pages.alarms;
    let edit = ui
        .add_enabled_ui(enabled, |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| page(ui, state, project, codec))
                .inner
        })
        .inner;
    let Some(edit) = edit else {
        return;
    };
    match apply(app.project(), &edit, codec) {
        Ok(project) => app.commit(project, edit.undo_label()),
        Err(message) => app.warn(ERROR_TITLE, message),
    }
}

fn page(ui: &mut egui::Ui, state: &mut State, project: &Project, codec: NameCodec) -> Option<Edit> {
    let channels = channel_choices(project, codec);
    let mut edit = None;
    group(ui, ALARM_GROUP, |ui| {
        edit = edit.take().or(alarms(ui, state, project, &channels, codec));
    });
    ui.add_space(6.0);
    group(ui, GPS_GROUP, |ui| {
        ui.horizontal(|ui| {
            ui.label(GPS_LABEL);
            let (items, current) = gps_items(project, codec);
            let width = combo_width(ui, items.iter().map(|(l, _)| l.as_str()));
            if let (Some(i), _) = choice_combo(ui, "gps-channel", &items, current, width) {
                edit = Some(Edit::Gps(unpack(items[i].1)));
            }
        });
    });
    ui.add_space(6.0);
    group(ui, KEY_GROUP, |ui| {
        edit = edit.take().or(keys(ui, state, project, codec));
    });
    edit
}

fn group(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.group(|ui| {
        ui.set_width(ui.available_width());
        ui.strong(title);
        ui.add_space(4.0);
        contents(ui);
    });
}

fn alarms(
    ui: &mut egui::Ui,
    state: &mut State,
    project: &Project,
    channels: &[(String, u32)],
    codec: NameCodec,
) -> Option<Edit> {
    let mut edit = None;
    let channel_width = combo_width(ui, channels.iter().map(|(l, _)| l.as_str()));
    egui::Grid::new("alarm-lists")
        .num_columns(ALARM_COLUMNS.len() + 1)
        .striped(true)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("");
            for title in ALARM_COLUMNS {
                ui.strong(title);
            }
            ui.end_row();
            for alarm in alarm_lists(&project.image) {
                if let Some(e) = alarm_row(ui, state, &alarm, channels, channel_width, codec) {
                    edit = Some(Edit::Alarm {
                        number: alarm.number,
                        edit: e,
                    });
                }
                ui.end_row();
            }
        });
    edit
}

fn alarm_row(
    ui: &mut egui::Ui,
    state: &mut State,
    alarm: &AlarmList,
    channels: &[(String, u32)],
    channel_width: f32,
    codec: NameCodec,
) -> Option<AlarmEdit> {
    let n = alarm.number;
    let mut edit = None;
    ui.weak(n.to_string());
    let name = alarm.name(codec);
    if let (Some(typed), _) =
        state
            .texts
            .edit(ui, &format!("alarm-name-{n}"), &name, NAME_WIDTH, None)
    {
        edit = Some(AlarmEdit {
            name: Some(typed),
            ..AlarmEdit::default()
        });
    }
    let (items, current) = type_items(alarm);
    let width = combo_width(ui, items.iter().map(|(l, _)| l.as_str()));
    if let (Some(i), _) = choice_combo(ui, ("alarm-type", n), &items, current, width) {
        edit = Some(AlarmEdit {
            alarm_type: Some(items[i].1 as u8),
            ..AlarmEdit::default()
        });
    }
    let (items, current) = mode_items(alarm);
    let width = combo_width(ui, items.iter().map(|(l, _)| l.as_str()));
    if let (Some(i), _) = choice_combo(ui, ("alarm-mode", n), &items, current, width) {
        edit = Some(AlarmEdit {
            mode: Some(items[i].1 as u8),
            ..AlarmEdit::default()
        });
    }
    let (items, current) = alarm_channel_items(channels, alarm);
    if let (Some(i), _) = choice_combo(ui, ("alarm-channel", n), &items, current, channel_width) {
        edit = Some(AlarmEdit {
            channel: Some(unpack(items[i].1)),
            ..AlarmEdit::default()
        });
    }
    let raw = alarm.impolite_attempts();
    let id = egui::Id::new(("alarm-attempts", n));
    if let (Some(v), _) = spin(ui, id, shown(raw, &ATTEMPTS), ATTEMPTS, 1, "") {
        if v != i64::from(raw) {
            edit = Some(AlarmEdit {
                impolite_attempts: u8::try_from(v).ok(),
                ..AlarmEdit::default()
            });
        }
    }
    let raw = alarm.mic_seconds();
    let range = 0..=i64::from(MAX_MIC_SECONDS);
    let id = egui::Id::new(("alarm-mic", n));
    if let (Some(v), _) = spin(ui, id, shown(raw, &range), range, 1, MIC_SUFFIX) {
        if v != i64::from(raw) {
            edit = Some(AlarmEdit {
                mic_seconds: u8::try_from(v).ok(),
                ..AlarmEdit::default()
            });
        }
    }
    edit
}

fn keys(ui: &mut egui::Ui, state: &mut State, project: &Project, codec: NameCodec) -> Option<Edit> {
    let mut edit = None;
    egui::Grid::new("encryption-keys")
        .num_columns(3)
        .striped(true)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("");
            for title in KEY_COLUMNS {
                ui.strong(title);
            }
            ui.end_row();
            for (row, [alias, key]) in key_rows(&project.image, codec).into_iter().enumerate() {
                let number = row + 1;
                ui.weak(number.to_string());
                let texts = &mut state.texts;
                if let (Some(typed), _) =
                    texts.edit(ui, &format!("key-alias-{number}"), &alias, KEY_WIDTH, None)
                {
                    edit = Some(Edit::Key {
                        number,
                        alias: typed,
                        key: key.clone(),
                    });
                }
                if let (Some(typed), _) =
                    texts.edit(ui, &format!("key-hex-{number}"), &key, KEY_WIDTH, None)
                {
                    edit = Some(Edit::Key {
                        number,
                        alias: alias.clone(),
                        key: typed,
                    });
                }
                ui.end_row();
            }
        });
    edit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alarm(raw: [u8; 0x12]) -> AlarmList {
        AlarmList {
            number: 1,
            raw: raw.to_vec(),
        }
    }

    #[test]
    fn unknown_values_get_their_own_entry() {
        let mut raw = [0u8; 0x12];
        raw[0x0A] = 7;
        raw[0x0B] = 2;
        raw[0x0C..0x10].copy_from_slice(&[3, 0, 50, 0]);
        let a = alarm(raw);
        let (items, index) = type_items(&a);
        assert_eq!((items[index].0.as_str(), items.len()), ("未知值 7", 5));
        let (items, index) = mode_items(&a);
        assert_eq!(items[index].0, "緊急報警和呼叫");
        let channels = vec![("Off".to_string(), 0)];
        let (items, index) = alarm_channel_items(&channels, &a);
        assert_eq!(items[index].0, "未知值 196658");
        assert_eq!(unpack(pack((3, 50))), (3, 50));
    }

    #[test]
    fn spins_show_stored_values_within_range() {
        assert_eq!(shown(0, &ATTEMPTS), 1);
        assert_eq!(shown(200, &(0..=150)), 150);
        assert_eq!(shown(8, &ATTEMPTS), 8);
    }

    #[test]
    fn key_rows_and_edits() {
        let mut image = vec![0u8; 0x1400];
        image[0x122F..0x123F].copy_from_slice(b"K1\0\0\0\0\0\0\0\0\x12\x34\x56\x78\x9a\xbc");
        let rows = key_rows(&image, NameCodec::Big5);
        assert_eq!(rows[0], ["K1".to_string(), "12 34 56 78 9A BC".to_string()]);
        assert_eq!(rows[1], [String::new(), String::new()]);
        let project = Project {
            image,
            ..Project::default()
        };
        let delete = Edit::Key {
            number: 1,
            alias: " ".into(),
            key: "12 34 56 78 9A BC".into(),
        };
        let cleared = apply(&project, &delete, NameCodec::Big5).expect("delete");
        assert_eq!(key_rows(&cleared.image, NameCodec::Big5)[0][0], "");
        let no_key = Edit::Key {
            number: 2,
            alias: "K2".into(),
            key: String::new(),
        };
        let stored = apply(&project, &no_key, NameCodec::Big5).expect("zero key");
        assert_eq!(
            key_rows(&stored.image, NameCodec::Big5)[1][1],
            "00 00 00 00 00 00"
        );
        let bad = Edit::Key {
            number: 3,
            alias: "K3".into(),
            key: "123".into(),
        };
        assert!(apply(&project, &bad, NameCodec::Big5).is_err());
        assert_eq!(bad.undo_label(), "加密金鑰 3");
        assert_eq!(Edit::Gps((0, 0)).undo_label(), "GPS上報信道");
    }
}
