"""Release build helpers (packaging/build.py) and the --self-test the build relies on."""

from __future__ import annotations

import hashlib
import importlib.util
import json
import os
import struct
import sys
import types
from pathlib import Path

import pytest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
ROOT = Path(__file__).resolve().parents[1]


def _load_build() -> types.ModuleType:
    spec = importlib.util.spec_from_file_location("opendrs_build", ROOT / "packaging" / "build.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


build = _load_build()


def test_version_matches_package_and_pyproject() -> None:
    from opendrs import __version__

    assert build.read_version() == __version__
    assert f'version = "{__version__}"' in (ROOT / "pyproject.toml").read_text(encoding="utf-8")


def test_changelog_has_an_entry_for_this_version() -> None:
    from opendrs import __version__

    notes = build.changelog_section(__version__, (ROOT / "CHANGELOG.md").read_text(encoding="utf-8"))
    assert notes and not notes.startswith("## ")  # the release job publishes this as the Release text


def test_changelog_section_picks_exactly_one_version() -> None:
    text = ("# 更新紀錄\n\n說明\n\n## v0.2.0rc1（2026-10-10）\n\n- 預覽\n\n"
            "## v0.2.0（2026-10-11）\n\n### 新功能\n\n- 甲\n- 乙\n\n## v0.1.0（2026-10-09）\n\n第一版\n")
    assert build.changelog_section("0.2.0", text) == "### 新功能\n\n- 甲\n- 乙"
    assert build.changelog_section("0.2.0rc1", text) == "- 預覽"
    assert build.changelog_section("0.1.0", text) == "第一版"  # the last section runs to the end
    assert build.changelog_section("0.2", text) == "" and build.changelog_section("0.3.0", text) == ""


def test_release_notes_require_a_changelog_entry(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    changelog = tmp_path / "CHANGELOG.md"
    changelog.write_text("# 更新紀錄\n\n## v9.9.9（2030-01-01）\n\n- 新東西\n", encoding="utf-8")
    monkeypatch.setattr(build, "CHANGELOG", changelog)
    monkeypatch.setattr(build, "RELEASE_NOTES", tmp_path / "release-notes.md")
    assert build.write_release_notes("9.9.9").read_bytes() == "- 新東西\n".encode()
    with pytest.raises(SystemExit, match="沒有 v9.9.8 的更新紀錄"):
        build.write_release_notes("9.9.8")


@pytest.mark.parametrize(("text", "fields"), [
    ("0.1.0", (0, 1, 0, 0)), ("1.2", (1, 2, 0, 0)), ("1.2.3rc1", (1, 2, 3, 0)), ("2.0.0.7", (2, 0, 0, 7)),
])
def test_version_tuple(text: str, fields: tuple[int, ...]) -> None:
    assert build.version_tuple(text) == fields


def test_ico_container_points_at_each_png() -> None:
    pngs = {16: b"\x89PNG16", 256: b"\x89PNG256-large"}
    data = build.ico_bytes(pngs)
    reserved, kind, count = struct.unpack_from("<HHH", data, 0)
    assert (reserved, kind, count) == (0, 1, 2)
    for i, size in enumerate(sorted(pngs)):
        width, height, _, _, planes, bits, length, offset = struct.unpack_from("<BBBBHHII", data, 6 + 16 * i)
        assert (width, height) == ((0, 0) if size == 256 else (size, size)) and (planes, bits) == (1, 32)
        assert data[offset:offset + length] == pngs[size]


def test_checksum_file_works_with_sha256sum(tmp_path: Path) -> None:
    archive = tmp_path / "OpenDRS-9.9.9-win64.zip"
    archive.write_bytes(b"zip bytes")
    digest = build.write_checksum(archive)
    line = (tmp_path / "OpenDRS-9.9.9-win64.zip.sha256").read_bytes()
    assert line == f"{digest}  OpenDRS-9.9.9-win64.zip\n".encode()  # LF only: a CR breaks `sha256sum -c`
    assert digest == hashlib.sha256(b"zip bytes").hexdigest()


def test_rendered_icon_has_every_size(tmp_path: Path) -> None:
    out = tmp_path / "app.ico"
    build.render_icon(out)
    data = out.read_bytes()
    assert struct.unpack_from("<H", data, 4)[0] == len(build.ICON_SIZES)
    first_offset = struct.unpack_from("<I", data, 6 + 12)[0]
    assert data[first_offset:first_offset + 8] == b"\x89PNG\r\n\x1a\n"


def test_version_resource_loads_in_pyinstaller(tmp_path: Path) -> None:
    pytest.importorskip("PyInstaller")
    from PyInstaller.utils.win32.versioninfo import load_version_info_from_text_file

    path = tmp_path / "version_info.txt"
    path.write_text(build.version_info("1.2.3"), encoding="utf-8")
    info = load_version_info_from_text_file(str(path))
    assert info.ffi.fileVersionMS == (1 << 16 | 2) and info.ffi.fileVersionLS == (3 << 16)
    assert "OpenDRS" in str(info) and "windasd" in str(info)


def test_wrong_release_tag_stops_before_building(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sys, "argv", ["build.py", "--expect-tag", "v9.9.9"])
    monkeypatch.setattr(build, "run_pyinstaller", lambda: pytest.fail("must not build"))
    with pytest.raises(SystemExit, match="標籤 v9.9.9 與程式版本"):
        build.main()


def test_self_test_passes_from_source(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    from PySide6.QtWidgets import QApplication

    from opendrs import __version__, selftest

    QApplication.instance() or QApplication([])
    monkeypatch.setenv("APPDATA", str(tmp_path / "mine"))
    report = tmp_path / "report.json"
    assert selftest.run(report) == 0
    result = json.loads(report.read_text(encoding="utf-8"))
    assert result["ok"] and result["version"] == __version__ and not result["frozen"]
    assert set(result["checks"]) == {"qt", "translations", "icon", "main_window", "pdf", "printer", "ssl",
                                     "serial", "codecs", "update"}
    assert os.environ["APPDATA"] == str(tmp_path / "mine")  # restored
    assert not (tmp_path / "mine").exists()  # the user's data folder was never touched


def test_version_flag(capsys: pytest.CaptureFixture[str]) -> None:
    from opendrs import __version__
    from opendrs.__main__ import main

    assert main(["--version"]) == 0
    assert capsys.readouterr().out.strip() == __version__
