//! Firmware checks, the record-read patch and the supported-radio texts must come out exactly as
//! the Python implementation computes them (tests/parity.json, sections "firmware_check",
//! "fwpatch" and "hardware").

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use common::{assert_same, diff, hex, hex_of, oracle, temp_dir, xorshift};
use opendrs_firmware::firmware_check::{
    self, BLOCKED_IMAGES, FirmwareFile, KNOWN_PATCHED_IMAGES, KNOWN_VENDOR_IMAGES, RF_CHIP_CODES,
    TEMPLATE_CHIP_CODE, check, inspect, normalise_firmware_id, rf_chip, same_build, verdict,
};
use opendrs_firmware::fwpatch::{
    self, A_BASE, CAVE_END, CAVE_START, CUT, DEAD_CODE, RECORD_READ, patch_record_read,
    record_read_status, refs_into,
};
use opendrs_firmware::{hardware, sha256_hex};
use serde_json::{Value, json};

fn table(entries: &[(&str, &str)]) -> Value {
    json!(entries.iter().copied().collect::<BTreeMap<_, _>>())
}

#[test]
fn firmware_ids_match_python() {
    let oracle = oracle();
    let fc = &oracle["firmware_check"];
    let mut mismatches = Vec::new();
    for case in fc["ids"].as_array().expect("ids") {
        let id = case["id"].as_str().expect("id");
        let actual =
            json!({"id": id, "normalised": normalise_firmware_id(id), "rf_chip": rf_chip(id)});
        diff("ids", case, &actual, &mut mismatches);
    }
    for case in fc["same_build"].as_array().expect("same_build") {
        let (a, b) = (case[0].as_str().expect("a"), case[1].as_str().expect("b"));
        diff(
            "same_build",
            case,
            &json!([a, b, same_build(a, b)]),
            &mut mismatches,
        );
    }
    let patched: BTreeMap<_, _> = KNOWN_PATCHED_IMAGES
        .iter()
        .map(|&(sha, (name, file))| (sha, json!([name, file])))
        .collect();
    let mut tables = json!({
        "blocked": table(BLOCKED_IMAGES), "vendor": table(KNOWN_VENDOR_IMAGES), "patched": patched,
        "rf_chip_codes": table(RF_CHIP_CODES), "template": TEMPLATE_CHIP_CODE,
        "max_image": firmware_check::MAX_IMAGE,
        "default_iap_exe": firmware_check::default_iap_exe().map(|p| p.display().to_string()),
    });
    if !cfg!(windows) {
        // the vendor tool only exists on Windows
        tables["default_iap_exe"] = fc["tables"]["default_iap_exe"].clone();
    }
    diff("tables", &fc["tables"], &tables, &mut mismatches);
    diff(
        "verdict_empty",
        &fc["verdict_empty"],
        &json!(verdict(&[]).value()),
        &mut mismatches,
    );
    assert_same(&mismatches);
}

#[test]
fn inspect_matches_python() {
    let oracle = oracle();
    let fc = &oracle["firmware_check"];
    let dir = temp_dir("parity_inspect");
    let mut mismatches = Vec::new();
    for (i, case) in fc["inspect"]
        .as_array()
        .expect("inspect")
        .iter()
        .enumerate()
    {
        let path = dir.join(format!("{i}.bin"));
        std::fs::write(&path, hex(case["data"].as_str().expect("data"))).expect("write");
        let img = inspect(&path).expect("inspect");
        let actual = json!({"data": case["data"], "firmware_id": img.firmware_id,
                            "build_date": img.build_date, "hardware": img.hardware(),
                            "size": img.size, "sha256": img.sha256});
        diff(&format!("inspect[{i}]"), case, &actual, &mut mismatches);
    }
    let big = dir.join("big.bin");
    std::fs::write(&big, vec![0u8; 4 * 1024 * 1024 + 1]).expect("write");
    let too_large = inspect(&big).err().map(|e| e.to_string());
    diff(
        "too_large",
        &fc["too_large"],
        &json!(too_large),
        &mut mismatches,
    );
    assert_same(&mismatches);
}

#[test]
fn checks_match_python() {
    let oracle = oracle();
    let fc = &oracle["firmware_check"];
    let texts = fc["check_texts"].as_array().expect("texts");
    let mut mismatches = Vec::new();
    for case in fc["check"].as_array().expect("check") {
        let s = |i: usize| case[i].as_str().expect("text");
        let image = FirmwareFile {
            path: PathBuf::from("fw.bin"),
            size: 1024,
            sha256: fc["shas"][s(0)].as_str().expect("sha").to_owned(),
            firmware_id: s(1).to_owned(),
            build_date: s(2).to_owned(),
        };
        let backup = case[3].as_bool().expect("backup");
        let findings = check(&image, s(4), backup, s(5));
        let expected: Vec<Value> = case[6]
            .as_array()
            .expect("findings")
            .iter()
            .map(|f| json!([f[0], texts[f[1].as_u64().expect("index") as usize]]))
            .collect();
        let actual: Vec<Value> = findings
            .iter()
            .map(|f| json!([f.level.value(), f.text]))
            .collect();
        let label = format!("check {case}");
        diff(&label, &json!(expected), &json!(actual), &mut mismatches);
        diff(
            &label,
            &case[7],
            &json!(verdict(&findings).value()),
            &mut mismatches,
        );
    }
    assert_same(&mismatches);
}

// ---------------------------------------------------------------------- fwpatch

fn synthetic(extra: &[(u32, Vec<u8>)], size: usize) -> Vec<u8> {
    let mut img = vec![0u8; size];
    let sites = RECORD_READ.iter().map(|s| (s.va, s.original.to_vec()));
    for (va, data) in sites.chain(extra.iter().cloned()) {
        let o = (va - A_BASE) as usize;
        img[o..o + data.len()].copy_from_slice(&data);
    }
    img
}

fn patch_facts(data: &[u8]) -> Value {
    let mut out = json!({"status": record_read_status(data).to_string(),
                         "is_patched": fwpatch::is_record_read_patched(data)});
    match patch_record_read(data) {
        Ok(r) => {
            out["result"] = json!({"sha256": r.sha256, "base_sha256": r.base_sha256,
                "changed": r.changed, "base_name": r.base_name,
                "status_after": record_read_status(&r.data).to_string(),
                "is_patched_after": fwpatch::is_record_read_patched(&r.data)});
            out["error"] = Value::Null;
        }
        Err(e) => {
            out["result"] = Value::Null;
            out["error"] = json!(e.0);
        }
    }
    out
}

fn refs_facts(data: &[u8], lo: u32, hi: u32) -> Value {
    let refs: Vec<String> = refs_into(data, i64::from(lo), i64::from(hi))
        .iter()
        .map(ToString::to_string)
        .collect();
    json!({"count": refs.len(), "sha256": sha256_hex(refs.join("\n").as_bytes()),
           "first": refs.iter().take(20).collect::<Vec<_>>(),
           "last": refs[refs.len().saturating_sub(5)..].to_vec()})
}

fn extras(spec: &Value) -> Vec<(u32, Vec<u8>)> {
    spec.as_object()
        .expect("extra")
        .iter()
        .map(|(va, data)| (va.parse().expect("va"), hex(data.as_str().expect("hex"))))
        .collect()
}

#[test]
fn record_read_patch_matches_python() {
    let oracle = oracle();
    let fp = &oracle["fwpatch"];
    let size = fp["constants"]["size"].as_u64().expect("size") as usize;
    let mut mismatches = Vec::new();
    let constants = json!({
        "a_base": A_BASE, "cut": CUT, "b_base": fwpatch::B_BASE, "cave": [CAVE_START, CAVE_END],
        "dead_code": [DEAD_CODE.0, DEAD_CODE.1], "branch_sites": fwpatch::BRANCH_SITES,
        "known_bases": table(fwpatch::KNOWN_BASES), "size": size,
    });
    diff("constants", &fp["constants"], &constants, &mut mismatches);
    let sites: Vec<Value> = RECORD_READ
        .iter()
        .map(|s| json!([s.va, hex_of(s.original), hex_of(s.patched), s.why]))
        .collect();
    diff("sites", &fp["sites"], &json!(sites), &mut mismatches);

    let mut cases = serde_json::Map::new();
    for (name, spec) in fp["extras"].as_object().expect("extras") {
        cases.insert(name.clone(), patch_facts(&synthetic(&extras(spec), size)));
    }
    let plain = synthetic(&[], size);
    let mut wrong = plain.clone();
    wrong[(0x0302_6E5C - A_BASE) as usize] ^= 1;
    cases.insert("one_bit_wrong".into(), patch_facts(&wrong));
    cases.insert("too_small_100".into(), patch_facts(&[0u8; 100]));
    cases.insert("exactly_cut".into(), patch_facts(&synthetic(&[], CUT)));
    cases.insert(
        "cut_plus_one_zeros".into(),
        patch_facts(&vec![0u8; CUT + 1]),
    );
    let patched = patch_record_read(&plain).expect("patch").data;
    cases.insert("patched_again".into(), patch_facts(&patched));
    let mut reverted = patched.clone();
    let o = (0x0302_7024 - A_BASE) as usize;
    reverted[o..o + 4].copy_from_slice(&[0xD2, 0x14, 0x26, 0x14]);
    cases.insert("patched_one_site_reverted".into(), patch_facts(&reverted));
    diff(
        "cases",
        &fp["cases"],
        &Value::Object(cases),
        &mut mismatches,
    );

    let noise = xorshift(CUT + 0x4000, 0x0123_4567);
    let refs = json!({
        "synthetic_cave": refs_facts(&plain, CAVE_START, CAVE_END),
        "patched_cave": refs_facts(&patched, CAVE_START, CAVE_END),
        "patched_dead_code": refs_facts(&patched, DEAD_CODE.0, DEAD_CODE.1),
        "noise_unit_a": refs_facts(&noise, 0x0300_0000, 0x0310_0000),
        "noise_unit_b": refs_facts(&noise, 0x0001_0000, 0x0002_0000),
        "noise_narrow": refs_facts(&noise, 0x0305_0000, 0x0305_0400),
        "noise_odd_length": refs_facts(&noise[..4097], 0x0300_0000, 0x0300_2000),
    });
    diff("refs", &fp["refs"], &refs, &mut mismatches);
    assert_same(&mismatches);
}

#[test]
fn hardware_matches_python() {
    let oracle = oracle();
    let expected = &oracle["hardware"];
    let known_as: Vec<Value> = expected["known_as"]
        .as_array()
        .expect("known_as")
        .iter()
        .map(|case| {
            let model = case[0].as_str().expect("model");
            json!([model, hardware::known_as(model)])
        })
        .collect();
    let radios: Vec<Value> = hardware::SUPPORTED_RADIOS
        .iter()
        .map(|r| json!([r.brand, r.model, r.note, r.name()]))
        .collect();
    let actual = json!({
        "reported_model": hardware::REPORTED_MODEL, "model_number": hardware::MODEL_NUMBER,
        "platform": hardware::PLATFORM, "radios": radios,
        "supported_names": hardware::supported_names(), "supported_text": hardware::supported_text(),
        "known_as": known_as,
    });
    let mut mismatches = Vec::new();
    diff("hardware", expected, &actual, &mut mismatches);
    assert_same(&mismatches);
}
