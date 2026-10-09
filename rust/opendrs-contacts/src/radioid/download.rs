//! Refreshing the cached `user.csv` from radioid.net (`download` of `radioid.py`).

use std::cmp::Ordering;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::http::{HttpClient, HttpError, HttpRequest, HttpResponse};
use super::meta::{py_str, read_meta, truthy, write_meta};
use super::{
    CHUNK, CSV_NAME, CSV_URL, MAX_DOWNLOAD, RadioIdError, USER_AGENT, UTF8_BOM, header_matches,
    message,
};
use crate::pystr::{self, PyInt, str_repr, thousands};

/// The longest first line `_check_header` reads.
const HEADER_PEEK: u64 = 512;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadOptions {
    /// Until the response headers arrive.
    pub timeout: Duration,
    /// For the whole file, so a stalled server cannot hang the download forever.
    pub body_timeout: Duration,
    /// Size cap for the download (`MAX_DOWNLOAD`).
    pub max_size: u64,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        DownloadOptions {
            timeout: Duration::from_secs(60),
            body_timeout: Duration::from_secs(30 * 60),
            max_size: MAX_DOWNLOAD,
        }
    }
}

/// Asked between chunks: `true` stops the download (the GUI's Stop button).
pub type Cancelled<'a> = &'a dyn Fn() -> bool;

/// `progress(received, total)`: `total` is the Content-Length as the server sent it, 0 when it
/// did not say (signed, like the Python int it mirrors).
pub type Progress<'a> = &'a mut dyn FnMut(u64, i64);

/// Refresh the cache in `directory`. Returns `false` if the server says it has not changed.
///
/// The new file replaces the cached one only when it arrived complete, within the size cap and
/// starting with the expected header; a failed download leaves the old cache untouched.
pub fn download(
    directory: &Path,
    client: &dyn HttpClient,
    options: &DownloadOptions,
    progress: Option<Progress<'_>>,
) -> Result<bool, RadioIdError> {
    download_cancellable(directory, client, options, progress, &|| false)
}

/// [`download`] that stops (with an error, leaving the cache untouched) once `cancelled` says so.
pub fn download_cancellable(
    directory: &Path,
    client: &dyn HttpClient,
    options: &DownloadOptions,
    progress: Option<Progress<'_>>,
    cancelled: Cancelled<'_>,
) -> Result<bool, RadioIdError> {
    fs::create_dir_all(directory)?;
    let target = directory.join(CSV_NAME);
    let request = build_request(directory, &target, options);
    let mut response = match client.get(&request) {
        Ok(response) => response,
        Err(HttpError::Status(304)) => return Ok(false),
        Err(HttpError::Status(code)) => {
            return Err(message(format!("radioid.net 回應 HTTP {code}")));
        }
        Err(HttpError::Connect(detail)) => {
            return Err(message(format!("無法連線 radioid.net：{detail}")));
        }
    };
    let total = content_length(&response)?;
    let cap = i64::try_from(options.max_size).unwrap_or(i64::MAX);
    if total.cmp_i64(cap) == Ordering::Greater {
        return Err(message(format!("檔案過大（{} bytes）", total.thousands())));
    }
    let got = receive_into(
        directory,
        &target,
        &mut response,
        &total,
        options,
        progress,
        cancelled,
    )?;
    let last_modified = response.header("Last-Modified").unwrap_or_default();
    write_meta(directory, last_modified, now(), got)?;
    Ok(true)
}

fn build_request(directory: &Path, target: &Path, options: &DownloadOptions) -> HttpRequest {
    let meta = read_meta(directory);
    let mut headers = vec![("User-Agent".to_string(), USER_AGENT.to_string())];
    let last_modified = meta.get("last_modified").filter(|v| truthy(v));
    if let Some(value) = last_modified.filter(|_| target.is_file()) {
        headers.push(("If-Modified-Since".to_string(), py_str(value)));
    }
    HttpRequest {
        url: CSV_URL.to_string(),
        headers,
        timeout: options.timeout,
        body_timeout: Some(options.body_timeout),
    }
}

/// `int(response.headers.get("Content-Length") or 0)`.
fn content_length(response: &HttpResponse) -> Result<PyInt, RadioIdError> {
    match response.header("Content-Length").filter(|v| !v.is_empty()) {
        None => Ok(PyInt::from(0)),
        Some(value) => PyInt::parse(value).ok_or_else(|| {
            RadioIdError::InvalidNumber(format!(
                "invalid literal for int() with base 10: {}",
                str_repr(value)
            ))
        }),
    }
}

fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

/// Receive the body into a `.part` file next to the cache and move it over the cache once it
/// checks out; the part file never survives a failure.
fn receive_into(
    directory: &Path,
    target: &Path,
    response: &mut HttpResponse,
    total: &PyInt,
    options: &DownloadOptions,
    progress: Option<Progress<'_>>,
    cancelled: Cancelled<'_>,
) -> Result<u64, RadioIdError> {
    let (tmp, file) = create_part_file(directory)?;
    let result = receive(
        file,
        response.body.as_mut(),
        total,
        options.max_size,
        progress,
        cancelled,
    )
    .and_then(|got| {
        check_complete(got, total)?;
        check_header(&tmp)?;
        fs::rename(&tmp, target)?;
        Ok(got)
    });
    if result.is_err() {
        // Best effort: the error being returned matters more than a leftover part file.
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn create_part_file(directory: &Path) -> io::Result<(PathBuf, File)> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let mut attempt = 0u32;
    loop {
        let name = format!("tmp{}-{stamp:x}-{attempt}.part", std::process::id());
        let path = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists && attempt < 100 => {
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    }
}

/// `response.read(CHUNK)`: up to `buf.len()` bytes, fewer only at the end of the body.
fn read_chunk(body: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match body.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(filled)
}

fn receive(
    mut file: File,
    body: &mut dyn Read,
    total: &PyInt,
    max_size: u64,
    mut progress: Option<Progress<'_>>,
    cancelled: Cancelled<'_>,
) -> Result<u64, RadioIdError> {
    let total_hint = total.to_i64().unwrap_or(i64::MIN); // larger totals were refused earlier
    let mut buf = vec![0u8; CHUNK];
    let mut got: u64 = 0;
    loop {
        if cancelled() {
            return Err(message("已取消下載"));
        }
        let n = read_chunk(body, &mut buf)?;
        if n == 0 {
            return Ok(got);
        }
        got += n as u64;
        if got > max_size {
            return Err(message("檔案超過大小上限，已中止"));
        }
        file.write_all(&buf[..n])?;
        if let Some(report) = progress.as_mut() {
            report(got, total_hint);
        }
    }
}

fn check_complete(got: u64, total: &PyInt) -> Result<(), RadioIdError> {
    let got_i64 = i64::try_from(got).unwrap_or(i64::MAX);
    if total.is_zero() || total.cmp_i64(got_i64) == Ordering::Equal {
        return Ok(());
    }
    Err(message(format!(
        "下載不完整（{} / {} bytes）",
        thousands(got),
        total.thousands()
    )))
}

/// The first line must be the user.csv header (an error page is not).
fn check_header(path: &Path) -> Result<(), RadioIdError> {
    let mut head = Vec::new();
    File::open(path)?.take(HEADER_PEEK).read_to_end(&mut head)?;
    let line = head
        .iter()
        .position(|&b| b == b'\n')
        .map_or(&head[..], |i| &head[..=i]);
    let line = line.strip_prefix(UTF8_BOM).unwrap_or(line);
    let text = String::from_utf8_lossy(line);
    let fields: Vec<&str> = pystr::strip(&text).split(',').collect();
    if header_matches(&fields) {
        Ok(())
    } else {
        Err(message(
            "下載內容不是 radioid.net user.csv（可能是錯誤頁面）",
        ))
    }
}
