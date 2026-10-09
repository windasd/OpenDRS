//! Port of tests/test_radioid.py: parsing, search, the download with its safety checks, and the
//! cache helpers. The HTTP layer is always a fake; nothing goes to the network.

mod common;

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fs;

use common::{CSV, LAST_MODIFIED, TempDir, download_csv, failing, opener_for};
use opendrs_contacts::radioid::http::HttpError;
use opendrs_contacts::radioid::{
    self, DownloadOptions, MAX_DOWNLOAD, Query, RadioIdError, UserDb, parse_csv,
};

fn db() -> UserDb {
    UserDb::new(parse_csv(CSV.as_bytes()).unwrap().users)
}

fn query(country: &str, id_prefix: &str, text: &str) -> Query {
    Query {
        country: country.into(),
        id_prefix: id_prefix.into(),
        text: text.into(),
    }
}

fn message(err: RadioIdError) -> String {
    err.to_string()
}

fn part_files(dir: &std::path::Path) -> usize {
    fs::read_dir(dir)
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .is_ok_and(|e| e.path().extension().is_some_and(|x| x == "part"))
        })
        .count()
}

#[test]
fn parse_validates_rows() {
    let result = parse_csv(CSV.as_bytes()).unwrap();
    assert_eq!(result.users.len(), 7);
    assert_eq!(result.skipped, 4); // non-numeric id, duplicate id, out of range, short row
    let w1 = result.users.iter().find(|u| u.callsign == "QA0CT").unwrap();
    assert_eq!(w1.first_name, "Smith, Jr");
}

#[test]
fn parse_rejects_wrong_header_and_bad_utf8() {
    let err = message(parse_csv(b"<html>error</html>").unwrap_err());
    assert!(err.contains("格式"), "{err}");
    let err = message(
        parse_csv(b"RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\n1,\xff,,,,,X\n")
            .unwrap_err(),
    );
    assert!(err.contains("UTF-8"), "{err}");
}

#[test]
fn control_characters_are_stripped() {
    let data = b"RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\n\
                 9990099,\"BV2\r\nZZ\",A\tB,,,,Taiwan\n";
    let user = parse_csv(data).unwrap().users.remove(0);
    assert_eq!(
        (user.callsign.as_str(), user.first_name.as_str()),
        ("BV2 ZZ", "A B")
    );
}

#[test]
fn countries_sorted_by_count() {
    let db = db();
    assert_eq!(db.countries()[0], ("Taiwan".to_string(), 3));
    assert_eq!(db.len(), 7);
}

#[test]
fn search() {
    let cases: [(Query, &[&str]); 10] = [
        (query("", "999", ""), &["QA0BD", "QA0ADN", "QA0AHJ"]),
        (query("Taiwan", "", ""), &["QA0BD", "QA0ADN", "QA0AHJ"]),
        (query("", "", "qa0bd"), &["QA0BD"]),
        (query("", "", "9990002"), &["QA0ADN"]),
        (query("", "", "taipei"), &["QA0BD", "QA0AHJ"]),
        (query("", "", "new taipei"), &["QA0BD"]),
        (query("Taiwan", "", "chen"), &["QA0ADN"]),
        (query("", "999001", ""), &["QA0AHJ"]),
        (query("", "", "小明"), &["QA0AB"]),
        (query("Spain", "999", ""), &[]),
    ];
    let db = db();
    for (q, expected) in cases {
        let got: BTreeSet<&str> = db.search(&q).iter().map(|u| u.callsign.as_str()).collect();
        assert_eq!(got, expected.iter().copied().collect(), "{q:?}");
    }
}

#[test]
fn empty_query_flag() {
    assert!(Query::default().is_empty());
    assert!(!query("", " 4", "").is_empty());
}

#[test]
fn download_can_be_cancelled_and_has_a_body_deadline() {
    let tmp = TempDir::new("cancel");
    let seen = RefCell::new(Vec::new());
    let client = opener_for(CSV.as_bytes(), &[], &seen);
    let options = DownloadOptions::default();
    let err = radioid::download_cancellable(tmp.path(), &client, &options, None, &|| true);
    assert_eq!(message(err.unwrap_err()), "已取消下載");
    assert!(
        radioid::cache_info(tmp.path()).is_none(),
        "no cache from a cancelled download"
    );
    let leftovers = fs::read_dir(tmp.path()).unwrap().count();
    assert_eq!(leftovers, 0, "the part file is removed");
    assert_eq!(seen.borrow()[0].body_timeout, Some(options.body_timeout));
}

#[test]
fn download_caches_and_loads() {
    let tmp = TempDir::new("caches");
    let seen = RefCell::new(Vec::new());
    let client = opener_for(CSV.as_bytes(), &[], &seen);
    let mut progress = Vec::new();
    let mut report = |got, total| progress.push((got, total));
    let changed = radioid::download(
        tmp.path(),
        &client,
        &DownloadOptions::default(),
        Some(&mut report),
    );
    assert!(changed.unwrap());
    let n = CSV.len();
    assert_eq!(progress.last(), Some(&(n as u64, n as i64)));
    let info = radioid::cache_info(tmp.path()).unwrap();
    assert_eq!(info.size, n as u64);
    assert!(info.last_modified.starts_with("Thu"));
    let (db, skipped) = radioid::load_cached(tmp.path()).unwrap().unwrap();
    assert_eq!((db.len(), skipped), (7, 4));
}

#[test]
fn second_download_is_conditional_and_handles_304() {
    let tmp = TempDir::new("conditional");
    download_csv(tmp.path());
    let seen = RefCell::new(Vec::new());
    let not_modified = |request: &radioid::http::HttpRequest| {
        seen.borrow_mut().push(request.clone());
        Err(HttpError::Status(304))
    };
    let changed = radioid::download(tmp.path(), &not_modified, &DownloadOptions::default(), None);
    assert!(!changed.unwrap());
    let request = seen.borrow()[0].clone();
    assert_eq!(request.header("If-modified-since"), Some(LAST_MODIFIED));
    assert!(request.header("User-agent").unwrap().starts_with("OpenDRS"));
    assert_eq!(
        fs::read(tmp.path().join("user.csv")).unwrap(),
        CSV.as_bytes()
    );
}

#[test]
fn bad_downloads_never_replace_cache() {
    let longer = (CSV.len() + 10).to_string();
    let too_big = (MAX_DOWNLOAD + 1).to_string();
    type Case<'a> = (&'a [u8], Vec<(&'a str, &'a str)>, &'a str);
    let cases: [Case; 3] = [
        (b"<html>maintenance</html>", vec![], "錯誤頁面"),
        (
            CSV.as_bytes(),
            vec![("Content-Length", longer.as_str())],
            "不完整",
        ),
        (
            CSV.as_bytes(),
            vec![("Content-Length", too_big.as_str())],
            "過大",
        ),
    ];
    for (body, overrides, expected) in cases {
        let tmp = TempDir::new("bad");
        download_csv(tmp.path());
        let seen = RefCell::new(Vec::new());
        let client = opener_for(body, &overrides, &seen);
        let err = radioid::download(tmp.path(), &client, &DownloadOptions::default(), None);
        let err = message(err.unwrap_err());
        assert!(err.contains(expected), "{err}");
        assert_eq!(
            fs::read(tmp.path().join("user.csv")).unwrap(),
            CSV.as_bytes()
        );
        assert_eq!(part_files(tmp.path()), 0);
    }
}

#[test]
fn download_stops_past_size_cap() {
    let tmp = TempDir::new("cap");
    let seen = RefCell::new(Vec::new());
    let client = opener_for(CSV.as_bytes(), &[("Content-Length", "0")], &seen);
    let options = DownloadOptions {
        max_size: 100,
        ..DownloadOptions::default()
    };
    let err = message(radioid::download(tmp.path(), &client, &options, None).unwrap_err());
    assert!(err.contains("大小上限"), "{err}");
    assert!(radioid::cache_info(tmp.path()).is_none());
    assert_eq!(part_files(tmp.path()), 0);
}

#[test]
fn network_errors_are_reported() {
    let cases = [
        (HttpError::Status(503), "HTTP 503"),
        (
            HttpError::Connect("<urlopen error no route>".into()),
            "無法連線",
        ),
    ];
    for (error, expected) in cases {
        let tmp = TempDir::new("net");
        let client = failing(error);
        let err = radioid::download(tmp.path(), &client, &DownloadOptions::default(), None);
        let err = message(err.unwrap_err());
        assert!(err.contains(expected), "{err}");
    }
}

#[test]
fn download_creates_the_folder_and_writes_python_style_meta() {
    let tmp = TempDir::new("meta");
    let dir = tmp.path().join("nested").join("radioid");
    download_csv(&dir);
    let meta = fs::read_to_string(dir.join("meta.json")).unwrap();
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    let lines: Vec<&str> = meta.split(nl).collect();
    assert_eq!(lines[0], "{");
    assert_eq!(
        lines[1],
        format!("  \"last_modified\": \"{LAST_MODIFIED}\",")
    );
    assert!(lines[2].starts_with("  \"downloaded_at\": 1") && lines[2].ends_with(','));
    assert_eq!(lines[3], format!("  \"size\": {},", CSV.len()));
    assert_eq!(
        lines[4],
        "  \"source\": \"https://radioid.net/static/user.csv\""
    );
    assert_eq!(lines[5], "}");
    assert_eq!(lines.len(), 6);
}

#[test]
fn cache_helpers_without_data() {
    let tmp = TempDir::new("nodata");
    assert!(radioid::cache_info(tmp.path()).is_none());
    assert!(radioid::load_cached(tmp.path()).unwrap().is_none());
    fs::write(tmp.path().join("user.csv"), CSV).unwrap();
    fs::write(tmp.path().join("meta.json"), "[1,2]").unwrap();
    let info = radioid::cache_info(tmp.path()).unwrap();
    assert_eq!(info.last_modified, "");
}

#[test]
fn lookup_cached_finds_one_id() {
    let tmp = TempDir::new("lookup");
    assert!(
        radioid::lookup_cached(tmp.path(), 9990002)
            .unwrap()
            .is_none()
    ); // not downloaded yet
    download_csv(tmp.path());
    let user = radioid::lookup_cached(tmp.path(), 9990002)
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            user.callsign.as_str(),
            user.first_name.as_str(),
            user.city.as_str()
        ),
        ("QA0ADN", "Chen", "Taichung")
    );
    let smith = radioid::lookup_cached(tmp.path(), 3100001)
        .unwrap()
        .unwrap();
    assert_eq!(smith.first_name, "Smith, Jr");
    assert!(
        radioid::lookup_cached(tmp.path(), 466000)
            .unwrap()
            .is_none()
    ); // prefix of a real ID
    assert!(
        radioid::lookup_cached(tmp.path(), 1234567)
            .unwrap()
            .is_none()
    );
}

#[test]
fn lookup_cached_matches_whole_ids_and_the_last_line() {
    // The data of test_recording_dialog.py's RadioID cache test.
    let tmp = TempDir::new("lookup2");
    let mut data = b"RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n\
                     19990186,QA0AKN,Wrong,,,,\r\n\
                     9990186,QA0ABT,Zhi Ming,,New Taipei City,Taipei,Taiwan\r\n"
        .to_vec();
    data.extend("9991032,QA0AU,小美,林,New Taipei City,Taipei,Taiwan".as_bytes()); // no newline
    fs::write(tmp.path().join("user.csv"), data).unwrap();
    let zhi = radioid::lookup_cached(tmp.path(), 9990186)
        .unwrap()
        .unwrap();
    assert_eq!(
        (zhi.callsign.as_str(), zhi.full_name()),
        ("QA0ABT", "Zhi Ming".into())
    );
    let mei = radioid::lookup_cached(tmp.path(), 9991032)
        .unwrap()
        .unwrap();
    assert_eq!(mei.full_name(), "小美 林");
    assert!(radioid::lookup_cached(tmp.path(), 46621).unwrap().is_none());
    assert!(radioid::lookup_cached(tmp.path(), 1).unwrap().is_none());
}

#[test]
fn the_cache_folder_lives_in_the_data_folder() {
    assert_eq!(
        radioid::default_cache_dir(),
        opendrs_core::paths::data_dir().join("radioid")
    );
}
