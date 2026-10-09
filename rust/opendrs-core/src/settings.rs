//! Per-user settings, `settings.json` (UTF-8) in the data folder (port of `settings.py`).
//!
//! On Windows the folder is the Python version's (`%APPDATA%\OpenDRS`), so both read the same
//! file. Values are read as leniently as Python does (`str(value)`, `bool(value)`), and keys this
//! version does not know are kept when saving, so neither version drops the other's settings.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::paths;
use crate::pyjson::{py_str, py_str_items, truthy};
use crate::textcodec::NameCodec;

pub const FILE_NAME: &str = "settings.json";
pub const MAX_RECENT: usize = 8;
/// The data folder's name before the project was renamed OpenDRS (2026-10-09).
pub const LEGACY_DIR_NAME: &str = "DRSCPS";

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub name_codec: NameCodec,
    pub recent_files: Vec<String>,
    pub last_dir: String,
    /// Last port used for reading / writing the radio.
    pub radio_port: String,
    /// "auto", "light" or "dark".
    pub theme: String,
    /// Vendor IAP+Flash.exe (empty = default install location).
    pub iap_path: String,
    /// Vendor ambep2.exe for recordings (empty = the default location).
    pub ambep2_path: String,
    /// Where downloaded recordings go (empty = `<data dir>/recordings`).
    pub recordings_dir: String,
    /// Look for a newer release at start-up, at most daily.
    pub check_updates: bool,
    /// ISO timestamp (UTC) of the last automatic check.
    pub last_update_check: String,
    /// Keys written by another version, kept as they are.
    pub extra: BTreeMap<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            name_codec: NameCodec::Big5,
            recent_files: Vec::new(),
            last_dir: String::new(),
            radio_port: String::new(),
            theme: "auto".into(),
            iap_path: String::new(),
            ambep2_path: String::new(),
            recordings_dir: String::new(),
            check_updates: true,
            last_update_check: String::new(),
            extra: BTreeMap::new(),
        }
    }
}

/// `str(Path(path).parent)`: pathlib drops repeated separators and `.` parts, and an empty
/// parent is ".".
fn parent_text(path: &str) -> String {
    let parts: Vec<Component> = Path::new(path).components().collect();
    let keep = match parts.last() {
        Some(Component::Normal(_) | Component::ParentDir | Component::CurDir) => parts.len() - 1,
        _ => parts.len(),
    };
    let parent: PathBuf = parts[..keep].iter().collect();
    if parent.as_os_str().is_empty() {
        ".".into()
    } else {
        parent.to_string_lossy().into_owned()
    }
}

impl Settings {
    /// `path` first in the recent files (at most [`MAX_RECENT`]), its folder as the last one.
    pub fn with_recent(&self, path: &str) -> Settings {
        let recent = std::iter::once(path.to_string())
            .chain(self.recent_files.iter().filter(|p| *p != path).cloned())
            .take(MAX_RECENT)
            .collect();
        Settings {
            recent_files: recent,
            last_dir: parent_text(path),
            ..self.clone()
        }
    }

    fn known(&self) -> Vec<(&'static str, Value)> {
        let text = |s: &str| Value::String(s.to_string());
        vec![
            ("name_codec", text(self.name_codec.value())),
            (
                "recent_files",
                Value::Array(self.recent_files.iter().map(|p| text(p)).collect()),
            ),
            ("last_dir", text(&self.last_dir)),
            ("radio_port", text(&self.radio_port)),
            ("theme", text(&self.theme)),
            ("iap_path", text(&self.iap_path)),
            ("ambep2_path", text(&self.ambep2_path)),
            ("recordings_dir", text(&self.recordings_dir)),
            ("check_updates", Value::Bool(self.check_updates)),
            ("last_update_check", text(&self.last_update_check)),
        ]
    }

    /// The file's text: the known keys in order, then the others.
    pub fn to_json(&self) -> String {
        let known = self
            .known()
            .into_iter()
            .map(|(key, value)| (key.to_string(), value));
        let extra = self
            .extra
            .iter()
            .filter(|(key, _)| !KEYS.contains(&key.as_str()))
            .map(|(key, value)| (key.clone(), value.clone()));
        json_object(known.chain(extra))
    }
}

/// The keys [`Settings`] reads, in file order.
const KEYS: [&str; 10] = [
    "name_codec",
    "recent_files",
    "last_dir",
    "radio_port",
    "theme",
    "iap_path",
    "ambep2_path",
    "recordings_dir",
    "check_updates",
    "last_update_check",
];

/// A JSON object laid out like Python's `json.dumps(data, ensure_ascii=False, indent=2)`.
fn json_object(entries: impl IntoIterator<Item = (String, Value)>) -> String {
    let lines: Vec<String> = entries
        .into_iter()
        .map(|(key, value)| {
            let value = serde_json::to_string_pretty(&value).unwrap_or_else(|_| "null".into());
            format!("  {}: {}", Value::String(key), value.replace('\n', "\n  "))
        })
        .collect();
    if lines.is_empty() {
        return "{}".into();
    }
    format!("{{\n{}\n}}", lines.join(",\n"))
}

fn from_json(text: &str) -> Result<Settings, String> {
    let data: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    let Value::Object(map) = data else {
        return Err("不是 JSON 物件".into());
    };
    let text = |key: &str, default: &str| map.get(key).map_or_else(|| default.to_string(), py_str);
    let codec_text = text("name_codec", NameCodec::Big5.value());
    let name_codec = NameCodec::ALL
        .into_iter()
        .find(|c| c.value() == codec_text)
        .ok_or_else(|| format!("未知的字碼「{codec_text}」"))?;
    let recent_files = match map.get("recent_files") {
        None => Vec::new(),
        Some(value) => py_str_items(value).ok_or("recent_files 不是清單")?,
    };
    Ok(Settings {
        name_codec,
        recent_files,
        last_dir: text("last_dir", ""),
        radio_port: text("radio_port", ""),
        theme: text("theme", "auto"),
        iap_path: text("iap_path", ""),
        ambep2_path: text("ambep2_path", ""),
        recordings_dir: text("recordings_dir", ""),
        check_updates: map.get("check_updates").is_none_or(truthy),
        last_update_check: text("last_update_check", ""),
        extra: map
            .into_iter()
            .filter(|(key, _)| !KEYS.contains(&key.as_str()))
            .collect(),
    })
}

/// The settings in `directory`; defaults when there is no file yet, and defaults plus the reason
/// when the file cannot be used.
pub fn load_settings(directory: &Path) -> (Settings, Option<String>) {
    let path = directory.join(FILE_NAME);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return (Settings::default(), None),
        Err(e) => {
            return (
                Settings::default(),
                Some(format!("無法讀取 {}：{e}", path.display())),
            );
        }
    };
    let parsed = String::from_utf8(bytes)
        .map_err(|e| e.to_string())
        .and_then(|text| from_json(&text));
    match parsed {
        Ok(settings) => (settings, None),
        Err(e) => {
            // Kept aside: the next save would otherwise overwrite whatever the file held.
            let aside = directory.join(format!("{FILE_NAME}.bad"));
            let note = match fs::copy(&path, &aside) {
                Ok(_) => format!("（原檔另存為 {}）", aside.display()),
                Err(_) => String::new(),
            };
            (
                Settings::default(),
                Some(format!(
                    "設定檔 {} 無法使用，改用預設值：{e}{note}",
                    path.display()
                )),
            )
        }
    }
}

/// Write the settings through a temporary file, so a crash mid-write never leaves half a file.
pub fn save_settings(settings: &Settings, directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    let temp = directory.join(format!("{FILE_NAME}.tmp"));
    fs::write(&temp, settings.to_json())?;
    fs::rename(&temp, directory.join(FILE_NAME)).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })
}

// ---------------------------------------------------------------- legacy folder
/// `os.path.normpath(path)` as parts: no `.`, and `..` applied where it can be.
fn normalized(path: &Path) -> Vec<Component<'_>> {
    let mut parts: Vec<Component> = Vec::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir if matches!(parts.last(), Some(Component::Normal(_))) => {
                parts.pop();
            }
            Component::ParentDir if matches!(parts.last(), Some(Component::RootDir)) => {}
            other => parts.push(other),
        }
    }
    parts
}

/// Equal path parts (`os.path.normcase`: case-insensitive on Windows).
fn same_part(a: &Component, b: &Component) -> bool {
    let (a, b) = (
        a.as_os_str().to_string_lossy(),
        b.as_os_str().to_string_lossy(),
    );
    if cfg!(windows) {
        a.to_lowercase() == b.to_lowercase()
    } else {
        a == b
    }
}

/// `text` moved from under `old` to under `new`, or `None` when it is not inside `old`.
fn repointed(text: &str, old: &Path, new: &Path) -> Option<String> {
    let (base, full) = (normalized(old), normalized(Path::new(text)));
    let inside = !base.is_empty()
        && full.len() >= base.len()
        && base.iter().zip(&full).all(|(a, b)| same_part(a, b));
    if !inside {
        return None;
    }
    let rest: PathBuf = full[base.len()..].iter().collect();
    let moved = if rest.as_os_str().is_empty() {
        new.to_path_buf()
    } else {
        new.join(rest)
    };
    Some(moved.to_string_lossy().into_owned())
}

/// `value` with a leading `old` folder replaced by `new` (strings, and lists of them).
fn repoint(value: Value, old: &Path, new: &Path) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|v| repoint(v, old, new)).collect())
        }
        Value::String(s) if !s.is_empty() => match repointed(&s, old, new) {
            Some(moved) => Value::String(moved),
            None => Value::String(s),
        },
        other => other,
    }
}

fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn repoint_settings(new: &Path, old: &Path) -> Result<(), String> {
    let file = new.join(FILE_NAME);
    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
        return Ok(());
    };
    // The file's own keys and values, re-pointed; the known keys first.
    let mut entries: BTreeMap<String, Value> = map
        .into_iter()
        .map(|(key, value)| (key, repoint(value, old, new)))
        .collect();
    let known: Vec<(String, Value)> = KEYS
        .iter()
        .filter_map(|key| entries.remove_entry(*key))
        .collect();
    fs::write(&file, json_object(known.into_iter().chain(entries))).map_err(|e| e.to_string())
}

/// Move the pre-rename data folder `old` (settings, backups, recordings, RadioID cache, logs) to
/// `new` once and re-point saved paths inside it; returns what happened ("" = nothing). A folder
/// that cannot be renamed is copied through a staging folder instead, so a half-finished copy
/// never looks like a migrated folder.
pub fn migrate_dir(old: &Path, new: &Path) -> String {
    if new.exists() || !old.is_dir() {
        return String::new();
    }
    let how = if fs::rename(old, new).is_ok() {
        "搬到"
    } else {
        let staging = new.with_file_name(format!(
            "{}.migrating",
            new.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        ));
        let _ = fs::remove_dir_all(&staging); // a leftover from an interrupted attempt
        if let Err(e) = copy_tree(old, &staging).and_then(|()| fs::rename(&staging, new)) {
            let _ = fs::remove_dir_all(&staging);
            return format!(
                "無法把舊資料夾 {} 搬到 {}（{e}）；舊資料仍在原處。",
                old.display(),
                new.display()
            );
        }
        "複製到"
    };
    let note = match repoint_settings(new, old) {
        Ok(()) => String::new(),
        Err(e) => format!("（設定檔內的路徑未更新：{e}）"),
    };
    format!(
        "已將舊資料夾 {} {how} {}（設定、備份、錄音、RadioID 資料）。{note}",
        old.display(),
        new.display()
    )
}

/// [`migrate_dir`] for the default folders; nothing when the data folder is overridden.
pub fn migrate_legacy_dir() -> String {
    if std::env::var_os(paths::DATA_DIR_ENV).is_some_and(|v| !v.is_empty()) {
        return String::new();
    }
    let new = paths::data_dir();
    let old = new.with_file_name(LEGACY_DIR_NAME);
    migrate_dir(&old, &new)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("opendrs-settings-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn round_trip_keeps_unknown_keys_and_python_layout() {
        let dir = scratch("roundtrip");
        let (missing, problem) = load_settings(&dir);
        assert_eq!((missing, problem), (Settings::default(), None));
        let mut settings = Settings::default().with_recent("C:/radio/a.F136");
        settings
            .extra
            .insert("future".into(), serde_json::json!({"x": [1, "中"]}));
        save_settings(&settings, &dir).unwrap();
        let text = fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.starts_with(
            "{\n  \"name_codec\": \"big5\",\n  \"recent_files\": [\n    \"C:/radio/a.F136\"\n  ],"
        ));
        assert!(
            text.ends_with("  \"future\": {\n    \"x\": [\n      1,\n      \"中\"\n    ]\n  }\n}")
        );
        assert_eq!(load_settings(&dir).0, settings);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn values_are_read_like_python() {
        let settings = from_json(
            r#"{"recent_files": "ab", "last_dir": 5, "theme": null, "check_updates": "false",
                "radio_port": [1.0, "x", true]}"#,
        )
        .unwrap();
        assert_eq!(settings.recent_files, ["a", "b"]);
        assert_eq!(
            (settings.last_dir.as_str(), settings.theme.as_str()),
            ("5", "None")
        );
        assert!(settings.check_updates); // a non-empty string is true
        assert_eq!(settings.radio_port, "[1.0, 'x', True]");
        assert!(from_json(r#"{"check_updates": 0}"#).is_ok_and(|s| !s.check_updates));
        for bad in [
            r#"{"name_codec": "utf8"}"#,
            r#"{"recent_files": 3}"#,
            "[]",
            "{",
            "\u{feff}{}",
        ] {
            assert!(from_json(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn unusable_file_is_kept_aside_and_saves_replace_it_whole() {
        let dir = scratch("unusable");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(FILE_NAME), "[1").unwrap();
        let (settings, problem) = load_settings(&dir);
        assert_eq!(settings, Settings::default());
        assert!(problem.is_some_and(|p| p.contains(".bad")));
        assert_eq!(fs::read(dir.join("settings.json.bad")).unwrap(), b"[1");
        save_settings(&Settings::default().with_recent("x.F136"), &dir).unwrap();
        assert!(!dir.join("settings.json.tmp").exists());
        assert_eq!(load_settings(&dir).0.recent_files, ["x.F136"]);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recent_files_and_parent_folder() {
        let s = (0..10).fold(Settings::default(), |s, i| {
            s.with_recent(&format!("f{i}.F136"))
        });
        assert_eq!(s.recent_files.len(), MAX_RECENT);
        assert_eq!(
            s.with_recent("f5.F136").recent_files[..2],
            ["f5.F136", "f9.F136"]
        );
        assert_eq!(s.last_dir, ".");
        let nested = Settings::default().with_recent("a//b/./c.F136");
        assert_eq!(nested.last_dir, Path::new("a").join("b").to_string_lossy());
    }

    #[test]
    fn legacy_folder_is_moved_and_paths_repointed() {
        let root = scratch("migrate");
        let (old, new) = (root.join(LEGACY_DIR_NAME), root.join("OpenDRS"));
        fs::create_dir_all(old.join("backups")).unwrap();
        fs::write(old.join("backups").join("x.F136"), b"x").unwrap();
        let inside = old.join("rec").to_string_lossy().into_owned();
        let roundabout = old.join("..").join(LEGACY_DIR_NAME).join("a.F136");
        let json = serde_json::json!({
            "recordings_dir": inside,
            "last_dir": old.to_string_lossy(),
            "recent_files": ["/elsewhere/p.F136", roundabout.to_string_lossy()],
        });
        fs::write(old.join(FILE_NAME), json.to_string()).unwrap();
        let message = migrate_dir(&old, &new);
        assert!(
            message.starts_with("已將舊資料夾") && !old.exists(),
            "{message}"
        );
        assert!(new.join("backups").join("x.F136").is_file());
        let (settings, problem) = load_settings(&new);
        assert_eq!(problem, None);
        assert_eq!(Path::new(&settings.recordings_dir), new.join("rec"));
        assert_eq!(settings.recent_files[0], "/elsewhere/p.F136");
        assert_eq!(Path::new(&settings.recent_files[1]), new.join("a.F136"));
        assert_eq!(Path::new(&settings.last_dir), new); // no trailing separator
        assert_eq!(migrate_dir(&old, &new), ""); // only once
        fs::remove_dir_all(&root).unwrap();
    }
}
