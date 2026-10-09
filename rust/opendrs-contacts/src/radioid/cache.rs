//! The local copy of `user.csv`: where it lives, what is known about it, loading it and looking
//! up one ID without parsing the whole file.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::meta::{py_float, py_str, read_meta, truthy};
use super::{CSV_NAME, EXPECTED_HEADER, ParseResult, RadioIdError, RadioIdUser, UserDb, parse_csv};
use crate::pycsv;

/// The cache folder the application uses (`default_dir() / "radioid"` in the Python GUI).
pub fn default_cache_dir() -> PathBuf {
    opendrs_core::paths::data_dir().join("radioid")
}

#[derive(Clone, Debug, PartialEq)]
pub struct CacheInfo {
    pub path: PathBuf,
    pub size: u64,
    /// Seconds since the Unix epoch.
    pub downloaded_at: f64,
    pub last_modified: String,
}

fn seconds_since_epoch(time: SystemTime) -> f64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_secs_f64(),
        Err(before) => -before.duration().as_secs_f64(),
    }
}

/// What is cached in `directory`, or `None` before the first download.
///
/// `downloaded_at` comes from `meta.json`, else the file's modification time. Where Python's
/// `float()` would raise on a hand-edited value (text that is not a number, a list), the
/// modification time is used instead of failing.
pub fn cache_info(directory: &Path) -> Option<CacheInfo> {
    let path = directory.join(CSV_NAME);
    let stat = fs::metadata(&path).ok().filter(|m| m.is_file())?;
    let meta = read_meta(directory);
    let mtime = || stat.modified().map_or(0.0, seconds_since_epoch);
    let downloaded_at = meta
        .get("downloaded_at")
        .filter(|v| truthy(v))
        .and_then(py_float)
        .unwrap_or_else(mtime);
    let last_modified = meta.get("last_modified").map_or_else(String::new, py_str);
    Some(CacheInfo {
        path,
        size: stat.len(),
        downloaded_at,
        last_modified,
    })
}

/// The cached user list and how many rows were skipped; `None` before the first download.
pub fn load_cached(directory: &Path) -> Result<Option<(UserDb, usize)>, RadioIdError> {
    let Some(info) = cache_info(directory) else {
        return Ok(None);
    };
    let ParseResult { users, skipped } = parse_csv(&fs::read(&info.path)?)?;
    Ok(Some((UserDb::new(users), skipped)))
}

/// Where `needle` (which starts with a newline) first occurs.
fn find_line(data: &[u8], needle: &[u8]) -> Option<usize> {
    data.iter()
        .enumerate()
        .filter(|&(_, &b)| b == b'\n')
        .map(|(i, _)| i)
        .find(|&i| data[i..].starts_with(needle))
}

/// Find one ID in the downloaded user.csv without parsing the whole file.
pub fn lookup_cached(directory: &Path, radio_id: u32) -> Result<Option<RadioIdUser>, RadioIdError> {
    let Some(info) = cache_info(directory) else {
        return Ok(None);
    };
    let data = fs::read(&info.path)?;
    let Some(start) = find_line(&data, format!("\n{radio_id},").as_bytes()) else {
        return Ok(None);
    };
    let rest = &data[start + 1..];
    let end = rest.iter().position(|&b| b == b'\n').unwrap_or(rest.len());
    let line = String::from_utf8_lossy(&rest[..end]);
    let row = pycsv::Reader::new(std::iter::once(&*line))
        .next()
        .transpose()?
        .unwrap_or_default();
    if row.len() != EXPECTED_HEADER.len() {
        return Ok(None);
    }
    Ok(Some(RadioIdUser::from_row(radio_id, &row)))
}
