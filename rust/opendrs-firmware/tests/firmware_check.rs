//! Pre-flight firmware checks (port of the non-GUI tests in tests/test_firmware_check.py).

mod common;

use std::path::{Path, PathBuf};

use common::{analysis_dir, temp_dir};
use opendrs_firmware::firmware_check::{
    BLOCKED_IMAGES, Finding, FirmwareFile, InspectError, Level, blocked_image, check, inspect,
    known_patched_image, known_vendor_image, normalise_firmware_id, rf_chip, same_build, verdict,
};

const RADIO: &str = "308E.BBKTARSCB.003";
const RECORD_PATCH_SHA256: &str =
    "86d1ea6579eeabb4d1c2dee451bc3f89f3583a3c444d9c592fe26e7e8b3cb252";

fn vendor() -> PathBuf {
    analysis_dir().join("C7000_DR8500UV_4819_03_136174_400480.bin")
}

fn image_file(dir: &Path, firmware_id: &[u8], date: &[u8], name: &str) -> PathBuf {
    let path = dir.join(name);
    let data = [
        &[0u8; 64][..],
        firmware_id,
        &[0; 32],
        date,
        b"\x0017:02:38",
        &[0xff; 64],
    ]
    .concat();
    std::fs::write(&path, data).unwrap();
    path
}

fn inspected(dir: &Path, firmware_id: &[u8], name: &str) -> FirmwareFile {
    inspect(&image_file(dir, firmware_id, b"Mar  9 2024", name)).unwrap()
}

#[test]
fn inspect_reads_id_and_date() {
    let dir = temp_dir("inspect");
    let img = inspected(&dir, b"308E.B6.TARSCB.003.", "fw.bin");
    assert_eq!(
        (
            img.firmware_id.as_str(),
            img.build_date.as_str(),
            img.hardware()
        ),
        ("308E.B6.TARSCB.003", "Mar 9 2024", "308E")
    );
    assert!(img.sha256.len() == 64 && img.size > 0);
    let blank = inspect(&image_file(&dir, b"no id here", b"", "blank.bin")).unwrap();
    assert!(blank.firmware_id.is_empty() && blank.build_date.is_empty());
}

#[test]
fn rf_chip_codes_normalise_to_the_template() {
    for (radio_id, chip) in [
        ("308E.BBKTARSCB.003", "BK4819"),
        ("308E.BAxTARSCB.003", "RDA1846／AT1846S"),
        ("308E.B6.TARSCB.003", "0x6818"),
        ("308E.B64TARSCB.003", "0x7813"),
    ] {
        assert_eq!(normalise_firmware_id(radio_id), "308E.B6.TARSCB.003");
        assert_eq!(rf_chip(radio_id), Some(chip));
        assert!(same_build("308E.B6.TARSCB.003", radio_id));
        assert!(same_build(radio_id, "308E.B6.TARSCB.003"));
    }
}

#[test]
fn unknown_chip_code_is_a_real_difference() {
    for unknown in [
        "308E.BXYTARSCB.003",
        "308E.B6xTARSCB.003",
        "308E.CBKTARSCB.003",
        "308EBBKTARSCB",
        "",
    ] {
        assert_eq!(normalise_firmware_id(unknown), unknown);
        assert_eq!(rf_chip(unknown), None);
    }
    assert!(!same_build("308E.B6.TARSCB.003", "308E.BXYTARSCB.003"));
    assert!(!same_build("308E.B6.TARSCB.003", "308E.BBKTARSCB.004")); // another version
    assert!(!same_build("", "") && !same_build("308E.B6.TARSCB.003", ""));
    // the slot follows the hardware prefix
    assert_eq!(normalise_firmware_id("1234.BAxQ"), "1234.B6.Q");
}

#[test]
fn verdicts() {
    let dir = temp_dir("verdicts");
    let same = inspected(&dir, b"308E.BBKTARSCB.003.", "same.bin");
    let findings = check(&same, RADIO, true, "Mar 9 2024");
    assert_eq!(verdict(&findings), Level::Ok);
    assert!(findings.iter().any(|f| f.text.contains("相同")));
    let template = inspected(&dir, b"308E.B6.TARSCB.003.", "b6.bin");
    let findings = check(&template, RADIO, true, "Mar 9 2024");
    // the same build: the radio filled in BK for its BK4819
    assert_eq!(verdict(&findings), Level::Ok);
    assert_eq!(
        findings[0].text,
        "與對講機目前韌體為同一版本（對講機回報的 BBK 是開機時偵測到 BK4819 射頻晶片後\
         填入的代碼，檔案內為樣板 B6.）"
    );
    let newer = inspected(&dir, b"308E.B6.TARSCB.004.", "b6-004.bin");
    assert_eq!(
        verdict(&check(&newer, RADIO, true, "Mar 9 2024")),
        Level::Warn
    );
    let unknown_chip = inspected(&dir, b"308E.BXYTARSCB.003.", "xy.bin");
    let findings = check(&unknown_chip, RADIO, true, "Mar 9 2024");
    assert_eq!(verdict(&findings), Level::Warn);
    assert!(findings[0].text.contains("版本不同"));
    let other = inspected(&dir, b"412A.XX.TARSCB.001.", "other.bin");
    assert_eq!(verdict(&check(&other, RADIO, true, "")), Level::Block);
    let unknown_radio = check(&template, "", true, "");
    assert_eq!(verdict(&unknown_radio), Level::Warn);
    assert!(unknown_radio[0].text.contains("尚未從對講機讀取"));
    let no_backup = check(&same, RADIO, false, "Mar 9 2024");
    assert_eq!(verdict(&no_backup), Level::Warn);
    assert!(no_backup.last().unwrap().text.contains("備份"));
    let older = check(&same, RADIO, true, "Dec 19 2023");
    assert_eq!(verdict(&older), Level::Warn);
    assert!(older[older.len() - 2].text.contains("編譯日期不同"));
    let blank = inspected(&dir, b"nothing", "blank.bin");
    assert_eq!(verdict(&check(&blank, RADIO, true, "")), Level::Block);
}

#[test]
fn too_large_file_is_rejected() {
    let dir = temp_dir("too_large");
    let big = dir.join("big.bin");
    std::fs::write(&big, vec![0u8; 4 * 1024 * 1024 + 1]).unwrap();
    match inspect(&big) {
        Err(InspectError::TooLarge(message)) => assert!(message.contains("太大")),
        other => panic!("expected TooLarge, got {other:?}"),
    }
    assert!(matches!(
        inspect(&dir.join("missing.bin")),
        Err(InspectError::Io(_))
    ));
}

#[test]
fn vendor_file_and_band_patches() {
    if !vendor().is_file() {
        eprintln!("skipped: vendor firmware not on this machine");
        return;
    }
    let img = inspect(&vendor()).unwrap();
    assert!(known_vendor_image(&img.sha256).is_some());
    assert_eq!(img.firmware_id, "308E.B6.TARSCB.003");
    let findings = check(&img, RADIO, true, "Mar 9 2024");
    // the radio's BBK is this build with the BK4819 code filled in
    assert_eq!(verdict(&findings), Level::Ok);
    assert!(findings.iter().any(|f| f.text.contains("原廠")));
    assert!(
        findings
            .iter()
            .any(|f| f.text.contains("編譯日期與對講機目前韌體相同"))
    );
    assert!(
        findings
            .iter()
            .any(|f| f.text.contains("同一版本") && f.text.contains("BK4819"))
    );
    // no backup today
    assert_eq!(
        verdict(&check(&img, RADIO, false, "Mar 9 2024")),
        Level::Warn
    );
    let folder = vendor().parent().unwrap().to_path_buf();
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !(name.starts_with("DR8500UV_136-180_400-520") && name.ends_with(".bin")) {
            continue;
        }
        let p = inspect(&path).unwrap();
        let band = check(&p, RADIO, true, "Mar 9 2024");
        assert!(blocked_image(&p.sha256).is_some(), "{name}");
        assert_eq!(verdict(&band), Level::Block);
        assert!(
            band.iter()
                .any(|f| f.level == Level::Block && f.text.contains("頻段限制被放寬"))
        );
    }
}

fn image_with(sha256: &str) -> FirmwareFile {
    FirmwareFile {
        path: PathBuf::from("fw.bin"),
        size: 1024,
        sha256: sha256.to_owned(),
        firmware_id: "308E.B6.TARSCB.003".into(),
        build_date: "Mar 9 2024".into(),
    }
}

fn patch_findings(radio: &str, date: &str) -> Vec<Finding> {
    check(&image_with(RECORD_PATCH_SHA256), radio, true, date)
        .into_iter()
        .filter(|f| f.text.contains("錄音讀取修補版"))
        .collect()
}

#[test]
fn record_read_patch_is_an_untested_warning() {
    let (_, vendor_file) = known_patched_image(RECORD_PATCH_SHA256).expect("known patch");
    let findings = check(&image_with(RECORD_PATCH_SHA256), RADIO, true, "Mar 9 2024");
    assert_eq!(verdict(&findings), Level::Warn); // needs the risk tick, but is not blocked
    let patch = patch_findings(RADIO, "Mar 9 2024");
    assert!(patch.len() == 1 && patch[0].level == Level::Warn);
    let text = &patch[0].text;
    assert!(text.contains("30 bytes") && text.contains(RADIO) && text.contains("實機"));
    assert!(text.contains(vendor_file) && text.contains("再刷一次") && !text.contains("原廠提供"));
    // BBK = B6 + BK4819 code
    assert!(
        findings
            .iter()
            .any(|f| f.level == Level::Ok && f.text.contains("同一版本"))
    );
    let cases = [
        ("", "", "無法確認"),
        (RADIO, "", "無法確認"),
        ("308E.B6.TARSCB.004", "Mar 9 2024", "不是同一版本"),
        (RADIO, "Dec 19 2023", "不是同一版本"),
    ];
    for (radio, date, wording) in cases {
        let cautious = patch_findings(radio, date);
        assert!(cautious.len() == 1 && cautious[0].level == Level::Warn);
        let text = &cautious[0].text;
        assert!(text.contains("實機") && text.contains(wording) && text.contains("原廠提供"));
        assert!(!text.contains("再刷一次"));
    }
    let band = check(&image_with(BLOCKED_IMAGES[0].0), RADIO, true, "Mar 9 2024");
    assert_eq!(verdict(&band), Level::Block);
    assert!(!band.iter().any(|f| f.text.contains("錄音讀取修補版")));
    let plain = check(&image_with(&"0".repeat(64)), RADIO, true, "Mar 9 2024");
    assert!(!plain.iter().any(|f| f.text.contains("錄音讀取修補版")));
}

#[test]
fn record_read_patch_file() {
    let path = analysis_dir().join("DR8500UV_B6_record_read.bin");
    if !path.is_file() {
        eprintln!("skipped: record-read patched firmware not on this machine");
        return;
    }
    let img = inspect(&path).unwrap();
    let (_, vendor_file) = known_patched_image(&img.sha256).expect("known patch");
    assert_eq!(img.firmware_id, "308E.B6.TARSCB.003");
    let findings = check(&img, RADIO, true, "Mar 9 2024");
    // not BLOCK: the dialog's explicit confirmation applies
    assert_eq!(verdict(&findings), Level::Warn);
    assert!(findings.iter().any(|f| f.level == Level::Warn
        && f.text.contains("錄音讀取修補版")
        && f.text.contains(RADIO)));
    // the way back is the vendor file in the same folder
    assert_eq!(vendor_file, vendor().file_name().unwrap().to_string_lossy());
    assert!(
        findings
            .iter()
            .any(|f| f.text.contains(vendor_file) && f.text.contains("實機"))
    );
}
