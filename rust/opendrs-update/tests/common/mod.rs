//! Shared helpers for the integration tests (test_update.py's `release_json`, `opener_for`,
//! `program_zip`): temporary folders, GitHub-shaped release data, synthetic release zips and a
//! fake server. The tests never touch the network.

#![allow(dead_code)] // each test binary uses a different subset

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use opendrs_update::http::{Body, HttpError, HttpRequest};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// The platform the tests pick archives for (the Windows build, whatever the host).
pub const PLATFORM: &str = "windows-x64";

/// A folder under the system temp directory, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "opendrs-update-test-{tag}-{}-{n}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("temp dir");
        TempDir(path)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `size` pseudo-random bytes (xorshift), which deflate cannot shrink.
pub fn noise(size: usize) -> Vec<u8> {
    let mut x: u32 = 0x2545_f491;
    (0..size)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x.to_le_bytes()[0]
        })
        .collect()
}

/// `f"{n:,}"`.
pub fn commas(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn download_url(version: &str, name: &str) -> String {
    format!("https://github.com/windasd/OpenDRS/releases/download/v{version}/{name}")
}

pub fn package(version: &str) -> String {
    format!("OpenDRS-{version}-{PLATFORM}.zip")
}

fn asset(version: &str, name: &str, size: usize, digest: Value) -> Value {
    json!({"name": name, "browser_download_url": download_url(version, name), "size": size,
           "digest": digest, "state": "uploaded"})
}

/// Shaped like GitHub's answer: the Python build's win64 zip (never picked), this platform's
/// archive with `digest` (`None`: the real SHA-256 of `zip_bytes`; `Some(Value::Null)`: none),
/// and its `.sha256` file when `with_checksum`.
pub fn release_json(
    version: &str,
    zip_bytes: &[u8],
    digest: Option<Value>,
    with_checksum: bool,
) -> Value {
    let sha = sha256_hex(zip_bytes);
    let digest = digest.unwrap_or_else(|| json!(format!("sha256:{sha}")));
    let python_zip = format!("OpenDRS-{version}-win64.zip");
    let mut assets = vec![
        asset(
            version,
            &python_zip,
            7,
            json!(format!("sha256:{}", "1".repeat(64))),
        ),
        asset(version, &format!("{python_zip}.sha256"), 91, json!(null)),
        asset(version, &package(version), zip_bytes.len(), digest),
    ];
    if with_checksum {
        let name = format!("{}.sha256", package(version));
        assets.push(asset(
            version,
            &name,
            91,
            json!(format!("sha256:{}", "0".repeat(64))),
        ));
    }
    json!({"tag_name": format!("v{version}"), "name": format!("OpenDRS v{version}"),
           "draft": false, "prerelease": false,
           "html_url": format!("https://github.com/windasd/OpenDRS/releases/tag/v{version}"),
           "published_at": "2026-11-01T10:00:00Z", "body": "**Full Changelog**: …",
           "assets": assets})
}

/// A release zip as `cargo xtask dist` builds it for Windows, plus `extra` members.
pub fn program_zip(extra: &[(&str, &[u8])]) -> Vec<u8> {
    let members: [(&str, &[u8]); 4] = [
        ("OpenDRS/OpenDRS.exe", b"new"),
        ("OpenDRS/opendrs-rs.exe", b"cli"),
        ("OpenDRS/README.txt", b"readme"),
        ("OpenDRS/licenses/THIRD_PARTY_LICENSES.txt", b"licenses"),
    ];
    zip_of(members.iter().chain(extra))
}

pub fn zip_of<'a>(members: impl IntoIterator<Item = &'a (&'a str, &'a [u8])>) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in members {
        zip.start_file(*name, options).expect("start member");
        zip.write_all(data).expect("member data");
    }
    zip.finish().expect("finish zip").into_inner()
}

/// `opener_for` of test_update.py: serves `files` by URL (404 for anything else) and records
/// every request in `seen`.
pub fn opener_for<'a>(
    files: &'a HashMap<String, Vec<u8>>,
    seen: &'a RefCell<Vec<HttpRequest>>,
) -> impl Fn(&HttpRequest) -> Result<Body, HttpError> + 'a {
    move |request: &HttpRequest| {
        seen.borrow_mut().push(request.clone());
        assert!(
            request
                .header("user-agent")
                .is_some_and(|ua| ua.starts_with("OpenDRS/"))
        );
        assert!(!request.timeout.is_zero());
        assert_eq!(request.header("X-GitHub-Api-Version"), Some("2022-11-28"));
        match files.get(&request.url) {
            Some(body) => Ok(Box::new(Cursor::new(body.clone())) as Body),
            None => Err(HttpError::Status(404)),
        }
    }
}

pub fn failing(error: HttpError) -> impl Fn(&HttpRequest) -> Result<Body, HttpError> {
    move |_: &HttpRequest| Err(error.clone())
}
