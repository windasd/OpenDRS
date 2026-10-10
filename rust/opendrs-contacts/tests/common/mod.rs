//! Shared helpers for the integration tests: fixture paths, temporary folders and fake HTTP
//! clients (the tests never touch the network).

#![allow(dead_code)] // each test binary uses a different subset

use std::cell::RefCell;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use opendrs_contacts::radioid::http::{HttpError, HttpRequest, HttpResponse};
use opendrs_contacts::radioid::{DownloadOptions, RadioIdUser, download};
use opendrs_core::f136::{self, Project};

pub const SAMPLES: [&str; 3] = [
    "dh98vu_20250811.F136",
    "mmdvm_20241124.F136",
    "mmdvm_20241127.F136",
];
pub const SMALL: &str = "mmdvm_20241127.F136";

pub fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../fixtures")
}

pub fn load(name: &str) -> Project {
    f136::load(fixtures().join(name)).expect("fixture project")
}

/// A folder under the system temp directory, removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("opendrs-contacts-{tag}-{}-{n}", std::process::id()));
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

/// The CSV of test_radioid.py.
pub const CSV: &str = concat!(
    "RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n",
    "9990001,QA0BD,James,,New Taipei,Northern Taiwan,Taiwan\r\n",
    "9990002,QA0ADN,Chen,Wei,Taichung,Central Taiwan,Taiwan\r\n",
    "9990010,QA0AHJ,Lin,,Taipei,Northern Taiwan,Taiwan\r\n",
    "1023007,QA0AKM,Hans Peter,,Cornwall,Ontario,Canada\r\n",
    "2141234,QA0AKK,José,Núñez,León,Castilla,Spain\r\n",
    "4600001,QA0AB,王,小明,北京,,China\r\n",
    "3100001,QA0CT,\"Smith, Jr\",,Boston,MA,United States\r\n",
    "bad,X,,,,,Nowhere\r\n",
    "9990001,DUP,dup,,,,Taiwan\r\n",
    "99999999,BIG,,,,,Taiwan\r\n",
    "short,row\r\n",
);

pub const LAST_MODIFIED: &str = "Thu, 08 Oct 2026 05:01:49 GMT";

pub fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn response(body: &[u8], headers: Vec<(String, String)>) -> HttpResponse {
    HttpResponse::new(headers, Box::new(Cursor::new(body.to_vec())))
}

/// `opener_for` of test_radioid.py: serves `body` with Content-Length and Last-Modified, the
/// given headers replacing those; every request is recorded in `seen`.
pub fn opener_for<'a>(
    body: &'a [u8],
    overrides: &'a [(&'a str, &'a str)],
    seen: &'a RefCell<Vec<HttpRequest>>,
) -> impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> + 'a {
    move |request: &HttpRequest| {
        seen.borrow_mut().push(request.clone());
        let length = body.len().to_string();
        let mut all = headers(&[
            ("Content-Length", length.as_str()),
            ("Last-Modified", LAST_MODIFIED),
        ]);
        for (name, value) in overrides {
            all.retain(|(k, _)| k != name);
            all.push((name.to_string(), value.to_string()));
        }
        Ok(response(body, all))
    }
}

pub fn failing(error: HttpError) -> impl Fn(&HttpRequest) -> Result<HttpResponse, HttpError> {
    move |_: &HttpRequest| Err(error.clone())
}

/// Download `CSV` into `directory` with a fake server.
pub fn download_csv(directory: &Path) {
    let seen = RefCell::new(Vec::new());
    let client = opener_for(CSV.as_bytes(), &[], &seen);
    assert!(download(directory, &client, &DownloadOptions::default(), None).expect("download"));
}

pub fn user(rid: u32, call: &str, first: &str, last: &str) -> RadioIdUser {
    RadioIdUser {
        radio_id: rid,
        callsign: call.into(),
        first_name: first.into(),
        last_name: last.into(),
        city: String::new(),
        state: String::new(),
        country: "Taiwan".into(),
    }
}
