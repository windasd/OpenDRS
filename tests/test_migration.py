"""Moving the pre-rename data folder %APPDATA%\\DRSCPS to %APPDATA%\\OpenDRS."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from opendrs import settings


@pytest.fixture
def appdata(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    monkeypatch.setenv("APPDATA", str(tmp_path))
    return tmp_path


def _legacy(appdata: Path) -> Path:
    old = appdata / "DRSCPS"
    (old / "backups").mkdir(parents=True)
    (old / "backups" / "20261009_103322_before_write_DRS-8500.F136").write_bytes(b"backup")
    (old / "recordings").mkdir()
    (old / "settings.json").write_text(json.dumps({
        "name_codec": "big5",
        "recent_files": [str(old / "backups" / "a.F136"), "D:/radio/mine.F136",
                         str(old).replace("\\", "/") + "/backups/b.F136"],  # as a file dialog returns it
        "last_dir": str(old / "backups"),
        "recordings_dir": str(old / "recordings"),
        "ambep2_path": "C:\\wav\\ambep2.exe",
        "radio_port": "COM4",
        "iap_path": str(appdata / "DRSCPS2" / "IAP.exe"),  # only looks alike: must stay
    }), encoding="utf-8")
    return old


def test_moves_the_old_folder_and_repoints_paths(appdata: Path) -> None:
    old = _legacy(appdata)
    new = settings.default_dir()
    message = settings.migrate_legacy_dir()
    assert "搬到" in message and str(new) in message
    assert not old.exists() and (new / "backups" / "20261009_103322_before_write_DRS-8500.F136").read_bytes() == b"backup"
    loaded = settings.load_settings()
    assert loaded.recent_files == (str(new / "backups" / "a.F136"), "D:/radio/mine.F136",
                                   str(new / "backups" / "b.F136"))
    assert loaded.last_dir == str(new / "backups") and loaded.recordings_dir == str(new / "recordings")
    assert loaded.ambep2_path == "C:\\wav\\ambep2.exe" and loaded.radio_port == "COM4"
    assert loaded.iap_path == str(appdata / "DRSCPS2" / "IAP.exe")
    assert settings.migrate_legacy_dir() == ""  # only once


def test_copies_when_the_folder_cannot_be_renamed(appdata: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    old = _legacy(appdata)
    real_rename = Path.rename

    def busy(self: Path, target: Path) -> Path:
        if self == old:
            raise PermissionError("in use")
        return real_rename(self, target)

    monkeypatch.setattr(Path, "rename", busy)
    message = settings.migrate_legacy_dir()
    assert "複製到" in message and old.is_dir()  # the old folder stays for the old program
    new = settings.default_dir()
    assert (new / "backups").is_dir() and not new.with_name("OpenDRS.migrating").exists()
    assert settings.load_settings().recordings_dir == str(new / "recordings")


def test_failed_copy_leaves_no_half_folder(appdata: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    _legacy(appdata)
    monkeypatch.setattr(Path, "rename", lambda self, target: (_ for _ in ()).throw(PermissionError("in use")))
    monkeypatch.setattr(settings.shutil, "copytree", lambda src, dst: (_ for _ in ()).throw(OSError("disk full")))
    message = settings.migrate_legacy_dir()
    assert "無法" in message and "舊資料仍在原處" in message
    assert not settings.default_dir().exists() and (appdata / "DRSCPS").is_dir()


def test_nothing_to_do(appdata: Path) -> None:
    assert settings.migrate_legacy_dir() == ""  # no old folder
    _legacy(appdata)
    settings.default_dir().mkdir()
    assert settings.migrate_legacy_dir() == ""  # new folder already in use: never overwrite it
    assert (appdata / "DRSCPS" / "settings.json").is_file()


@pytest.mark.parametrize("content", ["[]", "3", "null", "{", "\ufeff{}"])
def test_unusable_settings_file_gives_defaults(tmp_path: Path, content: str) -> None:
    (tmp_path / "settings.json").write_text(content, encoding="utf-8")
    assert settings.load_settings(tmp_path) == settings.Settings()
