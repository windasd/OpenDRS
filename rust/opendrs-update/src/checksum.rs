//! Which SHA-256 the archive must have (`parse_checksum_file`, `expected_sha256` and
//! `file_sha256` of `update.py`).

use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::release::{Release, current_platform, is_sha256};
use crate::{CHUNK, UpdateError, is_py_space, message};

/// `str.splitlines()` boundaries.
fn is_line_break(ch: char) -> bool {
    matches!(
        ch,
        '\n' | '\r'
            | '\u{0b}'
            | '\u{0c}'
            | '\u{1c}'
            | '\u{1d}'
            | '\u{1e}'
            | '\u{85}'
            | '\u{2028}'
            | '\u{2029}'
    )
}

/// The hex digest from a `sha256sum` line naming `file_name` (CR/LF tolerant; `*name`, the
/// binary-mode marker, is accepted).
pub fn parse_checksum_file(text: &str, file_name: &str) -> Result<String, UpdateError> {
    for line in text.split(is_line_break) {
        let parts: Vec<&str> = line.split(is_py_space).filter(|p| !p.is_empty()).collect();
        if let [digest, name] = parts.as_slice() {
            let digest = digest.to_lowercase();
            if name.trim_start_matches('*') == file_name && is_sha256(&digest) {
                return Ok(digest);
            }
        }
    }
    Err(message(format!(".sha256 檔裡沒有 {file_name} 的校驗碼")))
}

/// The digest the package must have: GitHub's and the `.sha256` file's must agree, and at least
/// one of them must exist.
pub fn expected_sha256(
    release: &Release,
    checksum_text: Option<&str>,
) -> Result<String, UpdateError> {
    let Some(package) = &release.package else {
        return Err(no_package(release));
    };
    let mut sources = Vec::new();
    if !package.sha256.is_empty() {
        sources.push(package.sha256.clone());
    }
    if let Some(text) = checksum_text {
        sources.push(parse_checksum_file(text, &package.name)?);
    }
    match sources.as_slice() {
        [] => Err(message(
            "這個版本沒有提供 SHA-256 校驗碼，為了安全不自動安裝",
        )),
        [first, rest @ ..] if rest.iter().any(|s| s != first) => {
            Err(message("GitHub 與 .sha256 檔的校驗碼不一致，不安裝"))
        }
        [first, ..] => Ok(first.clone()),
    }
}

pub(crate) fn no_package(release: &Release) -> UpdateError {
    message(format!(
        "{} 沒有這個平台（{}）的檔案可下載，請到發佈頁面手動下載",
        release.tag,
        current_platform()
    ))
}

/// `hashlib.sha256` of a file, as lowercase hex.
pub(crate) fn file_sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => digest.update(&buf[..n]),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => {}
            Err(err) => return Err(err),
        }
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// `bytes.decode("ascii", errors="replace")`.
pub(crate) fn ascii_lossy(data: &[u8]) -> String {
    data.iter()
        .map(|&b| {
            if b.is_ascii() {
                char::from(b)
            } else {
                '\u{fffd}'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_and_words_like_python() {
        let sha = "a".repeat(64);
        let text = format!("junk\u{2028}{sha}\u{1f}x.zip\u{0b}");
        assert_eq!(parse_checksum_file(&text, "x.zip").expect("found"), sha);
        assert!(parse_checksum_file(&format!("{sha} x.zip extra"), "x.zip").is_err());
    }

    #[test]
    fn non_ascii_bytes_become_replacement_characters() {
        assert_eq!(ascii_lossy(b"ab\xffc"), "ab\u{fffd}c");
    }
}
