//! The settings-area pages against the Python GUI's widgets (`parity_pages.json`, written by
//! `rust/tools/make_parity_pages.py`): scan lists, RX groups, radio settings, quick messages,
//! alarms and keys — texts, choices, enabled states, the member editors' initial state, and
//! edits with their undo labels or warnings. Each fixture is also checked patched (missing
//! channels, stale references, unknown values) and truncated (missing image areas).

use std::path::Path;

use opendrs_core::alarm_enc::{AlarmEdit, alarm_lists};
use opendrs_core::f136::{self, Project};
use opendrs_core::radio_settings::{self, RadioSetting, by_key};
use opendrs_core::rxgroup::{has_rx_groups, rx_groups};
use opendrs_core::scanlist::{Entry, has_scan_lists, scan_lists};
use opendrs_core::textcodec::NameCodec;
use opendrs_gui::pages::settings_page::EditorView;
use opendrs_gui::pages::slots::{self, MemberPicker, SlotRow};
use opendrs_gui::pages::{alarm_page, group_page, quickmsg_page, scan_page, settings_page};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CODEC: NameCodec = NameCodec::Big5;
const GREY: &str = "#a0a0a4";

fn oracle() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity_pages.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("oracle")).expect("JSON")
}

fn load(variant: &Value) -> Project {
    let name = variant["fixture"].as_str().expect("fixture");
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures")
        .join(name);
    let mut project = f136::load(path).expect("fixture");
    for patch in variant["patches"].as_array().expect("patches") {
        if patch[0] == "truncate" {
            project
                .image
                .truncate(patch[1].as_u64().expect("size") as usize);
        } else {
            let at = patch[0].as_u64().expect("offset") as usize;
            project.image[at] =
                u8::from_str_radix(patch[1].as_str().expect("byte"), 16).expect("hex");
        }
    }
    project
}

fn sha(image: &[u8]) -> String {
    Sha256::digest(image)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Collects differences with their place.
struct Check<'a> {
    place: String,
    failures: &'a mut Vec<String>,
}

impl Check<'_> {
    fn eq(&mut self, what: &str, actual: Value, expected: &Value) {
        if actual != *expected {
            self.failures
                .push(format!("{} {what}: {actual} vs {expected}", self.place));
        }
    }
}

fn row_cells(rows: &[SlotRow]) -> Value {
    json!(rows.iter().map(SlotRow::cells).collect::<Vec<_>>())
}

fn labels<T>(choices: &[(String, T)]) -> Value {
    json!(choices.iter().map(|(l, _)| l).collect::<Vec<_>>())
}

fn visible<T: Clone + PartialEq>(picker: &MemberPicker<T>, searches: &Value, check: &mut Check) {
    let mut picker = picker.clone();
    for (text, want) in searches.as_object().expect("visible") {
        picker.filter = text.clone();
        check.eq(&format!("visible {text:?}"), json!(picker.visible()), want);
    }
}

fn entries(value: &Value) -> Vec<Entry> {
    value
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| {
            (
                e[0].as_u64().expect("zone") as u16,
                e[1].as_u64().expect("channel") as u16,
            )
        })
        .collect()
}

fn compare_scan(project: &Project, facts: &Value, check: &mut Check) {
    let rows = scan_page::rows(project, CODEC);
    check.eq(
        "scan enabled",
        json!(has_scan_lists(&project.image)),
        &facts["enabled"],
    );
    check.eq("scan in use", json!(slots::in_use(&rows)), &facts["in_use"]);
    check.eq("scan rows", row_cells(&rows), &facts["rows"]);
    let grey: Vec<bool> = facts["grey"]
        .as_array()
        .expect("grey")
        .iter()
        .map(|c| c == GREY)
        .collect();
    check.eq(
        "scan grey",
        json!(rows.iter().map(|r| !r.in_use).collect::<Vec<_>>()),
        &json!(grey),
    );
    let channel_entries: Vec<[u16; 2]> = scan_page::channel_entries(project)
        .iter()
        .map(|e| [e.0, e.1])
        .collect();
    check.eq("channel entries", json!(channel_entries), &facts["entries"]);
    let lists = scan_lists(&project.image);
    for (scan, want) in lists
        .iter()
        .zip(facts["dialogs"].as_array().expect("dialogs"))
    {
        let editor = scan_page::ScanEditor::new(project, scan, CODEC, 0);
        let picker = &editor.picker;
        let n = scan.number;
        if n == 1 {
            check.eq("scan choices", labels(&picker.choices), &facts["choices"]);
            visible(picker, &facts["visible"], check);
        }
        check.eq(
            &format!("scan {n} title"),
            json!(picker.title),
            &want["title"],
        );
        check.eq(&format!("scan {n} name"), json!(picker.name), &want["name"]);
        let checked: Vec<[u16; 2]> = picker.values().iter().map(|e| [e.0, e.1]).collect();
        check.eq(
            &format!("scan {n} checked"),
            json!(checked),
            &want["checked"],
        );
        check.eq(
            &format!("scan {n} count"),
            json!(picker.count_text()),
            &want["count"],
        );
        check.eq(
            &format!("scan {n} ok"),
            json!(picker.can_accept()),
            &want["ok"],
        );
        let o = &editor.options;
        check.eq(
            &format!("scan {n} talkback"),
            json!(o.talkback),
            &want["talkback"]["index"],
        );
        check.eq(
            &format!("scan {n} tx mode"),
            json!(o.tx_mode),
            &want["tx_mode"]["index"],
        );
        check.eq(
            &format!("scan {n} designated items"),
            labels(&o.designated_items),
            &want["designated"]["items"],
        );
        check.eq(
            &format!("scan {n} designated"),
            json!(o.designated),
            &want["designated"]["index"],
        );
        check.eq(
            &format!("scan {n} designated enabled"),
            json!(o.designated_enabled()),
            &want["designated"]["enabled"],
        );
        let (talkback, mode, designated) = editor.settings();
        check.eq(
            &format!("scan {n} settings"),
            json!([talkback, mode, [designated.0, designated.1]]),
            &want["settings"],
        );
    }
}

fn compare_entry_labels(project: &Project, facts: &Value, check: &mut Check) {
    for probe in facts.as_array().expect("entry labels") {
        let entry = (
            probe[0].as_u64().expect("z") as u16,
            probe[1].as_u64().expect("c") as u16,
        );
        check.eq(
            &format!("entry label {entry:?}"),
            json!(scan_page::entry_label(project, entry, CODEC)),
            &probe[2],
        );
    }
}

fn compare_groups(project: &Project, facts: &Value, check: &mut Check) {
    let rows = group_page::rows(project, CODEC);
    check.eq(
        "group enabled",
        json!(has_rx_groups(&project.image)),
        &facts["enabled"],
    );
    check.eq(
        "group in use",
        json!(slots::in_use(&rows)),
        &facts["in_use"],
    );
    check.eq("group rows", row_cells(&rows), &facts["rows"]);
    for probe in facts["member_labels"].as_array().expect("member labels") {
        let m = probe[0].as_u64().expect("member") as u16;
        check.eq(
            &format!("member label {m}"),
            json!(group_page::member_labels(project, &[m], CODEC)[0]),
            &probe[1],
        );
    }
    for (group, want) in rx_groups(&project.image)
        .iter()
        .zip(facts["dialogs"].as_array().expect("dialogs"))
    {
        let picker = group_page::editor_for(project, group, CODEC, 0);
        let n = group.number;
        if n == 1 {
            check.eq("group choices", labels(&picker.choices), &facts["choices"]);
            visible(&picker, &facts["visible"], check);
        }
        check.eq(
            &format!("group {n} title"),
            json!(picker.title),
            &want["title"],
        );
        check.eq(
            &format!("group {n} name"),
            json!(picker.name),
            &want["name"],
        );
        check.eq(
            &format!("group {n} checked"),
            json!(picker.values()),
            &want["checked"],
        );
        check.eq(
            &format!("group {n} count"),
            json!(picker.count_text()),
            &want["count"],
        );
        check.eq(
            &format!("group {n} ok"),
            json!(picker.can_accept()),
            &want["ok"],
        );
    }
}

fn compare_settings(project: &Project, facts: &Value, check: &mut Check) {
    check.eq(
        "setting pages",
        json!(radio_settings::pages()),
        &facts["pages"],
    );
    let expected = facts["settings"].as_array().expect("settings");
    let actual: Vec<&RadioSetting> = radio_settings::pages()
        .into_iter()
        .flat_map(radio_settings::by_page)
        .collect();
    check.eq("setting count", json!(actual.len()), &json!(expected.len()));
    for (setting, want) in actual.into_iter().zip(expected) {
        let key = setting.key();
        check.eq(&format!("{key} key"), json!(key), &want["key"]);
        let view = settings_page::view(setting, &project.image);
        check.eq(
            &format!("{key} enabled"),
            json!(view.enabled),
            &want["enabled"],
        );
        check.eq(&format!("{key} tip"), json!(view.tip), &want["tip"]);
        let shown = match (&view.editor, setting) {
            (EditorView::Choice { items, current }, RadioSetting::Bits(s)) => json!({
                "kind": "choice", "text": items[*current].0, "count": items.len(),
                "extra": items[s.choices.len()..].iter().map(|(l, _)| l).collect::<Vec<_>>(),
            }),
            (EditorView::Number { value, unit, .. }, _) => {
                json!({"kind": "number", "value": value, "suffix": unit})
            }
            (EditorView::Text { text, limit }, _) => {
                json!({"kind": "text", "text": text, "limit": limit})
            }
            _ => json!(null),
        };
        let mut want = want.clone();
        for field in ["key", "page", "enabled", "tip"] {
            want.as_object_mut().expect("fact").remove(field);
        }
        check.eq(&format!("{key} editor"), shown, &want);
        check.eq(
            &format!("{key} label"),
            json!(view.label),
            &json!(format!("{}：", setting.label())),
        );
    }
}

fn compare_quick(project: &Project, facts: &Value, check: &mut Check) {
    let enabled = opendrs_core::quickmsg::has_quick_messages(&project.image);
    check.eq("quick enabled", json!(enabled), &facts["enabled"]);
    check.eq(
        "quick header",
        json!(quickmsg_page::header()),
        &facts["header"],
    );
    check.eq(
        "quick texts",
        json!(quickmsg_page::texts(&project.image, CODEC)),
        &facts["texts"],
    );
}

fn combo_json(items: &[(String, u32)], index: usize) -> Value {
    json!({"items": items.iter().map(|(l, _)| l).collect::<Vec<_>>(), "index": index, "enabled": true})
}

fn compare_alarm(project: &Project, facts: &Value, check: &mut Check) {
    let lists = alarm_lists(&project.image);
    check.eq("alarm enabled", json!(!lists.is_empty()), &facts["enabled"]);
    check.eq(
        "alarm headers",
        json!(alarm_page::ALARM_COLUMNS),
        &facts["headers"],
    );
    check.eq(
        "key headers",
        json!(alarm_page::KEY_COLUMNS),
        &facts["key_headers"],
    );
    let channels = alarm_page::channel_choices(project, CODEC);
    check.eq(
        "alarm channels",
        labels(&channels),
        &facts["channel_choices"],
    );
    for (alarm, want) in lists.iter().zip(facts["rows"].as_array().expect("rows")) {
        let n = alarm.number;
        check.eq(
            &format!("alarm {n} name"),
            json!(alarm.name(CODEC)),
            &want["name"],
        );
        let (items, index) = alarm_page::type_items(alarm);
        check.eq(
            &format!("alarm {n} type"),
            combo_json(&items, index),
            &want["type"],
        );
        let (items, index) = alarm_page::mode_items(alarm);
        check.eq(
            &format!("alarm {n} mode"),
            combo_json(&items, index),
            &want["mode"],
        );
        let (items, index) = alarm_page::alarm_channel_items(&channels, alarm);
        check.eq(
            &format!("alarm {n} channel"),
            json!({"index": index, "text": items[index].0, "count": items.len()}),
            &want["channel"],
        );
        let attempts = alarm_page::shown(alarm.impolite_attempts(), &alarm_page::ATTEMPTS);
        check.eq(
            &format!("alarm {n} attempts"),
            json!(attempts),
            &want["attempts"],
        );
        let mic = alarm_page::shown(alarm.mic_seconds(), &(0..=150));
        check.eq(&format!("alarm {n} mic"), json!(mic), &want["mic"]);
        check.eq(
            &format!("alarm {n} mic suffix"),
            json!(alarm_page::MIC_SUFFIX),
            &want["mic_suffix"],
        );
    }
    let (items, index) = alarm_page::gps_items(project, CODEC);
    check.eq(
        "gps",
        json!({"index": index, "text": items[index].0, "count": items.len()}),
        &facts["gps"],
    );
    check.eq(
        "keys",
        json!(alarm_page::key_rows(&project.image, CODEC)),
        &facts["keys"],
    );
}

fn alarm_edit(args: &Value) -> alarm_page::Edit {
    let number = args[1].as_u64().expect("number") as usize;
    let value = &args[3];
    let byte = || Some(value.as_u64().expect("value") as u8);
    match (
        args[0].as_str().expect("kind"),
        args[2].as_str().expect("field"),
    ) {
        ("gps", _) => alarm_page::Edit::Gps(entries(&json!([value]))[0]),
        ("key", _) => alarm_page::Edit::Key {
            number,
            alias: value[0].as_str().expect("alias").into(),
            key: value[1].as_str().expect("key").into(),
        },
        (_, field) => {
            let edit = match field {
                "name" => AlarmEdit {
                    name: Some(value.as_str().expect("name").into()),
                    ..AlarmEdit::default()
                },
                "alarm_type" => AlarmEdit {
                    alarm_type: byte(),
                    ..AlarmEdit::default()
                },
                "mode" => AlarmEdit {
                    mode: byte(),
                    ..AlarmEdit::default()
                },
                "channel" => AlarmEdit {
                    channel: Some(entries(&json!([value]))[0]),
                    ..AlarmEdit::default()
                },
                "impolite_attempts" => AlarmEdit {
                    impolite_attempts: byte(),
                    ..AlarmEdit::default()
                },
                _ => AlarmEdit {
                    mic_seconds: byte(),
                    ..AlarmEdit::default()
                },
            };
            alarm_page::Edit::Alarm { number, edit }
        }
    }
}

/// `(result, warning)` as the oracle records them.
fn outcome(result: Result<Project, String>, label: String, title: &str) -> (Value, Value) {
    match result {
        Ok(p) => (json!({"label": label, "sha": sha(&p.image)}), Value::Null),
        Err(message) => (Value::Null, json!([title, message])),
    }
}

fn compare_edits(project: &Project, cases: &Value, check: &mut Check) {
    for case in cases.as_array().expect("edits") {
        let args = &case["args"];
        let n = args[0].as_u64().unwrap_or(0) as usize;
        let (result, warning) = match case["kind"].as_str().expect("kind") {
            "scan" => {
                let s = &args[3];
                let settings = (
                    s[0].as_bool().expect("talkback"),
                    s[1].as_u64().expect("mode") as u8,
                    entries(&json!([s[2]]))[0],
                );
                let r = scan_page::apply(
                    project,
                    n,
                    args[1].as_str().expect("name"),
                    &entries(&args[2]),
                    settings,
                    CODEC,
                );
                outcome(r, scan_page::undo_label(n), scan_page::ERROR_TITLE)
            }
            "group" => {
                let members: Vec<u16> = args[2]
                    .as_array()
                    .expect("members")
                    .iter()
                    .map(|m| m.as_u64().expect("m") as u16)
                    .collect();
                let r =
                    group_page::apply(project, n, args[1].as_str().expect("name"), &members, CODEC);
                outcome(r, group_page::undo_label(n), group_page::ERROR_TITLE)
            }
            "setting" => {
                let setting = by_key(args[0].as_str().expect("key")).expect("setting");
                let r = match setting {
                    RadioSetting::Bits(s) => {
                        settings_page::apply_raw(project, s, args[1].as_i64().expect("raw"))
                    }
                    RadioSetting::Text(s) => {
                        settings_page::apply_text(project, s, args[1].as_str().expect("text"))
                    }
                };
                outcome(
                    r,
                    settings_page::undo_label(setting),
                    settings_page::ERROR_TITLE,
                )
            }
            "quick" => {
                let r = quickmsg_page::apply(project, n, args[1].as_str().expect("text"), CODEC);
                outcome(r, quickmsg_page::undo_label(n), quickmsg_page::ERROR_TITLE)
            }
            _ => {
                let edit = alarm_edit(args);
                let r = alarm_page::apply(project, &edit, CODEC);
                outcome(r, edit.undo_label(), alarm_page::ERROR_TITLE)
            }
        };
        let mut want = case["result"].clone();
        if let Some(object) = want.as_object_mut() {
            object.remove("patches");
        }
        check.eq(
            &format!("edit {} {args}", case["kind"]),
            json!([result, warning]),
            &json!([want, case["warning"]]),
        );
    }
}

#[test]
fn pages_match_the_python_gui() {
    let oracle = oracle();
    let mut failures = Vec::new();
    for variant in oracle["variants"].as_array().expect("variants") {
        let project = load(variant);
        let mut check = Check {
            place: format!("{} [{}]", variant["fixture"], variant["variant"]),
            failures: &mut failures,
        };
        compare_scan(&project, &variant["scan"], &mut check);
        compare_entry_labels(&project, &variant["entry_labels"], &mut check);
        compare_groups(&project, &variant["groups"], &mut check);
        compare_settings(&project, &variant["settings"], &mut check);
        compare_quick(&project, &variant["quick"], &mut check);
        compare_alarm(&project, &variant["alarm"], &mut check);
        compare_edits(&project, &variant["edits"], &mut check);
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures
            .iter()
            .take(40)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
