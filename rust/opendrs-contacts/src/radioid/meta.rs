//! `meta.json` beside the cached `user.csv` (`_read_meta` / `_write_meta` of `radioid.py`), and
//! the Python conversions applied to its values (`opendrs_core::pyjson`).

use std::fs;
use std::io;
use std::path::Path;

use serde_json::{Map, Value};

use super::{CSV_URL, META_NAME};
use crate::pystr::float_repr;

pub(crate) type Meta = Map<String, Value>;

/// The stored object; anything unreadable counts as empty.
pub(crate) fn read_meta(directory: &Path) -> Meta {
    let Ok(bytes) = fs::read(directory.join(META_NAME)) else {
        return Meta::new();
    };
    match String::from_utf8(bytes).map(|text| serde_json::from_str::<Value>(&text)) {
        Ok(Ok(Value::Object(map))) => map,
        _ => Meta::new(),
    }
}

/// `json.dumps(meta, ensure_ascii=False, indent=2)` written as text: Python translates the line
/// breaks to the platform's (CRLF on Windows).
pub(crate) fn write_meta(
    directory: &Path,
    last_modified: &str,
    downloaded_at: f64,
    size: u64,
) -> io::Result<()> {
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    let fields = [
        ("last_modified", Value::from(last_modified).to_string()),
        ("downloaded_at", float_repr(downloaded_at)),
        ("size", size.to_string()),
        ("source", Value::from(CSV_URL).to_string()),
    ];
    let body = fields
        .iter()
        .map(|(key, value)| format!("  \"{key}\": {value}"))
        .collect::<Vec<_>>()
        .join(&format!(",{nl}"));
    fs::write(directory.join(META_NAME), format!("{{{nl}{body}{nl}}}"))
}

pub(crate) use opendrs_core::pyjson::{py_float, py_str, truthy};
