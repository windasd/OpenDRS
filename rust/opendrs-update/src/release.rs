//! The latest release as GitHub's REST API describes it, and the archive this platform needs
//! (`Asset`, `Release` and `parse_release` of `update.py`).

use serde_json::Value;

use crate::version::parse_version;
use crate::{DOWNLOAD_PREFIX, RELEASES_URL, SITE_URL, UpdateError, message, py_strip};

/// A file attached to the release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    /// As reported by GitHub (lowercase hex; empty if it reported none).
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub title: String,
    /// Markdown.
    pub notes: String,
    pub page_url: String,
    pub published: String,
    /// `OpenDRS-<version>-<platform>.zip` (`.tar.gz` on Linux).
    pub package: Option<Asset>,
    /// Its `.sha256` file.
    pub checksum: Option<Asset>,
}

impl Release {
    /// Whether this release is newer than `current` (false if either cannot be parsed).
    pub fn is_newer_than(&self, current: &str) -> bool {
        match (parse_version(&self.version), parse_version(current)) {
            (Ok(latest), Ok(current)) => latest > current,
            _ => false,
        }
    }
}

/// The platform part of the archive names, as `cargo xtask dist` writes it (`layout::platform`):
/// `windows-x64`, `macos-arm64`, `linux-x64`, …
pub fn current_platform() -> String {
    platform_of(std::env::consts::OS, std::env::consts::ARCH)
}

fn platform_of(os: &str, arch: &str) -> String {
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}-{arch}")
}

/// `OpenDRS-<version>-<platform>.zip`, or `.tar.gz` for Linux (`xtask::write_archive`).
pub fn package_name(version: &str, platform: &str) -> String {
    let extension = if platform.starts_with("linux-") {
        "tar.gz"
    } else {
        "zip"
    };
    format!("OpenDRS-{version}-{platform}.{extension}")
}

fn unparsable() -> UpdateError {
    message("GitHub 回傳的資料無法解析")
}

/// Python's truthiness of a JSON value.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// `str(value)` of a JSON value in Python (containers as JSON text).
fn py_str(value: &Value) -> String {
    match value {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// `str(data.get(key) or default)`.
fn text_or(data: &Value, key: &str, default: &str) -> String {
    data.get(key)
        .filter(|v| truthy(v))
        .map_or_else(|| default.to_string(), py_str)
}

/// `int(text)` for the decimal integers Python accepts (sign, surrounding whitespace, `_`
/// between digits).
fn py_int(text: &str) -> Option<i128> {
    let text = py_strip(text);
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let groups_ok = !digits.is_empty()
        && digits
            .split('_')
            .all(|g| !g.is_empty() && g.bytes().all(|b| b.is_ascii_digit()));
    if !groups_ok {
        return None;
    }
    let value: i128 = digits.replace('_', "").parse().ok()?;
    Some(if negative { -value } else { value })
}

/// `int(item.get("size") or 0)`; a value Python's `int()` refuses, or a negative size, is
/// unparsable data.
fn asset_size(item: &Value) -> Result<u64, UpdateError> {
    let Some(value) = item.get("size").filter(|v| truthy(v)) else {
        return Ok(0);
    };
    let size = match value {
        Value::Bool(_) => Some(1),
        Value::Number(n) => n
            .as_i64()
            .map(i128::from)
            .or_else(|| n.as_u64().map(i128::from))
            .or_else(|| {
                n.as_f64()
                    .filter(|f| f.is_finite())
                    .map(|f| f.trunc() as i128)
            }),
        Value::String(s) => py_int(s),
        _ => None,
    };
    size.and_then(|s| u64::try_from(s).ok())
        .ok_or_else(unparsable)
}

/// GitHub's `digest` (`sha256:<hex>`), or "" when it gave none.
fn asset_sha256(item: &Value) -> String {
    let digest = item.get("digest").and_then(Value::as_str).unwrap_or("");
    let sha = digest
        .strip_prefix("sha256:")
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    if is_sha256(&sha) { sha } else { String::new() }
}

pub(crate) fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// `_asset(item)`; `None` for an entry whose name or URL is not a string (it can never be picked).
fn asset(item: &Value) -> Result<Option<Asset>, UpdateError> {
    let size = asset_size(item)?;
    let name = item.get("name").and_then(Value::as_str);
    let url = item.get("browser_download_url").and_then(Value::as_str);
    Ok(name.zip(url).map(|(name, url)| Asset {
        name: name.to_string(),
        url: url.to_string(),
        size,
        sha256: asset_sha256(item),
    }))
}

/// The release in GitHub's `releases/latest` answer, with the archive for `platform` (see
/// [`current_platform`]) and its `.sha256` file when the release has them.
///
/// Only assets under `https://github.com/windasd/OpenDRS/releases/download/<tag>/` count.
pub fn parse_release(data: &Value, platform: &str) -> Result<Release, UpdateError> {
    let Some(tag) = data
        .as_object()
        .and_then(|o| o.get("tag_name"))
        .and_then(Value::as_str)
    else {
        return Err(message("GitHub 回傳的發佈資料格式不符"));
    };
    if data.get("draft").is_some_and(truthy) || data.get("prerelease").is_some_and(truthy) {
        return Err(message("最新發佈是草稿或預先發行版"));
    }
    let version = tag.strip_prefix('v').unwrap_or(tag);
    parse_version(version)?;
    let wanted = package_name(version, platform);
    let wanted_checksum = format!("{wanted}.sha256");
    let prefix = format!("{DOWNLOAD_PREFIX}{tag}/");
    let mut assets = Vec::new();
    if let Some(items) = data.get("assets").and_then(Value::as_array) {
        for item in items.iter().filter(|i| i.is_object()) {
            assets.extend(asset(item)?);
        }
    }
    // A later asset of the same name wins, as in Python's dict comprehension. The URL must be
    // exactly the release's own download folder plus the name (no `..` or other segments).
    let pick = |name: &str| {
        assets
            .iter()
            .rev()
            .find(|a| a.name == name && a.url.strip_prefix(&prefix) == Some(name))
            .cloned()
    };
    // Opened in the browser: only this project's pages.
    let page_url = text_or(data, "html_url", RELEASES_URL);
    let page_url = if page_url.starts_with(&format!("{SITE_URL}/")) && !page_url.contains("..") {
        page_url
    } else {
        RELEASES_URL.to_string()
    };
    Ok(Release {
        tag: tag.to_string(),
        version: version.to_string(),
        title: text_or(data, "name", tag),
        notes: text_or(data, "body", ""),
        page_url,
        published: text_or(data, "published_at", ""),
        package: pick(&wanted),
        checksum: pick(&wanted_checksum),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn platforms_like_xtask() {
        assert_eq!(platform_of("windows", "x86_64"), "windows-x64");
        assert_eq!(platform_of("macos", "aarch64"), "macos-arm64");
        assert_eq!(platform_of("linux", "x86_64"), "linux-x64");
        assert_eq!(platform_of("linux", "riscv64"), "linux-riscv64");
        assert_eq!(
            current_platform(),
            platform_of(std::env::consts::OS, std::env::consts::ARCH)
        );
    }

    #[test]
    fn archive_names_like_xtask() {
        assert_eq!(
            package_name("0.2.0", "windows-x64"),
            "OpenDRS-0.2.0-windows-x64.zip"
        );
        assert_eq!(
            package_name("0.2.0", "macos-arm64"),
            "OpenDRS-0.2.0-macos-arm64.zip"
        );
        assert_eq!(
            package_name("0.2.0", "linux-x64"),
            "OpenDRS-0.2.0-linux-x64.tar.gz"
        );
    }

    #[test]
    fn sizes_like_int() {
        let size = |v: Value| asset_size(&json!({ "size": v })).ok();
        assert_eq!(size(json!(3)), Some(3));
        assert_eq!(size(json!(3.9)), Some(3));
        assert_eq!(size(json!(" 1_000 ")), Some(1000));
        assert_eq!(size(json!(true)), Some(1));
        assert_eq!(size(json!(null)), Some(0));
        assert_eq!(size(json!("")), Some(0));
        assert_eq!(asset_size(&json!({})).ok(), Some(0));
        assert_eq!(size(json!("abc")), None);
        assert_eq!(size(json!("1__0")), None);
        assert_eq!(size(json!(-1)), None);
        assert_eq!(size(json!([1])), None);
    }

    #[test]
    fn digests() {
        let sha = "AB".repeat(32);
        assert_eq!(
            asset_sha256(&json!({ "digest": format!("sha256:{sha}") })),
            sha.to_lowercase()
        );
        assert_eq!(asset_sha256(&json!({ "digest": "md5:00" })), "");
        assert_eq!(asset_sha256(&json!({ "digest": "sha256:abc" })), "");
        assert_eq!(asset_sha256(&json!({ "digest": 5 })), "");
    }

    #[test]
    fn text_fields_like_str() {
        let data = json!({ "name": 5, "body": null, "html_url": "", "published_at": true });
        assert_eq!(text_or(&data, "name", "v1"), "5");
        assert_eq!(text_or(&data, "body", ""), "");
        assert_eq!(text_or(&data, "html_url", RELEASES_URL), RELEASES_URL);
        assert_eq!(text_or(&data, "published_at", ""), "True");
        assert_eq!(text_or(&data, "missing", "v1"), "v1");
    }
}
