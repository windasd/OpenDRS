//! Record-read firmware patch (port of the non-GUI tests in tests/test_fwpatch.py).

mod common;

use std::path::PathBuf;

use common::analysis_dir;
use opendrs_firmware::firmware_check::{known_patched_image, known_vendor_image};
use opendrs_firmware::fwpatch::{
    A_BASE, KNOWN_BASES, RECORD_READ, RecordReadStatus, is_record_read_patched, known_base,
    patch_record_read, record_read_status,
};
use opendrs_firmware::sha256_hex;

/// Flashed and verified on a radio.
const PATCHED_03: &str = "86d1ea6579eeabb4d1c2dee451bc3f89f3583a3c444d9c592fe26e7e8b3cb252";
const PATCHED_05: &str = "8f5a225f5dbe76706830ce99331e40605b55549695e30e7ab0f05b188ba3e7fa";
const SIZE: usize = 841_904;

fn vendor_03() -> PathBuf {
    analysis_dir().join("C7000_DR8500UV_4819_03_136174_400480.bin")
}

fn vendor_05() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(
        PathBuf::from(home)
            .join("Downloads")
            .join("hora dh-98vu")
            .join("韌體")
            .join("C7000_DR8500UV_4819_05_144146_430440.bin"),
    )
}

fn offset(va: u32) -> usize {
    (va - A_BASE) as usize
}

/// An image that only has the original bytes at the patch sites (everything else zero).
fn synthetic(extra: &[(u32, &[u8])]) -> Vec<u8> {
    let mut img = vec![0u8; SIZE];
    let sites = RECORD_READ.iter().map(|s| (s.va, s.original));
    for (va, data) in sites.chain(extra.iter().copied()) {
        img[offset(va)..offset(va) + data.len()].copy_from_slice(data);
    }
    img
}

#[test]
fn patch_sites_are_known_hashes() {
    assert!(
        KNOWN_BASES
            .iter()
            .all(|(sha, _)| known_vendor_image(sha).is_some())
    );
    assert!(known_patched_image(PATCHED_03).is_some() && known_patched_image(PATCHED_05).is_some());
    assert_eq!(
        RECORD_READ.iter().map(|s| s.patched.len()).sum::<usize>(),
        30
    );
}

#[test]
fn synthetic_image_is_patched_after_the_safety_scans() {
    let img = synthetic(&[]);
    assert_eq!(record_read_status(&img), RecordReadStatus::Patchable);
    let result = patch_record_read(&img).unwrap();
    assert!(result.changed == 30 && result.base_name.is_empty());
    assert!(is_record_read_patched(&result.data));
    assert_eq!(record_read_status(&result.data), RecordReadStatus::Patched);
    let err = patch_record_read(&result.data).unwrap_err();
    assert!(err.0.contains("已經是錄音讀取版"));
}

#[test]
fn refuses_other_code_and_live_code_in_the_cave() {
    let mut wrong = synthetic(&[]);
    wrong[offset(0x0302_6E5C)] ^= 1;
    assert!(
        record_read_status(&wrong)
            .to_string()
            .contains("修補點不符")
    );
    assert!(
        patch_record_read(&wrong)
            .unwrap_err()
            .0
            .contains("修補點不符")
    );
    // a bsr32 somewhere else that calls into the dead code: the cave would not be dead
    let (at, target) = (0x0303_0000u32, 0x0302_7030u32);
    let off = (target.wrapping_sub(at) as i32 / 2) as u32 & 0x3FF_FFFF;
    let word = 0xE000_0000 | off;
    let call = [
        ((word >> 16) as u16).to_le_bytes(),
        ((word & 0xFFFF) as u16).to_le_bytes(),
    ]
    .concat();
    let err = patch_record_read(&synthetic(&[(at, &call)])).unwrap_err();
    assert!(err.0.contains("進入被覆寫的區域"), "{}", err.0);
    assert!(record_read_status(&[0; 100]).to_string().contains("太小"));
}

#[test]
fn vendor_03_gives_the_image_verified_on_the_radio() {
    let Ok(data) = std::fs::read(vendor_03()) else {
        eprintln!("skipped: vendor firmware not on this machine");
        return;
    };
    let result = patch_record_read(&data).unwrap();
    assert_eq!(result.sha256, PATCHED_03);
    assert_eq!(
        result.base_name,
        vendor_03().file_name().unwrap().to_string_lossy()
    );
}

#[test]
fn vendor_05_ham_band_build() {
    let Some(Ok(data)) = vendor_05().map(std::fs::read) else {
        eprintln!("skipped: ham-band vendor firmware not on this machine");
        return;
    };
    assert!(known_base(&sha256_hex(&data)).is_some());
    let result = patch_record_read(&data).unwrap();
    assert!(result.sha256 == PATCHED_05 && result.changed == 30);
}
