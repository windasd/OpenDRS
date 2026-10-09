"""The backups kept by every radio read and write (see :func:`radio_link.save_backup`).

Each backup is a pair in one folder: ``<YYYYmmdd_HHMMSS>_<kind>_<model>.F136`` (the
project) and the same name with ``.bin`` (the raw transfer stream); ``kind`` is
``read`` or ``before_write``. Only files that follow this pattern are listed, and
deleting a backup removes exactly its two files.
"""

from __future__ import annotations

import re
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path

from . import f136
from .codeplug import Counts

KIND_LABELS = {"read": "讀取", "before_write": "寫入前"}
BEFORE_WRITE = "before_write"
STAMP_FORMAT = "%Y%m%d_%H%M%S"
TIME_FORMAT = "%Y-%m-%d %H:%M:%S"
_NAME = re.compile(r"^(?P<stamp>\d{8}_\d{6})_(?P<kind>read|before_write)_(?P<model>[0-9A-Za-z_-]+)\.F136$")


@dataclass(frozen=True)
class Backup:
    path: Path  # the .F136 project
    when: datetime
    kind: str
    model: str
    size: int  # bytes of the .F136 and the .bin together
    counts: Counts | None  # None when the project cannot be read
    error: str = ""

    @property
    def raw_path(self) -> Path:
        return self.path.with_suffix(".bin")

    @property
    def kind_label(self) -> str:
        return KIND_LABELS.get(self.kind, self.kind)

    @property
    def time_text(self) -> str:
        return self.when.strftime(TIME_FORMAT)

    def files(self) -> list[Path]:
        """The backup's files that exist: the .F136, then the .bin."""
        return [p for p in (self.path, self.raw_path) if p.is_file()]


def parse_name(name: str) -> tuple[datetime, str, str] | None:
    """(time, kind, model) from a backup file name, or None when it is not one."""
    match = _NAME.match(name)
    if match is None:
        return None
    try:
        when = datetime.strptime(match["stamp"], STAMP_FORMAT)  # noqa: DTZ007 - local time, as saved
    except ValueError:  # e.g. month 13: not a name save_backup writes
        return None
    return when, match["kind"], match["model"]


def _size(path: Path) -> int:
    try:
        return path.stat().st_size
    except OSError:
        return 0


def load_backup(path: Path) -> Backup | None:
    """Describe one backup (reads the whole project, ~3 ms); None when ``path`` is not a backup."""
    parsed = parse_name(path.name)
    if parsed is None:
        return None
    when, kind, model = parsed
    counts: Counts | None = None
    error = ""
    try:
        counts = Counts.of(f136.load(path))
    except (OSError, ValueError) as exc:  # F136FormatError is a ValueError
        error = str(exc) or type(exc).__name__
    size = _size(path) + _size(path.with_suffix(".bin"))
    return Backup(path, when, kind, model, size, counts, error)


def newest_first(backups: Sequence[Backup]) -> list[Backup]:
    return sorted(backups, key=lambda b: (b.when, b.path.name), reverse=True)


def list_backups(folder: Path) -> list[Backup]:
    """Every backup in ``folder``, newest first; empty when the folder does not exist."""
    if not folder.is_dir():
        return []
    found = (load_backup(p) for p in folder.iterdir() if p.suffix == ".F136" and p.is_file())
    return newest_first([b for b in found if b is not None])


def delete_backup(backup: Backup) -> list[Path]:
    """Remove the backup's .F136 and .bin (nothing else); returns what was removed."""
    if parse_name(backup.path.name) is None:
        raise ValueError(f"不是備份檔：{backup.path.name}")
    removed = []
    for path in backup.files():
        path.unlink()
        removed.append(path)
    return removed


def cleanup_candidates(backups: Sequence[Backup], keep: int) -> list[Backup]:
    """The backups to delete so only the newest ``keep`` remain — plus the newest
    before_write backup, which is always kept (it is what a failed write rolls back to)."""
    if keep < 1:
        raise ValueError("至少要保留 1 份備份")
    ordered = newest_first(backups)
    kept = ordered[:keep]
    if not any(b.kind == BEFORE_WRITE for b in kept):
        last_write = next((b for b in ordered if b.kind == BEFORE_WRITE), None)
        if last_write is not None:
            kept.append(last_write)
    return [b for b in ordered if b not in kept]


def size_text(size: int) -> str:
    """「17 bytes」／「46.8 KB」／「1.2 MB」."""
    if size >= 1024 * 1024:
        return f"{size / (1024 * 1024):.1f} MB"
    return f"{size / 1024:.1f} KB" if size >= 1024 else f"{size} bytes"
