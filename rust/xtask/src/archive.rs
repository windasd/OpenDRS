//! The release archive (.zip for Windows and macOS, .tar.gz for Linux) of a staged folder, with
//! Unix permissions kept, and its `.sha256` file in `sha256sum` format.

use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::Result;

/// A file of the staged folder: its path inside the archive and whether it is a program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub source: PathBuf,
    pub name: String,
    pub executable: bool,
}

const EXECUTABLE: u32 = 0o755;
const REGULAR: u32 = 0o644;

/// Every file under `root`, named `prefix/relative/path` with forward slashes, sorted.
pub fn entries(
    root: &Path,
    prefix: &str,
    is_executable: &dyn Fn(&str) -> bool,
) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let listing = std::fs::read_dir(&dir).map_err(|e| format!("{}：{e}", dir.display()))?;
        for item in listing {
            let path = item.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push(Entry {
                executable: is_executable(&relative),
                name: format!("{prefix}/{relative}"),
                source: path,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// UTC (year, month, day, hour, minute, second) of `time` (Howard Hinnant's civil_from_days).
pub fn civil(time: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let secs = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (days, rest) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    let rest = rest as u32;
    (year, month, day, rest / 3600, rest / 60 % 60, rest % 60)
}

fn zip_time(time: SystemTime) -> zip::DateTime {
    let (y, mo, d, h, mi, s) = civil(time);
    u16::try_from(y)
        .ok()
        .and_then(|y| {
            zip::DateTime::from_date_and_time(y, mo as u8, d as u8, h as u8, mi as u8, s as u8).ok()
        })
        .unwrap_or_default()
}

pub fn write_zip(path: &Path, files: &[Entry], time: SystemTime) -> Result<()> {
    let file = File::create(path).map_err(|e| format!("{}：{e}", path.display()))?;
    let mut zip = zip::ZipWriter::new(BufWriter::new(file));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        // Unix attributes even when built on Windows (the default there is DOS, which has no
        // execute bit), so the macOS and Linux programs stay executable.
        .system(zip::System::Unix)
        .last_modified_time(zip_time(time));
    for entry in files {
        let mode = if entry.executable {
            EXECUTABLE
        } else {
            REGULAR
        };
        zip.start_file(entry.name.as_str(), options.unix_permissions(mode))
            .map_err(|e| e.to_string())?;
        let mut source = File::open(&entry.source).map_err(|e| e.to_string())?;
        io::copy(&mut source, &mut zip).map_err(|e| e.to_string())?;
    }
    zip.finish()
        .map_err(|e| e.to_string())?
        .flush()
        .map_err(|e| e.to_string())
}

pub fn write_tar_gz(path: &Path, files: &[Entry], time: SystemTime) -> Result<()> {
    let file = File::create(path).map_err(|e| format!("{}：{e}", path.display()))?;
    let encoder = flate2::write::GzEncoder::new(BufWriter::new(file), flate2::Compression::best());
    let mut tar = tar::Builder::new(encoder);
    let mtime = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    for entry in files {
        let data = std::fs::read(&entry.source).map_err(|e| e.to_string())?;
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(if entry.executable {
            EXECUTABLE
        } else {
            REGULAR
        });
        header.set_mtime(mtime);
        tar.append_data(&mut header, &entry.name, data.as_slice())
            .map_err(|e| e.to_string())?;
    }
    let encoder = tar.into_inner().map_err(|e| e.to_string())?;
    encoder
        .finish()
        .map_err(|e| e.to_string())?
        .flush()
        .map_err(|e| e.to_string())
}

/// SHA-256 of a file, as lowercase hex.
pub fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    loop {
        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `<archive>.sha256` holding `<hex>  <file name>`, as `sha256sum -c` reads it.
pub fn write_checksum(archive: &Path) -> Result<String> {
    let digest = sha256(archive)?;
    let name = archive
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let mut target = archive.as_os_str().to_owned();
    target.push(".sha256");
    std::fs::write(PathBuf::from(target), format!("{digest}  {name}\n"))
        .map_err(|e| e.to_string())?;
    Ok(digest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn civil_dates() {
        let at = |secs| civil(SystemTime::UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(at(0), (1970, 1, 1, 0, 0, 0));
        assert_eq!(at(951_782_400), (2000, 2, 29, 0, 0, 0));
        assert_eq!(at(1_791_633_600 + 3661), (2026, 10, 10, 13, 1, 1));
    }

    #[test]
    fn archives_keep_names_and_programs() {
        let dir = std::env::temp_dir().join(format!("xtask-archive-{}", std::process::id()));
        let root = dir.join("stage");
        std::fs::create_dir_all(root.join("licenses")).expect("stage");
        std::fs::write(root.join("opendrs"), b"#!bin").expect("write");
        std::fs::write(root.join("licenses/A.txt"), b"text").expect("write");
        let files = entries(&root, "OpenDRS", &|name| name == "opendrs").expect("entries");
        let names: Vec<&str> = files.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["OpenDRS/licenses/A.txt", "OpenDRS/opendrs"]);
        assert!(files[1].executable && !files[0].executable);

        let zip_path = dir.join("a.zip");
        write_zip(&zip_path, &files, SystemTime::now()).expect("zip");
        let mut archive = zip::ZipArchive::new(File::open(&zip_path).expect("open")).expect("read");
        let program = archive.by_name("OpenDRS/opendrs").expect("entry");
        assert_eq!(program.unix_mode().map(|m| m & 0o777), Some(0o755));
        drop(program);

        let tgz = dir.join("a.tar.gz");
        write_tar_gz(&tgz, &files, SystemTime::now()).expect("tar");
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(
            File::open(&tgz).expect("open"),
        ));
        let modes: Vec<(String, u32)> = tar
            .entries()
            .expect("entries")
            .map(|e| {
                let e = e.expect("entry");
                (
                    e.path().expect("path").display().to_string(),
                    e.header().mode().expect("mode"),
                )
            })
            .collect();
        assert_eq!(
            modes,
            [
                ("OpenDRS/licenses/A.txt".to_string(), 0o644),
                ("OpenDRS/opendrs".to_string(), 0o755)
            ]
        );
        let digest = write_checksum(&tgz).expect("checksum");
        let line = std::fs::read_to_string(dir.join("a.tar.gz.sha256")).expect("sha file");
        assert_eq!(line, format!("{digest}  a.tar.gz\n"));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
