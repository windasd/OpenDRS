//! The record tables against the Python GUI's Qt models (`parity_table.json`, written by
//! `rust/tools/make_parity_gui.py`): every cell's text, sort key, editor value and band problem,
//! the drop-down choices, filtered and sorted row orders, and cell edits.

use std::path::{Path, PathBuf};

use opendrs_core::f136::{self, Project};
use opendrs_core::textcodec::NameCodec;
use opendrs_gui::table::filter::{self, Filter, Order, Show};
use opendrs_gui::table::model::{self, Editor, Input, SortKey, Table};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const CODEC: NameCodec = NameCodec::Big5;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

fn oracle() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/parity_table.json");
    serde_json::from_str(&std::fs::read_to_string(path).expect("oracle")).expect("JSON")
}

fn table(name: &str) -> Table {
    match name {
        "channels" => Table::Channels,
        "zones" => Table::Zones,
        _ => Table::Contacts,
    }
}

fn sha(project: &Project) -> String {
    Sha256::digest(f136::serialize(project))
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn key_json(key: SortKey) -> Value {
    match key {
        SortKey::Number(n) => json!(n),
        SortKey::Text(t) => json!(t),
    }
}

/// Every cell of one table; returns the differences.
fn compare_table(project: &Project, t: Table, facts: &Value, failures: &mut Vec<String>) {
    let titles: Vec<&str> = model::columns(t).iter().map(|c| c.title.as_str()).collect();
    if json!(titles) != facts["titles"] {
        failures.push(format!("{t:?} titles: {:?} vs {}", titles, facts["titles"]));
    }
    let rows = model::row_count(project, t);
    for col in 0..titles.len() {
        if rows > 0 {
            let editable = model::editor(project, t, col, CODEC) != Editor::ReadOnly;
            if json!(editable) != facts["editable"][col] {
                failures.push(format!("{t:?} col {col} editable {editable}"));
            }
        }
        for row in 0..rows {
            let text = model::text(project, t, row, col, CODEC);
            if json!(text) != facts["text"][row][col] {
                failures.push(format!(
                    "{t:?} {row},{col} text {text:?} vs {}",
                    facts["text"][row][col]
                ));
            }
            let key = key_json(model::sort_key(project, t, row, col, CODEC));
            if key != facts["sort"][row][col] {
                failures.push(format!(
                    "{t:?} {row},{col} sort {key} vs {}",
                    facts["sort"][row][col]
                ));
            }
            let value = json!(model::value(project, t, row, col));
            if value != facts["value"][row][col] {
                failures.push(format!(
                    "{t:?} {row},{col} value {value} vs {}",
                    facts["value"][row][col]
                ));
            }
            let problem = json!(model::frequency_problem_of(project, t, row, col));
            let expected = facts["problems"]
                .get(format!("{row},{col}"))
                .cloned()
                .unwrap_or(Value::Null);
            if problem != expected {
                failures.push(format!("{t:?} {row},{col} problem {problem} vs {expected}"));
            }
        }
    }
}

fn compare_choices(project: &Project, expected: &Value, failures: &mut Vec<String>) {
    for (col, want) in expected.as_object().expect("choices") {
        let col: usize = col.parse().expect("column");
        let Editor::Choices { items, none, .. } =
            model::editor(project, Table::Channels, col, CODEC)
        else {
            failures.push(format!("column {col} has no drop-down"));
            continue;
        };
        let items: Vec<Value> = items
            .into_iter()
            .map(|(label, value)| json!([label, value]))
            .collect();
        if json!(items) != want["items"] || json!(none) != want["none"] {
            failures.push(format!(
                "column {col} choices differ: {:?} / {}",
                none, want["none"]
            ));
        }
    }
}

fn compare_filters(project: &Project, cases: &Value, failures: &mut Vec<String>) {
    for case in cases.as_array().expect("filters") {
        let t = table(case["table"].as_str().expect("table"));
        let show = match case["show"].as_str().expect("show") {
            "名稱含換行字元" => Show::ControlChars,
            "名稱重複" => Show::DuplicateNames,
            "類型與 ID 重複" => Show::DuplicateIds,
            _ => Show::All,
        };
        let filter = Filter {
            text: case["text"].as_str().expect("text").into(),
            scope: case["scope"].as_u64().map(|c| c as usize),
            kind: case["kind"].as_u64().map(|k| k as u8),
            show,
        };
        let sort = case["sort"].as_array().map(|s| {
            let order = if s[1] == "asc" {
                Order::Ascending
            } else {
                Order::Descending
            };
            (s[0].as_u64().expect("col") as usize, order)
        });
        let rows = filter::visible_rows(project, t, CODEC, &filter, sort);
        if json!(rows) != case["rows"] || json!(filter.is_filtered()) != case["filtered"] {
            failures.push(format!("filter {case}: rows {rows:?}"));
        }
    }
}

fn compare_edits(project: &Project, cases: &Value, failures: &mut Vec<String>) {
    for case in cases.as_array().expect("edits") {
        let t = table(case["table"].as_str().expect("table"));
        let (row, col) = (
            case["row"].as_u64().expect("row") as usize,
            case["col"].as_u64().expect("col") as usize,
        );
        let input = match case["input"].get("value") {
            Some(v) => Input::Value(v.as_i64().expect("value")),
            None => Input::Text(case["input"]["text"].as_str().expect("text").into()),
        };
        let actual = match model::apply(project, t, row, col, &input, CODEC) {
            Ok(None) => Value::Null,
            Ok(Some((edited, label))) => json!({"sha": sha(&edited), "label": label}),
            Err(e) => json!({"error": e}),
        };
        let expected = &case["result"];
        // Python shows int()'s English ValueError (or lets it escape); the port says it in Chinese.
        let english = expected["error"]
            .as_str()
            .is_some_and(|e| e.starts_with("invalid literal for int()"));
        let same = if english || expected.get("python_exception").is_some() {
            actual.get("error").is_some()
        } else {
            actual == *expected
        };
        if !same {
            failures.push(format!(
                "edit {t:?} {row},{col} {input:?}: {actual} vs {expected}"
            ));
        }
    }
}

#[test]
fn tables_match_the_python_gui() {
    let oracle = oracle();
    let mut failures = Vec::new();
    for (name, facts) in oracle["fixtures"].as_object().expect("fixtures") {
        let project = f136::load(fixtures().join(name)).expect("fixture");
        for (key, table_facts) in facts["tables"].as_object().expect("tables") {
            compare_table(&project, table(key), table_facts, &mut failures);
        }
        compare_choices(&project, &facts["choices"], &mut failures);
        compare_filters(&project, &facts["filters"], &mut failures);
        compare_edits(&project, &facts["edits"], &mut failures);
    }
    assert!(
        failures.is_empty(),
        "{} differences:\n{}",
        failures.len(),
        failures
            .iter()
            .take(30)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn the_printed_report_matches_the_python_gui() {
    let oracle = oracle();
    for (name, facts) in oracle["fixtures"].as_object().expect("fixtures") {
        let project = f136::load(fixtures().join(name)).expect("fixture");
        let html =
            opendrs_gui::report::report_html(&project, CODEC, "測試報表", "2026-10-10 12:00");
        let expected = facts["report"].as_str().expect("report");
        if html != expected {
            let at = html
                .char_indices()
                .zip(expected.chars())
                .find(|((_, a), b)| a != b)
                .map_or(html.len().min(expected.len()), |((i, _), _)| i);
            let start = html.floor_char_boundary(at.saturating_sub(80));
            panic!(
                "{name}: the report differs at byte {at}:\n{}",
                &html[start..]
            );
        }
    }
}
