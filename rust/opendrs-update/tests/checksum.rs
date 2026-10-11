//! The `.sha256` file and the digest the archive must have (test_update.py: "checksums").

mod common;

use common::{PLATFORM, release_json, sha256_hex};
use opendrs_update::{expected_sha256, parse_checksum_file, parse_release};
use serde_json::Value;

const SHA: &str = "862ce1e502caaa4d660a7fd17c920b2c80d40aca39a392b7cf63cce31355baca";

#[test]
fn checksum_file_tolerates_crlf_like_v010() {
    let text = format!("{SHA}  OpenDRS-0.1.0-windows-x64.zip\r\n"); // as Windows tools write it
    assert_eq!(
        parse_checksum_file(&text, "OpenDRS-0.1.0-windows-x64.zip").expect("found"),
        SHA
    );
    let err = parse_checksum_file(&text, "OpenDRS-0.2.0-windows-x64.zip").expect_err("other name");
    assert_eq!(
        err.to_string(),
        ".sha256 檔裡沒有 OpenDRS-0.2.0-windows-x64.zip 的校驗碼"
    );
}

#[test]
fn checksum_file_accepts_binary_mode_and_uppercase() {
    let upper = SHA.to_uppercase();
    let text = format!("# comment\n\n{upper} *OpenDRS-0.1.0-linux-x64.tar.gz\n");
    assert_eq!(
        parse_checksum_file(&text, "OpenDRS-0.1.0-linux-x64.tar.gz").expect("found"),
        SHA
    );
    assert!(parse_checksum_file(&format!("{}  a.zip", &SHA[1..]), "a.zip").is_err());
    assert!(parse_checksum_file("", "a.zip").is_err());
}

#[test]
fn expected_sha256_needs_agreeing_sources() {
    let zip_bytes = b"zip";
    let sha = sha256_hex(zip_bytes);
    let release =
        parse_release(&release_json("0.2.0", zip_bytes, None, true), PLATFORM).expect("ok");
    let line = format!("{sha}  OpenDRS-0.2.0-windows-x64.zip\n");
    assert_eq!(expected_sha256(&release, Some(&line)).expect("agree"), sha);
    assert_eq!(
        expected_sha256(&release, None).expect("GitHub's alone"),
        sha
    );
    let other = format!("{}  OpenDRS-0.2.0-windows-x64.zip\n", "1".repeat(64));
    let err = expected_sha256(&release, Some(&other)).expect_err("disagree");
    assert_eq!(
        err.to_string(),
        "GitHub 與 .sha256 檔的校驗碼不一致，不安裝"
    );
    let err = expected_sha256(&release, Some("nothing here")).expect_err("no line");
    assert!(err.to_string().contains("的校驗碼"));

    let bare_json = release_json("0.2.0", zip_bytes, Some(Value::Null), true);
    let bare = parse_release(&bare_json, PLATFORM).expect("ok");
    assert_eq!(bare.package.as_ref().expect("package").sha256, "");
    let err = expected_sha256(&bare, None).expect_err("no digest at all");
    assert_eq!(
        err.to_string(),
        "這個版本沒有提供 SHA-256 校驗碼，為了安全不自動安裝"
    );
    assert_eq!(
        expected_sha256(&bare, Some(&line)).expect("file alone"),
        sha
    );
}

#[test]
fn expected_sha256_without_a_package() {
    let release =
        parse_release(&release_json("0.2.0", b"zip", None, true), "plan9-other").expect("ok");
    let err = expected_sha256(&release, None).expect_err("no package");
    assert!(err.to_string().starts_with("v0.2.0 沒有這個平台（"));
}
