//! The PowerShell script that swaps the program files once OpenDRS has exited
//! (`UPDATER_SCRIPT`, `updater_script`, `updater_command` and `launch` of `update.py`).

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use crate::install::PreparedUpdate;
use crate::{PROGRAM_ITEMS, UpdateError, io_at};

/// Windows PowerShell 5.1 script (ASCII only; written with a BOM). It runs after OpenDRS exits.
const UPDATER_SCRIPT: &str = r#"param(
    [Parameter(Mandatory=$true)][int]$AppPid,
    [Parameter(Mandatory=$true)][string]$Source,
    [Parameter(Mandatory=$true)][string]$Target,
    [Parameter(Mandatory=$true)][string]$Staging,
    [Parameter(Mandatory=$true)][string]$Log,
    [switch]$NoRestart
)
$ErrorActionPreference = 'Stop'
$items = @(ITEMS)
function Write-Log([string]$m) {
    $line = '{0} {1}' -f (Get-Date -Format s), $m
    Add-Content -LiteralPath $Log -Value $line -Encoding UTF8
}
$installed = $false
try {
    Write-Log "update: waiting for OpenDRS (pid $AppPid) to exit"
    $deadline = (Get-Date).AddSeconds(60)
    while (Get-Process -Id $AppPid -ErrorAction SilentlyContinue) {
        if ((Get-Date) -gt $deadline) { throw "OpenDRS (pid $AppPid) is still running" }
        Start-Sleep -Milliseconds 300
    }
    $backup = Join-Path $Staging 'previous'
    New-Item -ItemType Directory -Path $backup -Force | Out-Null
    $saved = @()
    $placed = @()
    try {
        foreach ($i in $items) {
            $p = Join-Path $Target $i
            if (Test-Path -LiteralPath $p) { Move-Item -LiteralPath $p -Destination $backup; $saved += $i }
        }
        foreach ($i in $items) {
            $s = Join-Path $Source $i
            if (Test-Path -LiteralPath $s) { Move-Item -LiteralPath $s -Destination $Target; $placed += $i }
        }
    } catch {
        Write-Log "update: install failed: $($_.Exception.Message); restoring the previous version"
        foreach ($i in $placed) {
            Remove-Item -LiteralPath (Join-Path $Target $i) -Recurse -Force -ErrorAction SilentlyContinue
        }
        foreach ($i in $saved) {
            Move-Item -LiteralPath (Join-Path $backup $i) -Destination $Target -ErrorAction SilentlyContinue
        }
        throw
    }
    $installed = $true
    Write-Log "update: installed into $Target"
    Remove-Item -LiteralPath $Staging -Recurse -Force -ErrorAction SilentlyContinue
} catch {
    Write-Log "update: ERROR $($_.Exception.Message)"
}
if (-not $NoRestart) {
    $exe = Join-Path $Target 'OpenDRS.exe'
    if (Test-Path -LiteralPath $exe) { Start-Process -FilePath $exe }
}
if ($installed) { exit 0 } else { exit 1 }
"#;

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// The updater script with [`PROGRAM_ITEMS`] filled in (`\n` line ends).
pub fn updater_script() -> String {
    let items: Vec<String> = PROGRAM_ITEMS.iter().map(|i| format!("'{i}'")).collect();
    UPDATER_SCRIPT.replace("ITEMS", &items.join(", "))
}

/// Write `apply-update.ps1` into the staging folder (UTF-8 with a BOM, CRLF on Windows as
/// Python's text mode writes it) and return the command line that runs it.
fn command_line(
    prepared: &PreparedUpdate,
    pid: u32,
    log: &Path,
    restart: bool,
) -> Result<Vec<OsString>, UpdateError> {
    let script = prepared.staging.join("apply-update.ps1");
    let text = if cfg!(windows) {
        updater_script().replace('\n', "\r\n")
    } else {
        updater_script()
    };
    fs::write(&script, [UTF8_BOM, text.as_bytes()].concat()).map_err(io_at(&script))?;
    let mut command: Vec<OsString> = vec![powershell_exe().into_os_string()];
    command.extend(
        [
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-WindowStyle",
            "Hidden",
            "-File",
        ]
        .iter()
        .map(OsString::from),
    );
    command.push(script.into_os_string());
    let pid = pid.to_string();
    let paths = [
        ("-AppPid", Path::new(&pid)),
        ("-Source", prepared.source.as_path()),
        ("-Target", prepared.target.as_path()),
        ("-Staging", prepared.staging.as_path()),
        ("-Log", log),
    ];
    for (flag, value) in paths {
        command.push(flag.into());
        command.push(value.as_os_str().to_os_string());
    }
    if !restart {
        command.push("-NoRestart".into());
    }
    Ok(command)
}

/// Write `apply-update.ps1` into the staging folder and return the command that runs it: the
/// updater waits for process `pid` to exit, then installs; `restart` starts the new
/// `OpenDRS.exe` afterwards. (Paths that are not valid Unicode are shown lossily here;
/// [`launch`] passes them unchanged.)
pub fn updater_command(
    prepared: &PreparedUpdate,
    pid: u32,
    log: &Path,
    restart: bool,
) -> Result<Vec<String>, UpdateError> {
    Ok(command_line(prepared, pid, log, restart)?
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect())
}

/// Windows PowerShell 5.1 by its full path (`%SystemRoot%\System32\WindowsPowerShell\v1.0`), so
/// a `powershell.exe` in the program folder or on PATH is never the one started; the bare name
/// only where that file is missing.
pub fn powershell_exe() -> PathBuf {
    std::env::var_os("SystemRoot")
        .map(|root| PathBuf::from(root).join(r"System32\WindowsPowerShell\v1.0\powershell.exe"))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

/// Start the updater; the caller must exit right after so the files can be replaced.
#[cfg(windows)]
pub fn launch(prepared: &PreparedUpdate, log: &Path) -> Result<(), UpdateError> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent).map_err(io_at(parent))?;
    }
    let command = command_line(prepared, std::process::id(), log, true)?;
    let (program, args) = command
        .split_first()
        .ok_or_else(|| crate::message("無法啟動更新程式"))?;
    let cwd = prepared.staging.parent().unwrap_or(&prepared.staging);
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .current_dir(cwd)
        .spawn()
        .map(drop)
        .map_err(|err| crate::message(format!("無法啟動更新程式（{err}）")))
}

/// Installing is only supported on Windows: elsewhere this always fails.
#[cfg(not(windows))]
pub fn launch(_prepared: &PreparedUpdate, _log: &Path) -> Result<(), UpdateError> {
    Err(crate::message(crate::INSTALL_UNSUPPORTED))
}
