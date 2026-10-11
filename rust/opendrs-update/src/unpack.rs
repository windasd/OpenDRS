//! Unpacking the Windows release zip (`extract_program` of `update.py`): every member must stay
//! inside `OpenDRS/`, nothing is written before the whole archive has been checked.

use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use zip::ZipArchive;
use zip::read::ZipFileEntry;

use crate::{
    CHUNK, MAX_UNPACKED_BYTES, PROGRAM_DIR, PROGRAM_EXE, PROGRAM_ITEMS, UpdateError, io_at, message,
};

const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;
/// A release has a few hundred members at most.
pub const MAX_ENTRIES: usize = 10_000;

fn bad_zip() -> UpdateError {
    message("更新檔不是有效的 zip")
}

fn not_allowed(name: &str) -> UpdateError {
    message(format!("更新檔含有不允許的路徑：{name}"))
}

/// A name Windows would not store as written: a trailing `.` or space (dropped, so
/// `OpenDRS.exe.` would overwrite `OpenDRS.exe`) or a device name such as `NUL` or `COM1`, with
/// or without an extension (written to the device instead of a file).
fn windows_alias(component: &str) -> bool {
    if component.ends_with(['.', ' ']) {
        return true;
    }
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .trim_end_matches(' ')
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
}

/// The path components of a member inside `OpenDRS/` (without `OpenDRS` itself), or `None` when
/// the name is absolute, climbs out (`..`), uses `\` or `:`, has empty or `.` components or
/// control characters, lies outside `OpenDRS/`, or names something Windows would alias.
fn member_parts(name: &str, is_dir: bool) -> Option<Vec<&str>> {
    if name.starts_with('/') || name.contains(['\\', ':']) || name.chars().any(char::is_control) {
        return None;
    }
    let trimmed = if is_dir {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    let mut parts = trimmed.split('/');
    if parts.next() != Some(PROGRAM_DIR) {
        return None;
    }
    let rest: Vec<&str> = parts.collect();
    let plain = rest
        .iter()
        .all(|p| !p.is_empty() && *p != "." && *p != ".." && !windows_alias(p));
    // A file needs a name below OpenDRS/; `OpenDRS/` itself may appear as a directory entry.
    (plain && (is_dir || !rest.is_empty())).then_some(rest)
}

fn is_symlink(entry: &ZipFileEntry<'_>) -> bool {
    entry
        .unix_mode()
        .is_some_and(|mode| mode & S_IFMT == S_IFLNK)
}

/// A member to write: where it goes below the destination, and its declared size.
struct Member {
    index: usize,
    parts: Vec<String>,
    is_dir: bool,
    size: u64,
}

/// Check every member before anything is written (Python's first loop and the completeness
/// test).
fn plan<R: Read + io::Seek>(zip: &ZipArchive<R>) -> Result<Vec<Member>, UpdateError> {
    if zip.len() > MAX_ENTRIES {
        return Err(message("更新檔的檔案數量過多"));
    }
    let mut members = Vec::with_capacity(zip.len());
    let mut total: u64 = 0;
    let mut has_program = false;
    // Windows folds case: `README.txt` and `Readme.TXT` would land in one file.
    let mut seen = HashSet::new();
    for index in 0..zip.len() {
        let entry = zip.by_index_data(index).map_err(|_| bad_zip())?;
        let name = entry.name().map_err(|_| bad_zip())?;
        let is_dir = entry.is_dir();
        let parts = member_parts(&name, is_dir)
            .filter(|_| !is_symlink(&entry))
            .ok_or_else(|| not_allowed(&name))?;
        // Only what this version's updater swaps: anything else would be left behind silently.
        if let Some(top) = parts.first() {
            if !PROGRAM_ITEMS.contains(top) {
                return Err(message(format!(
                    "更新檔含有這個版本不認得的項目：{name}，請到發佈頁面手動下載"
                )));
            }
        }
        if !seen.insert(parts.join("/").to_lowercase()) {
            return Err(not_allowed(&name));
        }
        total = total.saturating_add(entry.size());
        has_program |= !is_dir && parts == [PROGRAM_EXE];
        members.push(Member {
            index,
            parts: parts.into_iter().map(str::to_string).collect(),
            is_dir,
            size: entry.size(),
        });
    }
    if total > MAX_UNPACKED_BYTES {
        return Err(message("更新檔解壓後過大"));
    }
    if !has_program {
        return Err(message(format!("更新檔內容不完整（缺少 {PROGRAM_EXE}）")));
    }
    Ok(members)
}

/// Copy at most `size` bytes of a member: reading errors (a bad CRC, broken data) mean a broken
/// zip, more data than declared means the archive lied about its size.
fn copy_member(mut from: impl Read, to: &Path, size: u64) -> Result<(), UpdateError> {
    let mut out = File::create(to).map_err(io_at(to))?;
    let mut buf = vec![0u8; CHUNK];
    let mut written: u64 = 0;
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(bad_zip()),
        };
        written += n as u64;
        if written > size {
            return Err(message("更新檔解壓後過大"));
        }
        out.write_all(&buf[..n]).map_err(io_at(to))?;
    }
    out.flush().map_err(io_at(to))
}

fn create_dir(path: &Path) -> Result<(), UpdateError> {
    fs::create_dir_all(path).map_err(io_at(path))
}

/// Unpack the release zip into `dest` after checking every member stays inside `OpenDRS/`;
/// returns `dest/OpenDRS`.
pub fn extract_program(archive: &Path, dest: &Path) -> Result<PathBuf, UpdateError> {
    let file = File::open(archive).map_err(io_at(archive))?;
    let mut zip = ZipArchive::new(io::BufReader::new(file)).map_err(|_| bad_zip())?;
    let members = plan(&zip)?;
    let root = dest.join(PROGRAM_DIR);
    create_dir(&root)?;
    for member in &members {
        let path = member.parts.iter().fold(root.clone(), |p, c| p.join(c));
        if member.is_dir {
            create_dir(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            create_dir(parent)?;
        }
        let entry = zip.by_index(member.index).map_err(|_| bad_zip())?;
        copy_member(entry, &path, member.size)?;
    }
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn member_names() {
        assert_eq!(
            member_parts("OpenDRS/licenses/A.txt", false),
            Some(vec!["licenses", "A.txt"])
        );
        assert_eq!(member_parts("OpenDRS/", true), Some(vec![]));
        assert_eq!(
            member_parts("OpenDRS/licenses/", true),
            Some(vec!["licenses"])
        );
        for bad in [
            "OpenDRS",
            "OpenDRS/",
            "OpenDRS/../evil.exe",
            "/OpenDRS/abs.exe",
            "Other/OpenDRS.exe",
            "OpenDRS/..\\evil.exe",
            "OpenDRS/C:evil.exe",
            "OpenDRS//x",
            "OpenDRS/./x",
            "OpenDRS/a\u{0}b",
            "opendrs/OpenDRS.exe",
            "OpenDRS/OpenDRS.exe.",
            "OpenDRS/README.txt ",
            "OpenDRS/NUL",
            "OpenDRS/licenses/nul.txt",
            "OpenDRS/com1",
            "OpenDRS/LPT9.log",
        ] {
            assert_eq!(member_parts(bad, false), None, "{bad:?}");
        }
    }
}
