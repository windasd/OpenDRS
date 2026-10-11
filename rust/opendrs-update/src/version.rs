//! Version numbers as the release tags write them (`parse_version` of `update.py`).

use crate::{UpdateError, message, py_strip};

/// A comparable version: the release numbers padded to four, then the pre-release rank
/// (`a` < `b` < `rc` < final release) and number, so `0.2.0rc1` sorts before `0.2.0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub release: [u64; 4],
    /// 0 = `a`, 1 = `b`, 2 = `rc`, 3 = a final release.
    pub pre_rank: u8,
    pub pre_number: u64,
}

const FINAL_RANK: u8 = 3;
const PRE_RELEASES: [(&str, u8); 3] = [("rc", 2), ("a", 0), ("b", 1)];

/// `'v0.2.0'`, `'0.2'`, `'1.0.0rc1'` -> comparable versions (`^v?(\d+(?:\.\d+){0,3})(?:(a|b|rc)(\d+))?$`
/// after stripping whitespace; ASCII digits only).
pub fn parse_version(text: &str) -> Result<Version, UpdateError> {
    parse(py_strip(text)).ok_or_else(|| message(format!("無法辨識的版本號「{text}」")))
}

fn number(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn parse(text: &str) -> Option<Version> {
    let text = text.strip_prefix('v').unwrap_or(text);
    let split = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let (head, tail) = text.split_at(split);
    let parts: Vec<&str> = head.split('.').collect();
    if parts.len() > 4 {
        return None;
    }
    let mut release = [0u64; 4];
    for (slot, part) in release.iter_mut().zip(&parts) {
        *slot = number(part)?;
    }
    let (pre_rank, pre_number) = if tail.is_empty() {
        (FINAL_RANK, 0)
    } else {
        PRE_RELEASES.iter().find_map(|(tag, rank)| {
            let digits = tail.strip_prefix(tag)?;
            Some((*rank, number(digits)?))
        })?
    };
    Some(Version {
        release,
        pre_rank,
        pre_number,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes_the_pattern_refuses() {
        for bad in [
            "",
            "v",
            "1.",
            ".1",
            "1..2",
            "1.2.3.4.5",
            "1rc",
            "1a1b1",
            "1.0-rc1",
            "V1.0",
            "1.0 rc1",
            "١.٠",
        ] {
            assert!(parse(py_strip(bad)).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn pre_release_numbers() {
        let rc = parse("1.0.0rc01").expect("rc");
        assert_eq!(
            (rc.release, rc.pre_rank, rc.pre_number),
            ([1, 0, 0, 0], 2, 1)
        );
        assert_eq!(parse("v1.2.3.4").expect("four").release, [1, 2, 3, 4]);
    }
}
