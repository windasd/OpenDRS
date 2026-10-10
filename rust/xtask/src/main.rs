//! Release packaging (port of packaging/build.py).
//!
//! `cargo xtask dist [--expect-tag vX.Y.Z] [--skip-build]` makes the release archive for the
//! platform this runs on:
//!
//! 1. `cargo build --release --locked` of the editor (`opendrs`) and the CLI (`opendrs-rs`);
//! 2. stage `target/dist/OpenDRS/` (see `layout`): the programs, README, licences, and on macOS
//!    the `.app` bundle, on Linux the desktop file and icon;
//! 3. run the staged editor's `--self-test` and `opendrs-rs --version`; fail unless every check
//!    passed in a release build of this version (on Windows the .exe must also not need the
//!    Visual C++ runtime DLL);
//! 4. archive it as `dist/OpenDRS-<version>-<os>-<arch>.zip` (`.tar.gz` on Linux) + `.sha256`.
//!
//! `cargo xtask licenses [--target TRIPLE]...` only checks that every crate of the shipped
//! binaries has a licence text, for any platform (CI checks all three from one runner).
//!
//! The GitHub Actions workflow runs `dist` on Windows, macOS and Linux.

mod archive;
mod cargo;
mod layout;
mod licenses;
mod selftest;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

use layout::Os;

pub type Result<T> = std::result::Result<T, String>;

const PRODUCT: &str = "OpenDRS";
const SHIPPED: [&str; 2] = ["opendrs-gui", "opendrs-cli"];
const USAGE: &str = "用法：cargo xtask dist [--expect-tag vX.Y.Z] [--skip-build]
      cargo xtask licenses [--target TRIPLE]...";
/// The DLL of the Visual C++ runtime, which a clean Windows does not have.
const VC_RUNTIME: &[u8] = b"VCRUNTIME140";

#[derive(Debug, PartialEq, Eq)]
enum Task {
    Dist {
        expect_tag: Option<String>,
        skip_build: bool,
    },
    Licenses {
        targets: Vec<String>,
    },
}

fn parse(args: &[String]) -> Result<Task> {
    let Some((command, rest)) = args.split_first() else {
        return Err(USAGE.into());
    };
    let mut rest = rest.iter();
    match command.as_str() {
        "dist" => {
            let (mut expect_tag, mut skip_build) = (None, false);
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--expect-tag" => expect_tag = Some(rest.next().ok_or(USAGE)?.clone()),
                    "--skip-build" => skip_build = true,
                    _ => return Err(format!("不認得的參數 {arg}\n{USAGE}")),
                }
            }
            Ok(Task::Dist {
                expect_tag,
                skip_build,
            })
        }
        "licenses" => {
            let mut targets = Vec::new();
            while let Some(arg) = rest.next() {
                match arg.as_str() {
                    "--target" => targets.push(rest.next().ok_or(USAGE)?.clone()),
                    _ => return Err(format!("不認得的參數 {arg}\n{USAGE}")),
                }
            }
            Ok(Task::Licenses { targets })
        }
        _ => Err(USAGE.into()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse(&args).and_then(|task| match task {
        Task::Dist {
            expect_tag,
            skip_build,
        } => dist(expect_tag.as_deref(), skip_build)
            .map(|archive| format!("完成：{}", archive.display())),
        Task::Licenses { targets } => check_licenses(&targets).map(|()| "授權檢查通過".into()),
    });
    match result {
        Ok(message) => {
            println!("{message}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("錯誤：{e}");
            ExitCode::FAILURE
        }
    }
}

fn workspace() -> Result<PathBuf> {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "找不到 workspace".into())
}

fn fresh_dir(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(|e| format!("無法清除 {}：{e}", path.display()))?;
    }
    std::fs::create_dir_all(path).map_err(|e| format!("無法建立 {}：{e}", path.display()))
}

fn check_licenses(targets: &[String]) -> Result<()> {
    let workspace = workspace()?;
    let targets = if targets.is_empty() {
        vec![cargo::host_triple()?]
    } else {
        targets.to_vec()
    };
    for triple in &targets {
        let crates = licenses::collect(&workspace, triple, &SHIPPED)?;
        println!("{triple}：{} 個套件都有授權全文", crates.len());
    }
    Ok(())
}

/// Refuse a Windows program that imports the Visual C++ runtime DLL.
fn check_static_runtime(os: Os, program: &Path) -> Result<()> {
    if os != Os::Windows {
        return Ok(());
    }
    let bytes = std::fs::read(program).map_err(|e| format!("{}：{e}", program.display()))?;
    let needs_dll = bytes
        .windows(VC_RUNTIME.len())
        .any(|w| w.eq_ignore_ascii_case(VC_RUNTIME));
    if needs_dll {
        return Err(format!(
            "{} 需要 VCRUNTIME140.dll（沒有以 +crt-static 建置？見 rust/.cargo/config.toml）",
            program.display()
        ));
    }
    Ok(())
}

fn dist(expect_tag: Option<&str>, skip_build: bool) -> Result<PathBuf> {
    let workspace = workspace()?;
    let triple = cargo::host_triple()?;
    let (os, platform) = layout::platform(&triple)?;
    let metadata = cargo::Metadata::load(&workspace, &triple)?;
    let version = metadata
        .package("opendrs-gui")
        .and_then(|p| p["version"].as_str())
        .ok_or("找不到 opendrs-gui 的版本")?
        .to_owned();
    if let Some(tag) = expect_tag {
        if tag != format!("v{version}") {
            return Err(format!(
                "標籤 {tag} 與程式版本 {version} 不符（請先修改 rust/Cargo.toml 的版本）"
            ));
        }
    }
    println!("{PRODUCT} {version}，{triple}");
    // Before building: a crate without a licence text stops the release early.
    let crates = licenses::collect(&workspace, &triple, &SHIPPED)?;
    if !skip_build {
        cargo::build_release(&workspace, &SHIPPED)?;
    }
    let release = metadata.target_directory().join("release");
    let stage_root = metadata.target_directory().join("dist");
    let stage = stage_root.join(PRODUCT);
    fresh_dir(&stage_root)?;
    layout::stage(os, &workspace, &release, &stage, &version)?;
    let notices = stage.join("licenses").join("THIRD_PARTY_LICENSES.txt");
    std::fs::write(
        &notices,
        licenses::render(PRODUCT, version.as_str(), &crates),
    )
    .map_err(|e| format!("{}：{e}", notices.display()))?;
    println!("授權：{} 個套件", crates.len());

    let program = layout::program(os, &stage);
    check_static_runtime(os, &program)?;
    let report = stage_root.join(format!("selftest-{platform}.json"));
    let guard = stage_root.join("selftest-data");
    selftest::run(&program, &report, &guard, &version)?;
    selftest::check_cli(&layout::cli(os, &stage), &version)?;

    write_archive(
        os,
        &metadata.workspace_root().join("dist"),
        &stage,
        &version,
        &platform,
    )
}

fn write_archive(
    os: Os,
    out: &Path,
    stage: &Path,
    version: &str,
    platform: &str,
) -> Result<PathBuf> {
    std::fs::create_dir_all(out).map_err(|e| format!("{}：{e}", out.display()))?;
    // Archives of this platform from earlier versions go (as build.py removed old zips).
    let suffix = format!("-{platform}.");
    for entry in std::fs::read_dir(out).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(&format!("{PRODUCT}-")) && name.contains(&suffix) {
            std::fs::remove_file(entry.path()).map_err(|e| format!("無法刪除 {name}：{e}"))?;
        }
    }
    let extension = if os == Os::Linux { "tar.gz" } else { "zip" };
    let archive = out.join(format!("{PRODUCT}-{version}-{platform}.{extension}"));
    let files = archive::entries(stage, PRODUCT, &|name| layout::is_executable(os, name))?;
    let now = SystemTime::now();
    if os == Os::Linux {
        archive::write_tar_gz(&archive, &files, now)?;
    } else {
        archive::write_zip(&archive, &files, now)?;
    }
    let digest = archive::write_checksum(&archive)?;
    let size = std::fs::metadata(&archive).map_or(0, |m| m.len());
    println!(
        "{}  {:.1} MB  sha256 {digest}",
        archive.display(),
        size as f64 / 1e6
    );
    Ok(archive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn tasks() {
        assert_eq!(
            parse(&args(&["dist"])),
            Ok(Task::Dist {
                expect_tag: None,
                skip_build: false
            })
        );
        assert_eq!(
            parse(&args(&["dist", "--expect-tag", "v1.2.3", "--skip-build"])),
            Ok(Task::Dist {
                expect_tag: Some("v1.2.3".into()),
                skip_build: true
            })
        );
        assert_eq!(
            parse(&args(&["licenses", "--target", "a", "--target", "b"])),
            Ok(Task::Licenses {
                targets: vec!["a".into(), "b".into()]
            })
        );
        assert!(parse(&args(&[])).is_err());
        assert!(parse(&args(&["build"])).is_err());
        assert!(parse(&args(&["dist", "--expect-tag"])).is_err());
        assert!(parse(&args(&["dist", "--fast"])).is_err());
        assert!(parse(&args(&["licenses", "--target"])).is_err());
    }

    #[test]
    fn a_program_needing_the_vc_runtime_is_refused() {
        let dir = std::env::temp_dir().join(format!("xtask-crt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let dynamic = dir.join("dynamic.exe");
        std::fs::write(&dynamic, b"MZ...vcruntime140.dll...").expect("write");
        let fixed = dir.join("static.exe");
        std::fs::write(&fixed, b"MZ...kernel32.dll...").expect("write");
        assert!(check_static_runtime(Os::Windows, &dynamic).is_err());
        assert!(check_static_runtime(Os::Windows, &fixed).is_ok());
        assert!(check_static_runtime(Os::Linux, &dynamic).is_ok());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn old_archives_of_the_platform_are_replaced() {
        let dir = std::env::temp_dir().join(format!("xtask-out-{}", std::process::id()));
        let stage = dir.join("stage");
        std::fs::create_dir_all(&stage).expect("stage");
        std::fs::write(stage.join("README.txt"), b"x").expect("write");
        let out = dir.join("dist");
        std::fs::create_dir_all(&out).expect("out");
        for old in [
            "OpenDRS-0.0.9-linux-x64.tar.gz",
            "OpenDRS-0.0.9-linux-x64.tar.gz.sha256",
        ] {
            std::fs::write(out.join(old), b"old").expect("write");
        }
        std::fs::write(out.join("OpenDRS-0.0.9-windows-x64.zip"), b"other").expect("write");
        write_archive(Os::Linux, &out, &stage, "0.1.0", "linux-x64").expect("archive");
        let mut names: Vec<String> = std::fs::read_dir(&out)
            .expect("list")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "OpenDRS-0.0.9-windows-x64.zip",
                "OpenDRS-0.1.0-linux-x64.tar.gz",
                "OpenDRS-0.1.0-linux-x64.tar.gz.sha256"
            ]
        );
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}
