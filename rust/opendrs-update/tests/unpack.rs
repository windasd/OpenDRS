//! Unpacking the release zip (test_update.py: "unpacking").

mod common;

use std::io::{Cursor, Write};

use common::{TempDir, program_zip, zip_of};
use opendrs_update::{MAX_UNPACKED_BYTES, extract_program};

#[test]
fn extract_program_unpacks_the_release() {
    let tmp = TempDir::new("extract");
    let archive = tmp.path().join("ok.zip");
    std::fs::write(&archive, program_zip(&[])).expect("zip");
    let source = extract_program(&archive, &tmp.path().join("out")).expect("ok");
    assert_eq!(source, tmp.path().join("out").join("OpenDRS"));
    assert_eq!(
        std::fs::read(source.join("OpenDRS.exe")).expect("exe"),
        b"new"
    );
    assert_eq!(
        std::fs::read(source.join("opendrs-rs.exe")).expect("cli"),
        b"cli"
    );
    assert_eq!(
        std::fs::read(source.join("README.txt")).expect("readme"),
        b"readme"
    );
    assert!(
        source
            .join("licenses")
            .join("THIRD_PARTY_LICENSES.txt")
            .is_file()
    );
}

#[test]
fn extract_keeps_directory_entries() {
    let tmp = TempDir::new("extract-dirs");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.add_directory("OpenDRS/", options).expect("dir");
    zip.add_directory("OpenDRS/licenses/", options)
        .expect("dir");
    zip.start_file("OpenDRS/OpenDRS.exe", options)
        .expect("file");
    zip.write_all(b"new").expect("data");
    let bytes = zip.finish().expect("zip").into_inner();
    let archive = tmp.path().join("dirs.zip");
    std::fs::write(&archive, bytes).expect("zip");
    let source = extract_program(&archive, &tmp.path().join("out")).expect("ok");
    assert!(source.join("licenses").is_dir());
}

#[test]
fn extract_rejects_paths_outside_the_program() {
    for bad in [
        "OpenDRS/../evil.exe",
        "/OpenDRS/abs.exe",
        "Other/OpenDRS.exe",
        "OpenDRS/..\\evil.exe",
        "OpenDRS/C:evil.exe",
        "C:/OpenDRS/evil.exe",
        "OpenDRS/./../evil.exe",
    ] {
        let tmp = TempDir::new("extract-bad");
        let archive = tmp.path().join("bad.zip");
        std::fs::write(&archive, program_zip(&[(bad, b"x")])).expect("zip");
        let err = extract_program(&archive, &tmp.path().join("out")).expect_err(bad);
        assert_eq!(err.to_string(), format!("更新檔含有不允許的路徑：{bad}"));
        assert!(!tmp.path().join("evil.exe").exists(), "{bad}");
        assert!(
            !tmp.path().join("out").exists(),
            "nothing unpacked for {bad}"
        );
    }
}

#[test]
fn extract_rejects_symlinks() {
    let tmp = TempDir::new("extract-link");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("OpenDRS/OpenDRS.exe", options)
        .expect("file");
    zip.write_all(b"new").expect("data");
    zip.add_symlink("OpenDRS/licenses", "../../outside", options)
        .expect("symlink");
    let archive = tmp.path().join("link.zip");
    std::fs::write(&archive, zip.finish().expect("zip").into_inner()).expect("zip");
    let err = extract_program(&archive, &tmp.path().join("out")).expect_err("symlink");
    assert_eq!(err.to_string(), "更新檔含有不允許的路徑：OpenDRS/licenses");
}

#[test]
fn extract_rejects_incomplete_or_broken_zips() {
    let tmp = TempDir::new("extract-broken");
    let a = tmp.path().join("a.zip");
    std::fs::write(&a, zip_of(&[("OpenDRS/README.txt", b"no exe".as_slice())])).expect("zip");
    let err = extract_program(&a, &tmp.path().join("out")).expect_err("no exe");
    assert_eq!(err.to_string(), "更新檔內容不完整（缺少 OpenDRS.exe）");
    // The Python build's layout is no Rust build either.
    let python = zip_of(&[("OpenDRS/_internal/python313.dll", b"dll".as_slice())]);
    std::fs::write(&a, python).expect("zip");
    assert!(extract_program(&a, &tmp.path().join("out")).is_err());

    let b = tmp.path().join("b.zip");
    std::fs::write(&b, b"not a zip").expect("file");
    let err = extract_program(&b, &tmp.path().join("out")).expect_err("not a zip");
    assert_eq!(err.to_string(), "更新檔不是有效的 zip");
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn extract_rejects_damaged_member_data() {
    let tmp = TempDir::new("extract-crc");
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let stored =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    zip.start_file("OpenDRS/OpenDRS.exe", stored).expect("file");
    zip.write_all(b"MARKER-new-program").expect("data");
    let mut bytes = zip.finish().expect("zip").into_inner();
    let at = bytes
        .windows(6)
        .position(|w| w == b"MARKER")
        .expect("stored data");
    bytes[at] = b'X'; // the CRC no longer matches
    let archive = tmp.path().join("crc.zip");
    std::fs::write(&archive, bytes).expect("zip");
    let err = extract_program(&archive, &tmp.path().join("out")).expect_err("bad crc");
    assert_eq!(err.to_string(), "更新檔不是有效的 zip");
}

/// A zip whose central directory claims `size` bytes for its one member.
fn zip_claiming(size: u32) -> Vec<u8> {
    let mut bytes = zip_of(&[("OpenDRS/OpenDRS.exe", b"new".as_slice())]);
    let central = bytes
        .windows(4)
        .position(|w| w == b"PK\x01\x02")
        .expect("central directory");
    bytes[central + 24..central + 28].copy_from_slice(&size.to_le_bytes());
    bytes
}

#[test]
fn extract_refuses_archives_that_unpack_too_large() {
    let tmp = TempDir::new("extract-large");
    let archive = tmp.path().join("large.zip");
    let claimed = u32::try_from(MAX_UNPACKED_BYTES + 1).expect("fits");
    std::fs::write(&archive, zip_claiming(claimed)).expect("zip");
    let err = extract_program(&archive, &tmp.path().join("out")).expect_err("too large");
    assert_eq!(err.to_string(), "更新檔解壓後過大");
    assert!(!tmp.path().join("out").exists());
}

#[test]
fn extract_refuses_members_larger_than_declared() {
    let tmp = TempDir::new("extract-lie");
    let archive = tmp.path().join("lie.zip");
    std::fs::write(&archive, zip_claiming(1)).expect("zip");
    let err = extract_program(&archive, &tmp.path().join("out")).expect_err("lying size");
    let text = err.to_string();
    assert!(
        text == "更新檔解壓後過大" || text == "更新檔不是有效的 zip",
        "{text}"
    );
}

#[test]
fn extract_refuses_items_this_version_does_not_install() {
    let tmp = TempDir::new("extract-unknown");
    let archive = tmp.path().join("unknown.zip");
    std::fs::write(&archive, program_zip(&[("OpenDRS/new-helper.dll", b"x")])).expect("zip");
    let err = extract_program(&archive, &tmp.path().join("out")).expect_err("unknown item");
    assert_eq!(
        err.to_string(),
        "更新檔含有這個版本不認得的項目：OpenDRS/new-helper.dll，請到發佈頁面手動下載"
    );
}

#[test]
fn extract_refuses_names_windows_would_merge() {
    let tmp = TempDir::new("extract-alias");
    // a trailing dot, a case variant of THIRD_PARTY_LICENSES.txt, a device name
    for (n, bad) in [
        "OpenDRS/OpenDRS.exe.",
        "OpenDRS/licenses/third_party_licenses.TXT",
        "OpenDRS/licenses/NUL",
    ]
    .into_iter()
    .enumerate()
    {
        let archive = tmp.path().join(format!("alias{n}.zip"));
        std::fs::write(&archive, program_zip(&[(bad, b"x")])).expect("zip");
        let err = extract_program(&archive, &tmp.path().join(format!("out{n}"))).expect_err(bad);
        assert_eq!(
            err.to_string(),
            format!("更新檔含有不允許的路徑：{bad}"),
            "{bad}"
        );
    }
}
