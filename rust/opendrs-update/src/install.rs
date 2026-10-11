//! Getting an update ready next to the installed program (`install_dir`, `writable`,
//! `prepare` and `stale_staging` of `update.py`).

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::checksum::{ascii_lossy, expected_sha256, file_sha256, no_package};
use crate::fetch::{Download, download};
use crate::http::{Fetcher, UreqFetcher};
use crate::release::Release;
use crate::unpack::extract_program;
use crate::{
    DOWNLOAD_BODY_TIMEOUT, DOWNLOAD_TIMEOUT, INSTALL_SUPPORTED, INSTALL_UNSUPPORTED,
    MAX_CHECKSUM_BYTES, MAX_ZIP_BYTES, STAGING_PREFIX, UpdateError, io_at, message,
};

/// An update downloaded, verified and unpacked, waiting for the updater script.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreparedUpdate {
    pub version: String,
    /// The unpacked `OpenDRS` folder.
    pub source: PathBuf,
    /// The installed program folder.
    pub target: PathBuf,
    /// Removed by the updater script when it is done.
    pub staging: PathBuf,
}

/// The folder of the running program when it is an installed release (`README.txt` and
/// `licenses/` next to it, as `cargo xtask dist` lays it out); `None` for a development build
/// (`cargo run`), like Python's `None` when running from source.
pub fn install_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    release_dir_of(&exe)
}

fn release_dir_of(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    (dir.join("README.txt").is_file() && dir.join("licenses").is_dir()).then(|| dir.to_path_buf())
}

/// Whether a file can be created in `folder` (a probe named like the staging folders).
fn writable(folder: &Path) -> bool {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    for attempt in 0..100u32 {
        let name = format!(
            "{STAGING_PREFIX}{}-{stamp:x}-{attempt}.probe",
            std::process::id()
        );
        let probe = folder.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&probe) {
            Ok(file) => {
                drop(file);
                // Best effort: a leftover probe file is harmless.
                let _ = fs::remove_file(&probe);
                return true;
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return false,
        }
    }
    false
}

/// A single file or folder name (no separators, not `.` or `..`): asset and staging names come
/// from GitHub's data and become paths.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':'])
}

/// Download, verify and unpack `release` next to the program in `target` (see
/// [`prepare_with`]); only where [`INSTALL_SUPPORTED`].
pub fn prepare(
    release: &Release,
    target: &Path,
    progress: &mut dyn FnMut(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<PreparedUpdate, UpdateError> {
    if !INSTALL_SUPPORTED {
        return Err(message(INSTALL_UNSUPPORTED));
    }
    prepare_with(release, target, &UreqFetcher, progress, cancelled)
}

/// [`prepare`] through `fetcher`, on any platform: the `.sha256` file first (when the release
/// has one), then the archive into `<target>/.opendrs-update-<version>/`, checked against the
/// expected digest and unpacked to `unpacked/OpenDRS`. On any failure the staging folder goes.
pub fn prepare_with(
    release: &Release,
    target: &Path,
    fetcher: &dyn Fetcher,
    progress: &mut dyn FnMut(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<PreparedUpdate, UpdateError> {
    let Some(package) = &release.package else {
        return Err(no_package(release));
    };
    if !writable(target) {
        return Err(message(format!(
            "沒有權限寫入程式所在的資料夾 {}；請手動下載並解壓縮",
            target.display()
        )));
    }
    let staging_name = format!("{STAGING_PREFIX}{}", release.version);
    let names_ok = plain_name(&staging_name)
        && plain_name(&package.name)
        && release
            .checksum
            .as_ref()
            .is_none_or(|c| plain_name(&c.name));
    if !names_ok {
        return Err(message("GitHub 回傳的發佈資料格式不符"));
    }
    let staging = target.join(staging_name);
    // A folder left by an earlier attempt is replaced.
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir(&staging).map_err(io_at(&staging))?;
    let result = fill_staging(release, &staging, fetcher, progress, cancelled);
    if result.is_err() {
        // Best effort: the error being returned matters more than a leftover folder.
        let _ = fs::remove_dir_all(&staging);
    }
    result.map(|source| PreparedUpdate {
        version: release.version.clone(),
        source,
        target: target.to_path_buf(),
        staging,
    })
}

fn fill_staging(
    release: &Release,
    staging: &Path,
    fetcher: &dyn Fetcher,
    progress: &mut dyn FnMut(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<PathBuf, UpdateError> {
    let Some(package) = &release.package else {
        return Err(no_package(release));
    };
    let checksum_text = match &release.checksum {
        Some(checksum) => {
            let path = staging.join(&checksum.name);
            let file = Download {
                url: &checksum.url,
                limit: MAX_CHECKSUM_BYTES,
                expected_size: 0,
                timeout: DOWNLOAD_TIMEOUT,
                body_timeout: DOWNLOAD_TIMEOUT,
            };
            download(fetcher, &file, &path, None, cancelled)?;
            Some(ascii_lossy(&fs::read(&path).map_err(io_at(&path))?))
        }
        None => None,
    };
    let expected = expected_sha256(release, checksum_text.as_deref())?;
    if package.size > MAX_ZIP_BYTES {
        return Err(message("下載的檔案超過大小上限，已中止"));
    }
    let archive = staging.join(&package.name);
    let file = Download {
        url: &package.url,
        limit: MAX_ZIP_BYTES,
        expected_size: package.size,
        timeout: DOWNLOAD_TIMEOUT,
        body_timeout: DOWNLOAD_BODY_TIMEOUT,
    };
    download(fetcher, &file, &archive, Some(progress), cancelled)?;
    if file_sha256(&archive).map_err(io_at(&archive))? != expected {
        return Err(message(
            "下載的檔案校驗碼不符（可能下載不完整或被竄改），未安裝",
        ));
    }
    // Hashing and unpacking take a while: a cancel meanwhile still counts.
    if cancelled() {
        return Err(UpdateError::Cancelled);
    }
    let source = extract_program(&archive, &staging.join("unpacked"))?;
    if cancelled() {
        return Err(UpdateError::Cancelled);
    }
    // Best effort: the update is already verified and unpacked; the folder goes after the swap.
    let _ = fs::remove_file(&archive);
    Ok(source)
}

/// Remove leftover staging folders last changed more than `older_than` ago (younger ones may
/// belong to an update being prepared or applied right now); returns how many went.
pub fn remove_stale_staging(target: &Path, older_than: std::time::Duration) -> usize {
    let now = SystemTime::now();
    stale_staging(target)
        .into_iter()
        .filter(|dir| {
            fs::metadata(dir)
                .and_then(|m| m.modified())
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > older_than)
        })
        .filter(|dir| fs::remove_dir_all(dir).is_ok())
        .count()
}

/// Leftover staging folders from an update that was prepared but never applied (sorted).
pub fn stale_staging(target: &Path) -> Vec<PathBuf> {
    let Ok(listing) = fs::read_dir(target) else {
        return Vec::new();
    };
    let mut found: Vec<PathBuf> = listing
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(STAGING_PREFIX))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("opendrs-update-unit-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn an_installed_release_has_readme_and_licenses_next_to_the_program() {
        let dir = temp_dir("release-dir");
        let exe = dir.join("OpenDRS.exe");
        fs::write(&exe, b"exe").expect("exe");
        assert_eq!(release_dir_of(&exe), None);
        fs::write(dir.join("README.txt"), b"readme").expect("readme");
        assert_eq!(release_dir_of(&exe), None);
        fs::create_dir(dir.join("licenses")).expect("licenses");
        assert_eq!(release_dir_of(&exe), Some(dir.clone()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_test_runner_is_not_an_installed_release() {
        assert_eq!(install_dir(), None);
    }

    #[test]
    fn writable_leaves_no_probe_behind() {
        let dir = temp_dir("writable");
        assert!(writable(&dir));
        assert!(fs::read_dir(&dir).expect("list").next().is_none());
        assert!(!writable(&dir.join("missing")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn names_from_github_must_be_plain() {
        assert!(plain_name("OpenDRS-0.2.0-windows-x64.zip"));
        for bad in ["", ".", "..", "a/b", "a\\b", "C:x"] {
            assert!(!plain_name(bad), "{bad:?}");
        }
    }
}
