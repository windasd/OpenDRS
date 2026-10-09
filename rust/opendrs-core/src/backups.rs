//! The backups kept by every radio read and write (port of `backups.py`; written by
//! [`crate::radio_link::save_backup`]).
//!
//! Each backup is a pair in one folder: `<YYYYmmdd_HHMMSS>_<kind>_<model>.F136` (the project) and
//! the same name with `.bin` (the raw transfer stream); `kind` is `read` or `before_write`. Only
//! files that follow this pattern are listed, and deleting a backup removes exactly its two files.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::{Datelike, NaiveDateTime, Timelike};
use regex::Regex;

use crate::codeplug::Counts;
use crate::f136;

pub const BEFORE_WRITE: &str = "before_write";
pub const STAMP_FORMAT: &str = "%Y%m%d_%H%M%S";
pub const TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

static NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^([0-9]{8}_[0-9]{6})_(read|before_write)_([0-9A-Za-z_-]+)\.F136$")
        .expect("valid regex")
});

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct BackupError(pub String);

/// 「讀取」／「寫入前」 for a backup kind.
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "read" => "讀取",
        BEFORE_WRITE => "寫入前",
        other => other,
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backup {
    /// The .F136 project.
    pub path: PathBuf,
    pub when: NaiveDateTime,
    pub kind: String,
    pub model: String,
    /// Bytes of the .F136 and the .bin together.
    pub size: u64,
    /// `None` when the project cannot be read (see `error`).
    pub counts: Option<Counts>,
    pub error: String,
}

impl Backup {
    pub fn raw_path(&self) -> PathBuf {
        self.path.with_extension("bin")
    }

    pub fn kind_label(&self) -> &str {
        kind_label(&self.kind)
    }

    pub fn time_text(&self) -> String {
        self.when.format(TIME_FORMAT).to_string()
    }

    /// The backup's files that exist: the .F136, then the .bin.
    pub fn files(&self) -> Vec<PathBuf> {
        [self.path.clone(), self.raw_path()]
            .into_iter()
            .filter(|p| p.is_file())
            .collect()
    }
}

/// (time, kind, model) from a backup file name, or `None` when it is not one.
pub fn parse_name(name: &str) -> Option<(NaiveDateTime, String, String)> {
    let captures = NAME.captures(name)?;
    // Python's datetime has no year 0 and no leap second (chrono accepts both).
    let when = NaiveDateTime::parse_from_str(&captures[1], STAMP_FORMAT)
        .ok()
        .filter(|w| w.year() >= 1 && w.nanosecond() < 1_000_000_000)?;
    Some((when, captures[2].to_string(), captures[3].to_string()))
}

fn size_of(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| m.len())
}

/// Describe one backup (reads the whole project); `None` when `path` is not a backup.
pub fn load_backup(path: &Path) -> Option<Backup> {
    let (when, kind, model) = parse_name(path.file_name()?.to_str()?)?;
    let (counts, error) = match f136::load(path) {
        Ok(project) => (Some(Counts::of(&project)), String::new()),
        Err(e) => (None, e.to_string()),
    };
    Some(Backup {
        path: path.to_path_buf(),
        when,
        kind,
        model,
        size: size_of(path) + size_of(&path.with_extension("bin")),
        counts,
        error,
    })
}

pub fn newest_first(backups: &[Backup]) -> Vec<Backup> {
    let mut ordered = backups.to_vec();
    ordered.sort_by(|a, b| (b.when, b.path.file_name()).cmp(&(a.when, a.path.file_name())));
    ordered
}

/// Every backup in `folder`, newest first; empty when the folder does not exist.
pub fn list_backups(folder: &Path) -> io::Result<Vec<Backup>> {
    if !folder.is_dir() {
        return Ok(Vec::new());
    }
    let mut found = Vec::new();
    for entry in fs::read_dir(folder)? {
        let path = entry?.path();
        if path.extension().is_some_and(|e| e == "F136") && path.is_file() {
            found.extend(load_backup(&path));
        }
    }
    Ok(newest_first(&found))
}

/// Remove the backup's .F136 and .bin (nothing else); returns what was removed.
pub fn delete_backup(backup: &Backup) -> Result<Vec<PathBuf>, BackupError> {
    let name = backup
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    if parse_name(&name).is_none() {
        return Err(BackupError(format!("不是備份檔：{name}")));
    }
    let mut removed = Vec::new();
    for path in backup.files() {
        fs::remove_file(&path)
            .map_err(|e| BackupError(format!("無法刪除 {}：{e}", path.display())))?;
        removed.push(path);
    }
    Ok(removed)
}

/// The backups to delete so only the newest `keep` remain — plus the newest before_write
/// backup, which is always kept (it is what a failed write rolls back to).
pub fn cleanup_candidates(backups: &[Backup], keep: usize) -> Result<Vec<Backup>, BackupError> {
    if keep < 1 {
        return Err(BackupError("至少要保留 1 份備份".into()));
    }
    let ordered = newest_first(backups);
    let mut kept: Vec<&Backup> = ordered.iter().take(keep).collect();
    if !kept.iter().any(|b| b.kind == BEFORE_WRITE) {
        kept.extend(ordered.iter().find(|b| b.kind == BEFORE_WRITE));
    }
    Ok(ordered
        .iter()
        .filter(|b| !kept.contains(b))
        .cloned()
        .collect())
}

/// 「17 bytes」／「46.8 KB」／「1.2 MB」.
pub fn size_text(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{size} bytes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup(stamp: &str, kind: &str) -> Backup {
        let name = format!("{stamp}_{kind}_DR8600.F136");
        let (when, kind, model) = parse_name(&name).unwrap();
        Backup {
            path: PathBuf::from(name),
            when,
            kind,
            model,
            size: 0,
            counts: None,
            error: String::new(),
        }
    }

    #[test]
    fn names_follow_the_saved_pattern() {
        let (when, kind, model) = parse_name("20261009_235959_before_write_DR-8600.F136").unwrap();
        assert_eq!(when.format(TIME_FORMAT).to_string(), "2026-10-09 23:59:59");
        assert_eq!((kind.as_str(), model.as_str()), (BEFORE_WRITE, "DR-8600"));
        for bad in [
            "20261309_000000_read_X.F136",     // month 13
            "20260230_000000_read_X.F136",     // 30 February
            "20261009_235960_read_X.F136",     // leap second
            "00001009_000000_read_X.F136",     // year 0
            "２０２６1009_000000_read_X.F136", // only ASCII digits
            "20261009_000000_write_X.F136",
            "20261009_000000_read_X.bin",
            "20261009_000000_read_X.F136\n",
            "20261009_000000_read_X Y.F136",
        ] {
            assert_eq!(parse_name(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn cleanup_keeps_the_newest_and_the_last_before_write() {
        let all = [
            backup("20261001_000000", "read"),
            backup("20261002_000000", BEFORE_WRITE),
            backup("20261003_000000", "read"),
            backup("20261004_000000", "read"),
        ];
        let doomed = cleanup_candidates(&all, 2).unwrap();
        let names: Vec<_> = doomed.iter().map(|b| b.time_text()).collect();
        assert_eq!(names, ["2026-10-01 00:00:00"]);
        assert_eq!(cleanup_candidates(&all, 4).unwrap(), []);
        assert!(cleanup_candidates(&all, 0).is_err());
    }

    #[test]
    fn sizes_round_like_python() {
        assert_eq!(size_text(17), "17 bytes");
        assert_eq!(size_text(1280), "1.2 KB"); // 1.25: ties to even, as Python's format
        assert_eq!(size_text(2560), "2.5 KB");
        assert_eq!(size_text(1_310_720), "1.2 MB");
        assert_eq!(size_text(1_625_088), "1.5 MB");
    }

    #[test]
    fn list_and_delete_touch_only_backup_pairs() {
        let dir = std::env::temp_dir().join(format!("opendrs-backups-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let stem = dir.join("20261009_101010_read_DR8600");
        fs::write(stem.with_extension("F136"), b"x\r\n\r\n").unwrap(); // a blank line
        fs::write(stem.with_extension("bin"), b"raw").unwrap();
        fs::write(dir.join("notes.F136"), b"x").unwrap();
        let found = list_backups(&dir).unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].counts.is_none() && !found[0].error.is_empty());
        assert_eq!(found[0].size, 8); // both files
        assert_eq!(delete_backup(&found[0]).unwrap().len(), 2);
        assert!(dir.join("notes.F136").is_file());
        assert_eq!(list_backups(&dir.join("missing")).unwrap(), []);
        fs::remove_dir_all(&dir).unwrap();
    }
}
