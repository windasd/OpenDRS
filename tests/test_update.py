"""Update check and install (update.py): parsing, verification, unpacking, the updater script."""

from __future__ import annotations

import hashlib
import io
import json
import subprocess
import sys
import time
import zipfile
from datetime import UTC, datetime, timedelta
from pathlib import Path
from typing import Self
from urllib.error import HTTPError

import pytest

from opendrs import update
from opendrs.update import UpdateError

ZIP_URL = "https://github.com/windasd/OpenDRS/releases/download/v{v}/OpenDRS-{v}-win64.zip"


def release_json(version: str = "0.2.0", zip_bytes: bytes = b"", digest: str | None = None,
                 with_checksum: bool = True) -> dict:
    """Shaped like GitHub's answer for v0.1.0 (assets with browser_download_url and digest)."""
    url = ZIP_URL.format(v=version)
    sha = hashlib.sha256(zip_bytes).hexdigest()
    assets = [{"name": f"OpenDRS-{version}-win64.zip", "browser_download_url": url, "size": len(zip_bytes),
               "digest": f"sha256:{digest or sha}", "state": "uploaded"}]
    if with_checksum:
        assets.append({"name": f"OpenDRS-{version}-win64.zip.sha256", "browser_download_url": url + ".sha256",
                       "size": 91, "digest": "sha256:" + "0" * 64})
    return {"tag_name": f"v{version}", "name": f"OpenDRS v{version}", "draft": False, "prerelease": False,
            "html_url": f"https://github.com/windasd/OpenDRS/releases/tag/v{version}",
            "published_at": "2026-11-01T10:00:00Z", "body": "**Full Changelog**: …", "assets": assets}


class Response(io.BytesIO):
    def __enter__(self) -> Self:
        return self


def opener_for(files: dict[str, bytes], seen: list[str] | None = None):
    def opener(request, timeout: float):
        url = request.full_url
        if seen is not None:
            seen.append(url)
        assert request.get_header("User-agent", "").startswith("OpenDRS/") and timeout > 0
        if url not in files:
            raise HTTPError(url, 404, "Not Found", {}, None)
        return Response(files[url])
    return opener


def program_zip(extra: dict[str, bytes] | None = None, version: str = "new") -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        members = {"OpenDRS/OpenDRS.exe": version.encode(), "OpenDRS/_internal/python313.dll": b"dll",
                   "OpenDRS/licenses/LGPL-3.0.txt": b"lgpl", "OpenDRS/README.md": b"readme"}
        for name, data in {**members, **(extra or {})}.items():
            zf.writestr(name, data)
    return buf.getvalue()


# ---------------------------------------------------------------- versions and releases
@pytest.mark.parametrize(("older", "newer"), [
    ("0.1.0", "0.2.0"), ("0.9.9", "0.10.0"), ("0.2.0rc1", "0.2.0"), ("0.2.0a2", "0.2.0b1"), ("1.0", "1.0.1"),
])
def test_version_order(older: str, newer: str) -> None:
    assert update.parse_version(older) < update.parse_version(newer)


def test_version_forms() -> None:
    assert update.parse_version("v0.2.0") == update.parse_version("0.2") == update.parse_version("0.2.0.0")
    with pytest.raises(UpdateError, match="無法辨識"):
        update.parse_version("latest")


def test_parse_release_picks_the_windows_zip_and_digest() -> None:
    release = update.parse_release(release_json("0.2.0", b"zip"))
    assert (release.tag, release.version, release.title) == ("v0.2.0", "0.2.0", "OpenDRS v0.2.0")
    assert release.package.name == "OpenDRS-0.2.0-win64.zip" and release.package.size == 3
    assert release.package.sha256 == hashlib.sha256(b"zip").hexdigest()
    assert release.checksum.name == "OpenDRS-0.2.0-win64.zip.sha256"
    assert release.is_newer_than("0.1.0") and not release.is_newer_than("0.2.0")


def test_parse_release_ignores_assets_from_elsewhere() -> None:
    data = release_json("0.2.0", b"zip")
    data["assets"][0]["browser_download_url"] = "https://example.com/OpenDRS-0.2.0-win64.zip"
    assert update.parse_release(data).package is None
    with pytest.raises(UpdateError):
        update.parse_release({**release_json(), "prerelease": True})
    with pytest.raises(UpdateError):
        update.parse_release(["not", "a", "release"])


def test_fetch_latest_and_http_errors() -> None:
    files = {update.API_LATEST: json.dumps(release_json("0.3.0")).encode()}
    assert update.fetch_latest(opener_for(files)).version == "0.3.0"
    with pytest.raises(UpdateError, match="HTTP 404"):
        update.fetch_latest(opener_for({}))

    def limited(request, timeout):
        raise HTTPError(request.full_url, 403, "rate limited", {}, None)

    with pytest.raises(UpdateError, match="限制查詢次數"):
        update.fetch_latest(limited)

    def offline(request, timeout):
        raise OSError("no route to host")

    with pytest.raises(UpdateError, match="無法連線"):
        update.fetch_latest(offline)


def test_check_due() -> None:
    now = datetime(2026, 11, 2, 9, 0, tzinfo=UTC)
    assert update.check_due("", now)
    assert update.check_due((now - timedelta(hours=21)).isoformat(), now)
    assert not update.check_due((now - timedelta(hours=2)).isoformat(), now)


# ---------------------------------------------------------------- checksums
def test_checksum_file_tolerates_crlf_like_v010() -> None:
    sha = "862ce1e502caaa4d660a7fd17c920b2c80d40aca39a392b7cf63cce31355baca"
    text = f"{sha}  OpenDRS-0.1.0-win64.zip\r\n"  # as published with v0.1.0
    assert update.parse_checksum_file(text, "OpenDRS-0.1.0-win64.zip") == sha
    with pytest.raises(UpdateError):
        update.parse_checksum_file(text, "OpenDRS-0.2.0-win64.zip")


def test_expected_sha256_needs_agreeing_sources() -> None:
    zip_bytes = b"zip"
    sha = hashlib.sha256(zip_bytes).hexdigest()
    release = update.parse_release(release_json("0.2.0", zip_bytes))
    line = f"{sha}  OpenDRS-0.2.0-win64.zip\n"
    assert update.expected_sha256(release, line) == sha
    assert update.expected_sha256(release, None) == sha  # GitHub's digest alone
    with pytest.raises(UpdateError, match="不一致"):
        update.expected_sha256(release, f"{'1' * 64}  OpenDRS-0.2.0-win64.zip\n")
    bare = update.parse_release({**release_json("0.2.0", zip_bytes), "assets": [
        {**release_json("0.2.0", zip_bytes)["assets"][0], "digest": None}]})
    with pytest.raises(UpdateError, match="沒有提供 SHA-256"):
        update.expected_sha256(bare, None)


# ---------------------------------------------------------------- unpacking
def test_extract_program(tmp_path: Path) -> None:
    archive = tmp_path / "ok.zip"
    archive.write_bytes(program_zip())
    source = update.extract_program(archive, tmp_path / "out")
    assert (source / "OpenDRS.exe").read_bytes() == b"new" and (source / "_internal").is_dir()


@pytest.mark.parametrize("bad", ["OpenDRS/../evil.exe", "/OpenDRS/abs.exe", "Other/OpenDRS.exe",
                                 "OpenDRS/..\\evil.exe"])
def test_extract_rejects_paths_outside_the_program(tmp_path: Path, bad: str) -> None:
    archive = tmp_path / "bad.zip"
    archive.write_bytes(program_zip({bad: b"x"}))
    with pytest.raises(UpdateError, match="不允許的路徑"):
        update.extract_program(archive, tmp_path / "out")
    assert not (tmp_path / "evil.exe").exists()


def test_extract_rejects_incomplete_or_broken_zips(tmp_path: Path) -> None:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as zf:
        zf.writestr("OpenDRS/README.md", b"no exe")
    (tmp_path / "a.zip").write_bytes(buf.getvalue())
    with pytest.raises(UpdateError, match="不完整"):
        update.extract_program(tmp_path / "a.zip", tmp_path / "out")
    (tmp_path / "b.zip").write_bytes(b"not a zip")
    with pytest.raises(UpdateError, match="不是有效的 zip"):
        update.extract_program(tmp_path / "b.zip", tmp_path / "out")


# ---------------------------------------------------------------- prepare
def _served_release(zip_bytes: bytes, version: str = "0.2.0", checksum_line: str | None = None):
    data = release_json(version, zip_bytes)
    release = update.parse_release(data)
    sha = hashlib.sha256(zip_bytes).hexdigest()
    files = {release.package.url: zip_bytes,
             release.checksum.url: (checksum_line or f"{sha}  {release.package.name}\n").encode()}
    return release, files


def test_prepare_downloads_verifies_and_unpacks(tmp_path: Path) -> None:
    zip_bytes = program_zip()
    release, files = _served_release(zip_bytes)
    progress: list[tuple[int, int]] = []
    prepared = update.prepare(release, tmp_path, opener_for(files), progress=lambda d, n: progress.append((d, n)))
    assert prepared.version == "0.2.0" and prepared.target == tmp_path
    assert prepared.staging == tmp_path / ".opendrs-update-0.2.0"
    assert (prepared.source / "OpenDRS.exe").read_bytes() == b"new"
    assert not list(prepared.staging.glob("*.zip"))  # the archive is not kept
    assert progress[-1] == (len(zip_bytes), len(zip_bytes))


def test_prepare_refuses_a_corrupt_download(tmp_path: Path) -> None:
    zip_bytes = program_zip()
    release, files = _served_release(zip_bytes)
    files[release.package.url] = zip_bytes[:-1] + b"X"  # same size, different bytes
    with pytest.raises(UpdateError, match="校驗碼不符"):
        update.prepare(release, tmp_path, opener_for(files))
    assert not (tmp_path / ".opendrs-update-0.2.0").exists()


def test_prepare_can_be_cancelled_and_needs_write_access(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    release, files = _served_release(program_zip())
    with pytest.raises(UpdateError, match="已取消"):
        update.prepare(release, tmp_path, opener_for(files), cancelled=lambda: True)
    monkeypatch.setattr(update, "writable", lambda folder: False)
    with pytest.raises(UpdateError, match="沒有權限"):
        update.prepare(release, tmp_path, opener_for(files))


# ---------------------------------------------------------------- the updater script (PowerShell)
windows_only = pytest.mark.skipif(sys.platform != "win32", reason="the updater is a Windows PowerShell script")


def _installed(target: Path, version: str) -> None:
    (target / "_internal").mkdir(parents=True)
    (target / "licenses").mkdir()
    (target / "OpenDRS.exe").write_bytes(version.encode())
    (target / "_internal" / "python313.dll").write_bytes(version.encode())
    (target / "licenses" / "LGPL-3.0.txt").write_bytes(version.encode())
    (target / "README.md").write_bytes(version.encode())
    (target / "我的專案.F136").write_bytes(b"user file")  # must never be touched


def _prepared(tmp_path: Path) -> update.PreparedUpdate:
    target = tmp_path / "程式資料夾"
    _installed(target, "old")
    staging = target / ".opendrs-update-0.2.0"
    archive = tmp_path / "new.zip"
    archive.write_bytes(program_zip())
    source = update.extract_program(archive, staging / "unpacked")
    return update.PreparedUpdate("0.2.0", source, target, staging)


def _run_updater(prepared: update.PreparedUpdate, pid: int, log: Path) -> subprocess.CompletedProcess:
    command = update.updater_command(prepared, pid, log, restart=False)
    return subprocess.run(command, timeout=120, capture_output=True, check=False)


@windows_only
def test_updater_waits_for_the_app_then_swaps_only_program_files(tmp_path: Path) -> None:
    prepared = _prepared(tmp_path)
    app = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(2)"])
    started = time.monotonic()
    result = _run_updater(prepared, app.pid, tmp_path / "update.log")
    app.wait()
    log = (tmp_path / "update.log").read_text(encoding="utf-8-sig")
    assert result.returncode == 0, log
    assert time.monotonic() - started >= 1.5  # it waited for the "app" to exit
    target = prepared.target
    assert (target / "OpenDRS.exe").read_bytes() == b"new"
    assert (target / "_internal" / "python313.dll").read_bytes() == b"dll"
    assert (target / "README.md").read_bytes() == b"readme"
    assert (target / "我的專案.F136").read_bytes() == b"user file"
    assert not prepared.staging.exists() and "installed" in log


@windows_only
def test_updater_restores_the_old_version_when_a_file_is_locked(tmp_path: Path) -> None:
    prepared = _prepared(tmp_path)
    target = prepared.target
    with (target / "README.md").open("rb"):  # held open: it cannot be moved
        result = _run_updater(prepared, 999_999, tmp_path / "update.log")
    log = (tmp_path / "update.log").read_text(encoding="utf-8-sig")
    assert result.returncode == 1 and "restoring the previous version" in log
    assert (target / "OpenDRS.exe").read_bytes() == b"old"
    assert (target / "_internal" / "python313.dll").read_bytes() == b"old"
    assert (target / "licenses" / "LGPL-3.0.txt").read_bytes() == b"old"
    assert (target / "README.md").read_bytes() == b"old"
    assert (prepared.source / "OpenDRS.exe").read_bytes() == b"new"  # the download is kept for a retry
