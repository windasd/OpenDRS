//! Download, verify and unpack next to the program (test_update.py: "prepare").

mod common;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;

use common::{PLATFORM, TempDir, commas, noise, opener_for, program_zip, release_json, sha256_hex};
use opendrs_update::http::{Fetcher, HttpRequest};
use opendrs_update::{
    MAX_CHECKSUM_BYTES, PreparedUpdate, Release, UpdateError, current_platform, parse_release,
    prepare_with, stale_staging,
};
use serde_json::Value;

/// The release and the files the fake server holds for it (`_served_release`).
fn served_release(
    zip_bytes: &[u8],
    checksum_line: Option<&str>,
) -> (Release, HashMap<String, Vec<u8>>) {
    let release =
        parse_release(&release_json("0.2.0", zip_bytes, None, true), PLATFORM).expect("ok");
    let package = release.package.clone().expect("package");
    let checksum = release.checksum.clone().expect("checksum");
    let line = checksum_line.map_or_else(
        || format!("{}  {}\n", sha256_hex(zip_bytes), package.name),
        str::to_string,
    );
    let files = HashMap::from([
        (package.url, zip_bytes.to_vec()),
        (checksum.url, line.into_bytes()),
    ]);
    (release, files)
}

fn run(
    release: &Release,
    target: &Path,
    fetcher: &dyn Fetcher,
) -> Result<PreparedUpdate, UpdateError> {
    prepare_with(release, target, fetcher, &mut |_, _| {}, &|| false)
}

fn staging(target: &Path) -> std::path::PathBuf {
    target.join(".opendrs-update-0.2.0")
}

fn urls(seen: &RefCell<Vec<HttpRequest>>) -> Vec<String> {
    seen.borrow().iter().map(|r| r.url.clone()).collect()
}

#[test]
fn prepare_downloads_verifies_and_unpacks() {
    let tmp = TempDir::new("prepare");
    let zip_bytes = program_zip(&[]);
    let (release, files) = served_release(&zip_bytes, None);
    let seen = RefCell::new(Vec::new());
    let mut progress = Vec::new();
    let prepared = prepare_with(
        &release,
        tmp.path(),
        &opener_for(&files, &seen),
        &mut |done, total| progress.push((done, total)),
        &|| false,
    )
    .expect("prepared");
    assert_eq!(prepared.version, "0.2.0");
    assert_eq!(prepared.target, tmp.path());
    assert_eq!(prepared.staging, staging(tmp.path()));
    assert_eq!(
        prepared.source,
        staging(tmp.path()).join("unpacked").join("OpenDRS")
    );
    assert_eq!(
        std::fs::read(prepared.source.join("OpenDRS.exe")).expect("exe"),
        b"new"
    );
    assert!(
        !prepared
            .staging
            .join(release.package.as_ref().expect("pkg").name.as_str())
            .exists()
    );
    let len = zip_bytes.len() as u64;
    assert_eq!(progress.last(), Some(&(len, len)));
    // The .sha256 file first, then the archive, both as binary downloads.
    let checksum_url = release.checksum.as_ref().expect("checksum").url.clone();
    assert_eq!(
        urls(&seen),
        [checksum_url, release.package.expect("pkg").url]
    );
    assert!(
        seen.borrow()
            .iter()
            .all(|r| r.header("Accept") == Some("application/octet-stream"))
    );
    assert_eq!(
        stale_staging(tmp.path()),
        std::slice::from_ref(&prepared.staging)
    );
}

#[test]
fn prepare_replaces_an_earlier_staging_folder() {
    let tmp = TempDir::new("prepare-again");
    let leftover = staging(tmp.path()).join("leftover.txt");
    std::fs::create_dir_all(leftover.parent().expect("parent")).expect("dir");
    std::fs::write(&leftover, b"old").expect("file");
    let (release, files) = served_release(&program_zip(&[]), None);
    let seen = RefCell::new(Vec::new());
    run(&release, tmp.path(), &opener_for(&files, &seen)).expect("prepared");
    assert!(!leftover.exists());
}

#[test]
fn prepare_refuses_a_corrupt_download() {
    let tmp = TempDir::new("prepare-corrupt");
    let zip_bytes = program_zip(&[]);
    let (release, mut files) = served_release(&zip_bytes, None);
    let mut corrupt = zip_bytes.clone();
    *corrupt.last_mut().expect("bytes") ^= 0x55; // same size, different bytes
    files.insert(release.package.clone().expect("pkg").url, corrupt);
    let seen = RefCell::new(Vec::new());
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("corrupt");
    assert_eq!(
        err.to_string(),
        "下載的檔案校驗碼不符（可能下載不完整或被竄改），未安裝"
    );
    assert!(!staging(tmp.path()).exists());
}

#[test]
fn prepare_refuses_disagreeing_checksums_before_downloading_the_archive() {
    let tmp = TempDir::new("prepare-disagree");
    let line = format!("{}  OpenDRS-0.2.0-windows-x64.zip\n", "1".repeat(64));
    let (release, files) = served_release(&program_zip(&[]), Some(&line));
    let seen = RefCell::new(Vec::new());
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("disagree");
    assert!(err.to_string().contains("不一致"));
    assert_eq!(seen.borrow().len(), 1);
    assert!(!staging(tmp.path()).exists());
}

#[test]
fn prepare_needs_a_checksum() {
    let tmp = TempDir::new("prepare-no-digest");
    let zip_bytes = program_zip(&[]);
    let data = release_json("0.2.0", &zip_bytes, Some(Value::Null), false);
    let release = parse_release(&data, PLATFORM).expect("ok");
    let files = HashMap::from([(release.package.clone().expect("pkg").url, zip_bytes)]);
    let seen = RefCell::new(Vec::new());
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("no digest");
    assert!(err.to_string().contains("沒有提供 SHA-256"));
    assert!(seen.borrow().is_empty());
}

#[test]
fn prepare_can_be_cancelled() {
    let tmp = TempDir::new("prepare-cancel");
    let (release, files) = served_release(&program_zip(&[]), None);
    let seen = RefCell::new(Vec::new());
    let err = prepare_with(
        &release,
        tmp.path(),
        &opener_for(&files, &seen),
        &mut |_, _| {},
        &|| true,
    )
    .expect_err("cancelled");
    assert!(matches!(err, UpdateError::Cancelled));
    assert_eq!(err.to_string(), "已取消");
    assert!(!staging(tmp.path()).exists());
}

#[test]
fn prepare_can_be_cancelled_during_the_archive() {
    let tmp = TempDir::new("prepare-cancel-archive");
    let (release, files) = served_release(&program_zip(&[]), None);
    let seen = RefCell::new(Vec::new());
    let asked = Cell::new(0);
    // The checksum file is one chunk; the next question is about the archive.
    let cancelled = || {
        asked.set(asked.get() + 1);
        asked.get() > 1
    };
    let err = prepare_with(
        &release,
        tmp.path(),
        &opener_for(&files, &seen),
        &mut |_, _| {},
        &cancelled,
    )
    .expect_err("cancelled");
    assert!(matches!(err, UpdateError::Cancelled));
    assert_eq!(seen.borrow().len(), 2);
    assert!(!staging(tmp.path()).exists());
}

#[test]
fn prepare_notices_an_incomplete_download() {
    let tmp = TempDir::new("prepare-short");
    let big = noise(5000); // incompressible, so the archive is larger than 1,234 bytes
    let zip_bytes = program_zip(&[("OpenDRS/licenses/big.txt", &big)]);
    let (release, mut files) = served_release(&zip_bytes, None);
    let short = zip_bytes[..1234].to_vec();
    files.insert(release.package.clone().expect("pkg").url, short);
    let seen = RefCell::new(Vec::new());
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("short");
    assert_eq!(
        err.to_string(),
        format!("下載不完整（1,234 / {} bytes）", commas(zip_bytes.len()))
    );
    assert!(!staging(tmp.path()).exists());
}

#[test]
fn prepare_refuses_an_oversized_checksum_file() {
    let tmp = TempDir::new("prepare-big-sha");
    let (release, mut files) = served_release(&program_zip(&[]), None);
    let huge = vec![b' '; MAX_CHECKSUM_BYTES as usize + 1];
    files.insert(release.checksum.clone().expect("checksum").url, huge);
    let seen = RefCell::new(Vec::new());
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("too big");
    assert_eq!(err.to_string(), "下載的檔案超過大小上限，已中止");
}

#[test]
fn prepare_reports_a_missing_archive() {
    let tmp = TempDir::new("prepare-404");
    let (release, mut files) = served_release(&program_zip(&[]), None);
    files.remove(&release.package.clone().expect("pkg").url);
    let seen = RefCell::new(Vec::new());
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("404");
    assert_eq!(err.to_string(), "下載更新：找不到（HTTP 404）");
    assert!(!staging(tmp.path()).exists());
}

#[test]
fn prepare_needs_an_archive_for_this_platform() {
    let tmp = TempDir::new("prepare-no-package");
    let data = release_json("0.2.0", b"zip", None, true);
    let release = parse_release(&data, "plan9-other").expect("ok");
    let seen = RefCell::new(Vec::new());
    let files = HashMap::new();
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("no package");
    assert_eq!(
        err.to_string(),
        format!(
            "v0.2.0 沒有這個平台（{}）的檔案可下載，請到發佈頁面手動下載",
            current_platform()
        )
    );
    assert!(seen.borrow().is_empty());
}

#[test]
fn prepare_needs_write_access() {
    let tmp = TempDir::new("prepare-readonly");
    let missing = tmp.path().join("no such folder");
    let (release, files) = served_release(&program_zip(&[]), None);
    let seen = RefCell::new(Vec::new());
    let err = run(&release, &missing, &opener_for(&files, &seen)).expect_err("not writable");
    assert_eq!(
        err.to_string(),
        format!(
            "沒有權限寫入程式所在的資料夾 {}；請手動下載並解壓縮",
            missing.display()
        )
    );
    assert!(seen.borrow().is_empty() && !missing.exists());
}

#[test]
fn prepare_refuses_asset_names_that_are_paths() {
    let tmp = TempDir::new("prepare-names");
    let (mut release, files) = served_release(&program_zip(&[]), None);
    if let Some(package) = release.package.as_mut() {
        package.name = "../OpenDRS.exe".into();
    }
    let seen = RefCell::new(Vec::new());
    let err = run(&release, tmp.path(), &opener_for(&files, &seen)).expect_err("path");
    assert_eq!(err.to_string(), "GitHub 回傳的發佈資料格式不符");
    assert!(seen.borrow().is_empty());
}

#[test]
fn stale_staging_lists_only_staging_folders() {
    let tmp = TempDir::new("stale");
    assert!(stale_staging(tmp.path()).is_empty());
    for dir in [".opendrs-update-0.1.0", ".opendrs-update-0.2.0", "other"] {
        std::fs::create_dir(tmp.path().join(dir)).expect("dir");
    }
    std::fs::write(tmp.path().join(".opendrs-update-x.probe"), b"").expect("file");
    assert_eq!(
        stale_staging(tmp.path()),
        [
            tmp.path().join(".opendrs-update-0.1.0"),
            tmp.path().join(".opendrs-update-0.2.0")
        ]
    );
    assert!(stale_staging(&tmp.path().join("missing")).is_empty());
}
