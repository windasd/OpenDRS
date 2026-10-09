//! RadioID.net DMR user database: download, local cache and offline search (port of
//! `radioid.py`).
//!
//! The whole `user.csv` dump (~17 MB, refreshed daily upstream) is cached under the user profile
//! and every filter — region, ID prefix such as `466` (Taiwan), or free-text search over ID /
//! callsign / name / city — runs locally. That keeps a single code path, works offline after the
//! first download, and avoids the online API's paging and rate limits.
//!
//! The downloaded file is untrusted input: size-capped, header-checked before it replaces the
//! cache, decoded as strict UTF-8, and every row is validated.

mod cache;
mod download;
pub mod http;
mod meta;

use std::collections::{HashMap, HashSet};

pub use cache::{CacheInfo, cache_info, default_cache_dir, load_cached, lookup_cached};
pub use download::{Cancelled, DownloadOptions, download, download_cancellable};

use crate::pycsv::{self, CsvError};
use crate::pystr::{self, PyInt, casefold, is_control_or_format, is_space};

pub const CSV_URL: &str = "https://radioid.net/static/user.csv";
pub const CSV_NAME: &str = "user.csv";
pub const META_NAME: &str = "meta.json";
pub const USER_AGENT: &str = "OpenDRS/0.1 (codeplug editor)";
pub const MAX_DOWNLOAD: u64 = 64 * 1024 * 1024;
pub const CHUNK: usize = 256 * 1024;
/// Top of the individual/group ID range (CPS help text).
pub const MAX_RADIO_ID: u32 = 16_776_415;
pub const EXPECTED_HEADER: [&str; 7] = [
    "RADIO_ID",
    "CALLSIGN",
    "FIRST_NAME",
    "LAST_NAME",
    "CITY",
    "STATE",
    "COUNTRY",
];

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// Download or parse failure.
#[derive(Debug, thiserror::Error)]
pub enum RadioIdError {
    /// Python's `RadioIdError`: a message for the user.
    #[error("{0}")]
    Message(String),
    /// The server's Content-Length is not a number (Python lets `int()`'s ValueError escape).
    #[error("{0}")]
    InvalidNumber(String),
    /// Python's csv module rejected the data (`_csv.Error` escapes in Python too).
    #[error(transparent)]
    Csv(#[from] CsvError),
    /// File system or network read failure (an `OSError` that Python lets escape).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub(crate) fn message(text: impl Into<String>) -> RadioIdError {
    RadioIdError::Message(text.into())
}

/// Whitespace controls (tab, CR, LF) become spaces; other controls are dropped.
fn clean(value: &str) -> String {
    let kept: String = value
        .chars()
        .filter_map(|ch| {
            if is_space(ch) {
                Some(' ')
            } else if is_control_or_format(ch) {
                None
            } else {
                Some(ch)
            }
        })
        .collect();
    pystr::split(&kept).collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RadioIdUser {
    pub radio_id: u32,
    pub callsign: String,
    pub first_name: String,
    pub last_name: String,
    pub city: String,
    pub state: String,
    pub country: String,
}

impl RadioIdUser {
    pub fn full_name(&self) -> String {
        [&self.first_name, &self.last_name]
            .into_iter()
            .filter(|p| !p.is_empty())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// `RadioIdUser(radio_id, *(_clean(v) for v in row[1:]))` for a row of seven fields.
    pub(crate) fn from_row(radio_id: u32, row: &[String]) -> Self {
        let field = |i: usize| row.get(i).map_or_else(String::new, |v| clean(v));
        RadioIdUser {
            radio_id,
            callsign: field(1),
            first_name: field(2),
            last_name: field(3),
            city: field(4),
            state: field(5),
            country: field(6),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParseResult {
    pub users: Vec<RadioIdUser>,
    pub skipped: usize,
}

pub(crate) fn header_matches<S: AsRef<str>>(fields: &[S]) -> bool {
    fields.len() == EXPECTED_HEADER.len()
        && fields
            .iter()
            .zip(EXPECTED_HEADER)
            .all(|(h, expected)| pystr::strip(h.as_ref()).to_uppercase() == expected)
}

/// The radio ID of a data row, or `None` when the row is skipped.
fn row_id(row: &[String]) -> Option<u32> {
    if row.len() != EXPECTED_HEADER.len() {
        return None;
    }
    let id = PyInt::parse(pystr::strip(&row[0]))?.to_i64()?;
    u32::try_from(id)
        .ok()
        .filter(|id| (1..=MAX_RADIO_ID).contains(id))
}

pub fn parse_csv(data: &[u8]) -> Result<ParseResult, RadioIdError> {
    let body = data.strip_prefix(UTF8_BOM).unwrap_or(data);
    let text = std::str::from_utf8(body)
        .map_err(|e| message(format!("資料不是有效的 UTF-8（位置 {}）", e.valid_up_to())))?;
    let mut reader = pycsv::read(text);
    let header = reader.next().transpose()?;
    if !header.is_some_and(|h| header_matches(&h)) {
        return Err(message("檔案格式與 radioid.net user.csv 不符"));
    }
    let mut users = Vec::new();
    let mut seen = HashSet::new();
    let mut skipped = 0;
    for row in reader {
        let row = row?;
        match row_id(&row).filter(|id| seen.insert(*id)) {
            Some(id) => users.push(RadioIdUser::from_row(id, &row)),
            None => skipped += 1,
        }
    }
    Ok(ParseResult { users, skipped })
}

/// All given criteria must match; text terms are AND-ed and case-insensitive.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    pub country: String,
    pub id_prefix: String,
    pub text: String,
}

impl Query {
    pub fn is_empty(&self) -> bool {
        self.country.is_empty()
            && pystr::strip(&self.id_prefix).is_empty()
            && pystr::strip(&self.text).is_empty()
    }
}

/// The parsed user list with precomputed search keys.
#[derive(Clone, Debug, Default)]
pub struct UserDb {
    users: Vec<RadioIdUser>,
    ids: Vec<String>,
    hay: Vec<String>,
}

impl UserDb {
    pub fn new(users: Vec<RadioIdUser>) -> Self {
        let ids = users.iter().map(|u| u.radio_id.to_string()).collect();
        let hay = users
            .iter()
            .map(|u| {
                casefold(&format!(
                    "{} {} {} {} {}",
                    u.radio_id,
                    u.callsign,
                    u.full_name(),
                    u.city,
                    u.state
                ))
            })
            .collect();
        UserDb { users, ids, hay }
    }

    pub fn users(&self) -> &[RadioIdUser] {
        &self.users
    }

    pub fn len(&self) -> usize {
        self.users.len()
    }

    pub fn is_empty(&self) -> bool {
        self.users.is_empty()
    }

    /// Countries with their user counts, most users first (ties by name).
    pub fn countries(&self) -> Vec<(String, usize)> {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for user in self.users.iter().filter(|u| !u.country.is_empty()) {
            *counts.entry(&user.country).or_default() += 1;
        }
        let mut out: Vec<(String, usize)> = counts
            .into_iter()
            .map(|(country, n)| (country.to_string(), n))
            .collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        out
    }

    pub fn search(&self, query: &Query) -> Vec<&RadioIdUser> {
        let prefix = pystr::strip(&query.id_prefix);
        let folded = casefold(&query.text);
        let terms: Vec<&str> = pystr::split(&folded).collect();
        self.users
            .iter()
            .enumerate()
            .filter(|(_, user)| query.country.is_empty() || user.country == query.country)
            .filter(|(i, _)| self.ids[*i].starts_with(prefix))
            .filter(|(i, _)| terms.iter().all(|t| self.hay[*i].contains(t)))
            .map(|(_, user)| user)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_keeps_text_and_drops_controls() {
        assert_eq!(clean(" BV2\r\nZZ "), "BV2 ZZ");
        assert_eq!(clean("A\tB\u{200b}C\x1cD\u{7f}"), "A BC D");
    }
}
