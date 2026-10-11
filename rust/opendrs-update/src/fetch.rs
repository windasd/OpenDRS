//! Talking to GitHub: the release query and the downloads, with the size caps and the messages
//! of `update.py` (`_request`, `_open`, `fetch_latest`, `_download`).

use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::Duration;

use crate::http::{Body, Fetcher, HttpError, HttpRequest, UreqFetcher};
use crate::release::{Release, current_platform, parse_release};
use crate::{
    API_LATEST, CHUNK, MAX_JSON_BYTES, USER_AGENT, UpdateError, io_at, message, thousands,
};

const WHAT_CHECK: &str = "查詢最新版本";
const WHAT_DOWNLOAD: &str = "下載更新";

/// `progress(received, expected)`: `expected` is the size GitHub reported, 0 if none.
pub(crate) type Progress<'a> = &'a mut dyn FnMut(u64, u64);

fn request(url: &str, accept: &str, timeout: Duration, body_timeout: Duration) -> HttpRequest {
    let headers = [
        ("User-Agent", USER_AGENT),
        ("Accept", accept),
        ("X-GitHub-Api-Version", "2022-11-28"),
    ];
    HttpRequest {
        url: url.to_string(),
        headers: headers
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        timeout,
        body_timeout: Some(body_timeout),
    }
}

fn no_connection(what: &str, detail: impl std::fmt::Display) -> UpdateError {
    message(format!("{what}：無法連線（{detail}）"))
}

fn open(fetcher: &dyn Fetcher, request: &HttpRequest, what: &str) -> Result<Body, UpdateError> {
    fetcher.get(request).map_err(|err| match err {
        HttpError::Status(404) => message(format!("{what}：找不到（HTTP 404）")),
        HttpError::Status(code @ (403 | 429)) => message(format!(
            "{what}：GitHub 暫時限制查詢次數，請稍後再試（HTTP {code}）"
        )),
        HttpError::Status(code) => message(format!("{what}：HTTP {code}")),
        HttpError::Connect(detail) => no_connection(what, detail),
    })
}

/// The latest release of windasd/OpenDRS, with this platform's archive.
pub fn fetch_latest(timeout: Duration) -> Result<Release, UpdateError> {
    fetch_latest_with(&UreqFetcher, timeout, &current_platform())
}

/// [`fetch_latest`] through `fetcher`, picking the archive for `platform`.
pub fn fetch_latest_with(
    fetcher: &dyn Fetcher,
    timeout: Duration,
    platform: &str,
) -> Result<Release, UpdateError> {
    let request = request(API_LATEST, "application/vnd.github+json", timeout, timeout);
    let body = open(fetcher, &request, WHAT_CHECK)?;
    let mut raw = Vec::new();
    body.take(MAX_JSON_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|err| no_connection(WHAT_CHECK, err))?;
    if raw.len() as u64 > MAX_JSON_BYTES {
        return Err(message("GitHub 回傳的資料過大"));
    }
    let data = std::str::from_utf8(&raw)
        .ok()
        .and_then(|text| serde_json::from_str(text).ok())
        .ok_or_else(|| message("GitHub 回傳的資料無法解析"))?;
    parse_release(&data, platform)
}

/// Whatever the next read brings (up to `buf.len()` bytes; 0 only at the end of the body), so
/// the size cap, `cancelled` and progress are looked at after every read, not every full chunk:
/// on a slow or stalled connection a cancel takes effect at once.
fn read_chunk(body: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match body.read(buf) {
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            other => return other,
        }
    }
}

/// One file to download into `target`.
pub(crate) struct Download<'a> {
    pub url: &'a str,
    /// Size cap: more bytes than this stop the download.
    pub limit: u64,
    /// The size GitHub reported (0: unknown, not checked).
    pub expected_size: u64,
    pub timeout: Duration,
    pub body_timeout: Duration,
}

/// `_download`: stream the file into `target` in chunks, checking the cap and `cancelled`
/// before each chunk is written, then that the size is the one GitHub reported.
pub(crate) fn download(
    fetcher: &dyn Fetcher,
    file: &Download<'_>,
    target: &Path,
    mut progress: Option<Progress<'_>>,
    cancelled: &dyn Fn() -> bool,
) -> Result<(), UpdateError> {
    let request = request(
        file.url,
        "application/octet-stream",
        file.timeout,
        file.body_timeout,
    );
    let mut body = open(fetcher, &request, WHAT_DOWNLOAD)?;
    let mut out = File::create(target).map_err(io_at(target))?;
    let mut buf = vec![0u8; CHUNK];
    let mut got: u64 = 0;
    loop {
        let n =
            read_chunk(body.as_mut(), &mut buf).map_err(|err| no_connection(WHAT_DOWNLOAD, err))?;
        if n == 0 {
            break;
        }
        got += n as u64;
        if got > file.limit {
            return Err(message("下載的檔案超過大小上限，已中止"));
        }
        if cancelled() {
            return Err(UpdateError::Cancelled);
        }
        out.write_all(&buf[..n]).map_err(io_at(target))?;
        if let Some(report) = progress.as_mut() {
            report(got, file.expected_size);
        }
    }
    out.flush().map_err(io_at(target))?;
    if file.expected_size != 0 && got != file.expected_size {
        return Err(message(format!(
            "下載不完整（{} / {} bytes）",
            thousands(got),
            thousands(file.expected_size)
        )));
    }
    Ok(())
}
