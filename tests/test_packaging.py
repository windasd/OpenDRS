"""Release build helpers (packaging/build.py) and the --self-test the build relies on."""

from __future__ import annotations

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
                                     "serial", "codecs"}
    assert os.environ["APPDATA"] == str(tmp_path / "mine")  # restored
    assert not (tmp_path / "mine").exists()  # the user's data folder was never touched


def test_version_flag(capsys: pytest.CaptureFixture[str]) -> None:
    from opendrs import __version__
    from opendrs.__main__ import main

    assert main(["--version"]) == 0
    assert capsys.readouterr().out.strip() == __version__
