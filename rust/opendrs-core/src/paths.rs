//! Where OpenDRS keeps its data on each platform.
//!
//! | platform | folder                                          |
//! |----------|-------------------------------------------------|
//! | Windows  | `%APPDATA%\OpenDRS` (same as the Python version) |
//! | macOS    | `~/Library/Application Support/OpenDRS`         |
//! | Linux    | `$XDG_CONFIG_HOME/OpenDRS` or `~/.config/OpenDRS` |
//!
//! `OPENDRS_DATA_DIR` overrides it (tests, portable use).

use std::path::PathBuf;

pub const APP_DIR: &str = "OpenDRS";
pub const DATA_DIR_ENV: &str = "OPENDRS_DATA_DIR";

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|v| !v.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .map_or_else(|| PathBuf::from(APP_DIR), |base| base.join(APP_DIR))
}

/// Backups kept before every write and after every read.
pub fn backups_dir() -> PathBuf {
    data_dir().join("backups")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dir_ends_with_the_app_folder() {
        if std::env::var_os(DATA_DIR_ENV).is_none() {
            assert!(data_dir().ends_with(APP_DIR));
        }
        assert_eq!(backups_dir(), data_dir().join("backups"));
    }
}
