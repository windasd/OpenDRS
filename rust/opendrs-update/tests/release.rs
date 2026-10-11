//! Versions, release parsing, the release query and the daily check (test_update.py:
//! "versions and releases").

mod common;

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

use chrono::{TimeDelta, TimeZone, Utc};
use common::{PLATFORM, failing, opener_for, package, release_json, sha256_hex};
use opendrs_update::http::HttpError;
use opendrs_update::{
    API_LATEST, MAX_JSON_BYTES, RELEASES_URL, UpdateError, check_due, current_platform,
    fetch_latest_with, now_stamp, parse_release, parse_version,
};
use serde_json::{Value, json};

fn version(text: &str) -> opendrs_update::Version {
    parse_version(text).expect(text)
}

fn text_of<T: std::fmt::Debug>(result: Result<T, UpdateError>) -> String {
    result.expect_err("an error").to_string()
}

// ---------------------------------------------------------------- versions
#[test]
fn version_order() {
    for (older, newer) in [
        ("0.1.0", "0.2.0"),
        ("0.9.9", "0.10.0"),
        ("0.2.0rc1", "0.2.0"),
        ("0.2.0a2", "0.2.0b1"),
        ("0.2.0b2", "0.2.0rc1"),
        ("1.0", "1.0.1"),
        ("1.0.0.1", "1.0.1"),
    ] {
        assert!(version(older) < version(newer), "{older} < {newer}");
    }
}

#[test]
fn version_forms() {
    assert_eq!(version("v0.2.0"), version("0.2"));
    assert_eq!(version("0.2"), version("0.2.0.0"));
    assert_eq!(version(" 0.2.0\n"), version("0.2.0"));
    assert_eq!(
        text_of(parse_version("latest")),
        "無法辨識的版本號「latest」"
    );
    assert!(parse_version("1.2.3.4.5").is_err());
}

// ---------------------------------------------------------------- releases
#[test]
fn parse_release_picks_this_platforms_archive_and_digest() {
    let release = parse_release(&release_json("0.2.0", b"zip", None, true), PLATFORM).expect("ok");
    assert_eq!(
        (
            release.tag.as_str(),
            release.version.as_str(),
            release.title.as_str()
        ),
        ("v0.2.0", "0.2.0", "OpenDRS v0.2.0")
    );
    let pkg = release.package.as_ref().expect("package");
    assert_eq!(
        (pkg.name.as_str(), pkg.size),
        ("OpenDRS-0.2.0-windows-x64.zip", 3)
    );
    assert_eq!(pkg.sha256, sha256_hex(b"zip"));
    assert_eq!(
        release.checksum.as_ref().expect("checksum").name,
        "OpenDRS-0.2.0-windows-x64.zip.sha256"
    );
    assert_eq!(release.notes, "**Full Changelog**: …");
    assert_eq!(release.published, "2026-11-01T10:00:00Z");
    assert!(release.page_url.ends_with("/releases/tag/v0.2.0"));
    assert!(release.is_newer_than("0.1.0") && !release.is_newer_than("0.2.0"));
    assert!(!release.is_newer_than("not a version"));
}

#[test]
fn parse_release_never_picks_the_python_build() {
    let mut data = release_json("0.2.0", b"zip", None, true);
    // Only the Python build's win64 zip and its .sha256 are left.
    data["assets"].as_array_mut().expect("assets").truncate(2);
    for platform in ["windows-x64", "windows-arm64", "linux-x64", "macos-arm64"] {
        let release = parse_release(&data, platform).expect("ok");
        assert_eq!(
            (release.package, release.checksum),
            (None, None),
            "{platform}"
        );
    }
}

#[test]
fn parse_release_names_the_archive_per_platform() {
    let tar = "OpenDRS-0.2.0-linux-x64.tar.gz";
    let mut data = release_json("0.2.0", b"zip", None, true);
    data["assets"].as_array_mut().expect("assets").push(json!({
        "name": tar, "size": 5,
        "browser_download_url": common::download_url("0.2.0", tar),
    }));
    let release = parse_release(&data, "linux-x64").expect("ok");
    let pkg = release.package.expect("linux archive");
    assert_eq!(
        (pkg.name.as_str(), pkg.size, pkg.sha256.as_str()),
        (tar, 5, "")
    );
    assert!(
        parse_release(&data, "macos-arm64")
            .expect("ok")
            .package
            .is_none()
    );
}

#[test]
fn parse_release_ignores_assets_from_elsewhere() {
    let mut data = release_json("0.2.0", b"zip", None, true);
    data["assets"][2]["browser_download_url"] =
        json!("https://example.com/OpenDRS-0.2.0-windows-x64.zip");
    assert!(
        parse_release(&data, PLATFORM)
            .expect("ok")
            .package
            .is_none()
    );
    // Another tag's folder does not count either.
    data["assets"][2]["browser_download_url"] =
        json!(common::download_url("0.1.0", &package("0.2.0")));
    assert!(
        parse_release(&data, PLATFORM)
            .expect("ok")
            .package
            .is_none()
    );
}

#[test]
fn parse_release_refuses_drafts_prereleases_and_other_shapes() {
    let mut pre = release_json("0.2.0", b"", None, true);
    pre["prerelease"] = json!(true);
    assert_eq!(
        text_of(parse_release(&pre, PLATFORM)),
        "最新發佈是草稿或預先發行版"
    );
    let mut draft = release_json("0.2.0", b"", None, true);
    draft["draft"] = json!(1);
    assert!(parse_release(&draft, PLATFORM).is_err());
    for shape in [
        json!(["not", "a", "release"]),
        json!({"tag_name": 5}),
        json!({}),
    ] {
        assert_eq!(
            text_of(parse_release(&shape, PLATFORM)),
            "GitHub 回傳的發佈資料格式不符"
        );
    }
    assert!(text_of(parse_release(&json!({"tag_name": "nightly"}), PLATFORM)).contains("無法辨識"));
}

#[test]
fn parse_release_fills_in_missing_fields() {
    let release = parse_release(&json!({"tag_name": "v1.0"}), PLATFORM).expect("ok");
    assert_eq!(release.title, "v1.0");
    assert_eq!(release.page_url, RELEASES_URL);
    assert_eq!(
        (release.notes.as_str(), release.published.as_str()),
        ("", "")
    );
    assert!(release.package.is_none());
}

#[test]
fn a_later_asset_of_the_same_name_wins() {
    let mut data = release_json("0.2.0", b"zip", None, false);
    let mut again = data["assets"][2].clone();
    again["size"] = json!(99);
    data["assets"].as_array_mut().expect("assets").push(again);
    assert_eq!(
        parse_release(&data, PLATFORM)
            .expect("ok")
            .package
            .expect("pkg")
            .size,
        99
    );
}

#[test]
fn current_platform_is_shaped_like_the_archive_names() {
    let platform = current_platform();
    let (os, arch) = platform.split_once('-').expect("os-arch");
    assert_eq!(os, std::env::consts::OS);
    assert!(!arch.is_empty());
    if cfg!(all(windows, target_arch = "x86_64")) {
        assert_eq!(platform, "windows-x64");
    }
}

// ---------------------------------------------------------------- the release query
fn served(body: Vec<u8>) -> HashMap<String, Vec<u8>> {
    HashMap::from([(API_LATEST.to_string(), body)])
}

#[test]
fn fetch_latest_and_http_errors() {
    let seen = RefCell::new(Vec::new());
    let body = serde_json::to_vec(&release_json("0.3.0", b"zip", None, true)).expect("json");
    let files = served(body);
    let release = fetch_latest_with(
        &opener_for(&files, &seen),
        Duration::from_secs(15),
        PLATFORM,
    )
    .expect("release");
    assert_eq!(release.version, "0.3.0");
    let request = seen.borrow()[0].clone();
    assert_eq!(request.url, API_LATEST);
    assert_eq!(
        request.header("Accept"),
        Some("application/vnd.github+json")
    );

    let none = HashMap::new();
    let err = fetch_latest_with(&opener_for(&none, &seen), Duration::from_secs(1), PLATFORM);
    assert_eq!(text_of(err), "查詢最新版本：找不到（HTTP 404）");
    for code in [403, 429] {
        let err = fetch_latest_with(
            &failing(HttpError::Status(code)),
            Duration::from_secs(1),
            PLATFORM,
        );
        assert_eq!(
            text_of(err),
            format!("查詢最新版本：GitHub 暫時限制查詢次數，請稍後再試（HTTP {code}）")
        );
    }
    let err = fetch_latest_with(
        &failing(HttpError::Status(502)),
        Duration::from_secs(1),
        PLATFORM,
    );
    assert_eq!(text_of(err), "查詢最新版本：HTTP 502");
    let offline = failing(HttpError::Connect("no route to host".into()));
    let err = fetch_latest_with(&offline, Duration::from_secs(1), PLATFORM);
    assert_eq!(text_of(err), "查詢最新版本：無法連線（no route to host）");
}

#[test]
fn fetch_latest_refuses_large_or_broken_answers() {
    let seen = RefCell::new(Vec::new());
    let check = |body: Vec<u8>| {
        let files = served(body);
        text_of(fetch_latest_with(
            &opener_for(&files, &seen),
            Duration::from_secs(1),
            PLATFORM,
        ))
    };
    let mut big = serde_json::to_vec(&release_json("0.3.0", b"", None, true)).expect("json");
    big.resize(MAX_JSON_BYTES as usize + 1, b' ');
    assert_eq!(check(big), "GitHub 回傳的資料過大");
    assert_eq!(check(b"{not json".to_vec()), "GitHub 回傳的資料無法解析");
    assert_eq!(check(b"\xff\xfe".to_vec()), "GitHub 回傳的資料無法解析");
    assert_eq!(check(b"[1, 2]".to_vec()), "GitHub 回傳的發佈資料格式不符");
    let mut bad_size = release_json("0.3.0", b"", None, true);
    bad_size["assets"][0]["size"] = Value::from("many");
    assert_eq!(
        check(serde_json::to_vec(&bad_size).expect("json")),
        "GitHub 回傳的資料無法解析"
    );
}

#[test]
fn fetch_latest_accepts_an_answer_of_exactly_the_limit() {
    let seen = RefCell::new(Vec::new());
    let mut body = serde_json::to_vec(&release_json("0.3.0", b"", None, true)).expect("json");
    body.resize(MAX_JSON_BYTES as usize, b' ');
    let files = served(body);
    let release = fetch_latest_with(&opener_for(&files, &seen), Duration::from_secs(1), PLATFORM);
    assert_eq!(release.expect("release").version, "0.3.0");
}

// ---------------------------------------------------------------- the daily check
#[test]
fn check_due_runs_about_daily() {
    let now = Utc
        .with_ymd_and_hms(2026, 11, 2, 9, 0, 0)
        .single()
        .expect("time");
    let iso = |delta: TimeDelta| (now - delta).to_rfc3339();
    assert!(check_due("", now));
    assert!(check_due(&iso(TimeDelta::hours(21)), now));
    assert!(!check_due(&iso(TimeDelta::hours(2)), now));
    assert!(check_due(&iso(TimeDelta::hours(20)), now));
    assert!(!check_due(&iso(TimeDelta::hours(-3)), now)); // clock went back
}

#[test]
fn check_due_reads_what_python_wrote() {
    let now = Utc
        .with_ymd_and_hms(2026, 11, 2, 9, 0, 0)
        .single()
        .expect("time");
    for recent in [
        "2026-11-02T07:00:00+00:00",
        "2026-11-02T07:00:00.123456+00:00",
        "2026-11-02T07:00:00Z",
        "2026-11-02T15:00:00+08:00",
        "2026-11-02T07:00:00+0000",
    ] {
        assert!(!check_due(recent, now), "{recent}");
    }
    for due in [
        "2026-11-01T09:00:00+00:00",
        "2026-11-01T12:59:59.999999-00:00",
        "2026-11-02T07:00:00", // no offset
        "2026-11-02",
        "yesterday",
    ] {
        assert!(check_due(due, now), "{due}");
    }
}

#[test]
fn now_stamp_is_pythons_isoformat() {
    let now = Utc
        .with_ymd_and_hms(2026, 10, 11, 9, 5, 11)
        .single()
        .expect("time");
    assert_eq!(now_stamp(now), "2026-10-11T09:05:11+00:00");
    let with_nanos = now + TimeDelta::nanoseconds(123_456_789);
    assert_eq!(now_stamp(with_nanos), "2026-10-11T09:05:11+00:00");
    assert!(!check_due(&now_stamp(now), now));
    assert!(check_due(&now_stamp(now), now + TimeDelta::hours(20)));
}
