//! Reading and writing the simulated radio, including the failure paths (port of the simulated
//! radio tests in test_radio_link.py).

mod common;

use common::{Silent, cap};
use opendrs_core::codeplug::{Counts, with_synced_image};
use opendrs_core::radio_link::{RadioClient, RadioError, clock_write_project};
use opendrs_core::radio_sim::SimulatedRadio;
use opendrs_core::records::Contact;
use opendrs_core::textcodec::{NameCodec, encode_name};

fn radio() -> SimulatedRadio {
    SimulatedRadio::with_stream(&cap("read_stream.bin"), Counts::new(4, 27, 360))
}

#[test]
fn reads_the_stream_it_holds() {
    let mut radio = radio();
    let result = RadioClient::new(&mut radio).read_codeplug().unwrap();
    assert_eq!(result.stream, cap("read_stream.bin"));
    assert_eq!(result.identity.model, "DRS-8500");
}

#[test]
fn write_then_read_back() {
    let mut radio = radio();
    let project = RadioClient::new(&mut radio)
        .read_codeplug()
        .unwrap()
        .project;
    let renamed = project.channels[0]
        .with_name("NEW", NameCodec::Ascii)
        .unwrap();
    let edited = project.with_channel(0, renamed).unwrap();
    RadioClient::new(&mut radio)
        .write_codeplug(&edited, Some("2026/10/08 18:00"), None)
        .unwrap();
    assert_eq!(radio.commits, 1);
    let back = RadioClient::new(&mut radio).read_codeplug().unwrap();
    assert_eq!(back.project.channels[0].name(NameCodec::Ascii), "NEW");
    assert_eq!(&back.stream[0x60..0x70], b"2026/10/08 18:00");
    assert_eq!(back.project.contacts, project.contacts);
}

#[test]
fn a_bigger_contact_list_grows_part_one() {
    let mut radio = radio();
    let project = RadioClient::new(&mut radio)
        .read_codeplug()
        .unwrap()
        .project;
    let next = project
        .contacts
        .iter()
        .map(Contact::index)
        .max()
        .unwrap_or(0)
        + 1;
    let extra = (0..369u16).map(|i| {
        let name = encode_name(&format!("QX{i}"), 10, NameCodec::Ascii).unwrap();
        Contact::build(
            next + i,
            Contact::PRIVATE_CALL,
            &name,
            9_990_000 + u32::from(i),
        )
        .unwrap()
    });
    let mut big = project.clone();
    big.contacts.extend(extra);
    let big = with_synced_image(&big);
    RadioClient::new(&mut radio)
        .write_codeplug(&big, None, None)
        .unwrap();
    let back = RadioClient::new(&mut radio).read_codeplug().unwrap();
    assert_eq!(back.counts, Counts::new(4, 27, 729));
    assert_eq!(back.counts.part1_blocks(), 9);
    assert_eq!(back.project.contacts, big.contacts);
    assert_eq!(
        (back.project.zones.clone(), back.project.channels.clone()),
        (big.zones, big.channels)
    );
}

#[test]
fn an_identity_veto_sends_nothing() {
    let mut radio = radio();
    let project = RadioClient::new(&mut radio)
        .read_codeplug()
        .unwrap()
        .project;
    let veto = |_: &opendrs_core::radio_link::RadioIdentity| Some("機型不符".to_string());
    let err = RadioClient::new(&mut radio)
        .write_codeplug(&project, None, Some(&veto))
        .unwrap_err();
    assert!(
        err.to_string().contains("未寫入任何資料：機型不符"),
        "{err}"
    );
    assert_eq!(radio.commits, 0);
}

#[test]
fn a_missing_ack_aborts_without_commit() {
    let mut radio = radio();
    let project = RadioClient::new(&mut radio)
        .read_codeplug()
        .unwrap()
        .project;
    radio.stop_acking_after = Some(2);
    let err = RadioClient::new(&mut radio)
        .write_codeplug(&project, None, None)
        .unwrap_err();
    assert!(
        err.to_string().contains("第 3 塊未獲確認（b'(no reply)'）"),
        "{err}"
    );
    assert_eq!(radio.commits, 0);
}

#[test]
fn a_bad_write_checksum_is_reported() {
    let mut radio = radio();
    let project = RadioClient::new(&mut radio)
        .read_codeplug()
        .unwrap()
        .project;
    radio.corrupt_write_checksum = true;
    let err = RadioClient::new(&mut radio)
        .write_codeplug(&project, None, None)
        .unwrap_err();
    assert!(err.to_string().contains("校驗和不符"), "{err}");
}

#[test]
fn a_silent_radio_gets_the_checklist() {
    let err = RadioClient::new(&mut Silent).read_codeplug().unwrap_err();
    assert!(
        matches!(&err, RadioError::Timeout { received: 0, message } if message.contains("對講機沒有任何回應"))
    );
}

#[test]
fn clock_rewrite_needs_a_byte_exact_rebuild() {
    let mut radio = radio();
    let read = RadioClient::new(&mut radio).read_codeplug().unwrap();
    assert_eq!(clock_write_project(&read).unwrap(), read.project);
    let mut odd = read.clone();
    let last = odd.stream.len() - 1;
    odd.stream[last] = 1; // padding the parser drops
    assert!(
        clock_write_project(&odd)
            .unwrap_err()
            .to_string()
            .contains("未寫入任何資料")
    );
}

#[test]
fn identity_mismatch_rules() {
    use opendrs_core::radio_link::{RadioIdentity, identity_mismatch};
    let project =
        opendrs_core::f136::load(common::fixtures().join("dh98vu_20250811.F136")).unwrap();
    let ident = |raw: &[u8]| RadioIdentity::parse(raw);
    assert_eq!(
        identity_mismatch(&project, &ident(b"308E.X\0+DRS-8500+136-480MHZ+")),
        None
    );
    assert_eq!(
        identity_mismatch(&project, &ident(b"+DRS-8600+136-480MHZ+")).unwrap(),
        "專案機型為 DRS-8500，對講機是 DRS-8600"
    );
    assert_eq!(
        identity_mismatch(&project, &ident(b"+DRS-8500+400-480MHZ+")).unwrap(),
        "專案頻段為 136-480 MHz，對講機是 400-480 MHz"
    );
    assert_eq!(
        identity_mismatch(&project, &ident(b"garbage")).unwrap(),
        "無法辨識對講機的機型或頻段"
    );
}
