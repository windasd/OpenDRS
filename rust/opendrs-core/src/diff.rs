//! Byte-level comparison of two projects (port of `diff.py`). Main use: map unknown fields —
//! change one setting in the legacy CPS, save, and compare with the previous save.

use crate::device::DeviceInfo;
use crate::f136::Project;
use crate::pystr::{str_repr, thousands};
use crate::records::{Channel, Contact, Zone, hz_to_mhz};
use crate::textcodec::NameCodec;

pub const MERGE_GAP: usize = 4;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: usize,
    pub old: Vec<u8>,
    pub new: Vec<u8>,
}

fn slice(data: &[u8], start: usize, end: usize) -> Vec<u8> {
    data.get(start.min(data.len())..end.min(data.len()))
        .unwrap_or_default()
        .to_vec()
}

/// Differing spans; spans separated by at most `merge_gap` equal bytes are merged.
pub fn byte_ranges(a: &[u8], b: &[u8], merge_gap: usize) -> Vec<ByteRange> {
    let n = a.len().max(b.len());
    let same = |i: usize| i < a.len() && i < b.len() && a[i] == b[i];
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if same(i) {
            i += 1;
            continue;
        }
        let start = i;
        let mut last = i;
        i += 1;
        while i < n && (!same(i) || i - last <= merge_gap) {
            if !same(i) {
                last = i;
            }
            i += 1;
        }
        out.push(ByteRange {
            start,
            old: slice(a, start, last + 1),
            new: slice(b, start, last + 1),
        });
        i = last + 1;
    }
    out
}

fn ascii(data: &[u8]) -> String {
    data.iter()
        .map(|&x| {
            if (0x20..0x7F).contains(&x) {
                x as char
            } else {
                '.'
            }
        })
        .collect()
}

fn hex(data: &[u8]) -> String {
    data.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn fmt_range(r: &ByteRange, base: &str) -> Vec<String> {
    let or_none = |d: &[u8]| {
        if d.is_empty() {
            "(無)".to_string()
        } else {
            hex(d)
        }
    };
    vec![
        format!(
            "  {base}+0x{:04X}  ({} bytes)",
            r.start,
            r.old.len().max(r.new.len())
        ),
        format!("      舊 {:<48} |{}|", or_none(&r.old), ascii(&r.old)),
        format!("      新 {:<48} |{}|", or_none(&r.new), ascii(&r.new)),
    ]
}

/// What the report needs from a contact / zone / channel record.
trait Record {
    fn bytes(&self) -> &[u8];
    fn label(&self, codec: NameCodec) -> String;
    fn frequencies(&self) -> Option<(u32, u32)> {
        None
    }
}

impl Record for Contact {
    fn bytes(&self) -> &[u8] {
        self.raw()
    }
    fn label(&self, codec: NameCodec) -> String {
        self.name(codec)
    }
}

impl Record for Zone {
    fn bytes(&self) -> &[u8] {
        self.raw()
    }
    fn label(&self, codec: NameCodec) -> String {
        self.name(codec)
    }
}

impl Record for Channel {
    fn bytes(&self) -> &[u8] {
        self.raw()
    }
    fn label(&self, codec: NameCodec) -> String {
        self.name(codec)
    }
    fn frequencies(&self) -> Option<(u32, u32)> {
        Some((self.rx_hz(), self.tx_hz()))
    }
}

fn records<R: Record>(title: &str, old: &[R], new: &[R], codec: NameCodec) -> Vec<String> {
    let mut lines = Vec::new();
    for row in 0..old.len().max(new.len()) {
        match (old.get(row), new.get(row)) {
            (Some(a), Some(b)) if a.bytes() == b.bytes() => {}
            (None, Some(b)) => lines.push(format!("  第 {} 列 新增：{}", row + 1, b.label(codec))),
            (Some(a), None) => lines.push(format!("  第 {} 列 刪除：{}", row + 1, a.label(codec))),
            (Some(a), Some(b)) => {
                let (na, nb) = (a.label(codec), b.label(codec));
                let label = if na == nb {
                    na
                } else {
                    format!("{na} → {nb}")
                };
                lines.push(format!("  第 {} 列（{label}）", row + 1));
                if let (Some((rx_a, tx_a)), Some((rx_b, tx_b))) = (a.frequencies(), b.frequencies())
                {
                    for (title, x, y) in [("接收", rx_a, rx_b), ("發射", tx_a, tx_b)] {
                        if x != y {
                            lines.push(format!(
                                "      {title}：{} → {} MHz",
                                hz_to_mhz(x),
                                hz_to_mhz(y)
                            ));
                        }
                    }
                }
                for r in byte_ranges(a.bytes(), b.bytes(), MERGE_GAP) {
                    lines.push(format!(
                        "      +0x{:02X}: {} → {}",
                        r.start,
                        hex(&r.old),
                        hex(&r.new)
                    ));
                }
            }
            (None, None) => {}
        }
    }
    if lines.is_empty() && old.len() == new.len() {
        return Vec::new();
    }
    let mut out = vec![format!("■ {title}（{} → {} 筆）", old.len(), new.len())];
    out.extend(lines);
    out
}

fn device_fields(d: &DeviceInfo) -> [(&'static str, String); 10] {
    [
        ("band_label", str_repr(&d.band_label)),
        ("write_date", str_repr(&d.write_date)),
        ("model", str_repr(&d.model)),
        ("alias", str_repr(&d.alias)),
        ("dmr_id", d.dmr_id.map_or("None".into(), |v| v.to_string())),
        ("firmware", str_repr(&d.firmware)),
        ("factory_number", str_repr(&d.factory_number)),
        ("serial_number", str_repr(&d.serial_number)),
        ("model_number", str_repr(&d.model_number)),
        ("firmware_date", str_repr(&d.firmware_date)),
    ]
}

pub fn diff_projects(old: &Project, new: &Project, codec: NameCodec) -> Vec<String> {
    let mut report = Vec::new();
    let dev_lines: Vec<String> = device_fields(&old.device())
        .into_iter()
        .zip(device_fields(&new.device()))
        .filter(|((_, a), (_, b))| a != b)
        .map(|((name, a), (_, b))| format!("  {name}: {a} → {b}"))
        .collect();
    if !dev_lines.is_empty() {
        report.push("■ 裝置資訊".to_string());
        report.extend(dev_lines);
    }
    report.extend(records("聯絡人", &old.contacts, &new.contacts, codec));
    report.extend(records("分區", &old.zones, &new.zones, codec));
    report.extend(records("頻道", &old.channels, &new.channels, codec));
    let ranges = byte_ranges(&old.image, &new.image, MERGE_GAP);
    if !ranges.is_empty() {
        let changed: usize = ranges.iter().map(|r| r.old.len().max(r.new.len())).sum();
        report.push(format!(
            "■ 設定區（{} → {} bytes，{} 處、約 {changed} bytes 不同）",
            thousands(old.image.len() as u64),
            thousands(new.image.len() as u64),
            ranges.len()
        ));
        for r in &ranges {
            report.extend(fmt_range(r, "image"));
        }
    }
    if report.is_empty() {
        report.push("兩個專案內容完全相同。".into());
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_merge_across_small_gaps() {
        let r = byte_ranges(b"abcdefghij", b"aXcdeXghij", 4);
        assert_eq!(
            r,
            vec![ByteRange {
                start: 1,
                old: b"bcdef".to_vec(),
                new: b"XcdeX".to_vec()
            }]
        );
        assert_eq!(
            byte_ranges(b"ab", b"abcd", 4)[0],
            ByteRange {
                start: 2,
                old: vec![],
                new: b"cd".to_vec()
            }
        );
        assert_eq!(thousands(11_000), "11,000");
        assert_eq!(thousands(999), "999");
    }
}
