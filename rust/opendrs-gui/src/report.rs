//! The printable project report (port of `print_report.py`): device info, channels by zone,
//! contacts, RX groups and scan lists as one HTML document. The Rust version prints it through
//! the system browser and turns it into a PDF with a headless Edge or Chrome (`print.rs`).

use std::collections::HashMap;

use opendrs_core::channels_csv::{FIELDS, View};
use opendrs_core::f136::Project;
use opendrs_core::layout::zone_channel_rows;
use opendrs_core::rxgroup::{has_rx_groups, rx_groups};
use opendrs_core::scanlist::{TX_MODES, has_scan_lists, scan_lists};
use opendrs_core::textcodec::NameCodec;
use opendrs_firmware::hardware;

use crate::pages::scan_page::entry_label;

pub const SECTIONS: [(&str, &str); 5] = [
    ("device", "裝置資訊"),
    ("channels", "頻道"),
    ("contacts", "聯絡人"),
    ("groups", "接收群組"),
    ("scans", "掃描清單"),
];
pub const CHANNEL_COLUMNS: [&str; 13] = [
    "頻道",
    "名稱",
    "接收MHz",
    "發射MHz",
    "模式",
    "色碼",
    "時隙",
    "功率",
    "發話對象",
    "接收群組",
    "掃描列表",
    "接收亞音",
    "發射亞音",
];
const STYLE: &str = "
body { font-size: 8pt; }
h1 { font-size: 14pt; margin: 0; }
h2 { font-size: 11pt; margin: 10px 0 4px 0; }
th.caption { background: #ffffff; border: none; font-size: 9pt; padding: 8px 0 2px 0; }
table { border-collapse: collapse; }
th { background: #e8e8e8; text-align: left; }
th, td { border: 1px solid #999; padding: 1px 4px; }
.meta { color: #555; }
";

/// `html.escape(text)`: `& < > " '`.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
    out
}

/// A table whose header rows (caption included) repeat on every printed page.
fn table(header: &[&str], rows: &[Vec<String>], caption: &str) -> String {
    let mut head: String = header
        .iter()
        .map(|h| format!("<th>{}</th>", escape(h)))
        .collect();
    if !caption.is_empty() {
        head = format!(
            "<th class=\"caption\" colspan=\"{}\">{}</th></tr><tr>{head}",
            header.len(),
            escape(caption)
        );
    }
    let body: String = rows
        .iter()
        .map(|row| {
            let cells: String = row
                .iter()
                .map(|c| format!("<td>{}</td>", escape(c)))
                .collect();
            format!("<tr>{cells}</tr>")
        })
        .collect();
    format!(
        "<table width=\"100%\" cellspacing=\"0\" cellpadding=\"2\"><thead><tr>{head}</tr></thead>{body}</table>"
    )
}

fn device(project: &Project) -> String {
    let dev = project.device();
    let also = hardware::known_as(&dev.model);
    let model = if also.is_empty() {
        dev.model.clone()
    } else {
        format!("{}（{also}）", dev.model)
    };
    let rows: Vec<Vec<String>> = [
        ("機型", model),
        ("型號", dev.model_number.clone()),
        ("機名", dev.alias.clone()),
        (
            "DMR ID",
            dev.dmr_id.map_or_else(String::new, |id| id.to_string()),
        ),
        (
            "韌體",
            format!("{} {}", dev.firmware, dev.firmware_date)
                .trim()
                .to_string(),
        ),
        ("頻段", dev.band_label.clone()),
        ("寫入時間", dev.write_date.clone()),
        ("出廠編號", dev.factory_number.clone()),
        ("序號", dev.serial_number.clone()),
    ]
    .into_iter()
    .filter(|(_, value)| !value.is_empty())
    .map(|(label, value)| vec![label.to_string(), value])
    .collect();
    table(&["項目", "內容"], &rows, "")
}

fn channels(project: &Project, codec: NameCodec) -> String {
    let view = View::new(project, codec);
    let fields: Vec<_> = CHANNEL_COLUMNS[1..]
        .iter()
        .map(|h| {
            FIELDS
                .iter()
                .find(|f| f.header == *h)
                .expect("a channel CSV column")
        })
        .collect();
    let mut parts = String::new();
    for (zone_row, zone) in project.zones.iter().enumerate() {
        let rows: Vec<Vec<String>> = zone_channel_rows(project, zone_row)
            .enumerate()
            .map(|(position, row)| {
                let ch = &project.channels[row];
                std::iter::once((position + 1).to_string())
                    .chain(fields.iter().map(|f| f.text(&view, ch)))
                    .collect()
            })
            .collect();
        let caption = format!(
            "分區 {}：{}（{} 個頻道）",
            zone_row + 1,
            zone.name(codec),
            rows.len()
        );
        parts.push_str(&table(&CHANNEL_COLUMNS, &rows, &caption));
    }
    parts
}

fn contacts(project: &Project, codec: NameCodec) -> String {
    let rows: Vec<Vec<String>> = project
        .contacts
        .iter()
        .map(|c| {
            vec![
                c.index().to_string(),
                c.name(codec),
                c.kind_label(),
                c.call_id().to_string(),
            ]
        })
        .collect();
    table(&["編號", "名稱", "類型", "ID"], &rows, "")
}

fn groups(project: &Project, codec: NameCodec) -> String {
    let names: HashMap<u16, String> = project
        .contacts
        .iter()
        .map(|c| (c.index(), c.name(codec)))
        .collect();
    let rows: Vec<Vec<String>> = rx_groups(&project.image)
        .iter()
        .filter(|g| g.in_use())
        .map(|g| {
            let members: Vec<String> = g
                .members()
                .iter()
                .map(|m| names.get(m).cloned().unwrap_or_else(|| format!("#{m}")))
                .collect();
            vec![g.number.to_string(), g.name(codec), members.join("、")]
        })
        .collect();
    table(&["編號", "名稱", "成員"], &rows, "")
}

fn scans(project: &Project, codec: NameCodec) -> String {
    let rows: Vec<Vec<String>> = scan_lists(&project.image)
        .iter()
        .filter(|s| s.in_use())
        .map(|scan| {
            let mut mode = TX_MODES
                .iter()
                .find(|(_, value)| *value == scan.tx_mode())
                .map_or_else(
                    || scan.tx_mode().to_string(),
                    |(label, _)| label.to_string(),
                );
            if scan.tx_mode() == 2 && scan.designated() != (0, 0) {
                mode.push_str(&format!(
                    "：{}",
                    entry_label(project, scan.designated(), codec)
                ));
            }
            let members: Vec<String> = scan
                .entries()
                .iter()
                .map(|e| entry_label(project, *e, codec))
                .collect();
            vec![
                scan.number.to_string(),
                scan.name(codec),
                if scan.talkback() { "開" } else { "關" }.into(),
                mode,
                members.join("、"),
            ]
        })
        .collect();
    table(&["編號", "名稱", "回講", "發射模式", "成員"], &rows, "")
}

/// The whole report as HTML; `stamp` is the print time as `%Y-%m-%d %H:%M`.
pub fn report_html(project: &Project, codec: NameCodec, title: &str, stamp: &str) -> String {
    let title = if title.is_empty() {
        "寫頻資料"
    } else {
        title
    };
    let mut out = vec![
        format!("<html><head><meta charset='utf-8'><style>{STYLE}</style></head><body>"),
        format!("<h1>{}</h1>", escape(title)),
        format!(
            "<p class='meta'>列印時間 {stamp}　·　OpenDRS{}</p>",
            env!("CARGO_PKG_VERSION")
        ),
    ];
    for (key, heading) in SECTIONS {
        let body = match key {
            "device" => device(project),
            "channels" => channels(project, codec),
            "contacts" => contacts(project, codec),
            "groups" if has_rx_groups(&project.image) => groups(project, codec),
            "scans" if has_scan_lists(&project.image) => scans(project, codec),
            _ => String::new(),
        };
        if !body.is_empty() {
            out.push(format!("<h2>{heading}</h2>{body}"));
        }
    }
    out.push("</body></html>".into());
    out.concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_like_python() {
        assert_eq!(escape("a&b<c>\"d'"), "a&amp;b&lt;c&gt;&quot;d&#x27;");
    }
}
