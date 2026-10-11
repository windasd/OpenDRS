//! Pre-flight checks before a firmware file is flashed (port of `firmware_check.py`). A wrong
//! image can leave the radio unbootable, so the chosen file is compared with what the radio
//! reported on its last read:
//!
//! * the firmware ID string (e.g. `308E.B6.TARSCB.003`) is found inside the image;
//! * the same ID as the radio -> OK. The ID in the image is a template: at boot the firmware
//!   reads the RF chip ID and overwrites the two characters after `<hw>.B` ("BK" BK4819,
//!   "Ax" RDA1846/AT1846S, "6." 0x6818 = unchanged, "64" 0x7813), so a file
//!   `308E.B6.TARSCB.003` and a radio reporting `308E.BBKTARSCB.003` are the same build -> OK;
//! * same hardware prefix, otherwise different -> warning, the user must confirm explicitly;
//! * a different hardware prefix or no ID -> blocked;
//! * the band-only patched images produced during the firmware analysis -> blocked;
//! * the known record-read patched images ([`crate::fwpatch`]) -> warning that needs the
//!   explicit tick; on a radio running the same build it is undone by flashing the vendor file;
//! * the band-widened images with the record-read fix (and the VFO keypad fix), made on request
//!   for a test -> the same kind of warning, saying what is unverified.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::bytes::Regex;

use crate::pytext::{group_thousands, sha256_hex};

pub const MAX_IMAGE: usize = 4 * 1024 * 1024;

static FIRMWARE_ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"([0-9]{3}[0-9A-F])\.([0-9A-Z][0-9A-Z.]{4,24}\.[0-9]{3})\.").expect("valid regex")
});
static BUILD_DATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Z][a-z]{2} [ 0-9][0-9] [0-9]{4}").expect("valid regex"));

/// The vendor's IAP tool (Windows only; `None` elsewhere).
pub fn default_iap_exe() -> Option<PathBuf> {
    cfg!(windows).then(|| PathBuf::from(r"C:\Program Files (x86)\IAP(APP+FLASH)\IAP+Flash.exe"))
}

/// SHA-256 of the band-only patched images made during the analysis (never flash these).
pub const BLOCKED_IMAGES: &[(&str, &str)] = &[
    (
        "7d10b46b350ff818d7d9a6882baf9add21a093b6c1d4a683a759bbeb950830bf",
        "136-180／400-520 改頻段版",
    ),
    (
        "7b623a85f0b29874b78e9f721cb8c564190b3dd409914012d87d90d0cf12c967",
        "136-180／400-520 改頻段版（改標示）",
    ),
];

/// Vendor images whose hash is known (the files the user received from the vendor).
pub const KNOWN_VENDOR_IMAGES: &[(&str, &str)] = &[
    (
        "fd52d62d980c59039da869c05f2c5802fb60a9fc639f4cd6d73680c3b896cc36",
        "原廠 C7000_DR8500UV_4819_03（DR-8500UV／DH-98VU，136-174／400-480，Mar 9 2024）",
    ),
    (
        "2267ee7a650490368bbf0de4197b542353864ef1ab82ea8cc54d9f20f0e526ab",
        "原廠 C7000_DR8500UV_4819_05（DR-8500UV／DH-98VU，144-146／430-440 業餘頻段，Mar 16 2024；\
         與 03 版程式相同，只差頻段限制）",
    ),
];

/// What was verified for the 30-byte record-read patch (the last sentence of its warning).
pub const RECORD_READ_NOTE: &str =
    "這 30 bytes 的修補已於 2026-10-09 在 BK4819 實機（03 版）刷入並讀取 25 筆錄音驗證";

/// What the band-widened test images share (a macro so `concat!` can build on it).
macro_rules! band_widened {
    () => {
        "頻段限制放寬為 136–180／400–520 MHz（對講機改回報 F136-520）：超出原廠 136–174／400–480 MHz \
         的部分沒有出廠校準，超出校準表最後節點時發射功率、頻偏與靜噪如何計算尚未查明，未經驗證，\
         發射測試請接假負載；錄音讀取的 30 bytes 與已實機驗證的版本相同。"
    };
}
/// The 136-180/400-520 test image with the record-read fix, as flashed on 2026-10-11.
pub const BAND_WIDENED_NOTE: &str = concat!(
    band_widened!(),
    "2026-10-11 已在 BK4819 實機（03 版）刷入，旋鈕可調到 5XX MHz，\
     但 VFO 鍵盤無法輸入 5 開頭的頻率（改用加了鍵盤修正的版本）"
);
/// The same image plus the VFO keypad first-digit fix.
pub const BAND_WIDENED_KEYPAD_NOTE: &str = concat!(
    band_widened!(),
    "另修正 VFO 鍵盤首位數字只接受 1、4 的檢查，可輸入 5 開頭的頻率；\
     2026-10-11 已在 BK4819 實機（03 版）刷入，確認鍵盤可正常輸入 5XX MHz"
);

/// Modified images made on request; flashable only after confirmation.
/// SHA-256 -> (description, the vendor file it was made from, what is verified).
pub const KNOWN_PATCHED_IMAGES: &[(&str, (&str, &str, &str))] = &[
    (
        "86d1ea6579eeabb4d1c2dee451bc3f89f3583a3c444d9c592fe26e7e8b3cb252",
        (
            "錄音讀取修補版（原廠韌體改 30 bytes，讓對講機回應電腦讀取錄音的指令）",
            "C7000_DR8500UV_4819_03_136174_400480.bin",
            RECORD_READ_NOTE,
        ),
    ),
    (
        "8f5a225f5dbe76706830ce99331e40605b55549695e30e7ab0f05b188ba3e7fa",
        (
            "錄音讀取修補版（業餘頻段 05 版原廠韌體改 30 bytes，讓對講機回應電腦讀取錄音的指令）",
            "C7000_DR8500UV_4819_05_144146_430440.bin",
            RECORD_READ_NOTE,
        ),
    ),
    (
        "b31dc8893580c3c6e6ccc2ae922aefc9bd1a72bb1c756ae8339a9c037403cad3",
        (
            "136-180／400-520 改頻段測試版加錄音讀取修補（原廠韌體改 76 bytes）",
            "C7000_DR8500UV_4819_03_136174_400480.bin",
            BAND_WIDENED_NOTE,
        ),
    ),
    (
        "8953e3e30285c0c55bff59dea8dd894f50a0816312f20758de9383f58e86d8ae",
        (
            "136-180／400-520 改頻段測試版加錄音讀取修補與鍵盤修正（原廠韌體改 108 bytes）",
            "C7000_DR8500UV_4819_03_136174_400480.bin",
            BAND_WIDENED_KEYPAD_NOTE,
        ),
    ),
];

/// The two characters after "<hw>.B" in a firmware ID: the image holds the template "6.",
/// which the firmware overwrites at boot according to the RF chip it finds.
pub const TEMPLATE_CHIP_CODE: &str = "6.";
pub const RF_CHIP_CODES: &[(&str, &str)] = &[
    ("BK", "BK4819"),
    ("Ax", "RDA1846／AT1846S"),
    ("6.", "0x6818"),
    ("64", "0x7813"),
];

fn lookup<V: Copy>(table: &[(&str, V)], key: &str) -> Option<V> {
    table.iter().find(|(k, _)| *k == key).map(|&(_, v)| v)
}

pub fn blocked_image(sha256: &str) -> Option<&'static str> {
    lookup(BLOCKED_IMAGES, sha256)
}

pub fn known_vendor_image(sha256: &str) -> Option<&'static str> {
    lookup(KNOWN_VENDOR_IMAGES, sha256)
}

/// (description, vendor file it was made from, what is verified).
pub fn known_patched_image(sha256: &str) -> Option<(&'static str, &'static str, &'static str)> {
    lookup(KNOWN_PATCHED_IMAGES, sha256)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Ok,
    Warn,
    Block,
}

impl Level {
    pub fn value(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Block => "block",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub text: String,
}

fn finding(level: Level, text: impl Into<String>) -> Finding {
    Finding {
        level,
        text: text.into(),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FirmwareFile {
    pub path: PathBuf,
    pub size: usize,
    pub sha256: String,
    /// "" when no ID string was found.
    pub firmware_id: String,
    pub build_date: String,
}

impl FirmwareFile {
    pub fn hardware(&self) -> &str {
        hardware(&self.firmware_id)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InspectError {
    /// Python: `ValueError`.
    #[error("{0}")]
    TooLarge(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

pub fn inspect(path: &Path) -> Result<FirmwareFile, InspectError> {
    let data = fs::read(path)?;
    if data.len() > MAX_IMAGE {
        return Err(InspectError::TooLarge(format!(
            "檔案太大（{} bytes），不像對講機韌體",
            group_thousands(data.len())
        )));
    }
    let id = FIRMWARE_ID.captures(&data);
    let firmware_id = id.as_ref().map_or_else(String::new, |m| {
        format!(
            "{}.{}",
            String::from_utf8_lossy(&m[1]),
            String::from_utf8_lossy(&m[2])
        )
    });
    let from = id.as_ref().and_then(|m| m.get(0)).map_or(0, |m| m.end());
    let build_date = BUILD_DATE
        .find_at(&data, from)
        .map_or_else(String::new, |m| {
            String::from_utf8_lossy(m.as_bytes())
                .split_ascii_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        });
    Ok(FirmwareFile {
        path: path.to_path_buf(),
        size: data.len(),
        sha256: sha256_hex(&data),
        firmware_id,
        build_date,
    })
}

fn hardware(firmware_id: &str) -> &str {
    firmware_id.split('.').next().unwrap_or_default()
}

/// `str.partition(".")`: (head, found, tail).
fn partition(text: &str) -> (&str, bool, &str) {
    match text.split_once('.') {
        Some((head, tail)) => (head, true, tail),
        None => (text, false, ""),
    }
}

/// The text after the first `skip` characters.
fn chars_from(text: &str, skip: usize) -> &str {
    text.char_indices()
        .nth(skip)
        .map_or("", |(at, _)| &text[at..])
}

/// The two characters after "<hw>.B" when they are a known RF-chip code, else None.
fn chip_code(firmware_id: &str) -> Option<&'static str> {
    let (_hw, dot, rest) = partition(firmware_id);
    if !dot || !rest.starts_with('B') {
        return None;
    }
    let code: String = rest.chars().skip(1).take(2).collect();
    RF_CHIP_CODES
        .iter()
        .find(|(c, _)| *c == code)
        .map(|&(c, _)| c)
}

/// `firmware_id` with its RF-chip code put back to the template ("308E.BBK…" -> "308E.B6.…").
pub fn normalise_firmware_id(firmware_id: &str) -> String {
    if chip_code(firmware_id).is_none() {
        return firmware_id.to_owned();
    }
    let (hw, _dot, rest) = partition(firmware_id);
    format!("{hw}.B{TEMPLATE_CHIP_CODE}{}", chars_from(rest, 3))
}

/// The RF chip a radio-reported firmware ID indicates, None when the code is unknown.
pub fn rf_chip(firmware_id: &str) -> Option<&'static str> {
    chip_code(firmware_id).and_then(|code| lookup(RF_CHIP_CODES, code))
}

/// True when the IDs are equal once the boot-time RF-chip code is ignored.
pub fn same_build(file_id: &str, radio_id: &str) -> bool {
    !file_id.is_empty() && normalise_firmware_id(file_id) == normalise_firmware_id(radio_id)
}

fn same_build_text(file_id: &str, radio_id: &str) -> String {
    let (file_code, radio_code) = (chip_code(file_id), chip_code(radio_id));
    let template = if file_code == Some(TEMPLATE_CHIP_CODE) {
        "樣板"
    } else {
        ""
    };
    format!(
        "與對講機目前韌體為同一版本（對講機回報的 B{} 是開機時偵測到 {} 射頻晶片後填入的代碼，\
         檔案內為{template} B{}）",
        radio_code.unwrap_or("None"),
        rf_chip(radio_id).unwrap_or("None"),
        file_code.unwrap_or("None")
    )
}

fn label(firmware_id: &str, date: &str) -> String {
    if date.is_empty() {
        firmware_id.to_owned()
    } else {
        format!("{firmware_id}（{date}）")
    }
}

/// A patched vendor image: undone by flashing the vendor file again, if that is the radio's
/// build.
fn patched_finding(
    (name, vendor_file, note): (&str, &str, &str),
    image: &FirmwareFile,
    radio_firmware: &str,
    radio_date: &str,
) -> Finding {
    let radio = label(radio_firmware, radio_date);
    let base = format!("修補所用的原廠檔 {vendor_file} ");
    let no_way_back = "刷入後要改回對講機原本的韌體，須有原廠提供的該版檔案";
    let dates_differ =
        !radio_date.is_empty() && !image.build_date.is_empty() && radio_date != image.build_date;
    let has_radio = !radio_firmware.is_empty();
    let way_back = if has_radio && (dates_differ || !same_build(&image.firmware_id, radio_firmware))
    {
        let image_label = label(&image.firmware_id, &image.build_date);
        format!("對講機目前是 {radio}，{base}是 {image_label}，不是同一版本：{no_way_back}")
    } else if has_radio && !radio_date.is_empty() && !image.build_date.is_empty() {
        format!("對講機目前的 {radio}與{base}是同一版本，要復原時再刷一次這個原廠檔即可")
    } else {
        // no radio read yet, or a build date is missing
        let current = if has_radio {
            format!("對講機目前是 {radio}")
        } else {
            "尚未從對講機讀取".to_owned()
        };
        format!("{current}，無法確認與{base}是同一版本；若不是，{no_way_back}")
    };
    finding(Level::Warn, format!("這是{name}。{way_back}。{note}"))
}

/// What the firmware ID comparison says.
fn id_finding(image: &FirmwareFile, radio_firmware: &str) -> Finding {
    let file_id = image.firmware_id.as_str();
    if file_id.is_empty() {
        return finding(
            Level::Block,
            "檔案中找不到韌體 ID，無法確認是這款對講機的韌體",
        );
    }
    if radio_firmware.is_empty() {
        return finding(
            Level::Warn,
            format!(
                "檔案韌體 {file_id}；尚未從對講機讀取過，無法比對目前版本（建議先「讀取對講機」）"
            ),
        );
    }
    if file_id == radio_firmware {
        return finding(
            Level::Ok,
            format!("韌體 ID 與對講機目前版本相同（{radio_firmware}）"),
        );
    }
    if same_build(file_id, radio_firmware) {
        return finding(Level::Ok, same_build_text(file_id, radio_firmware));
    }
    if hardware(file_id) == hardware(radio_firmware) {
        return finding(
            Level::Warn,
            format!(
                "同一硬體平台（{}），但版本不同：檔案 {file_id}，對講機 {radio_firmware}，\
                 刷入前請確認來源可靠",
                image.hardware()
            ),
        );
    }
    finding(
        Level::Block,
        format!("硬體平台不同：檔案 {file_id}，對講機 {radio_firmware}"),
    )
}

/// Everything the user should know before flashing `image`; BLOCK means do not.
pub fn check(
    image: &FirmwareFile,
    radio_firmware: &str,
    has_recent_backup: bool,
    radio_date: &str,
) -> Vec<Finding> {
    let mut out = Vec::new();
    if let Some(vendor) = known_vendor_image(&image.sha256) {
        out.push(finding(Level::Ok, format!("已知的原廠韌體檔：{vendor}")));
    }
    if let Some(blocked) = blocked_image(&image.sha256) {
        out.push(finding(
            Level::Block,
            format!(
                "這是分析時產生的{blocked}：頻段限制被放寬，超出原廠頻段的射頻表現未經驗證，\
                 本程式不允許刷入"
            ),
        ));
    }
    if let Some(patched) = known_patched_image(&image.sha256) {
        out.push(patched_finding(patched, image, radio_firmware, radio_date));
    }
    out.push(id_finding(image, radio_firmware));
    if !radio_date.is_empty() && !image.build_date.is_empty() {
        out.push(if image.build_date == radio_date {
            finding(
                Level::Ok,
                format!("編譯日期與對講機目前韌體相同（{radio_date}）"),
            )
        } else {
            finding(
                Level::Warn,
                format!(
                    "編譯日期不同：檔案 {}，對講機 {radio_date}",
                    image.build_date
                ),
            )
        });
    }
    out.push(if has_recent_backup {
        finding(Level::Ok, "已有今天從對講機讀取的備份")
    } else {
        finding(
            Level::Warn,
            "今天還沒有從對講機讀取的備份；刷韌體前建議先「讀取對講機」保留設定",
        )
    });
    out
}

/// The worst level among the findings (OK when there are none).
pub fn verdict(findings: &[Finding]) -> Level {
    findings.iter().map(|f| f.level).max().unwrap_or(Level::Ok)
}
