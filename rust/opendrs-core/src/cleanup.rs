//! Remove control characters (mostly CR/LF) that the legacy CPS wrote into names (port of
//! `cleanup.py`).
//!
//! Works on bytes, independent of the name codec: in Big5 and GBK every trail byte is >= 0x40, so
//! a byte below 0x20 can never be half of a CJK character. Only the name body (up to the first
//! 0x00/0xFF terminator) is rewritten; the terminator and every byte after it stay as they were.

use crate::codeplug::with_synced_image;
use crate::f136::Project;
use crate::records::{Channel, Contact, NAME_SIZE, Zone};

fn is_control(b: u8) -> bool {
    (0x01..0x20).contains(&b) || b == 0x7F
}

fn body_len(field: &[u8]) -> usize {
    field
        .iter()
        .position(|b| *b == 0x00 || *b == 0xFF)
        .unwrap_or(field.len())
}

fn strip_spaces(part: &[u8]) -> &[u8] {
    let start = part.iter().position(|b| *b != b' ').unwrap_or(part.len());
    let end = part
        .iter()
        .rposition(|b| *b != b' ')
        .map_or(start, |e| e + 1);
    &part[start..end]
}

/// `field` with control bytes removed from its name body: leading/trailing control runs are
/// dropped, interior runs become one space, the freed bytes take the field's own terminator byte.
pub fn clean_field(field: &[u8]) -> Vec<u8> {
    let n = body_len(field);
    let body = &field[..n];
    if !body.iter().any(|b| is_control(*b)) {
        return field.to_vec();
    }
    let parts: Vec<&[u8]> = body
        .split(|b| is_control(*b))
        .map(strip_spaces)
        .filter(|p| !p.is_empty())
        .collect();
    let cleaned = parts.join(&b' ');
    let pad = field.get(n).copied().unwrap_or(0x00);
    let mut out = cleaned;
    out.resize(n, pad);
    out.extend_from_slice(&field[n..]);
    out
}

fn needs_cleaning_field(field: &[u8]) -> bool {
    field[..body_len(field)].iter().any(|b| is_control(*b))
}

fn cleaned<const N: usize>(raw: &[u8; N], offset: usize) -> [u8; N] {
    let mut out = *raw;
    out[offset..offset + NAME_SIZE].copy_from_slice(&clean_field(&raw[offset..offset + NAME_SIZE]));
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CleanupReport {
    pub contacts: usize,
    pub zones: usize,
    pub channels: usize,
}

impl CleanupReport {
    pub fn total(&self) -> usize {
        self.contacts + self.zones + self.channels
    }

    pub fn summary(&self) -> String {
        let parts: Vec<String> = [
            ("聯絡人", self.contacts),
            ("分區", self.zones),
            ("頻道", self.channels),
        ]
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(label, n)| format!("{label} {n} 筆"))
        .collect();
        if parts.is_empty() {
            "沒有".into()
        } else {
            parts.join("、")
        }
    }
}

pub fn scan(project: &Project) -> CleanupReport {
    CleanupReport {
        contacts: project
            .contacts
            .iter()
            .filter(|c| needs_cleaning_field(c.name_field()))
            .count(),
        zones: project
            .zones
            .iter()
            .filter(|z| needs_cleaning_field(z.name_field()))
            .count(),
        channels: project
            .channels
            .iter()
            .filter(|c| needs_cleaning_field(c.name_field()))
            .count(),
    }
}

pub fn clean_names(project: &Project) -> (Project, CleanupReport) {
    let report = scan(project);
    if report.total() == 0 {
        return (project.clone(), report);
    }
    let result = Project {
        contacts: project
            .contacts
            .iter()
            .map(|c| Contact(cleaned(&c.0, Contact::NAME_OFFSET)))
            .collect(),
        zones: project
            .zones
            .iter()
            .map(|z| Zone(cleaned(&z.0, Zone::NAME_OFFSET)))
            .collect(),
        channels: project
            .channels
            .iter()
            .map(|c| Channel(cleaned(&c.0, Channel::NAME_OFFSET)))
            .collect(),
        ..project.clone()
    };
    (with_synced_image(&result), report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_runs_are_dropped_or_become_one_space() {
        assert_eq!(clean_field(b"QA0EL\r\n\0\0\0"), b"QA0EL\0\0\0\0\0");
        assert_eq!(
            clean_field(b"\rA\r\nB \xff\xff"),
            b"A B\xff\xff\xff\xff\xff"
        );
        assert_eq!(clean_field(b"plain\0\0\0\0\0"), b"plain\0\0\0\0\0");
    }
}
