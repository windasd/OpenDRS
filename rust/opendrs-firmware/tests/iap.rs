//! IAP firmware upgrade: paging, command bytes, and a full run against a simulated bootloader
//! (port of tests/test_iap.py).

mod common;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use common::{ScriptedTransport, Shared, analysis_dir, fixtures, pattern};
use opendrs_core::radio_link::Transport;
use opendrs_firmware::iap::{IapClient, IapError, IapImage, PAD_BYTE, PAGE_SIZE};
use opendrs_firmware::iap_sim::SimulatedBootloader;
use opendrs_firmware::sha256_hex;

const VENDOR_TX_SHA256: &str = "04e16d9c9daa9591f00e8bf833701f8d73cebcfefbd753d9029f2d3fff064e2e";

fn image(size: usize) -> Vec<u8> {
    pattern(size, 3)
}

fn upgrade_error<T: Transport>(transport: &mut T, img: &IapImage) -> IapError {
    IapClient::new(transport)
        .upgrade(img)
        .expect_err("upgrade should fail")
}

fn boot(configure: impl FnOnce(&mut SimulatedBootloader)) -> SimulatedBootloader {
    let mut x = SimulatedBootloader::default();
    configure(&mut x);
    x
}

#[test]
fn paging_matches_the_vendor_tool() {
    let data = image(PAGE_SIZE * 2 + 5);
    let img = IapImage::from_bytes(&data).unwrap();
    assert_eq!((img.last_page(), img.page_count()), (2, 3));
    let mut last = data[PAGE_SIZE * 2..].to_vec();
    last.resize(PAGE_SIZE, 0);
    assert_eq!(img.page(2).unwrap(), last);
    // exact multiple: the vendor tool still sends len / 2048 + 1 pages
    let exact = IapImage::from_bytes(&image(PAGE_SIZE * 2)).unwrap();
    assert_eq!(exact.last_page(), 2);
    assert_eq!(exact.page(2).unwrap(), vec![0; PAGE_SIZE]);
    assert_eq!(
        img.erase_command(),
        [&b"Erase      "[..], &[0; 4], &2u32.to_be_bytes()].concat()
    );
    assert_eq!(img.stream().len(), 19 + 3 * PAGE_SIZE);
}

#[test]
fn rejects_bad_input() {
    assert!(IapImage::from_bytes(b"").unwrap_err().0.contains('空'));
    let big = vec![0; 4 * 1024 * 1024 + 1];
    assert!(IapImage::from_bytes(&big).unwrap_err().0.contains("太大"));
    let err = IapImage::from_bytes_at(b"x", -1, PAD_BYTE).unwrap_err();
    assert!(err.0.contains("位址"));
}

#[test]
fn full_upgrade_against_simulated_bootloader() {
    let data = image(PAGE_SIZE * 5 + 1234);
    let img = IapImage::from_bytes_at(&data, 0x4000, PAD_BYTE).unwrap();
    let mut boot = SimulatedBootloader::default();
    let mut steps = Vec::new();
    let result = IapClient::new(&mut boot)
        .with_progress(|d, t, s| steps.push((d, t, s.to_owned())))
        .upgrade(&img)
        .unwrap();
    assert!(boot.finished && boot.address == Some(0x4000) && boot.last_page == Some(5));
    assert_eq!(&boot.flash[..data.len()], &data[..]);
    assert_eq!(boot.flash.len(), 6 * PAGE_SIZE);
    assert_eq!(boot.sent, img.stream());
    let flash_sum = boot
        .flash
        .iter()
        .fold(0u32, |a, &b| a.wrapping_add(u32::from(b)));
    assert!(result.checksum == img.checksum() && img.checksum() == flash_sum);
    assert_eq!(result.pages, 6);
    assert!(result.erase_reply.contains("ok"));
    assert_eq!(steps.last().unwrap(), &(6, 6, "升級完成".to_owned()));
}

fn checksum_reply(img: &IapImage) -> Vec<Vec<u8>> {
    let tail = [&b"ChecksumI"[..], &img.checksum().to_be_bytes()].concat();
    vec![
        tail[..5].to_vec(),
        Vec::new(),
        tail[5..11].to_vec(),
        tail[11..].to_vec(),
    ]
}

fn chunks(parts: &[&[u8]]) -> Vec<Vec<u8>> {
    parts.iter().map(|p| p.to_vec()).collect()
}

#[test]
fn replies_split_across_reads_and_empty_reads() {
    let img = IapImage::from_bytes(&image(PAGE_SIZE + 3)).unwrap();
    let mut script = chunks(&[b"", b"IAP Erase o", b"", b"k\r\n", b"k", b"yd"]);
    script.extend(checksum_reply(&img));
    let mut t = ScriptedTransport::new(script, None, false);
    let result = IapClient::new(&mut t).upgrade(&img).unwrap();
    assert_eq!(result.checksum, img.checksum());
    assert_eq!(t.sent.concat(), img.stream());
}

#[test]
fn last_page_may_answer_with_the_checksum_only() {
    let img = IapImage::from_bytes(&image(PAGE_SIZE + 3)).unwrap();
    let tail = [&b"ChecksumI"[..], &img.checksum().to_be_bytes()].concat();
    let mut t = ScriptedTransport::new(vec![b"ok".to_vec(), b"kyd".to_vec(), tail], None, false);
    assert_eq!(IapClient::new(&mut t).upgrade(&img).unwrap().pages, 2);
}

#[test]
fn silence_after_erase_times_out_and_says_it_was_sent() {
    let mut t = ScriptedTransport::new(Vec::new(), None, false);
    let mut client = IapClient::new(&mut t);
    client.erase_timeout = Duration::from_millis(50);
    let err = client
        .upgrade(&IapImage::from_bytes(&image(100)).unwrap())
        .unwrap_err();
    assert!(err.message.contains("清除指令已送出，但沒有收到"));
    assert!(err.erase_sent);
}

#[test]
fn no_reply_to_erase() {
    let mut boot = boot(|x| x.silent = true);
    let err = upgrade_error(&mut boot, &IapImage::from_bytes(&image(100)).unwrap());
    assert!(err.message.contains("升級（IAP）模式"));
    assert!(err.erase_sent); // sent, even though nothing came back
}

#[test]
fn failures_after_erase_say_so() {
    let cases = [
        (
            boot(|x| x.stop_acking_after = Some(2)),
            "第 3/4 頁沒有收到確認",
        ),
        (
            boot(|x| x.bad_ack = Some(b"err".to_vec())),
            "第 1/4 頁沒有收到確認",
        ),
        (boot(|x| x.corrupt_checksum = true), "校驗和不符"),
    ];
    let img = IapImage::from_bytes(&image(PAGE_SIZE * 3 + 1)).unwrap();
    for (mut boot, text) in cases {
        let err = upgrade_error(&mut boot, &img);
        assert!(err.message.contains(text), "{}", err.message);
        if text == "校驗和不符" {
            assert!(err.message.contains("請勿直接開機"));
        }
        assert!(err.erase_sent && err.message.contains("升級模式"));
    }
}

#[test]
fn cancel_before_erase_sends_nothing() {
    let mut boot = SimulatedBootloader::default();
    let err = IapClient::new(&mut boot)
        .with_cancel(|| true)
        .upgrade(&IapImage::from_bytes(&image(100)).unwrap())
        .unwrap_err();
    assert!(err.message.contains("未送出任何資料"));
    assert!(!err.erase_sent && boot.sent.is_empty());
}

#[test]
fn cancel_while_waiting_for_erase() {
    let mut t = ScriptedTransport::new(Vec::new(), None, false);
    let mut calls = [false, false, false, true].into_iter(); // before erase, then while waiting
    let err = IapClient::new(&mut t)
        .with_cancel(move || calls.next().unwrap_or(true))
        .upgrade(&IapImage::from_bytes(&image(100)).unwrap())
        .unwrap_err();
    assert!(err.message.contains("已停止"));
    assert!(err.erase_sent && t.sent.len() == 1);
}

#[test]
fn cancel_between_pages() {
    let boot = Rc::new(RefCell::new(SimulatedBootloader::default()));
    let watched = Rc::clone(&boot);
    let mut transport = Shared(Rc::clone(&boot));
    let err = IapClient::new(&mut transport)
        .with_cancel(move || watched.borrow().flash.len() >= 2 * PAGE_SIZE)
        .upgrade(&IapImage::from_bytes(&image(PAGE_SIZE * 3 + 1)).unwrap())
        .unwrap_err();
    assert!(err.message.contains("已停止"));
    assert!(err.erase_sent && boot.borrow().flash.len() == 2 * PAGE_SIZE);
}

#[test]
fn serial_error_reports_whether_erase_was_sent() {
    let cases = [
        (None, true, false),    // failed before anything was written
        (Some(0), false, true), // the erase command itself may be partly out: assume it was sent
        (Some(2), false, true),
    ];
    let img = IapImage::from_bytes(&image(PAGE_SIZE * 3)).unwrap();
    for (fail_on_write, fail_reset, erase_sent) in cases {
        let script = chunks(&[b"ok", b"kyd", b"kyd"]);
        let mut t = ScriptedTransport::new(script, fail_on_write, fail_reset);
        let err = upgrade_error(&mut t, &img);
        assert!(err.message.contains("連線中斷"), "{}", err.message);
        assert_eq!(err.erase_sent, erase_sent);
    }
}

#[test]
fn tolerates_extra_text_in_replies() {
    let mut boot = boot(|x| {
        x.erase_reply = b"IAP V1.2\r\nErase ok\r\n".to_vec();
        x.ack_suffix = b"\r\n".to_vec()
    });
    let img = IapImage::from_bytes(&image(PAGE_SIZE * 2 + 9)).unwrap();
    let result = IapClient::new(&mut boot).upgrade(&img).unwrap();
    assert_eq!(result.checksum, img.checksum());
    assert!(result.erase_reply.contains("V1.2"));
}

/// `vendor_tx_*` is every byte IAP+Flash.exe wrote for `synth_*` (all bytes non-zero, so the
/// padding is visible; 6144 = 3 * 2048 shows the extra zero page).
#[test]
fn identical_to_the_vendor_tool() {
    for size in [5000, 6144] {
        let dir = fixtures().join("iap");
        let data = std::fs::read(dir.join(format!("synth_{size}.bin"))).unwrap();
        let sent = std::fs::read(dir.join(format!("vendor_tx_synth_{size}.bin"))).unwrap();
        assert_eq!(
            IapImage::from_bytes(&data).unwrap().stream(),
            sent,
            "{size}"
        );
    }
}

#[test]
fn vendor_image_matches_the_vendor_tool() {
    let vendor = analysis_dir().join("C7000_DR8500UV_4819_03_136174_400480.bin");
    let Ok(data) = std::fs::read(&vendor) else {
        eprintln!("skipped: vendor firmware not on this machine");
        return;
    };
    let img = IapImage::from_bytes(&data).unwrap();
    assert_eq!(data.len(), 841_904);
    assert_eq!((img.last_page(), img.page_count()), (411, 412));
    assert_eq!(
        img.erase_command(),
        [&b"Erase      "[..], &[0, 0, 0, 0], &[0, 0, 0x01, 0x9b]].concat()
    );
    // SHA-256 of the 843,795 bytes IAP+Flash.exe wrote for this file
    assert_eq!(sha256_hex(&img.stream()), VENDOR_TX_SHA256);
    assert_eq!(img.checksum(), 0x0498_A486); // what the vendor tool showed as its checksum
}
