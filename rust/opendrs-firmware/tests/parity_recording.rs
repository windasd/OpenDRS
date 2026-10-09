//! Recording frames, record lines, audio helpers and list / download runs against the simulated
//! recorder must come out exactly as the Python implementation computes them (tests/parity.json,
//! section "recording").

mod common;

use std::cell::RefCell;
use std::time::Duration;

use common::{
    Answers, Chunked, Truncated, assert_same, bytes_facts, diff, fixtures, hex, hex_of, oracle,
    xorshift,
};
use opendrs_core::radio_link::{RadioError, Transport};
use opendrs_firmware::recording::{
    self, BLOCK, RecordClient, RecordInfo, RecordingError, checksum16, h_frame, list_request,
    record_read_command, to_bit_stream, wav_bytes,
};
use opendrs_firmware::recording_sim::{SimulatedRecorder, demo_records};
use opendrs_firmware::sha256_hex;
use serde_json::{Value, json};

const FAST: Duration = Duration::from_millis(50);

fn info_facts(info: Option<&RecordInfo>) -> Value {
    info.map_or(Value::Null, |i| {
        json!({
            "number": i.number, "total": i.total, "direction": i.direction.to_string(),
            "call_id": i.call_id, "length": i.length, "time": i.time, "line": hex_of(&i.line()),
            "blocks": i.blocks(), "pages_field": i.pages_field(), "seconds": i.seconds(),
            "stem": i.stem(),
        })
    })
}

fn u32_of(value: &Value) -> u32 {
    u32::try_from(value.as_u64().expect("number")).expect("u32")
}

#[test]
fn frames_and_lines_match_python() {
    let oracle = oracle();
    let rec = &oracle["recording"];
    let mut mismatches = Vec::new();
    for case in rec["checksum16"].as_array().expect("checksum16") {
        let data = hex(case[0].as_str().expect("hex"));
        diff(
            "checksum16",
            case,
            &json!([case[0], checksum16(&data)]),
            &mut mismatches,
        );
    }
    for case in rec["h_frame"].as_array().expect("h_frame") {
        let cmd = u8::try_from(case[0].as_u64().expect("cmd")).expect("u8");
        let frame = h_frame(cmd, &hex(case[1].as_str().expect("payload"))).expect("fits");
        diff(
            "h_frame",
            case,
            &json!([case[0], case[1], hex_of(&frame)]),
            &mut mismatches,
        );
    }
    for case in rec["list_request"].as_array().expect("list_request") {
        let n = u16::try_from(case[0].as_u64().expect("n")).expect("u16");
        diff(
            "list_request",
            case,
            &json!([n, hex_of(&list_request(n))]),
            &mut mismatches,
        );
    }
    for case in rec["record_read_command"].as_array().expect("commands") {
        let (n, p) = (u32_of(&case[0]), u32_of(&case[1]));
        let actual = json!([n, p, hex_of(&record_read_command(n, p))]);
        diff("record_read_command", case, &actual, &mut mismatches);
    }
    for case in rec["lines"].as_array().expect("lines") {
        let text = hex(case["text"].as_str().expect("text"));
        let actual =
            json!({"text": case["text"], "info": info_facts(RecordInfo::parse(&text).as_ref())});
        diff("lines", case, &actual, &mut mismatches);
    }
    let demo = demo_records();
    let actual: Vec<Value> = demo
        .iter()
        .map(|(i, d)| json!({"info": info_facts(Some(i)), "data": bytes_facts(d)}))
        .collect();
    diff(
        "demo_records",
        &rec["demo_records"],
        &json!(actual),
        &mut mismatches,
    );
    assert_same(&mismatches);
}

#[test]
fn audio_matches_python() {
    let oracle = oracle();
    let rec = &oracle["recording"];
    let mut mismatches = Vec::new();
    for case in rec["bit_stream"].as_array().expect("bit_stream") {
        let bits = to_bit_stream(&hex(case[0].as_str().expect("hex")));
        diff(
            "bit_stream",
            case,
            &json!([case[0], bytes_facts(&bits)]),
            &mut mismatches,
        );
    }
    let demo = demo_records();
    let actual: Vec<Value> = demo
        .iter()
        .map(|(_, d)| bytes_facts(&to_bit_stream(d)))
        .collect();
    diff(
        "bit_stream_demo",
        &rec["bit_stream_demo"],
        &json!(actual),
        &mut mismatches,
    );
    let (info, data) = &demo[1];
    let mut block = data.clone();
    block.resize(BLOCK, 0xFF);
    assert_eq!(info.length as usize, data.len());
    let fixture = std::fs::read(fixtures().join("recording/wav_exe_block.bit")).expect("fixture");
    let actual = json!({"input_sha256": sha256_hex(&block), "bits": bytes_facts(&to_bit_stream(&block)),
                        "fixture_sha256": sha256_hex(&fixture)});
    diff(
        "bit_stream_block",
        &rec["bit_stream_block"],
        &actual,
        &mut mismatches,
    );
    for case in rec["wav"].as_array().expect("wav") {
        let pcm = hex(case["pcm"].as_str().expect("pcm"));
        let actual = json!({"pcm": case["pcm"], "wav": bytes_facts(&wav_bytes(&pcm))});
        diff("wav", case, &actual, &mut mismatches);
    }
    let c = &rec["constants"];
    let mut actual = json!({
        "block": BLOCK, "frame_bytes": recording::FRAME_BYTES, "sample_rate": recording::SAMPLE_RATE,
        "list_cmd": recording::LIST_CMD, "default_list_max": recording::DEFAULT_LIST_MAX,
        "max_records": recording::MAX_RECORDS, "record_verb": hex_of(recording::RECORD_VERB),
        "checksum_read": hex_of(recording::CHECKSUM_READ),
        "default_ambep2": recording::default_ambep2().map(|p| p.display().to_string()),
    });
    if !cfg!(windows) {
        // no default decoder outside Windows
        actual["default_ambep2"] = c["default_ambep2"].clone();
    }
    diff("constants", c, &actual, &mut mismatches);
    assert_same(&mismatches);
}

// ---------------------------------------------------------------------- simulated runs

/// The transports the scenarios use, all around one simulated recorder.
enum Link {
    Direct(SimulatedRecorder),
    Chunked(Chunked<SimulatedRecorder>),
    Truncated(Truncated<SimulatedRecorder>),
}

impl Link {
    fn radio(&mut self) -> &mut SimulatedRecorder {
        match self {
            Link::Direct(r) => r,
            Link::Chunked(c) => &mut c.inner,
            Link::Truncated(t) => &mut t.inner,
        }
    }

    fn transport(&mut self) -> &mut dyn Transport {
        match self {
            Link::Direct(r) => r,
            Link::Chunked(c) => c,
            Link::Truncated(t) => t,
        }
    }
}

fn recorder(case: &Value) -> SimulatedRecorder {
    let mut radio = match case["records"].as_array() {
        None => SimulatedRecorder::default(),
        Some(specs) => SimulatedRecorder::with_records(specs.iter().map(record_of).collect()),
    };
    let flags = &case["flags"];
    let flag = |name: &str, default: bool| flags[name].as_bool().unwrap_or(default);
    radio.supports_list = flag("supports_list", true);
    radio.supports_record_read = flag("supports_record_read", true);
    radio.corrupt_checksum = flag("corrupt_checksum", false);
    radio
}

fn record_of(spec: &Value) -> (RecordInfo, Vec<u8>) {
    let i = &spec["info"];
    let n = |k: usize| i[k].as_u64().expect("number");
    let info = RecordInfo {
        number: n(0),
        total: n(1),
        direction: i[2]
            .as_str()
            .and_then(|s| s.chars().next())
            .expect("direction"),
        call_id: n(3),
        length: n(4),
        time: i[5].as_str().expect("time").to_owned(),
    };
    (info, hex(spec["data"].as_str().expect("data")))
}

fn link(case: &Value) -> Link {
    let radio = recorder(case);
    match &case["transport"] {
        Value::String(kind) if kind == "chunked" => Link::Chunked(Chunked::new(radio)),
        Value::Object(spec) => {
            let limit = spec["truncate"].as_u64().expect("limit") as usize;
            Link::Truncated(Truncated::new(radio, limit))
        }
        _ => Link::Direct(radio),
    }
}

fn error_facts(error: &RecordingError) -> Value {
    match error {
        RecordingError::Failed(message) => json!({"type": "RecordingError", "message": message}),
        RecordingError::Radio(RadioError::Timeout { message, received }) => {
            json!({"type": "RadioTimeout", "message": message, "received": received})
        }
        RecordingError::Radio(RadioError::Failed(message)) => {
            json!({"type": "RadioError", "message": message})
        }
    }
}

/// One operation with a fresh client (the client keeps nothing between operations: each one
/// starts by clearing its buffer).
fn run_op(
    op: &Value,
    link: &mut Link,
    chunked: bool,
    steps: &RefCell<Vec<Value>>,
    answers: &RefCell<Answers>,
) -> Value {
    if op[0] == "write" {
        let data = hex(op[1].as_str().expect("hex"));
        link.radio().write(&data).expect("simulator write");
        return json!({"ok": null});
    }
    let mut client = RecordClient::new(link.transport())
        .with_progress(|d, n, s| steps.borrow_mut().push(json!([d, n, s])))
        .with_cancel(|| answers.borrow_mut().next());
    if chunked {
        client.block_timeout = Duration::from_secs(30);
    } else {
        client.list_timeout = FAST;
        client.reply_timeout = FAST;
        client.block_timeout = FAST;
        client.checksum_timeout = FAST;
    }
    if op[0] == "list" {
        let max = op[1].as_u64().expect("max") as usize;
        return match client.list_records(max) {
            Ok(infos) => {
                let facts: Vec<Value> = infos.iter().map(|i| info_facts(Some(i))).collect();
                json!({"ok": facts, "list_stopped": client.list_stopped()})
            }
            Err(e) => json!({"error": error_facts(&e)}),
        };
    }
    match client.read_record(u32_of(&op[1]), u32_of(&op[2])) {
        Ok((header, data)) => {
            json!({"ok": {"header": info_facts(Some(&header)), "data": bytes_facts(&data)}})
        }
        Err(e) => json!({"error": error_facts(&e)}),
    }
}

#[test]
fn simulated_runs_match_python() {
    let oracle = oracle();
    let mut mismatches = Vec::new();
    for case in oracle["recording"]["scenarios"]
        .as_array()
        .expect("scenarios")
    {
        let name = case["name"].as_str().expect("name");
        let mut link = link(case);
        let chunked = case["transport"] == "chunked";
        let steps = RefCell::new(Vec::new());
        let answers = case["cancel"]["answers"]
            .as_array()
            .expect("answers")
            .iter()
            .map(|a| a.as_bool().expect("bool"))
            .collect();
        let answers = RefCell::new(Answers::new(answers));
        let results: Vec<Value> = case["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .map(|op| run_op(op, &mut link, chunked, &steps, &answers))
            .collect();
        diff(
            &format!("{name}.results"),
            &case["results"],
            &json!(results),
            &mut mismatches,
        );
        diff(
            &format!("{name}.steps"),
            &case["steps"],
            &json!(steps.into_inner()),
            &mut mismatches,
        );
        diff(
            &format!("{name}.sent"),
            &case["sent"],
            &bytes_facts(&link.radio().sent),
            &mut mismatches,
        );
    }
    assert_same(&mismatches);
}

#[test]
fn checksum16_of_random_data_matches_python_formula() {
    // sanity for the oracle's xorshift generator: the same bytes as the Python script
    assert_eq!(hex_of(&xorshift(3, 11)), {
        let oracle = oracle();
        oracle["recording"]["checksum16"][9][0]
            .as_str()
            .expect("hex")
            .to_owned()
    });
}
