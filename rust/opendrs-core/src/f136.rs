//! Read and write `.F136` project files, also the legacy `backup.ini` format (port of `f136.py`).
//!
//! * CRLF lines, the last one also terminated by CRLF.
//! * Labelled rows first (`Contacts\t1\t0\t4\t…`): one record per row, decimal values separated by
//!   single tabs.
//! * Then the settings image as unlabelled rows of up to 100 decimal byte values.
//!
//! Unmodified projects are written back byte-for-byte identical.

use std::fs;
use std::path::Path;

use crate::codeplug::{MAX_CONTACTS, with_synced_image};
use crate::device::{self, DeviceError, DeviceInfo, frequency_problem};
use crate::records::{Channel, Contact, Zone};
use crate::textcodec::NameCodec;

pub const ROW_WIDTH: usize = 100;
pub const EOL: &[u8] = b"\r\n";

#[derive(Debug, thiserror::Error)]
pub enum F136Error {
    /// The file does not follow the project layout, or the project cannot be saved.
    #[error("{0}")]
    Format(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

fn format_error(message: impl Into<String>) -> F136Error {
    F136Error::Format(message.into())
}

/// A project: the three record lists, the settings image and any rows this program does not know.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Project {
    pub contacts: Vec<Contact>,
    pub zones: Vec<Zone>,
    pub channels: Vec<Channel>,
    pub image: Vec<u8>,
    pub section_order: Vec<String>,
    pub unknown_rows: Vec<(String, Vec<u8>)>,
}

impl Project {
    pub fn device(&self) -> DeviceInfo {
        DeviceInfo::from_image(&self.image)
    }

    pub fn with_dmr_id(&self, dmr_id: u32) -> Result<Project, DeviceError> {
        Ok(Project {
            image: device::with_dmr_id(&self.image, dmr_id)?,
            ..self.clone()
        })
    }

    pub fn with_alias(&self, alias: &str) -> Result<Project, DeviceError> {
        Ok(Project {
            image: device::with_alias(&self.image, alias)?,
            ..self.clone()
        })
    }

    /// Replace one contact; the settings image's copy of the contacts follows.
    pub fn with_contact(&self, row: usize, contact: Contact) -> Option<Project> {
        let contacts = replaced(&self.contacts, row, contact)?;
        Some(with_synced_image(&Project {
            contacts,
            ..self.clone()
        }))
    }

    pub fn with_zone(&self, row: usize, zone: Zone) -> Option<Project> {
        Some(Project {
            zones: replaced(&self.zones, row, zone)?,
            ..self.clone()
        })
    }

    pub fn with_channel(&self, row: usize, channel: Channel) -> Option<Project> {
        Some(Project {
            channels: replaced(&self.channels, row, channel)?,
            ..self.clone()
        })
    }

    /// Conditions that must be fixed before saving.
    pub fn problems(&self) -> Vec<String> {
        let bands = self.device().bands();
        let mut out = Vec::new();
        if self.contacts.len() > MAX_CONTACTS {
            out.push(format!(
                "聯絡人 {} 筆，超過對講機上限 {MAX_CONTACTS} 筆",
                self.contacts.len()
            ));
        }
        let numbers: std::collections::HashSet<u16> =
            self.contacts.iter().map(Contact::index).collect();
        for (row, ch) in self.channels.iter().enumerate().map(|(i, c)| (i + 1, c)) {
            if ch.tx_contact() != 0 && !numbers.contains(&ch.tx_contact()) {
                out.push(format!(
                    "頻道第 {row} 列 發話對象：聯絡人 #{} 不存在",
                    ch.tx_contact()
                ));
            }
            for (label, hz) in [("接收頻率", ch.rx_hz()), ("發射頻率", ch.tx_hz())] {
                if let Some(reason) = frequency_problem(hz, bands.as_deref()) {
                    out.push(format!("頻道第 {row} 列 {label}：{reason}"));
                }
            }
        }
        out
    }

    /// Every name field: contacts, then zones, then channels.
    pub fn name_fields(&self) -> Vec<&[u8]> {
        self.contacts
            .iter()
            .map(Contact::name_field)
            .chain(self.zones.iter().map(Zone::name_field))
            .chain(self.channels.iter().map(Channel::name_field))
            .collect()
    }

    pub fn names_decodable(&self, codec: NameCodec) -> bool {
        !self
            .name_fields()
            .into_iter()
            .map(|f| crate::textcodec::display_name(f, codec))
            .any(|n| n.starts_with('‹'))
    }
}

fn replaced<T: Copy>(items: &[T], row: usize, value: T) -> Option<Vec<T>> {
    (row < items.len()).then(|| {
        let mut out = items.to_vec();
        out[row] = value;
        out
    })
}

/// Python's `int(b"…")`: surrounding whitespace, an optional sign, digits (single underscores
/// allowed between digits).
fn parse_int(field: &[u8]) -> Option<i64> {
    let text = std::str::from_utf8(field)
        .ok()?
        .trim_matches(|c: char| c.is_ascii_whitespace());
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
        || !digits.chars().all(|c| c.is_ascii_digit() || c == '_')
    {
        return None;
    }
    text.replace('_', "").parse().ok()
}

fn parse_values(fields: &[&[u8]], line_no: usize) -> Result<Vec<u8>, F136Error> {
    let values = fields
        .iter()
        .map(|f| parse_int(f))
        .collect::<Option<Vec<i64>>>()
        .ok_or_else(|| format_error(format!("第 {line_no} 行含非數字欄位")))?;
    values
        .into_iter()
        .map(u8::try_from)
        .collect::<Result<Vec<u8>, _>>()
        .map_err(|_| format_error(format!("第 {line_no} 行有超出 0–255 的數值")))
}

fn split_lines(data: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut rest = data;
    while let Some(at) = rest.windows(2).position(|w| w == EOL) {
        lines.push(&rest[..at]);
        rest = &rest[at + 2..];
    }
    lines
}

pub fn parse(data: &[u8]) -> Result<Project, F136Error> {
    let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
    let owned;
    let data = if !data.is_empty() && !data.ends_with(EOL) {
        owned = [data, EOL].concat();
        &owned[..]
    } else {
        data
    };
    let mut project = Project::default();
    for (line_no, line) in split_lines(data)
        .into_iter()
        .enumerate()
        .map(|(i, l)| (i + 1, l))
    {
        if line.is_empty() {
            return Err(format_error(format!("第 {line_no} 行是空行")));
        }
        let fields: Vec<&[u8]> = line.split(|b| *b == b'\t').collect();
        if fields[0].first().is_some_and(u8::is_ascii_digit) {
            project.image.extend(parse_values(&fields, line_no)?);
            continue;
        }
        let label: String = fields[0]
            .iter()
            .map(|&b| if b.is_ascii() { b as char } else { '\u{FFFD}' })
            .collect();
        let payload = parse_values(&fields[1..], line_no)?;
        if !project.section_order.contains(&label) {
            project.section_order.push(label.clone());
        }
        let wrap =
            |e: crate::records::RecordError| format_error(format!("第 {line_no} 行 {label}：{e}"));
        match label.as_str() {
            "Contacts" => project
                .contacts
                .push(Contact::from_bytes(&payload).map_err(wrap)?),
            "Zone" => project
                .zones
                .push(Zone::from_bytes(&payload).map_err(wrap)?),
            "CH" => project
                .channels
                .push(Channel::from_bytes(&payload).map_err(wrap)?),
            _ => project.unknown_rows.push((label, payload)),
        }
    }
    Ok(project)
}

fn row(label: Option<&str>, payload: &[u8]) -> Vec<u8> {
    let numbers = payload
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join("\t");
    let mut out = match label {
        Some(label) if !label.is_empty() => format!("{label}\t{numbers}").into_bytes(),
        _ => numbers.into_bytes(),
    };
    out.extend_from_slice(EOL);
    out
}

/// Write the project. The settings image's contact copy and counts are synced to the rows first
/// (a no-op for projects saved by the legacy CPS).
pub fn serialize(project: &Project) -> Vec<u8> {
    let project = with_synced_image(project);
    let mut by_label: Vec<(String, Vec<Vec<u8>>)> = vec![
        (
            "Contacts".into(),
            project.contacts.iter().map(|r| r.raw().to_vec()).collect(),
        ),
        (
            "Zone".into(),
            project.zones.iter().map(|r| r.raw().to_vec()).collect(),
        ),
        (
            "CH".into(),
            project.channels.iter().map(|r| r.raw().to_vec()).collect(),
        ),
    ];
    for (label, payload) in &project.unknown_rows {
        match by_label.iter_mut().find(|(l, _)| l == label) {
            Some((_, rows)) => rows.push(payload.clone()),
            None => by_label.push((label.clone(), vec![payload.clone()])),
        }
    }
    let mut order = project.section_order.clone();
    order.extend(
        by_label
            .iter()
            .map(|(l, _)| l.clone())
            .filter(|l| !project.section_order.contains(l)),
    );
    let mut out = Vec::new();
    for label in &order {
        if let Some((_, rows)) = by_label.iter().find(|(l, _)| l == label) {
            for payload in rows {
                out.extend(row(Some(label), payload));
            }
        }
    }
    for chunk in project.image.chunks(ROW_WIDTH) {
        out.extend(row(None, chunk));
    }
    out
}

pub fn load(path: impl AsRef<Path>) -> Result<Project, F136Error> {
    parse(&fs::read(path)?)
}

/// Atomically write `project`; refuses while [`Project::problems`] is non-empty. The first save
/// over an existing file keeps a `.bak` copy of it.
pub fn save(project: &Project, path: impl AsRef<Path>, keep_backup: bool) -> Result<(), F136Error> {
    let issues = project.problems();
    if !issues.is_empty() {
        return Err(format_error(format!(
            "尚有問題未修正：\n{}",
            issues.join("\n")
        )));
    }
    let target = path.as_ref();
    if keep_backup && target.exists() {
        let mut name = target.file_name().unwrap_or_default().to_os_string();
        name.push(".bak");
        let backup = target.with_file_name(name);
        if !backup.exists() {
            fs::copy(target, &backup)?;
        }
    }
    let dir = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let tmp = dir.join(format!(
        ".opendrs-{}-{}.tmp",
        std::process::id(),
        unique_suffix()
    ));
    let result = fs::write(&tmp, serialize(project)).and_then(|()| fs::rename(&tmp, target));
    if result.is_err() {
        let _ = fs::remove_file(&tmp); // best effort: the error that matters is returned below
    }
    Ok(result?)
}

fn unique_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_errors_name_the_line() {
        let err = parse(b"Contacts\t1\t2\r\n\r\n").unwrap_err().to_string();
        assert!(
            err.contains("第 1 行 Contacts") && err.contains("16 bytes"),
            "{err}"
        );
        assert_eq!(
            parse(b"1\t2\r\n\r\n").unwrap_err().to_string(),
            "第 2 行是空行"
        );
        assert_eq!(
            parse(b"1\tx\r\n").unwrap_err().to_string(),
            "第 1 行含非數字欄位"
        );
        assert_eq!(
            parse(b"1\t256\r\n").unwrap_err().to_string(),
            "第 1 行有超出 0–255 的數值"
        );
    }

    #[test]
    fn unknown_rows_survive_and_missing_final_crlf_is_tolerated() {
        let project = parse(b"Odd\t1\t2\r\n7\t8").unwrap();
        assert_eq!(project.unknown_rows, vec![("Odd".to_string(), vec![1, 2])]);
        assert_eq!(project.image, vec![7, 8]);
        assert_eq!(serialize(&project), b"Odd\t1\t2\r\n7\t8\r\n");
        assert_eq!(parse_int(b" +1_0 "), Some(10));
        assert_eq!(parse_int(b"1__0"), None);
    }
}
