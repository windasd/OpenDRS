//! Export / import the contact list as CSV (port of `contacts_csv.py`).
//!
//! Exports are UTF-8 with a BOM and CRLF line ends, which Excel opens without mojibake. Imports
//! accept UTF-8 (with or without BOM) and legacy Big5 / GBK files, English or Chinese headers,
//! and call types given as a number or a label.

use std::collections::HashMap;

use opendrs_core::f136::Project;
use opendrs_core::records::Contact;
use opendrs_core::textcodec::{NameCodec, read_legacy_text};

use crate::contact_import::NewContact;
use crate::pycsv::{self, CsvError};
use crate::pystr::{PyInt, casefold, strip};

pub const HEADER: [&str; 4] = ["No", "CallType", "Name", "ID"];

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// Header text (stripped, case-folded, spaces removed) → column key.
fn column_key(header: &str) -> Option<&'static str> {
    Some(match header {
        "no" | "編號" | "编号" | "序號" => "no",
        "calltype" | "type" | "類型" | "类型" | "呼叫類型" | "呼叫类型" => "type",
        "name" | "名稱" | "名称" | "姓名" => "name",
        "id" | "callid" | "呼叫id" | "號碼" | "号码" => "id",
        _ => return None,
    })
}

/// Call type text (case-folded) → contact kind.
fn call_type(text: &str) -> Option<u8> {
    Some(match text {
        "4" | "組呼" | "组呼" | "group" | "group call" | "tg" => 4,
        "5" | "個呼" | "个呼" | "private" | "private call" => 5,
        "6" | "全呼" | "all" | "all call" => 6,
        "7" | "無地址呼" | "无地址呼" => 7,
        _ => return None,
    })
}

pub fn export_contacts(project: &Project, codec: NameCodec) -> Vec<u8> {
    let mut text = pycsv::write_row(&HEADER);
    for c in &project.contacts {
        text.push_str(&pycsv::write_row(&[
            c.index().to_string(),
            c.kind_label(),
            c.name(codec),
            c.call_id().to_string(),
        ]));
    }
    [UTF8_BOM, text.as_bytes()].concat()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CsvParseResult {
    pub entries: Vec<NewContact>,
    pub errors: Vec<String>,
    /// The encoding the file was read with (`read_legacy_text`).
    pub encoding: &'static str,
}

fn failed(error: String, encoding: &'static str) -> CsvParseResult {
    CsvParseResult {
        entries: Vec::new(),
        errors: vec![error],
        encoding,
    }
}

/// Read a contact CSV. Unusable rows are reported in `errors` (with their row number) and
/// skipped; `Err` only where Python's csv module raises (a field over 131072 characters).
pub fn parse_contacts_csv(data: &[u8]) -> Result<CsvParseResult, CsvError> {
    let (text, encoding) = read_legacy_text(data);
    let rows = pycsv::read(&text).collect::<Result<Vec<_>, _>>()?;
    let Some(header) = rows.first() else {
        return Ok(failed("檔案是空的".into(), encoding));
    };
    let columns: HashMap<&str, usize> = header
        .iter()
        .enumerate()
        .filter_map(|(i, h)| column_key(&casefold(strip(h)).replace(' ', "")).map(|k| (k, i)))
        .collect();
    let missing: Vec<&str> = ["name", "id"]
        .into_iter()
        .filter(|c| !columns.contains_key(c))
        .collect();
    if !missing.is_empty() {
        let error = format!("缺少欄位：{}（需要 Name 與 ID）", missing.join("、"));
        return Ok(failed(error, encoding));
    }
    let mut entries = Vec::new();
    let mut errors = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(1) {
        if row.iter().all(|cell| strip(cell).is_empty()) {
            continue;
        }
        match parse_row(row, &columns, i + 1) {
            Ok(entry) => entries.push(entry),
            Err(error) => errors.push(error),
        }
    }
    Ok(CsvParseResult {
        entries,
        errors,
        encoding,
    })
}

fn parse_row(
    row: &[String],
    columns: &HashMap<&str, usize>,
    line_no: usize,
) -> Result<NewContact, String> {
    let cell = |key: &str| {
        columns
            .get(key)
            .and_then(|&i| row.get(i))
            .map_or("", |c| strip(c))
    };
    let call_id = PyInt::parse(cell("id"))
        .ok_or_else(|| format!("第 {line_no} 列：ID「{}」不是數字", cell("id")))?;
    let id = call_id
        .to_i64()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| (1..=Contact::MAX_ID).contains(v))
        .ok_or_else(|| format!("第 {line_no} 列：ID {call_id} 超出範圍"))?;
    let raw_type = casefold(cell("type"));
    let kind = if raw_type.is_empty() {
        Some(Contact::PRIVATE_CALL)
    } else {
        call_type(&raw_type)
    }
    .ok_or_else(|| format!("第 {line_no} 列：無法辨識的類型「{}」", cell("type")))?;
    let name = match cell("name") {
        "" => id.to_string(),
        name => name.to_string(),
    };
    Ok(NewContact::new(kind, name, id))
}
