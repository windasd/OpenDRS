//! Everything the Python implementation computes on the oracle's inputs (tests/parity.json,
//! made by rust/tools/make_parity_contacts.py) must come out the same here. Each test collects
//! every difference before failing.

mod common;

use std::cell::RefCell;
use std::fs;
use std::path::Path;

use common::{TempDir, load, response};
use opendrs_contacts::contact_import::{
    DEFAULT_CONTACT_CAPACITY, ImportReport, NameFormat, NewContact, append_contacts, contact_text,
    import_users, is_radioid_style_name, looks_like_callsign,
};
use opendrs_contacts::contacts_csv::{export_contacts, parse_contacts_csv};
use opendrs_contacts::pystr;
use opendrs_contacts::radioid::http::{HttpError, HttpRequest, HttpResponse};
use opendrs_contacts::radioid::{
    self, DownloadOptions, Query, RadioIdError, RadioIdUser, UserDb, parse_csv,
};
use opendrs_core::f136::{self, Project};
use opendrs_core::records::Contact;
use opendrs_core::textcodec::NameCodec;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

fn oracle() -> Value {
    serde_json::from_str(include_str!("parity.json")).expect("parity.json")
}

fn sha(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn hex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).expect("hex"))
        .collect()
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn text(value: &Value) -> &str {
    value.as_str().expect("string")
}

fn number<T: TryFrom<u64>>(value: &Value) -> T {
    T::try_from(value.as_u64().expect("number"))
        .ok()
        .expect("in range")
}

const BIG_HEADER: &str = "RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n";

/// The oracle's byte strings: hex, the generated big download, or a run of one byte.
fn blob(value: &Value) -> Vec<u8> {
    if let Some(s) = value.as_str() {
        return hex(s);
    }
    if let Some(rows) = value.get("big_body") {
        let rows: u32 = number(rows);
        let body: String = (0..rows)
            .map(|i| format!("{},QA{i:05},Name{i},,City,State,Taiwan\r\n", 9_000_000 + i))
            .collect();
        return format!("{BIG_HEADER}{body}").into_bytes();
    }
    let byte: u8 = number(&value["byte"]);
    let count: usize = number(&value["count"]);
    [
        hex(text(&value["prefix"])),
        vec![byte; count],
        hex(text(&value["suffix"])),
    ]
    .concat()
}

fn codec(value: &Value) -> NameCodec {
    NameCodec::from_value(text(value)).expect("codec")
}

/// Field-by-field comparison so a failure names every difference, not just the first.
fn diff(path: &str, expected: &Value, actual: &Value, out: &mut Vec<String>) {
    match (expected, actual) {
        (Value::Object(e), Value::Object(a)) => {
            for (key, value) in e {
                diff(
                    &format!("{path}.{key}"),
                    value,
                    a.get(key).unwrap_or(&Value::Null),
                    out,
                );
            }
        }
        (Value::Array(e), Value::Array(a)) if e.len() == a.len() => {
            for (i, (ev, av)) in e.iter().zip(a).enumerate() {
                diff(&format!("{path}[{i}]"), ev, av, out);
            }
        }
        (Value::Number(e), Value::Number(a)) if e.as_f64() == a.as_f64() => {}
        _ if expected == actual => {}
        _ => out.push(format!("{path}: python {expected} / rust {actual}")),
    }
}

fn assert_no_differences(mismatches: Vec<String>) {
    assert!(
        mismatches.is_empty(),
        "{} differences:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

// ------------------------------------------------------------------ Unicode tables

fn ranges(code_points: impl Iterator<Item = u32>) -> Value {
    let mut out: Vec<[u32; 2]> = Vec::new();
    for cp in code_points {
        match out.last_mut() {
            Some(last) if last[1] + 1 == cp => last[1] = cp,
            _ => out.push([cp, cp]),
        }
    }
    json!(out)
}

/// Code points with `test`; surrogates (not Rust chars) count as `surrogate`.
fn select(test: impl Fn(char) -> bool, surrogate: bool) -> impl Iterator<Item = u32> {
    (0..0x11_0000u32).filter(move |&cp| char::from_u32(cp).map_or(surrogate, &test))
}

#[test]
fn unicode_tables_match_python() {
    let expected = &oracle()["unicode"];
    let casefold: Vec<Value> = select(|c| pystr::casefold(&c.to_string()) != c.to_string(), false)
        .filter_map(char::from_u32)
        .map(|c| json!([c as u32, pystr::casefold(&c.to_string())]))
        .collect();
    let actual = json!({
        "python": expected["python"],
        "unicode": expected["unicode"],
        "space": ranges(select(pystr::is_space, false)),
        "nonprintable": ranges(select(|c| !pystr::is_printable(c), true)),
        "format": ranges(select(|c| pystr::is_control_or_format(c) && !c.is_control(), false)),
        "decimal": ranges(select(|c| pystr::decimal_value(c).is_some(), false)),
        "casefold": casefold,
    });
    let mut mismatches = Vec::new();
    diff("unicode", expected, &actual, &mut mismatches);
    assert_no_differences(mismatches);
}

// ------------------------------------------------------------------ radioid

fn user_row(u: &RadioIdUser) -> Value {
    json!([
        u.radio_id,
        u.callsign,
        u.first_name,
        u.last_name,
        u.city,
        u.state,
        u.country,
        u.full_name()
    ])
}

fn parse_facts(data: &[u8]) -> Value {
    match parse_csv(data) {
        Ok(result) => json!({
            "users": result.users.iter().map(user_row).collect::<Vec<_>>(),
            "skipped": result.skipped,
        }),
        Err(RadioIdError::Csv(err)) => json!({"error": err.to_string(), "type": "csv.Error"}),
        Err(err) => json!({"error": err.to_string(), "type": "RadioIdError"}),
    }
}

/// `_clean` is private: check it through a quoted CSV field, which arrives unchanged.
fn clean_via_parse(value: &str) -> Value {
    let data = format!(
        "RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\n1,\"{}\",,,,,X\n",
        value.replace('"', "\"\"")
    );
    let users = parse_csv(data.as_bytes()).expect("parse").users;
    json!([value, users[0].callsign])
}

fn search_facts(oracle: &Value) -> Value {
    let users: Vec<RadioIdUser> = oracle["search_csvs"]
        .as_array()
        .expect("csvs")
        .iter()
        .flat_map(|data| parse_csv(&blob(data)).expect("parse").users)
        .collect();
    let db = UserDb::new(users);
    let queries: Vec<Value> = oracle["search"]["queries"]
        .as_array()
        .expect("queries")
        .iter()
        .map(|case| {
            let q = &case["query"];
            let query = Query {
                country: text(&q[0]).into(),
                id_prefix: text(&q[1]).into(),
                text: text(&q[2]).into(),
            };
            let ids: Vec<u32> = db.search(&query).iter().map(|u| u.radio_id).collect();
            json!({"query": q, "is_empty": query.is_empty(), "ids": ids})
        })
        .collect();
    json!({"countries": db.countries(), "len": db.len(), "queries": queries})
}

fn lookup_facts(case: &Value) -> Value {
    let tmp = TempDir::new("parity-lookup");
    fs::write(tmp.path().join("user.csv"), hex(text(&case["data"]))).expect("write");
    let id: u32 = number(&case["id"]);
    let base = json!({"file": case["file"], "data": case["data"], "id": id});
    let extra = match radioid::lookup_cached(tmp.path(), id) {
        Ok(found) => json!({"user": found.as_ref().map(user_row)}),
        Err(err) => json!({"error": err.to_string()}),
    };
    merge(base, extra)
}

fn merge(mut base: Value, extra: Value) -> Value {
    if let (Some(b), Value::Object(e)) = (base.as_object_mut(), extra) {
        b.extend(e);
    }
    base
}

fn mtime_of(path: &Path) -> f64 {
    let modified = fs::metadata(path)
        .and_then(|m| m.modified())
        .expect("mtime");
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .expect("after 1970")
        .as_secs_f64()
}

fn meta_facts(case: &Value, test_csv: &[u8], out: &mut Vec<String>) -> Value {
    let tmp = TempDir::new("parity-meta");
    let csv_path = tmp.path().join("user.csv");
    fs::write(&csv_path, test_csv).expect("write");
    fs::write(tmp.path().join("meta.json"), hex(text(&case["meta"]))).expect("write");
    let info = radioid::cache_info(tmp.path()).expect("cache info");
    let from_mtime = info.downloaded_at == mtime_of(&csv_path);
    if case.get("raises").is_some() {
        // Documented difference: Python's float() raises; Rust falls back to the file time.
        if !from_mtime {
            out.push(format!(
                "meta {}: expected the mtime fallback",
                case["meta"]
            ));
        }
        return case.clone();
    }
    let downloaded_at = if from_mtime {
        json!("mtime")
    } else {
        json!(info.downloaded_at)
    };
    json!({"meta": case["meta"], "size": info.size, "last_modified": info.last_modified,
           "downloaded_at": downloaded_at})
}

fn ok_client(body: Vec<u8>) -> impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> {
    move |_: &HttpRequest| {
        let headers = vec![
            ("Content-Length".to_string(), body.len().to_string()),
            (
                "Last-Modified".to_string(),
                common::LAST_MODIFIED.to_string(),
            ),
        ];
        Ok(response(&body, headers))
    }
}

fn scenario_client<'a>(
    scenario: &'a Value,
    seen: &'a RefCell<Vec<HttpRequest>>,
) -> impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + 'a {
    move |request: &HttpRequest| {
        seen.borrow_mut().push(request.clone());
        if let Some(status) = scenario.get("status") {
            return Err(HttpError::Status(number(status)));
        }
        if let Some(detail) = scenario.get("connect") {
            return Err(HttpError::Connect(text(detail).into()));
        }
        let headers = scenario["headers"]
            .as_array()
            .expect("headers")
            .iter()
            .map(|pair| (text(&pair[0]).to_string(), text(&pair[1]).to_string()))
            .collect();
        Ok(response(&blob(&scenario["body"]), headers))
    }
}

/// meta.json with the platform's line breaks checked, then as the oracle stores it.
fn normalized_meta(path: &Path, linesep_out: &mut Vec<String>) -> Value {
    let Ok(raw) = fs::read_to_string(path) else {
        return Value::Null;
    };
    if !raw.contains("\"downloaded_at\"") {
        return json!(raw);
    }
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    if raw.matches('\n').count() != raw.matches(nl).count() {
        linesep_out.push(format!("meta.json does not use {nl:?}: {raw:?}"));
    }
    let lines: Vec<&str> = raw
        .split(nl)
        .map(|ln| {
            if ln.starts_with("  \"downloaded_at\"") {
                "  \"downloaded_at\": 0,"
            } else {
                ln
            }
        })
        .collect();
    json!(lines.join("\n"))
}

fn download_facts(scenario: &Value, test_csv: &[u8], out: &mut Vec<String>) -> Value {
    let tmp = TempDir::new("parity-download");
    let dir = tmp.path().join("cache");
    let defaults = DownloadOptions::default();
    if scenario.get("prepare").is_some_and(|p| p == true) {
        radioid::download(&dir, &ok_client(test_csv.to_vec()), &defaults, None).expect("prepare");
    }
    if let Some(meta) = scenario.get("meta") {
        fs::write(dir.join("meta.json"), blob(meta)).expect("meta");
    }
    let seen = RefCell::new(Vec::new());
    let client = scenario_client(scenario, &seen);
    let options = match scenario.get("max") {
        Some(max) => DownloadOptions {
            max_size: number(max),
            ..defaults
        },
        None => defaults,
    };
    let mut progress: Vec<(u64, i64)> = Vec::new();
    let mut report = |got, total| progress.push((got, total));
    let outcome = radioid::download(&dir, &client, &options, Some(&mut report));
    let (result, error) = match outcome {
        Ok(changed) => (json!(changed), Value::Null),
        Err(err) => (Value::Null, json!(err.to_string())),
    };
    let request = seen.borrow()[0].clone();
    let csv_path = dir.join("user.csv");
    let part_files = fs::read_dir(&dir).map_or(0, |entries| {
        entries
            .filter(|e| {
                e.as_ref()
                    .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "part"))
            })
            .count()
    });
    json!({
        "result": result,
        "error": error,
        "user_agent": request.header("User-Agent"),
        "if_modified_since": request.header("If-Modified-Since"),
        "progress": progress,
        "csv_sha256": fs::read(&csv_path).ok().map(|d| sha(&d)),
        "part_files": part_files,
        "meta": normalized_meta(&dir.join("meta.json"), out),
    })
}

#[test]
fn radioid_matches_python() {
    let oracle = oracle();
    let expected = &oracle["radioid"];
    let test_csv = hex(text(&expected["test_csv"]));
    let mut mismatches = Vec::new();
    for (name, case) in expected["parse"].as_object().expect("parse") {
        let actual = parse_facts(&blob(&case["data"]));
        diff(
            &format!("parse.{name}"),
            &case["result"],
            &actual,
            &mut mismatches,
        );
    }
    for case in expected["clean"].as_array().expect("clean") {
        diff(
            "clean",
            case,
            &clean_via_parse(text(&case[0])),
            &mut mismatches,
        );
    }
    diff(
        "search",
        &expected["search"],
        &search_facts(expected),
        &mut mismatches,
    );
    for case in expected["lookup"].as_array().expect("lookup") {
        diff("lookup", case, &lookup_facts(case), &mut mismatches);
    }
    let mut extra = Vec::new();
    for case in expected["meta"].as_array().expect("meta") {
        let actual = meta_facts(case, &test_csv, &mut extra);
        diff("meta", case, &actual, &mut mismatches);
    }
    for case in expected["download"].as_array().expect("download") {
        let scenario = &case["scenario"];
        let actual = download_facts(scenario, &test_csv, &mut extra);
        let path = format!("download.{}", text(&scenario["name"]));
        diff(&path, &case["python"], &actual, &mut mismatches);
    }
    mismatches.extend(extra);
    assert_no_differences(mismatches);
}

// ------------------------------------------------------------------ contact_import

fn user_from(row: &Value) -> RadioIdUser {
    RadioIdUser {
        radio_id: number(&row[0]),
        callsign: text(&row[1]).into(),
        first_name: text(&row[2]).into(),
        last_name: text(&row[3]).into(),
        city: text(&row[4]).into(),
        state: text(&row[5]).into(),
        country: text(&row[6]).into(),
    }
}

fn project_summary(project: &Project, codec: NameCodec) -> Value {
    let tail: Vec<Value> = project.contacts[project.contacts.len().saturating_sub(4)..]
        .iter()
        .map(|c| json!([c.index(), c.kind(), c.call_id(), c.name(codec)]))
        .collect();
    json!({
        "count": project.contacts.len(),
        "section_order": project.section_order,
        "serialize_sha256": sha(&f136::serialize(project)),
        "tail": tail,
    })
}

fn report_facts(report: &ImportReport) -> Value {
    let shortened: Vec<Value> = report
        .shortened
        .iter()
        .map(|(id, wanted, stored)| json!([id, wanted, stored]))
        .collect();
    json!({"added": report.added, "updated": report.updated, "unchanged": report.unchanged,
           "over_capacity": report.over_capacity, "kept": report.kept, "shortened": shortened,
           "summary": report.summary()})
}

fn outcome(
    result: Result<(Project, ImportReport), opendrs_core::records::RecordError>,
    codec: NameCodec,
) -> Value {
    match result {
        Ok((project, report)) => json!({
            "report": report_facts(&report),
            "project": project_summary(&project, codec),
        }),
        Err(err) => json!({"error": err.to_string()}),
    }
}

fn capacity(case: &Value) -> usize {
    case["capacity"]
        .as_u64()
        .map_or(DEFAULT_CONTACT_CAPACITY, |c| c as usize)
}

fn fixture_project(case: &Value) -> Project {
    match case["fixture"].as_str() {
        None => Project::default(),
        Some("index_65535") => Project {
            contacts: vec![Contact::build(65535, 5, &[0; 10], 1).expect("contact")],
            ..Project::default()
        },
        Some(name) => load(name),
    }
}

fn import_facts(case: &Value, expected: &Value) -> Value {
    let users_of = case["fixture"].as_str().or(case["users_of"].as_str());
    let users: Vec<RadioIdUser> = expected["fixture_users"][users_of.expect("users")]
        .as_array()
        .expect("users")
        .iter()
        .map(user_from)
        .collect();
    let fmt = NameFormat::from_value(text(&case["fmt"])).expect("format");
    let codec = codec(&case["codec"]);
    let project = fixture_project(case);
    let result = import_users(&project, &users, fmt, codec, capacity(case));
    let mut actual = outcome(result, codec);
    for key in ["fixture", "fmt", "codec", "capacity", "users_of"] {
        if let Some(v) = case.get(key) {
            actual[key] = v.clone();
        }
    }
    actual
}

fn append_facts(case: &Value, entries: &[NewContact]) -> Value {
    let codec = codec(&case["codec"]);
    let project = fixture_project(case);
    let result = append_contacts(&project, entries, codec, capacity(case));
    let mut actual = outcome(result, codec);
    for key in ["fixture", "codec", "capacity"] {
        actual[key] = case[key].clone();
    }
    actual
}

#[test]
fn contact_import_matches_python() {
    let oracle = oracle();
    let expected = &oracle["contact_import"];
    let mut mismatches = Vec::new();
    let labels: Vec<Value> = NameFormat::ALL
        .iter()
        .map(|f| json!([f.value(), f.label()]))
        .collect();
    diff(
        "labels",
        &expected["labels"],
        &json!(labels),
        &mut mismatches,
    );
    for case in expected["contact_text"].as_array().expect("contact_text") {
        let fmt = NameFormat::from_value(text(&case[1])).expect("format");
        let actual = json!([case[0], case[1], contact_text(&user_from(&case[0]), fmt)]);
        diff("contact_text", case, &actual, &mut mismatches);
    }
    for case in expected["callsign"].as_array().expect("callsign") {
        let actual = json!([case[0], looks_like_callsign(text(&case[0]))]);
        diff("callsign", case, &actual, &mut mismatches);
    }
    for case in expected["radioid_style"].as_array().expect("radioid_style") {
        let user = RadioIdUser {
            radio_id: 9990001,
            callsign: text(&case[1]).into(),
            first_name: text(&case[2]).into(),
            last_name: String::new(),
            city: String::new(),
            state: String::new(),
            country: "Taiwan".into(),
        };
        let replace = is_radioid_style_name(text(&case[0]), &user);
        diff(
            "radioid_style",
            case,
            &json!([case[0], case[1], case[2], replace]),
            &mut mismatches,
        );
    }
    for (i, case) in expected["imports"]
        .as_array()
        .expect("imports")
        .iter()
        .enumerate()
    {
        diff(
            &format!("imports[{i}]"),
            case,
            &import_facts(case, expected),
            &mut mismatches,
        );
    }
    let entries: Vec<NewContact> = expected["append_entries"]
        .as_array()
        .expect("entries")
        .iter()
        .map(|e| NewContact::new(number(&e[0]), text(&e[1]), number(&e[2])))
        .collect();
    for (i, case) in expected["appends"]
        .as_array()
        .expect("appends")
        .iter()
        .enumerate()
    {
        let actual = if case["fixture"] == "index_65535" {
            append_facts(case, &[NewContact::new(5, "X", 2)])
        } else {
            append_facts(case, &entries)
        };
        diff(&format!("appends[{i}]"), case, &actual, &mut mismatches);
    }
    assert_no_differences(mismatches);
}

// ------------------------------------------------------------------ contacts_csv

fn tricky_project(fields: &Value) -> Project {
    let contacts = fields
        .as_array()
        .expect("fields")
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut field = hex(text(f));
            field.resize(10, 0);
            let i = u16::try_from(i).expect("index");
            Contact::build(i + 1, 4 + (i % 4) as u8, &field, 46000 + u32::from(i)).expect("contact")
        })
        .collect();
    Project {
        contacts,
        ..Project::default()
    }
}

fn parse_result(data: &[u8]) -> Value {
    match parse_contacts_csv(data) {
        Ok(result) => {
            let entries: Vec<Value> = result
                .entries
                .iter()
                .map(|e| json!([e.kind, e.text, e.call_id]))
                .collect();
            json!({"entries": entries, "errors": result.errors, "encoding": result.encoding})
        }
        Err(err) => json!({"error": err.to_string()}),
    }
}

#[test]
fn contacts_csv_matches_python() {
    let oracle = oracle();
    let expected = &oracle["contacts_csv"];
    let mut mismatches = Vec::new();
    let tricky = tricky_project(&expected["tricky_fields"]);
    for case in expected["exports"].as_array().expect("exports") {
        let project = match text(&case["fixture"]) {
            "tricky" => tricky.clone(),
            name => load(name),
        };
        let data = export_contacts(&project, codec(&case["codec"]));
        let head = if case["fixture"] == "tricky" {
            &data[..]
        } else {
            &data[..data.len().min(120)]
        };
        let actual = json!({"fixture": case["fixture"], "codec": case["codec"], "sha256": sha(&data),
                            "len": data.len(), "head": hex_of(head)});
        diff("exports", case, &actual, &mut mismatches);
    }
    for (i, case) in expected["parse"]
        .as_array()
        .expect("parse")
        .iter()
        .enumerate()
    {
        let actual = parse_result(&blob(&case["data"]));
        diff(
            &format!("parse[{i}]"),
            &case["result"],
            &actual,
            &mut mismatches,
        );
    }
    for case in expected["round_trips"].as_array().expect("round trips") {
        let project = load(text(&case["fixture"]));
        let parsed =
            parse_contacts_csv(&export_contacts(&project, NameCodec::Big5)).expect("parse");
        let (_, report) = append_contacts(
            &project,
            &parsed.entries,
            NameCodec::Big5,
            DEFAULT_CONTACT_CAPACITY,
        )
        .expect("append");
        let actual = json!({"fixture": case["fixture"], "entries": parsed.entries.len(),
                            "errors": parsed.errors, "report": report_facts(&report)});
        diff("round_trips", case, &actual, &mut mismatches);
    }
    assert_no_differences(mismatches);
}
