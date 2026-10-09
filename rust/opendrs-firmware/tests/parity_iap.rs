//! IAP images and upgrades must come out exactly as the Python implementation computes them
//! (tests/parity.json, section "iap").

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::{
    Answers, ScriptedTransport, Shared, assert_same, bytes_facts, diff, fixtures, hex, hex_of,
    oracle, pattern,
};
use opendrs_core::radio_link::Transport;
use opendrs_firmware::iap::{IapClient, IapImage, PAD_BYTE, RETRY};
use opendrs_firmware::iap_sim::SimulatedBootloader;
use opendrs_firmware::sha256_hex;
use serde_json::{Value, json};

fn image_facts(img: &IapImage) -> Value {
    json!({
        "size": img.size(), "address": img.address(), "last_page": img.last_page(),
        "pages": img.page_count(), "checksum": img.checksum(),
        "erase_command": hex_of(&img.erase_command()),
        "last_page_sha256": sha256_hex(img.page(img.last_page()).expect("last page")),
        "stream_sha256": sha256_hex(&img.stream()),
    })
}

fn usize_of(value: &Value) -> usize {
    usize::try_from(value.as_u64().expect("number")).expect("usize")
}

#[test]
fn images_match_python() {
    let oracle = oracle();
    let images = &oracle["iap"]["images"];
    let mut mismatches = Vec::new();
    for (name, expected) in images["fixtures"].as_object().expect("fixtures") {
        let data = std::fs::read(fixtures().join("iap").join(name)).expect("fixture");
        let tx = std::fs::read(fixtures().join("iap").join(format!("vendor_tx_{name}")))
            .expect("vendor tx");
        let mut actual = image_facts(&IapImage::from_bytes(&data).expect("image"));
        actual["vendor_tx_sha256"] = json!(sha256_hex(&tx));
        actual["file_sha256"] = json!(sha256_hex(&data));
        diff(name, expected, &actual, &mut mismatches);
    }
    for case in images["synthetic"].as_array().expect("synthetic") {
        let size = usize_of(&case["size"]);
        let address = case["address"].as_i64().expect("address");
        let pad = u8::try_from(case["pad"].as_u64().expect("pad")).expect("u8");
        let img = IapImage::from_bytes_at(&pattern(size, 3), address, pad).expect("image");
        diff(
            &format!("synthetic {size}"),
            &case["facts"],
            &image_facts(&img),
            &mut mismatches,
        );
    }
    for case in images["errors"].as_array().expect("errors") {
        let size = usize_of(&case["size"]);
        let address = case["address"].as_i64().expect("address");
        let error = IapImage::from_bytes_at(&pattern(size, 3), address, PAD_BYTE)
            .err()
            .map(|e| e.0);
        diff(
            &format!("error {size}/{address}"),
            &case["error"],
            &json!(error),
            &mut mismatches,
        );
    }
    diff(
        "retry",
        &oracle["iap"]["retry"],
        &json!(RETRY),
        &mut mismatches,
    );
    assert_same(&mismatches);
}

fn bootloader(spec: &Value) -> SimulatedBootloader {
    let mut boot = SimulatedBootloader::default();
    boot.erase_reply = hex(spec["erase_reply"].as_str().expect("reply"));
    boot.ack_suffix = hex(spec["ack_suffix"].as_str().expect("suffix"));
    boot.silent = spec["silent"].as_bool().expect("silent");
    boot.stop_acking_after = spec["stop_acking_after"].as_u64().map(|n| n as usize);
    boot.bad_ack = spec["bad_ack"].as_str().map(hex);
    boot.corrupt_checksum = spec["corrupt_checksum"].as_bool().expect("corrupt");
    boot
}

fn run_upgrade<T: Transport>(
    transport: &mut T,
    img: &IapImage,
    cancelled: impl FnMut() -> bool,
    erase_timeout: Option<f64>,
) -> Value {
    let mut steps = Vec::new();
    let mut client = IapClient::new(transport)
        .with_progress(|d, t, s| steps.push(json!([d, t, s])))
        .with_cancel(cancelled);
    if let Some(seconds) = erase_timeout {
        client.erase_timeout = Duration::from_secs_f64(seconds);
    }
    let outcome = client.upgrade(img);
    let client_erase_sent = client.erase_sent();
    drop(client);
    let (result, error) = match outcome {
        Ok(r) => (
            json!({"checksum": r.checksum, "pages": r.pages, "erase_reply": r.erase_reply}),
            Value::Null,
        ),
        Err(e) => (
            Value::Null,
            json!({"message": e.message, "erase_sent": e.erase_sent}),
        ),
    };
    json!({"result": result, "error": error, "steps": steps, "client_erase_sent": client_erase_sent})
}

fn boot_scenario(case: &Value, img: &IapImage) -> Value {
    let boot = Rc::new(RefCell::new(bootloader(&case["transport"]["boot"])));
    let mut transport = Shared(Rc::clone(&boot));
    let cancel = &case["cancel"];
    let mut run = if let Some(limit) = cancel["after_flash"].as_u64() {
        let watched = Rc::clone(&boot);
        let limit = limit as usize;
        run_upgrade(
            &mut transport,
            img,
            move || watched.borrow().flash.len() >= limit,
            None,
        )
    } else {
        let mut answers = Answers::new(answers_of(cancel));
        run_upgrade(&mut transport, img, move || answers.next(), None)
    };
    let boot = boot.borrow();
    run["boot"] = json!({
        "sent": bytes_facts(&boot.sent), "flash": bytes_facts(&boot.flash),
        "address": boot.address, "last_page": boot.last_page, "erased": boot.erased,
        "finished": boot.finished,
    });
    run
}

fn answers_of(cancel: &Value) -> Vec<bool> {
    cancel["answers"]
        .as_array()
        .expect("answers")
        .iter()
        .map(|a| a.as_bool().expect("bool"))
        .collect()
}

fn scripted_scenario(case: &Value, img: &IapImage) -> Value {
    let script = &case["transport"]["script"];
    let chunks = script["chunks"]
        .as_array()
        .expect("chunks")
        .iter()
        .map(|c| hex(c.as_str().expect("hex")))
        .collect();
    let mut transport = ScriptedTransport::new(
        chunks,
        script["fail_on_write"].as_u64().map(|n| n as usize),
        script["fail_reset"].as_bool().expect("fail_reset"),
    );
    let mut answers = Answers::new(answers_of(&case["cancel"]));
    let mut run = run_upgrade(
        &mut transport,
        img,
        move || answers.next(),
        case["erase_timeout"].as_f64(),
    );
    run["scripted"] = json!({"sent": bytes_facts(&transport.sent.concat()),
                             "writes": transport.sent.len()});
    run
}

#[test]
fn upgrades_match_python() {
    let oracle = oracle();
    let mut mismatches = Vec::new();
    for case in oracle["iap"]["scenarios"].as_array().expect("scenarios") {
        let name = case["name"].as_str().expect("name");
        let size = usize_of(&case["size"]);
        let address = case["address"].as_i64().expect("address");
        let img = IapImage::from_bytes_at(&pattern(size, 3), address, PAD_BYTE).expect("image");
        let run = if case["transport"].get("boot").is_some() {
            boot_scenario(case, &img)
        } else {
            scripted_scenario(case, &img)
        };
        diff(name, &case["run"], &run, &mut mismatches);
    }
    assert_same(&mismatches);
}
