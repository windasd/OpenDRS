"""Per-user settings, stored as UTF-8 JSON under the user profile.

The legacy CPS kept ``Type.ini`` inside ``Program Files``; Windows silently
redirected those writes to ``VirtualStore``, leaving two diverging copies, and
wrote them in the ANSI code page (hence the ``???`` seen there). Settings here
live in ``%APPDATA%\\OpenDRS`` and are always UTF-8. Before the project was renamed
OpenDRS (2026-10-09) the folder was ``%APPDATA%\\DRSCPS``; migrate_legacy_dir() moves it once.
"""

from __future__ import annotations

import json
import logging
import os
import shutil
from dataclasses import asdict, dataclass, field, replace
from pathlib import Path

from .textcodec import NameCodec

log = logging.getLogger(__name__)
MAX_RECENT = 8


LEGACY_DIR_NAME = "DRSCPS"


def _data_base() -> Path:
    return Path(os.environ.get("APPDATA") or str(Path.home() / ".config"))


def default_dir() -> Path:
    return _data_base() / "OpenDRS"


def legacy_dir() -> Path:
    return _data_base() / LEGACY_DIR_NAME


def _repoint(value: object, old: Path, new: Path) -> object:
    """``value`` with a leading ``old`` folder replaced by ``new`` (strings and lists of them)."""
    if isinstance(value, list):
        return [_repoint(v, old, new) for v in value]
    if not isinstance(value, str) or not value:
        return value
    path = os.path.normpath(value)  # file dialogs hand back forward slashes
    base = os.path.normcase(str(old))
    if os.path.normcase(path) == base or os.path.normcase(path).startswith(base + os.sep):
        return str(new) + path[len(str(old)):]
    return value


def migrate_legacy_dir() -> str:
    """Move the pre-rename data folder (settings, backups, recordings, RadioID cache, logs) to
    default_dir() once and re-point saved paths inside it; returns what happened ("" = nothing).

    A folder that cannot be renamed (a file still open in the old program) is copied through a
    staging folder instead, so a half-finished copy never looks like a migrated folder.
    """
    old, new = legacy_dir(), default_dir()
    if new.exists() or not old.is_dir():
        return ""
    try:
        old.rename(new)
        how = "搬到"
    except OSError:
        staging = new.with_name(new.name + ".migrating")
        shutil.rmtree(staging, ignore_errors=True)
        try:
            shutil.copytree(old, staging)
            staging.rename(new)
        except OSError as exc:
            shutil.rmtree(staging, ignore_errors=True)
            log.warning("could not migrate %s to %s: %s", old, new, exc)
            return f"無法把舊資料夾 {old} 搬到 {new}（{exc}）；舊資料仍在原處。"
        how = "複製到"
    settings_file = new / "settings.json"
    try:
        data = json.loads(settings_file.read_text(encoding="utf-8"))
        if isinstance(data, dict):
            data = {key: _repoint(value, old, new) for key, value in data.items()}
            settings_file.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    except FileNotFoundError:
        pass
    except (ValueError, OSError) as exc:
        log.warning("settings paths not re-pointed after migration: %s", exc)
    return f"已將舊資料夾 {old} {how} {new}（設定、備份、錄音、RadioID 資料）。"


@dataclass(frozen=True)
class Settings:
    name_codec: NameCodec = NameCodec.BIG5
    recent_files: tuple[str, ...] = field(default=())
    last_dir: str = ""
    radio_port: str = ""  # last port used for reading/writing the radio
    theme: str = "auto"  # auto / light / dark
    iap_path: str = ""  # vendor IAP+Flash.exe (empty = default install location)
    ambep2_path: str = ""  # vendor ambep2.exe for recordings (empty = C:\wav\ambep2.exe)
    recordings_dir: str = ""  # where downloaded recordings go (empty = <data dir>\recordings)
    check_updates: bool = True  # look for a newer release on GitHub at start-up, at most daily
    last_update_check: str = ""  # ISO timestamp (UTC) of the last automatic check

    def with_recent(self, path: str) -> Settings:
        items = (path,) + tuple(p for p in self.recent_files if p != path)
        return replace(self, recent_files=items[:MAX_RECENT], last_dir=str(Path(path).parent))


def load_settings(directory: Path | None = None) -> Settings:
    path = (directory or default_dir()) / "settings.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(data, dict):
            raise TypeError("settings.json is not a JSON object")  # caught below: defaults
        return Settings(
            name_codec=NameCodec(data.get("name_codec", NameCodec.BIG5.value)),
            recent_files=tuple(str(p) for p in data.get("recent_files", [])),
            last_dir=str(data.get("last_dir", "")),
            radio_port=str(data.get("radio_port", "")),
            theme=str(data.get("theme", "auto")),
            iap_path=str(data.get("iap_path", "")),
            ambep2_path=str(data.get("ambep2_path", "")),
            recordings_dir=str(data.get("recordings_dir", "")),
            check_updates=bool(data.get("check_updates", True)),
            last_update_check=str(data.get("last_update_check", "")),
        )
    except FileNotFoundError:
        return Settings()
    except (ValueError, TypeError, OSError) as exc:
        log.warning("settings unreadable, using defaults: %s", exc)
        return Settings()


def save_settings(settings: Settings, directory: Path | None = None) -> None:
    folder = directory or default_dir()
    folder.mkdir(parents=True, exist_ok=True)
    data = asdict(settings)
    data["name_codec"] = settings.name_codec.value
    data["recent_files"] = list(settings.recent_files)
    (folder / "settings.json").write_text(
        json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8"
    )
