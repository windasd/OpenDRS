//! Chinese text needs a CJK font, and egui ships none. One is loaded from the operating system at
//! start-up (nothing is bundled): Microsoft JhengHei / YaHei on Windows, PingFang or Heiti on
//! macOS, Noto Sans CJK (found through fontconfig) or WenQuanYi on Linux.
//! `OPENDRS_FONT=path` or `path#face` picks a file instead.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

pub const FONT_ENV: &str = "OPENDRS_FONT";

/// At most this many fonts are installed as fallbacks (each can be ~20 MB).
const MAX_FONTS: usize = 2;

/// A font file and the face to use in it (`.ttc` collections hold several).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontFile {
    pub path: PathBuf,
    pub index: u32,
}

impl FontFile {
    fn new(path: impl Into<PathBuf>, index: u32) -> Self {
        FontFile {
            path: path.into(),
            index,
        }
    }
}

/// `path` or `path#face` (the face number is optional).
pub fn parse_font_spec(spec: &str) -> FontFile {
    match spec.rsplit_once('#') {
        Some((path, face)) if !path.is_empty() && face.parse::<u32>().is_ok() => {
            FontFile::new(path, face.parse().unwrap_or(0))
        }
        _ => FontFile::new(spec, 0),
    }
}

/// Files to try, best first.
pub fn candidates() -> Vec<FontFile> {
    let mut out = Vec::new();
    if let Some(spec) = std::env::var_os(FONT_ENV).filter(|v| !v.is_empty()) {
        out.push(parse_font_spec(&spec.to_string_lossy()));
    }
    out.extend(platform_candidates());
    out
}

#[cfg(windows)]
fn platform_candidates() -> Vec<FontFile> {
    let windir =
        std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let fonts = windir.join("Fonts");
    // Traditional first (the radio's names are usually Big5), then Simplified for GBK names.
    ["msjh.ttc", "mingliu.ttc", "msyh.ttc", "simsun.ttc"]
        .into_iter()
        .map(|name| FontFile::new(fonts.join(name), 0))
        .collect()
}

#[cfg(target_os = "macos")]
fn platform_candidates() -> Vec<FontFile> {
    [
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Medium.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "/System/Library/Fonts/Hiragino Sans GB.ttc",
        "/System/Library/Fonts/Supplemental/Songti.ttc",
        "/Library/Fonts/Arial Unicode.ttf",
    ]
    .into_iter()
    .map(|path| FontFile::new(path, 0))
    .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_candidates() -> Vec<FontFile> {
    let mut out: Vec<FontFile> = [
        "Noto Sans CJK TC:lang=zh-tw",
        "Noto Sans CJK SC:lang=zh-cn",
        ":lang=zh-tw",
    ]
    .into_iter()
    .filter_map(fc_match)
    .collect();
    // Noto Sans CJK collections hold JP, KR, SC, TC, HK in that order.
    for path in [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/google-noto-cjk/NotoSansCJK-Regular.ttc",
    ] {
        out.push(FontFile::new(path, 3));
    }
    for path in [
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
        "/usr/share/fonts/truetype/arphic/uming.ttc",
    ] {
        out.push(FontFile::new(path, 0));
    }
    out
}

/// The file fontconfig picks for `pattern`.
#[cfg(all(unix, not(target_os = "macos")))]
fn fc_match(pattern: &str) -> Option<FontFile> {
    let output = std::process::Command::new("fc-match")
        .args(["-f", "%{file}\n%{index}", pattern])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let (path, index) = text.split_once('\n')?;
    let path = PathBuf::from(path);
    let lower = path.to_string_lossy().to_lowercase();
    // fontconfig falls back to any font; only keep ones that can show Chinese.
    let cjk = [
        "cjk",
        "wqy",
        "uming",
        "ukai",
        "droidsansfallback",
        "sourcehan",
        "hei",
        "song",
    ];
    cjk.iter()
        .any(|hint| lower.contains(hint))
        .then(|| FontFile::new(path, index.trim().parse().unwrap_or(0)))
}

/// Fonts with the symbols the editor shows (␍␊ for line breaks in names, ⚠, ✔), tried after the
/// Chinese fonts.
#[cfg(windows)]
pub fn symbol_candidates() -> Vec<FontFile> {
    let windir =
        std::env::var_os("WINDIR").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    vec![FontFile::new(windir.join("Fonts").join("seguisym.ttf"), 0)]
}

#[cfg(target_os = "macos")]
pub fn symbol_candidates() -> Vec<FontFile> {
    vec![FontFile::new("/System/Library/Fonts/Apple Symbols.ttf", 0)]
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn symbol_candidates() -> Vec<FontFile> {
    [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
    ]
    .into_iter()
    .map(|path| FontFile::new(path, 0))
    .collect()
}

/// TrueType, OpenType or a collection of them.
fn looks_like_font(data: &[u8]) -> bool {
    matches!(
        data.get(..4),
        Some(b"ttcf" | b"OTTO" | b"true" | [0, 1, 0, 0])
    )
}

fn read_font(file: &FontFile) -> Option<FontData> {
    let data = std::fs::read(&file.path).ok()?;
    if !looks_like_font(&data) {
        return None;
    }
    let mut font = FontData::from_owned(data);
    font.index = file.index;
    Some(font)
}

/// Add the first loadable candidates as fallbacks after egui's own fonts (which keep drawing
/// Latin text); returns what was installed, empty when no Chinese font was found.
pub fn install(ctx: &egui::Context) -> Vec<FontFile> {
    let mut definitions = FontDefinitions::default();
    let mut installed: Vec<FontFile> = Vec::new();
    for file in candidates() {
        if installed.len() == MAX_FONTS {
            break;
        }
        if installed.iter().any(|f| f.path == file.path) {
            continue;
        }
        let Some(font) = read_font(&file) else {
            continue;
        };
        let name = format!("cjk-{}", installed.len());
        definitions.font_data.insert(name.clone(), Arc::new(font));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            definitions
                .families
                .entry(family)
                .or_default()
                .push(name.clone());
        }
        installed.push(file);
    }
    if let Some(font) = symbol_candidates().iter().find_map(read_font) {
        definitions
            .font_data
            .insert("symbols".into(), Arc::new(font));
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            definitions
                .families
                .entry(family)
                .or_default()
                .push("symbols".into());
        }
    }
    ctx.set_fonts(definitions);
    installed
}

/// Where a font came from, for the about box.
pub fn describe(files: &[FontFile]) -> String {
    if files.is_empty() {
        return "（找不到中文字型）".into();
    }
    files
        .iter()
        .map(|f| file_name(&f.path))
        .collect::<Vec<_>>()
        .join("、")
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_spec_takes_an_optional_face() {
        assert_eq!(parse_font_spec("/a/b.ttc#3"), FontFile::new("/a/b.ttc", 3));
        assert_eq!(parse_font_spec("/a/b.ttf"), FontFile::new("/a/b.ttf", 0));
        assert_eq!(
            parse_font_spec("C:/x#y.ttf"),
            FontFile::new("C:/x#y.ttf", 0)
        );
    }

    #[test]
    fn only_font_files_are_accepted() {
        assert!(looks_like_font(b"ttcf\0\0"));
        assert!(looks_like_font(&[0, 1, 0, 0, 9]));
        assert!(!looks_like_font(b"<html>"));
        assert!(read_font(&FontFile::new("/no/such/font.ttc", 0)).is_none());
    }
}
