//! Stage-2 parity: replays every case of `parity_model.json` (written by
//! `rust/tools/make_parity_model.py` from the Python implementation) and compares the results —
//! channel CSV, layout edits, contact deletion, the code-plug lists, radio settings, clean-up and
//! project diffs.

mod common;

use std::collections::BTreeSet;
use std::ops::Range;

use opendrs_core::alarm_enc::{self, AlarmEdit};
use opendrs_core::channels_csv;
use opendrs_core::cleanup;
use opendrs_core::codeplug::MAX_CONTACTS;
use opendrs_core::contact_edit;
use opendrs_core::diff;
use opendrs_core::f136::{self, Project};
use opendrs_core::layout::{self, BatchPlan};
use opendrs_core::quickmsg;
use opendrs_core::radio_settings::{self as rs, RadioSetting};
use opendrs_core::rxgroup;
use opendrs_core::scanlist::{self, ScanEdit};
use opendrs_core::textcodec::NameCodec;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CODEC: NameCodec = NameCodec::Big5;

fn sha(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn psha(project: &Project) -> String {
    sha(&f136::serialize(project))
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn edit_out(edit: &ScanEdit) -> Value {
    json!([edit.removed, edit.renumbered, edit.refs_changed])
}

/// `{"error": message}` for a failure, as the oracle writes Python's ValueError.
fn guarded<T, E: ToString>(result: Result<T, E>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => ok(value),
        Err(e) => json!({ "error": e.to_string() }),
    }
}

fn int(args: &[Value], i: usize) -> i64 {
    args[i].as_i64().expect("integer argument")
}

fn idx(args: &[Value], i: usize) -> usize {
    usize::try_from(int(args, i)).expect("index argument")
}

fn text(args: &[Value], i: usize) -> &str {
    args[i].as_str().expect("text argument")
}

fn pair(value: &Value) -> (u16, u16) {
    let n = |i: usize| u16::try_from(value[i].as_u64().expect("number")).expect("u16");
    (n(0), n(1))
}

fn range_out(rows: Range<usize>) -> Value {
    json!([rows.start, rows.end])
}

const HUGE_FIELD: usize = 200_000;

fn huge_csv() -> Vec<u8> {
    let mut data = "\u{feff}分區,頻道,名稱\r\n1,1,".as_bytes().to_vec();
    data.extend(std::iter::repeat_n(b'x', HUGE_FIELD));
    data.extend(b"\r\n");
    data
}

fn op_layout(p: &Project, op: &str, a: &[Value]) -> Option<Value> {
    Some(match op {
        "zone_rows" => json!(layout::zone_rows(p)),
        "check_layout" => guarded(layout::check_layout(p), |()| Value::Null),
        "channel_position" => guarded(layout::channel_position(p, idx(a, 0)), |(z, c)| {
            json!([z, c])
        }),
        "add_channel" => guarded(
            layout::add_channel(p, idx(a, 0)),
            |(q, row)| json!({"sha": psha(&q), "row": row}),
        ),
        "delete_channel" => guarded(
            layout::delete_channel(p, idx(a, 0)),
            |(q, edit)| json!({"sha": psha(&q), "edit": edit_out(&edit)}),
        ),
        "add_zone" => guarded(
            layout::add_zone(p, idx(a, 0), text(a, 1), CODEC),
            |(q, row)| json!({"sha": psha(&q), "row": row}),
        ),
        "delete_zone" => guarded(
            layout::delete_zone(p, idx(a, 0)),
            |(q, edit)| json!({"sha": psha(&q), "edit": edit_out(&edit)}),
        ),
        "move_channel" => guarded(
            layout::move_channel(p, idx(a, 0), idx(a, 1)),
            |(q, row, edit)| json!({"sha": psha(&q), "row": row, "edit": edit_out(&edit)}),
        ),
        "shift_channel" => {
            let delta = i32::try_from(int(a, 1)).expect("delta");
            guarded(
                layout::shift_channel(p, idx(a, 0), delta),
                |(q, row)| json!({"sha": psha(&q), "row": row}),
            )
        }
        "add_channels" => {
            let plan = BatchPlan {
                count: idx(a, 1),
                start_rx_hz: int(a, 2),
                step_hz: int(a, 3),
                tx_offset_hz: int(a, 4),
                name_prefix: text(a, 5).to_string(),
                first_number: int(a, 6),
            };
            guarded(
                layout::add_channels(p, idx(a, 0), &plan, CODEC),
                |(q, rows)| json!({"sha": psha(&q), "rows": range_out(rows)}),
            )
        }
        _ => return None,
    })
}

fn op_contacts(p: &Project, op: &str, a: &[Value]) -> Option<Value> {
    Some(match op {
        "delete_contacts" => {
            let rows: Vec<usize> = a[0]
                .as_array()
                .expect("rows")
                .iter()
                .map(|v| usize::try_from(v.as_u64().expect("row")).expect("row"))
                .collect();
            guarded(
                contact_edit::delete_contacts(p, &rows, CODEC),
                |(q, d)| json!({"sha": psha(&q), "deletion": [d.removed, d.group_members_dropped, d.renumbered]}),
            )
        }
        "shift_contact" => {
            let delta = i32::try_from(int(a, 1)).expect("delta");
            guarded(
                contact_edit::shift_contact(p, idx(a, 0), delta),
                |(q, row)| json!({"sha": psha(&q), "row": row}),
            )
        }
        "add_contact" => {
            let capacity = a.get(3).map_or(MAX_CONTACTS, |v| {
                usize::try_from(v.as_u64().expect("capacity")).expect("capacity")
            });
            guarded(
                contact_edit::add_contact(
                    p,
                    u8::try_from(int(a, 0)).expect("kind"),
                    text(a, 1),
                    u32::try_from(int(a, 2)).expect("call id"),
                    CODEC,
                    capacity,
                ),
                |(q, row)| json!({"sha": psha(&q), "row": row}),
            )
        }
        "channels_calling" => {
            let numbers: BTreeSet<u16> = a[0]
                .as_array()
                .expect("numbers")
                .iter()
                .map(|v| u16::try_from(v.as_u64().expect("number")).expect("u16"))
                .collect();
            json!(contact_edit::channels_calling(p, &numbers, CODEC))
        }
        _ => return None,
    })
}

fn op_lists(p: &Project, op: &str, a: &[Value]) -> Option<Value> {
    let img = &p.image;
    let entries = |v: &Value| -> Vec<(u16, u16)> {
        v.as_array().expect("entries").iter().map(pair).collect()
    };
    Some(match op {
        "rx_groups" => json!(
            rxgroup::rx_groups(img)
                .iter()
                .map(|g| json!([g.name(CODEC), g.members(), g.in_use()]))
                .collect::<Vec<_>>()
        ),
        "with_rx_group" => {
            let members: Vec<u16> = a[2]
                .as_array()
                .expect("members")
                .iter()
                .map(|v| u16::try_from(v.as_u64().expect("member")).expect("u16"))
                .collect();
            guarded(
                rxgroup::with_rx_group(img, idx(a, 0), text(a, 1), &members, CODEC),
                |image| json!(sha(&image)),
            )
        }
        "groups_containing" => {
            json!(rxgroup::groups_containing(
                img,
                u16::try_from(int(a, 0)).expect("u16")
            ))
        }
        "scan_lists" => json!(
            scanlist::scan_lists(img)
                .iter()
                .map(|s| {
                    let entries: Vec<_> = s.entries().iter().map(|(z, c)| json!([z, c])).collect();
                    let (dz, dc) = s.designated();
                    json!([
                        s.name(CODEC),
                        entries,
                        s.talkback(),
                        s.tx_mode(),
                        [dz, dc],
                        s.in_use()
                    ])
                })
                .collect::<Vec<_>>()
        ),
        "with_scan_list" => guarded(
            scanlist::with_scan_list(img, idx(a, 0), text(a, 1), &entries(&a[2]), CODEC),
            |image| json!(sha(&image)),
        ),
        "with_scan_settings" => {
            let talkback = a[1].as_bool().expect("talkback");
            let tx_mode = u8::try_from(int(a, 2)).expect("u8");
            guarded(
                scanlist::with_scan_settings(img, idx(a, 0), talkback, tx_mode, pair(&a[3])),
                |image| json!(sha(&image)),
            )
        }
        "references" => {
            let zone = u16::try_from(int(a, 0)).expect("u16");
            let channel = a[1].as_u64().map(|c| u16::try_from(c).expect("u16"));
            json!(scanlist::references(img, zone, channel))
        }
        "alarm_lists" => json!(
            alarm_enc::alarm_lists(img)
                .iter()
                .map(|l| {
                    let (z, c) = l.channel();
                    json!([
                        l.name(CODEC),
                        l.alarm_type(),
                        l.mode(),
                        [z, c],
                        l.impolite_attempts(),
                        l.mic_seconds()
                    ])
                })
                .collect::<Vec<_>>()
        ),
        "with_alarm" => {
            let f = &a[1];
            let small = |key: &str| {
                f.get(key)
                    .map(|v| u8::try_from(v.as_u64().expect("number")).expect("u8"))
            };
            let edit = AlarmEdit {
                name: f.get("name").map(|v| v.as_str().expect("name").to_string()),
                alarm_type: small("alarm_type"),
                mode: small("mode"),
                channel: f.get("channel").map(pair),
                impolite_attempts: small("impolite_attempts"),
                mic_seconds: small("mic_seconds"),
            };
            guarded(
                alarm_enc::with_alarm(img, idx(a, 0), &edit, CODEC),
                |image| json!(sha(&image)),
            )
        }
        "encryption_keys" => json!(
            alarm_enc::encryption_keys(img)
                .iter()
                .map(|k| json!([k.alias(CODEC), hex(k.key()), k.in_use()]))
                .collect::<Vec<_>>()
        ),
        "parse_key_hex" => guarded(alarm_enc::parse_key_hex(text(a, 0)), |key| json!(hex(&key))),
        "with_encryption_key" => guarded(
            alarm_enc::with_encryption_key(img, idx(a, 0), text(a, 1), &unhex(text(a, 2)), CODEC),
            |image| json!(sha(&image)),
        ),
        "gps_channel" => {
            let (z, c) = alarm_enc::gps_channel(img);
            json!([z, c])
        }
        "with_gps_channel" => guarded(alarm_enc::with_gps_channel(img, pair(&a[0])), |image| {
            json!(sha(&image))
        }),
        "quick_messages" => json!(quickmsg::quick_messages(img, CODEC)),
        "with_quick_message" => guarded(
            quickmsg::with_quick_message(img, idx(a, 0), text(a, 1), CODEC),
            |image| json!(sha(&image)),
        ),
        _ => return None,
    })
}

fn op_settings(p: &Project, op: &str, a: &[Value]) -> Option<Value> {
    let img = &p.image;
    if op == "settings" {
        return Some(json!(
            rs::SETTINGS
                .iter()
                .map(|s| match s {
                    RadioSetting::Bits(b) => json!([b.key, b.display(img)]),
                    RadioSetting::Text(t) => json!([t.key, t.text(img)]),
                })
                .collect::<Vec<_>>()
        ));
    }
    if !matches!(op, "with_raw" | "with_value" | "with_text") {
        return None;
    }
    let setting = rs::by_key(text(a, 0)).expect("known setting");
    Some(match (op, setting) {
        ("with_raw", RadioSetting::Bits(b)) => {
            guarded(b.with_raw(img, int(a, 1)), |i| json!(sha(&i)))
        }
        ("with_value", RadioSetting::Bits(b)) => {
            guarded(b.with_value(img, int(a, 1)), |i| json!(sha(&i)))
        }
        ("with_text", RadioSetting::Text(t)) => {
            guarded(t.with_text(img, text(a, 1)), |i| json!(sha(&i)))
        }
        _ => panic!("{op} does not apply to {}", setting.key()),
    })
}

fn op_misc(p: &Project, op: &str, a: &[Value]) -> Option<Value> {
    Some(match op {
        "cleanup" => {
            let report = cleanup::scan(p);
            let (cleaned, again) = cleanup::clean_names(p);
            json!({
                "report": [report.contacts, report.zones, report.channels],
                "total": report.total(),
                "summary": report.summary(),
                "sha": psha(&cleaned),
                "cleaned": [again.contacts, again.zones, again.channels],
            })
        }
        "export_csv" => guarded(channels_csv::export_channels(p, CODEC), |data| {
            let body = data.strip_prefix(b"\xef\xbb\xbf").expect("BOM");
            json!({"sha": sha(&data), "text": String::from_utf8(body.to_vec()).expect("UTF-8")})
        }),
        "import_csv" | "import_csv_huge" => {
            let data = if op == "import_csv" {
                unhex(text(a, 0))
            } else {
                huge_csv()
            };
            let r = channels_csv::import_channels(p, &data, CODEC);
            json!({
                "sha": psha(&r.project),
                "updated": r.updated,
                "added": r.added,
                "unchanged": r.unchanged,
                "errors": r.errors,
                "encoding": r.encoding,
                "summary": r.summary(),
            })
        }
        _ => return None,
    })
}

fn run(p: &Project, op: &str, args: &[Value]) -> Value {
    op_layout(p, op, args)
        .or_else(|| op_contacts(p, op, args))
        .or_else(|| op_lists(p, op, args))
        .or_else(|| op_settings(p, op, args))
        .or_else(|| op_misc(p, op, args))
        .unwrap_or_else(|| panic!("unknown op {op}"))
}

fn oracle() -> Value {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/parity_model.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("parity_model.json")).expect("JSON")
}

fn short(value: &Value) -> String {
    let text = value.to_string();
    match text.char_indices().nth(400) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text,
    }
}

#[test]
fn every_case_matches_python() {
    let oracle = oracle();
    let mut failures = Vec::new();
    let mut total = 0;
    for (name, cases) in oracle["fixtures"].as_object().expect("fixtures") {
        let project = f136::load(common::fixtures().join(name)).expect("fixture");
        for case in cases.as_array().expect("cases") {
            total += 1;
            let op = case["op"].as_str().expect("op");
            let args = case["args"].as_array().expect("args");
            let actual = run(&project, op, args);
            if actual != case["out"] {
                failures.push(format!(
                    "{name} {op} {}\n  python: {}\n  rust:   {}",
                    short(&case["args"]),
                    short(&case["out"]),
                    short(&actual)
                ));
            }
        }
    }
    assert!(total > 2000, "only {total} cases");
    assert!(
        failures.is_empty(),
        "{} of {total} cases differ:\n{}",
        failures.len(),
        failures
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn diffs_match_python() {
    let oracle = oracle();
    let dh98 = f136::load(common::fixtures().join("dh98vu_20250811.F136")).expect("fixture");
    let edit = "\u{feff}分區,頻道,名稱,接收MHz\r\n1,1,CHANGED,431.5\r\n";
    let edited = channels_csv::import_channels(&dh98, edit.as_bytes(), CODEC).project;
    let (smaller, _) = layout::delete_zone(&dh98, 0).expect("delete zone");
    let renamed = dh98
        .with_contact(
            0,
            dh98.contacts[0].with_name("RENAMED", CODEC).expect("name"),
        )
        .expect("contact");
    let load = |name: &str| f136::load(common::fixtures().join(name)).expect("fixture");
    let pairs = [
        (load("mmdvm_20241124.F136"), load("mmdvm_20241127.F136")),
        (dh98.clone(), edited),
        (dh98.clone(), smaller.clone()),
        (smaller, dh98.clone()),
        (dh98.clone(), dh98.clone()),
        (dh98.clone(), renamed),
    ];
    let expected = oracle["diffs"].as_array().expect("diffs");
    assert_eq!(expected.len(), pairs.len());
    for ((old, new), case) in pairs.iter().zip(expected) {
        assert_eq!(
            (psha(old), psha(new)),
            (
                case["old_sha"].as_str().expect("sha").to_string(),
                case["new_sha"].as_str().expect("sha").to_string()
            )
        );
        let lines = diff::diff_projects(old, new, CODEC);
        assert_eq!(
            json!(lines),
            case["lines"],
            "{} -> {}",
            case["old"],
            case["new"]
        );
        let ranges: Vec<Value> = diff::byte_ranges(&old.image, &new.image, diff::MERGE_GAP)
            .iter()
            .map(|r| json!([r.start, hex(&r.old), hex(&r.new)]))
            .collect();
        assert_eq!(
            json!(ranges),
            case["image_ranges"],
            "{} -> {}",
            case["old"],
            case["new"]
        );
    }
}
