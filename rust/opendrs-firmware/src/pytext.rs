//! The SHA-256 hex digest used for image identification (`hashlib.sha256(data).hexdigest()`).
//! Python's string behaviour (`strip`, `splitlines`, `repr`, `f"{n:,}"`) comes from
//! `opendrs_core::pystr`.

use sha2::{Digest, Sha256};

/// `hashlib.sha256(data).hexdigest()`.
pub fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `f"{n:,}"`: decimal digits grouped by three with commas.
pub(crate) fn group_thousands(n: usize) -> String {
    opendrs_core::pystr::thousands(n as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendrs_core::pystr::{splitlines, str_repr, strip};

    #[test]
    fn thousands_like_python() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(4_194_305), "4,194,305");
    }

    #[test]
    fn strip_and_splitlines_like_python() {
        assert_eq!(strip("\u{1c} a b \n"), "a b");
        assert_eq!(splitlines("a\r\nb\rc\n\nd"), ["a", "b", "c", "", "d"]);
        assert_eq!(splitlines("a\n"), ["a"]);
        assert!(splitlines("").is_empty());
    }

    #[test]
    fn repr_like_python() {
        assert_eq!(str_repr(r"C:\wav\ambep2.exe"), r"'C:\\wav\\ambep2.exe'");
        assert_eq!(str_repr("it's"), "\"it's\"");
        assert_eq!(str_repr("a\u{1}b"), "'a\\x01b'");
        assert_eq!(str_repr("錄音"), "'錄音'");
    }
}
