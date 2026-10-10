//! What cargo knows: the host platform, the workspace's version and folders, the resolved
//! dependency graph, and the release build itself.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use crate::Result;

/// The `cargo` that runs this task (so `cargo +nightly xtask` builds with the same toolchain).
fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn output(command: &mut Command, what: &str) -> Result<String> {
    let out = command
        .output()
        .map_err(|e| format!("無法執行 {what}：{e}"))?;
    if !out.status.success() {
        return Err(format!(
            "{what} 失敗：{}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("{what} 的輸出不是 UTF-8：{e}"))
}

/// The host's target triple, e.g. `x86_64-pc-windows-msvc`.
pub fn host_triple() -> Result<String> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let text = output(Command::new(rustc).arg("-vV"), "rustc -vV")?;
    text.lines()
        .find_map(|l| l.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV 沒有列出 host".into())
}

/// `cargo metadata` for the workspace, the dependency graph resolved for `triple`.
pub struct Metadata {
    json: Value,
}

impl Metadata {
    pub fn load(workspace: &Path, triple: &str) -> Result<Self> {
        let text = output(
            cargo()
                .current_dir(workspace)
                .args(["metadata", "--format-version", "1", "--locked"])
                .args(["--filter-platform", triple]),
            "cargo metadata",
        )?;
        let json = serde_json::from_str(&text).map_err(|e| format!("cargo metadata：{e}"))?;
        Ok(Metadata { json })
    }

    pub fn packages(&self) -> &[Value] {
        self.json["packages"].as_array().map_or(&[], Vec::as_slice)
    }

    pub fn package(&self, name: &str) -> Option<&Value> {
        let members: Vec<&str> = self.json["workspace_members"]
            .as_array()
            .map(|m| m.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        self.packages()
            .iter()
            .find(|p| p["name"] == name && p["id"].as_str().is_some_and(|id| members.contains(&id)))
    }

    pub fn is_member(&self, id: &str) -> bool {
        self.json["workspace_members"]
            .as_array()
            .is_some_and(|m| m.iter().any(|v| v == id))
    }

    pub fn target_directory(&self) -> PathBuf {
        PathBuf::from(self.json["target_directory"].as_str().unwrap_or("target"))
    }

    pub fn workspace_root(&self) -> PathBuf {
        PathBuf::from(self.json["workspace_root"].as_str().unwrap_or("."))
    }
}

/// (name, version) of every crate the binaries of `packages` are built from for `triple`:
/// `cargo tree` over normal dependencies, with the features of a release build (unlike
/// `cargo metadata`, whose graph unifies the features of tests and other workspace members).
pub fn tree(workspace: &Path, triple: &str, packages: &[&str]) -> Result<Vec<(String, String)>> {
    let mut command = cargo();
    command
        .current_dir(workspace)
        .args([
            "tree", "--locked", "-e", "normal", "--prefix", "none", "-f", "{p}",
        ])
        .args(["--target", triple]);
    for package in packages {
        command.args(["-p", package]);
    }
    let text = output(&mut command, "cargo tree")?;
    let mut crates: Vec<(String, String)> = text
        .lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let version = words.next()?.strip_prefix('v')?;
            Some((name.to_owned(), version.to_owned()))
        })
        .collect();
    crates.sort();
    crates.dedup();
    Ok(crates)
}

/// `cargo build --release --locked` of the shipped binaries.
pub fn build_release(workspace: &Path, packages: &[&str]) -> Result<()> {
    let mut command = cargo();
    command
        .current_dir(workspace)
        .args(["build", "--release", "--locked"]);
    for package in packages {
        command.args(["-p", package]);
    }
    let status = command
        .status()
        .map_err(|e| format!("無法執行 cargo build：{e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo build --release 失敗（{status}）"))
    }
}
