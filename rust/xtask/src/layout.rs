//! What goes into the release folder on each platform:
//!
//! - Windows: `OpenDRS.exe` (icon and version information embedded by opendrs-gui's build.rs),
//!   `opendrs-rs.exe`;
//! - macOS: `OpenDRS.app` (Info.plist, icns icon), `opendrs-rs`;
//! - Linux: `opendrs`, `opendrs-rs`, `opendrs.desktop`, `opendrs.png`;
//!
//! plus `README.txt` and `licenses/` everywhere.

use std::path::{Path, PathBuf};

use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    Macos,
    Linux,
}

/// The platform of a target triple: (os, archive suffix such as `windows-x64`).
pub fn platform(triple: &str) -> Result<(Os, String)> {
    let os = if triple.contains("-windows") {
        Os::Windows
    } else if triple.contains("-apple-darwin") {
        Os::Macos
    } else if triple.contains("-linux") {
        Os::Linux
    } else {
        return Err(format!("不支援的平台 {triple}"));
    };
    let arch = match triple.split('-').next().unwrap_or_default() {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let name = match os {
        Os::Windows => "windows",
        Os::Macos => "macos",
        Os::Linux => "linux",
    };
    Ok((os, format!("{name}-{arch}")))
}

fn exe(os: Os, name: &str) -> String {
    if os == Os::Windows {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// The editor inside the staged folder (what the self-test runs).
pub fn program(os: Os, stage: &Path) -> PathBuf {
    match os {
        Os::Windows => stage.join("OpenDRS.exe"),
        Os::Macos => stage.join("OpenDRS.app/Contents/MacOS/OpenDRS"),
        Os::Linux => stage.join("opendrs"),
    }
}

/// The command-line tool inside the staged folder.
pub fn cli(os: Os, stage: &Path) -> PathBuf {
    stage.join(exe(os, "opendrs-rs"))
}

/// Files that must be executable in the archive (paths relative to the staged folder).
pub fn is_executable(os: Os, relative: &str) -> bool {
    match os {
        Os::Windows => false,
        Os::Macos => matches!(
            relative,
            "OpenDRS.app/Contents/MacOS/OpenDRS" | "opendrs-rs"
        ),
        Os::Linux => matches!(relative, "opendrs" | "opendrs-rs"),
    }
}

fn copy(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}：{e}", parent.display()))?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("無法複製 {} 到 {}：{e}", from.display(), to.display()))
}

fn write(to: &Path, text: &str) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}：{e}", parent.display()))?;
    }
    std::fs::write(to, text).map_err(|e| format!("{}：{e}", to.display()))
}

/// A template from `rust/packaging` with `{version}` filled in; CRLF line ends on Windows.
pub fn template(workspace: &Path, name: &str, version: &str, os: Os) -> Result<String> {
    let path = workspace.join("packaging").join(name);
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("{}：{e}", path.display()))?
        .replace("\r\n", "\n")
        .replace("{version}", version);
    Ok(if os == Os::Windows {
        text.replace('\n', "\r\n")
    } else {
        text
    })
}

/// Fill `stage` from the release binaries in `release`.
pub fn stage(os: Os, workspace: &Path, release: &Path, stage: &Path, version: &str) -> Result<()> {
    let gui = workspace.join("opendrs-gui").join("assets");
    copy(&release.join(exe(os, "opendrs-rs")), &cli(os, stage))?;
    copy(&release.join(exe(os, "opendrs")), &program(os, stage))?;
    match os {
        Os::Windows => {}
        Os::Macos => {
            let contents = stage.join("OpenDRS.app/Contents");
            write(
                &contents.join("Info.plist"),
                &template(workspace, "Info.plist", version, os)?,
            )?;
            write(&contents.join("PkgInfo"), "APPL????")?;
            copy(
                &gui.join("opendrs.icns"),
                &contents.join("Resources/opendrs.icns"),
            )?;
        }
        Os::Linux => {
            write(
                &stage.join("opendrs.desktop"),
                &template(workspace, "opendrs.desktop", version, os)?,
            )?;
            copy(&gui.join("opendrs.png"), &stage.join("opendrs.png"))?;
        }
    }
    write(
        &stage.join("README.txt"),
        &template(workspace, "README.txt", version, os)?,
    )?;
    for (_, _, file) in crate::licenses::STANDARD {
        copy(
            &workspace.join("packaging/licenses").join(file),
            &stage.join("licenses").join(file),
        )?;
    }
    write(
        &stage.join("licenses/THIRD_PARTY_NOTICES.md"),
        &template(workspace, "THIRD_PARTY_NOTICES.md", version, os)?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platforms_of_the_ci_runners() {
        assert_eq!(
            platform("x86_64-pc-windows-msvc").expect("windows"),
            (Os::Windows, "windows-x64".into())
        );
        assert_eq!(
            platform("aarch64-apple-darwin").expect("macos"),
            (Os::Macos, "macos-arm64".into())
        );
        assert_eq!(
            platform("x86_64-unknown-linux-gnu").expect("linux"),
            (Os::Linux, "linux-x64".into())
        );
        assert!(platform("wasm32-unknown-unknown").is_err());
    }

    #[test]
    fn only_the_programs_are_executable() {
        assert!(is_executable(Os::Linux, "opendrs") && is_executable(Os::Linux, "opendrs-rs"));
        assert!(!is_executable(Os::Linux, "README.txt"));
        assert!(is_executable(
            Os::Macos,
            "OpenDRS.app/Contents/MacOS/OpenDRS"
        ));
        assert!(!is_executable(Os::Macos, "OpenDRS.app/Contents/Info.plist"));
    }

    #[test]
    fn templates_name_the_version() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        for name in [
            "README.txt",
            "Info.plist",
            "opendrs.desktop",
            "THIRD_PARTY_NOTICES.md",
        ] {
            let text = template(&workspace, name, "9.8.7", Os::Linux).expect(name);
            assert!(
                !text.contains("{version}") && !text.contains('\r'),
                "{name}"
            );
        }
        let readme = template(&workspace, "README.txt", "9.8.7", Os::Windows).expect("readme");
        assert!(readme.contains("9.8.7") && readme.contains("\r\n"));
        let plist = template(&workspace, "Info.plist", "9.8.7", Os::Macos).expect("plist");
        assert!(plist.contains("<string>9.8.7</string>"));
    }
}
