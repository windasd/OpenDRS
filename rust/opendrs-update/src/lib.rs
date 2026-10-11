//! Check GitHub Releases for a newer OpenDRS and install it (Windows) — a Rust port of the
//! Python module `update.py` (src/opendrs), with the same checks and the same messages.
//!
//! * [`fetch_latest`] reads the latest published release of windasd/OpenDRS and picks this
//!   platform's archive, `OpenDRS-<version>-<platform>.zip` (`.tar.gz` on Linux), as
//!   `cargo xtask dist` names it. The Python build's `OpenDRS-<version>-win64.zip` is never
//!   picked: a Rust build must not install the Python build over itself.
//! * [`prepare`] downloads the archive, checks its SHA-256 against the digest GitHub reports for
//!   the asset *and* the release's `.sha256` file (whichever exist; at least one is required, and
//!   they must agree), and unpacks it into a staging folder next to the installed program (same
//!   volume, so the swap is a rename).
//! * [`launch`] starts a PowerShell script that waits for this process to exit, swaps only the
//!   program files ([`PROGRAM_ITEMS`]; anything else in the folder is left alone, and settings,
//!   backups and recordings live in %APPDATA%), keeps the old files until the swap succeeded,
//!   rolls back on failure, and starts the new version.
//!
//! The checksums come from the same release as the archive: they catch corrupt or truncated
//! downloads, not a compromised repository.

mod checksum;
mod fetch;
pub mod http;
mod install;
mod release;
mod schedule;
mod unpack;
mod updater;
mod version;

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::TimeDelta;

pub use checksum::{expected_sha256, parse_checksum_file};
pub use fetch::{fetch_latest, fetch_latest_with};
pub use install::{
    PreparedUpdate, install_dir, prepare, prepare_with, remove_stale_staging, stale_staging,
};
pub use release::{Asset, Release, current_platform, package_name, parse_release};
pub use schedule::{check_due, now_stamp};
pub use unpack::{MAX_ENTRIES, extract_program};
pub use updater::{launch, powershell_exe, updater_command, updater_script};
pub use version::{Version, parse_version};

pub const REPO: &str = "windasd/OpenDRS";
pub const SITE_URL: &str = "https://github.com/windasd/OpenDRS";
pub const API_LATEST: &str = "https://api.github.com/repos/windasd/OpenDRS/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/windasd/OpenDRS/releases";
/// Only assets under `<DOWNLOAD_PREFIX><tag>/` are considered.
pub const DOWNLOAD_PREFIX: &str = "https://github.com/windasd/OpenDRS/releases/download/";
pub const USER_AGENT: &str = concat!("OpenDRS/", env!("CARGO_PKG_VERSION"), " (update check)");
/// The top folder of the Windows release archive.
pub const PROGRAM_DIR: &str = "OpenDRS";
/// The program the updater restarts; an archive without it is refused.
pub const PROGRAM_EXE: &str = "OpenDRS.exe";
/// What the updater replaces in the program folder (see `rust/xtask/src/layout.rs`).
pub const PROGRAM_ITEMS: [&str; 4] = ["OpenDRS.exe", "opendrs-rs.exe", "licenses", "README.txt"];
pub const STAGING_PREFIX: &str = ".opendrs-update-";
/// About once a day without drifting later each day.
pub const CHECK_INTERVAL: TimeDelta = TimeDelta::hours(20);
pub const MAX_JSON_BYTES: u64 = 2_000_000;
pub const MAX_ZIP_BYTES: u64 = 300_000_000;
pub const MAX_UNPACKED_BYTES: u64 = 1_000_000_000;
pub const MAX_CHECKSUM_BYTES: u64 = 4096;
pub const CHUNK: usize = 256 * 1024;
/// The default timeout of the release query (`fetch_latest(timeout=15.0)` in Python).
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(15);
/// Until the response headers of a download arrive (`prepare(timeout=60.0)` in Python).
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);
/// For the whole archive, so a stalled server cannot hang the download forever.
pub const DOWNLOAD_BODY_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Whether this build can install an update by itself ([`prepare`] and [`launch`]).
pub const INSTALL_SUPPORTED: bool = cfg!(windows);
/// What [`prepare`] and [`launch`] say where [`INSTALL_SUPPORTED`] is false.
pub const INSTALL_UNSUPPORTED: &str = "自動安裝更新目前只支援 Windows，請到發佈頁面手動下載";

/// A problem worth showing to the user as is (`Display` is the message).
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// Python's `UpdateError`.
    #[error("{0}")]
    Message(String),
    /// The user stopped the download (Python: `UpdateError("已取消")`).
    #[error("已取消")]
    Cancelled,
    /// A local file could not be read or written (an `OSError` that Python lets escape).
    #[error("{}：{source}", path.display())]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

pub(crate) fn message(text: impl Into<String>) -> UpdateError {
    UpdateError::Message(text.into())
}

/// `map_err(io_at(path))`: an I/O error about `path`.
pub(crate) fn io_at(path: &Path) -> impl FnOnce(std::io::Error) -> UpdateError + '_ {
    move |source| UpdateError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// `f"{n:,}"`: decimal digits grouped by three with commas.
pub(crate) fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `ch.isspace()` in Python: Unicode White_Space plus the separators U+001C..U+001F.
pub(crate) fn is_py_space(ch: char) -> bool {
    ch.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&ch)
}

/// `text.strip()` in Python.
pub(crate) fn py_strip(text: &str) -> &str {
    text.trim_matches(is_py_space)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thousands_like_python() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(4_194_305), "4,194,305");
        assert_eq!(thousands(300_000_000), "300,000,000");
    }

    #[test]
    fn strip_like_python() {
        assert_eq!(py_strip("\u{1c} v1.0 \r\n"), "v1.0");
        assert_eq!(py_strip("\u{3000}x\u{a0}"), "x");
    }

    #[test]
    fn urls_name_the_repository() {
        assert_eq!(SITE_URL, format!("https://github.com/{REPO}"));
        assert_eq!(RELEASES_URL, format!("{SITE_URL}/releases"));
        assert_eq!(
            API_LATEST,
            format!("https://api.github.com/repos/{REPO}/releases/latest")
        );
        assert_eq!(DOWNLOAD_PREFIX, format!("{SITE_URL}/releases/download/"));
        assert!(USER_AGENT.starts_with("OpenDRS/") && USER_AGENT.ends_with(" (update check)"));
    }
}
