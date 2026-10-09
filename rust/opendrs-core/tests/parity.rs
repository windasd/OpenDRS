//! Everything the Python implementation computes for the fixtures (tests/parity.json, made by
//! rust/tools/make_parity.py) must come out the same here.

mod common;

use opendrs_core::codeplug::{self, Counts, check_writable, counts_in_image};
use opendrs_core::f136::{self, Project};
use opendrs_core::radio_link::RadioIdentity;
use opendrs_core::records::{hz_to_mhz, mhz_to_hz};
use opendrs_core::textcodec::{self, NameCodec};
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

fn load(name: &str) -> Project {
    let path = common::fixtures().join(name);
    match name {
        "capture/read_stream.bin" => codeplug::parse_stream(
            &std::fs::read(path).expect("stream"),
            Counts::new(4, 27, 360),
        )
        .expect("parse"),
        "capture/read1_stream.bin" => codeplug::parse_stream(
            &std::fs::read(path).expect("stream"),
            Counts::new(4, 26, 360),
        )
        .expect("parse"),
        _ => f136::load(path).expect("load"),
    }
}

fn facts(project: &Project) -> Value {
    let dev = project.device();
    let codecs = NameCodec::ALL;
    let fields: Vec<&[u8]> = project
        .contacts
        .iter()
        .map(|c| c.name_field())
        .chain(project.zones.iter().map(|z| z.name_field()))
        .chain(project.channels.iter().map(|c| c.name_field()))
        .collect();
    json!({
        "contacts": project.contacts.iter().map(|c| json!([c.index(), c.kind(), c.kind_label(),
            c.name(codecs[0]), c.name(codecs[1]), c.name(codecs[2]), c.call_id()])).collect::<Vec<_>>(),
        "zones": project.zones.iter().map(|z| json!([z.index(), z.name(NameCodec::Big5), z.first_record(),
            z.channel_count()])).collect::<Vec<_>>(),
        "channels": project.channels.iter().map(|c| json!([c.index(), c.name(NameCodec::Big5), c.rx_hz(),
            c.tx_hz(), c.mode(), c.mode_label(), c.color_code(), c.rx_group(), c.tx_contact(),
            hz_to_mhz(c.rx_hz()), hz_to_mhz(c.tx_hz())])).collect::<Vec<_>>(),
        "device": {"band_label": dev.band_label, "write_date": dev.write_date, "model": dev.model,
                   "alias": dev.alias, "dmr_id": dev.dmr_id, "firmware": dev.firmware,
                   "firmware_date": dev.firmware_date, "factory_number": dev.factory_number,
                   "serial_number": dev.serial_number, "model_number": dev.model_number,
                   "bands": dev.bands().map(|b| b.iter().map(|(lo, hi)| json!([lo, hi])).collect::<Vec<_>>())},
        "problems": project.problems(),
        "check_writable": check_writable(project),
        "names_decodable": {"big5": project.names_decodable(NameCodec::Big5),
                            "gbk": project.names_decodable(NameCodec::Gbk),
                            "ascii": project.names_decodable(NameCodec::Ascii)},
        "suggest_codec": textcodec::suggest_codec(&fields).map(NameCodec::value),
        "counts_in_image": counts_in_image(&project.image).map(|c| json!([c.zones, c.channels, c.contacts])),
        "section_order": project.section_order,
        "image_len": project.image.len(),
        "serialize_sha256": sha(&f136::serialize(project)),
        "build_stream_sha256": codeplug::build_stream(project, None).ok().map(|s| sha(&s)),
    })
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
        _ if expected == actual => {}
        _ => out.push(format!("{path}: python {expected} / rust {actual}")),
    }
}

#[test]
fn every_fixture_matches_python() {
    let oracle = oracle();
    let mut mismatches = Vec::new();
    for (name, expected) in oracle["fixtures"].as_object().expect("fixtures") {
        let project = load(name);
        let mut actual = facts(&project);
        if let Some(file_sha) = expected.get("file_sha256") {
            let bytes = std::fs::read(common::fixtures().join(name)).expect("fixture");
            actual["file_sha256"] = json!(sha(&bytes));
            assert_eq!(
                file_sha, &actual["file_sha256"],
                "{name}: fixture differs from the one Python saw"
            );
        }
        diff(name, expected, &actual, &mut mismatches);
    }
    assert!(
        mismatches.is_empty(),
        "{} differences:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn unmodified_projects_are_written_back_byte_for_byte() {
    for name in [
        "dh98vu_20250811.F136",
        "mmdvm_20241124.F136",
        "mmdvm_20241127.F136",
        "capture/written_project.F136",
    ] {
        let bytes = std::fs::read(common::fixtures().join(name)).expect("fixture");
        assert_eq!(
            f136::serialize(&f136::parse(&bytes).expect("parse")),
            bytes,
            "{name}"
        );
    }
}

#[test]
fn textcodec_matches_python() {
    let cases = &oracle()["textcodec"];
    let mut mismatches = Vec::new();
    for case in cases["display"].as_array().expect("display") {
        let (raw, codec) = (
            hex(case[0].as_str().unwrap()),
            NameCodec::from_value(case[1].as_str().unwrap()).unwrap(),
        );
        let actual = json!([
            case[0],
            case[1],
            textcodec::display_name(&raw, codec),
            textcodec::decode_name(&raw, codec)
        ]);
        diff("display", case, &actual, &mut mismatches);
    }
    for case in cases["encode"].as_array().expect("encode") {
        let (text, codec) = (
            case[0].as_str().unwrap(),
            NameCodec::from_value(case[1].as_str().unwrap()).unwrap(),
        );
        let actual = match textcodec::encode_name(text, 10, codec) {
            Ok(field) => json!([text, case[1], hex_of(&field), null]),
            Err(e) => json!([text, case[1], null, e.0]),
        };
        diff("encode", case, &actual, &mut mismatches);
    }
    for case in cases["fit"].as_array().expect("fit") {
        let (text, codec) = (
            case[0].as_str().unwrap(),
            NameCodec::from_value(case[1].as_str().unwrap()).unwrap(),
        );
        let (field, stored) = textcodec::fit_name(text, 10, codec);
        diff(
            "fit",
            case,
            &json!([text, case[1], hex_of(&field), stored]),
            &mut mismatches,
        );
    }
    for case in cases["cjk"].as_array().expect("cjk") {
        let result = textcodec::best_cjk_decoding(&hex(case[0].as_str().unwrap()));
        let actual = match result {
            Some((text, codec)) => json!([case[0], text, codec.value()]),
            None => json!([case[0], null, null]),
        };
        diff("cjk", case, &actual, &mut mismatches);
    }
    for case in cases["legacy"].as_array().expect("legacy") {
        let (text, encoding) = textcodec::read_legacy_text(&hex(case[0].as_str().unwrap()));
        diff(
            "legacy",
            case,
            &json!([case[0], text, encoding]),
            &mut mismatches,
        );
    }
    assert!(
        mismatches.is_empty(),
        "{} differences:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn frequencies_match_python() {
    let cases = &oracle()["mhz"];
    let mut mismatches = Vec::new();
    for case in cases["mhz_to_hz"].as_array().expect("mhz") {
        let text = case[0].as_str().unwrap();
        let actual = match mhz_to_hz(text) {
            Ok(hz) => json!([text, hz, null]),
            Err(e) => json!([text, null, e.to_string()]),
        };
        diff("mhz_to_hz", case, &actual, &mut mismatches);
    }
    for case in cases["hz_to_mhz"].as_array().expect("hz") {
        let hz = u32::try_from(case[0].as_u64().unwrap()).unwrap();
        diff(
            "hz_to_mhz",
            case,
            &json!([hz, hz_to_mhz(hz)]),
            &mut mismatches,
        );
    }
    assert!(
        mismatches.is_empty(),
        "{} differences:\n{}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn radio_identity_matches_python() {
    let expected = &oracle()["identity"];
    let identity = RadioIdentity::parse(&common::cap("read_reply.bin")[22..]);
    let actual = json!({"firmware": identity.firmware, "model": identity.model, "band": identity.band,
                        "summary": identity.summary()});
    assert_eq!(expected, &actual);
}
