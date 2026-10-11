"""Check GitHub Releases for a newer OpenDRS and install it (the Windows .exe build).

* fetch_latest() reads the latest published release of windasd/OpenDRS.
* prepare() downloads ``OpenDRS-<version>-win64.zip``, checks its SHA-256 against the digest
  GitHub reports for the asset *and* the release's ``.sha256`` file (whichever exist; at least
  one is required, and they must agree), and unpacks it into a staging folder next to the
  installed program (same volume, so the swap is a rename).
* launch() starts a PowerShell script that waits for this process to exit, swaps only the
  program files (PROGRAM_ITEMS; anything else in the folder is left alone, and settings,
  backups and recordings live in %APPDATA%), keeps the old files until the swap succeeded,
  rolls back on failure, and starts the new version.

The checksums come from the same release as the zip: they catch corrupt or truncated
downloads, not a compromised repository.
"""

from __future__ import annotations

import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile
from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path, PurePosixPath
from urllib.error import HTTPError

from . import __url__, __version__

REPO = "windasd/OpenDRS"
API_LATEST = f"https://api.github.com/repos/{REPO}/releases/latest"
RELEASES_URL = f"{__url__}/releases"
DOWNLOAD_PREFIX = f"https://github.com/{REPO}/releases/download/"
USER_AGENT = f"OpenDRS/{__version__} (update check)"
PROGRAM_DIR = "OpenDRS"
PROGRAM_ITEMS = ("OpenDRS.exe", "_internal", "licenses", "README.md")
STAGING_PREFIX = ".opendrs-update-"
CHECK_INTERVAL = timedelta(hours=20)  # about once a day without drifting later each day
MAX_JSON_BYTES = 2_000_000
MAX_ZIP_BYTES = 300_000_000
MAX_UNPACKED_BYTES = 1_000_000_000
MAX_CHECKSUM_BYTES = 4096
CHUNK = 256 * 1024
_VERSION = re.compile(r"^v?(\d+(?:\.\d+){0,3})(?:(a|b|rc)(\d+))?$")
_PRE_RANK = {"a": 0, "b": 1, "rc": 2}
_SHA256 = re.compile(r"^[0-9a-f]{64}$")

Opener = Callable[..., object]
Progress = Callable[[int, int], None]


class UpdateError(Exception):
    """A problem worth showing to the user as is."""


def parse_version(text: str) -> tuple[int, ...]:
    """'v0.2.0', '0.2', '1.0.0rc1' -> comparable tuples; a pre-release sorts before its release."""
    match = _VERSION.match(text.strip())
    if not match:
        raise UpdateError(f"無法辨識的版本號「{text}」")
    numbers = [int(n) for n in match.group(1).split(".")]
    pre = (_PRE_RANK[match.group(2)], int(match.group(3))) if match.group(2) else (3, 0)
    return (*(numbers + [0] * 4)[:4], *pre)


@dataclass(frozen=True)
class Asset:
    name: str
    url: str
    size: int
    sha256: str  # as reported by GitHub ("" if it reported none)


@dataclass(frozen=True)
class Release:
    tag: str
    version: str
    title: str
    notes: str  # Markdown
    page_url: str
    published: str
    package: Asset | None  # OpenDRS-<version>-win64.zip
    checksum: Asset | None  # its .sha256 file

    def is_newer_than(self, current: str) -> bool:
        return parse_version(self.version) > parse_version(current)


def _asset(item: dict) -> Asset:
    digest = str(item.get("digest") or "")
    sha = digest.removeprefix("sha256:").lower() if digest.startswith("sha256:") else ""
    return Asset(str(item.get("name", "")), str(item.get("browser_download_url", "")),
                 int(item.get("size") or 0), sha if _SHA256.match(sha) else "")


def parse_release(data: object) -> Release:
    if not isinstance(data, dict) or not isinstance(data.get("tag_name"), str):
        raise UpdateError("GitHub 回傳的發佈資料格式不符")
    if data.get("draft") or data.get("prerelease"):
        raise UpdateError("最新發佈是草稿或預先發行版")
    tag = data["tag_name"]
    version = tag.removeprefix("v")
    parse_version(version)
    wanted = f"OpenDRS-{version}-win64.zip"
    assets = [_asset(a) for a in data.get("assets") or [] if isinstance(a, dict)]
    by_name = {a.name: a for a in assets if a.url.startswith(DOWNLOAD_PREFIX + tag + "/")}
    return Release(tag=tag, version=version, title=str(data.get("name") or tag),
                   notes=str(data.get("body") or ""), page_url=str(data.get("html_url") or RELEASES_URL),
                   published=str(data.get("published_at") or ""),
                   package=by_name.get(wanted), checksum=by_name.get(wanted + ".sha256"))


def _request(url: str, accept: str) -> urllib.request.Request:
    return urllib.request.Request(url, headers={"User-Agent": USER_AGENT, "Accept": accept,
                                                "X-GitHub-Api-Version": "2022-11-28"})


def _open(opener: Opener, request: urllib.request.Request, timeout: float, what: str) -> object:
    try:
        return opener(request, timeout=timeout)
    except HTTPError as exc:
        if exc.code == 404:
            raise UpdateError(f"{what}：找不到（HTTP 404）") from exc
        if exc.code in (403, 429):
            raise UpdateError(f"{what}：GitHub 暫時限制查詢次數，請稍後再試（HTTP {exc.code}）") from exc
        raise UpdateError(f"{what}：HTTP {exc.code}") from exc
    except OSError as exc:
        raise UpdateError(f"{what}：無法連線（{exc}）") from exc


def fetch_latest(opener: Opener = urllib.request.urlopen, timeout: float = 15.0) -> Release:
    response = _open(opener, _request(API_LATEST, "application/vnd.github+json"), timeout, "查詢最新版本")
    with response:
        raw = response.read(MAX_JSON_BYTES + 1)
    if len(raw) > MAX_JSON_BYTES:
        raise UpdateError("GitHub 回傳的資料過大")
    try:
        return parse_release(json.loads(raw.decode("utf-8")))
    except (ValueError, UnicodeDecodeError) as exc:
        raise UpdateError("GitHub 回傳的資料無法解析") from exc


def check_due(last_check: str, now: datetime) -> bool:
    """Whether the daily automatic check should run (``last_check`` is an ISO timestamp)."""
    try:
        then = datetime.fromisoformat(last_check)
    except ValueError:
        return True
    if then.tzinfo is None or now.tzinfo is None:
        return True
    return now - then >= CHECK_INTERVAL


# ---------------------------------------------------------------- install
def install_dir() -> Path | None:
    """The folder of the running OpenDRS.exe, or None when running from source."""
    if not getattr(sys, "frozen", False):
        return None
    return Path(sys.executable).resolve().parent


def writable(folder: Path) -> bool:
    try:
        fd, name = tempfile.mkstemp(dir=folder, prefix=STAGING_PREFIX, suffix=".probe")
    except OSError:
        return False
    os.close(fd)
    Path(name).unlink(missing_ok=True)
    return True


def _download(opener: Opener, url: str, target: Path, limit: int, expected_size: int, timeout: float,
              progress: Progress | None, cancelled: Callable[[], bool]) -> None:
    response = _open(opener, _request(url, "application/octet-stream"), timeout, "下載更新")
    got = 0
    with response, target.open("wb") as fh:
        while chunk := response.read(CHUNK):
            got += len(chunk)
            if got > limit:
                raise UpdateError("下載的檔案超過大小上限，已中止")
            if cancelled():
                raise UpdateError("已取消")
            fh.write(chunk)
            if progress:
                progress(got, expected_size)
    if expected_size and got != expected_size:
        raise UpdateError(f"下載不完整（{got:,} / {expected_size:,} bytes）")


def parse_checksum_file(text: str, file_name: str) -> str:
    """The hex digest from a ``sha256sum`` line naming ``file_name`` (CR/LF tolerant)."""
    for line in text.splitlines():
        parts = line.strip().split()
        if len(parts) == 2 and parts[1].lstrip("*") == file_name and _SHA256.match(parts[0].lower()):
            return parts[0].lower()
    raise UpdateError(f".sha256 檔裡沒有 {file_name} 的校驗碼")


def expected_sha256(release: Release, checksum_text: str | None) -> str:
    """The digest the package must have: GitHub's and the .sha256 file's must agree."""
    assert release.package is not None
    sources = {}
    if release.package.sha256:
        sources["GitHub"] = release.package.sha256
    if checksum_text is not None:
        sources[".sha256 檔"] = parse_checksum_file(checksum_text, release.package.name)
    if not sources:
        raise UpdateError("這個版本沒有提供 SHA-256 校驗碼，為了安全不自動安裝")
    if len(set(sources.values())) > 1:
        raise UpdateError("GitHub 與 .sha256 檔的校驗碼不一致，不安裝")
    return next(iter(sources.values()))


def file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        while chunk := fh.read(CHUNK):
            digest.update(chunk)
    return digest.hexdigest()


def extract_program(archive: Path, dest: Path) -> Path:
    """Unpack the release zip after checking every member stays inside ``OpenDRS/``."""
    try:
        with zipfile.ZipFile(archive) as zf:
            infos = zf.infolist()
            total = 0
            for info in infos:
                parts = PurePosixPath(info.filename).parts
                if (not parts or parts[0] != PROGRAM_DIR or ".." in parts or info.filename.startswith("/")
                        or "\\" in info.filename or ":" in info.filename):
                    raise UpdateError(f"更新檔含有不允許的路徑：{info.filename}")
                total += info.file_size
            if total > MAX_UNPACKED_BYTES:
                raise UpdateError("更新檔解壓後過大")
            names = {i.filename for i in infos}
            if f"{PROGRAM_DIR}/OpenDRS.exe" not in names or not any(
                    n.startswith(f"{PROGRAM_DIR}/_internal/") for n in names):
                raise UpdateError("更新檔內容不完整（缺少 OpenDRS.exe 或 _internal）")
            zf.extractall(dest)
    except zipfile.BadZipFile as exc:
        raise UpdateError("更新檔不是有效的 zip") from exc
    return dest / PROGRAM_DIR


@dataclass(frozen=True)
class PreparedUpdate:
    version: str
    source: Path  # unpacked OpenDRS folder
    target: Path  # the installed program folder
    staging: Path  # removed by the updater script when it is done


def prepare(release: Release, target: Path, opener: Opener = urllib.request.urlopen,
            progress: Progress | None = None, cancelled: Callable[[], bool] = lambda: False,
            timeout: float = 60.0) -> PreparedUpdate:
    """Download, verify and unpack ``release`` next to the program in ``target``."""
    if release.package is None:
        raise UpdateError(f"{release.tag} 沒有 Windows 版的 zip 可下載，請到發佈頁面手動下載")
    if not writable(target):
        raise UpdateError(f"沒有權限寫入程式所在的資料夾 {target}；請手動下載並解壓縮")
    staging = target / f"{STAGING_PREFIX}{release.version}"
    shutil.rmtree(staging, ignore_errors=True)
    staging.mkdir()
    try:
        checksum_text = None
        if release.checksum is not None:
            path = staging / release.checksum.name
            _download(opener, release.checksum.url, path, MAX_CHECKSUM_BYTES, 0, timeout, None, cancelled)
            checksum_text = path.read_text(encoding="ascii", errors="replace")
        expected = expected_sha256(release, checksum_text)
        archive = staging / release.package.name
        _download(opener, release.package.url, archive, MAX_ZIP_BYTES, release.package.size, timeout,
                  progress, cancelled)
        if file_sha256(archive) != expected:
            raise UpdateError("下載的檔案校驗碼不符（可能下載不完整或被竄改），未安裝")
        source = extract_program(archive, staging / "unpacked")
        archive.unlink()
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    return PreparedUpdate(release.version, source, target, staging)


def stale_staging(target: Path) -> list[Path]:
    """Leftover staging folders from an update that was prepared but never applied."""
    return [p for p in target.glob(STAGING_PREFIX + "*") if p.is_dir()]


# Windows PowerShell 5.1 script (ASCII only; written with a BOM). It runs after OpenDRS exits.
UPDATER_SCRIPT = r"""param(
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
"""


def updater_script() -> str:
    return UPDATER_SCRIPT.replace("ITEMS", ", ".join(f"'{i}'" for i in PROGRAM_ITEMS))


def updater_command(prepared: PreparedUpdate, pid: int, log: Path, restart: bool = True) -> list[str]:
    script = prepared.staging / "apply-update.ps1"
    script.write_text(updater_script(), encoding="utf-8-sig")
    command = ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
               "-WindowStyle", "Hidden", "-File", str(script), "-AppPid", str(pid),
               "-Source", str(prepared.source), "-Target", str(prepared.target),
               "-Staging", str(prepared.staging), "-Log", str(log)]
    return command if restart else [*command, "-NoRestart"]


def launch(prepared: PreparedUpdate, log: Path) -> None:
    """Start the updater; the caller must exit right after so the files can be replaced."""
    log.parent.mkdir(parents=True, exist_ok=True)
    flags = getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0) | getattr(subprocess, "CREATE_NO_WINDOW", 0)
    subprocess.Popen(updater_command(prepared, os.getpid(), log), creationflags=flags,
                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                     close_fds=True, cwd=str(prepared.staging.parent))
