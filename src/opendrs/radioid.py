"""RadioID.net DMR user database: download, local cache and offline search.

The whole ``user.csv`` dump (~17 MB, refreshed daily upstream) is cached under
the user profile and every filter — region, ID prefix such as ``466`` (Taiwan),
or free-text search over ID / callsign / name / city — runs locally. That keeps
a single code path, works offline after the first download, and avoids the
online API's paging and rate limits.

The downloaded file is untrusted input: size-capped, header-checked before it
replaces the cache, decoded as strict UTF-8, and every row is validated.
"""

from __future__ import annotations

import csv
import io
import json
import os
import tempfile
import time
import unicodedata
import urllib.request
from collections import Counter
from collections.abc import Callable, Iterable
from dataclasses import dataclass
from pathlib import Path
from urllib.error import HTTPError

CSV_URL = "https://radioid.net/static/user.csv"
CSV_NAME = "user.csv"
META_NAME = "meta.json"
USER_AGENT = "OpenDRS/0.1 (codeplug editor)"
MAX_DOWNLOAD = 64 * 1024 * 1024
CHUNK = 256 * 1024
MAX_RADIO_ID = 16_776_415  # top of the individual/group ID range (CPS help text)
EXPECTED_HEADER = ("RADIO_ID", "CALLSIGN", "FIRST_NAME", "LAST_NAME", "CITY", "STATE", "COUNTRY")

Progress = Callable[[int, int], None]


class RadioIdError(RuntimeError):
    """Download or parse failure, with a user-facing message."""


def _clean(value: str) -> str:
    """Whitespace controls (tab, CR, LF) become spaces; other controls are dropped."""
    kept = (" " if ch.isspace() else ch for ch in value
            if ch.isspace() or unicodedata.category(ch) not in ("Cc", "Cf"))
    return " ".join("".join(kept).split())


@dataclass(frozen=True)
class RadioIdUser:
    radio_id: int
    callsign: str
    first_name: str
    last_name: str
    city: str
    state: str
    country: str

    @property
    def full_name(self) -> str:
        return " ".join(p for p in (self.first_name, self.last_name) if p)


@dataclass(frozen=True)
class ParseResult:
    users: tuple[RadioIdUser, ...]
    skipped: int


def parse_csv(data: bytes) -> ParseResult:
    try:
        text = data.decode("utf-8-sig")
    except UnicodeDecodeError as exc:
        raise RadioIdError(f"資料不是有效的 UTF-8（位置 {exc.start}）") from exc
    reader = csv.reader(io.StringIO(text, newline=""))
    header = next(reader, None)
    if header is None or tuple(h.strip().upper() for h in header) != EXPECTED_HEADER:
        raise RadioIdError("檔案格式與 radioid.net user.csv 不符")
    users: list[RadioIdUser] = []
    seen: set[int] = set()
    skipped = 0
    for row in reader:
        if len(row) != len(EXPECTED_HEADER):
            skipped += 1
            continue
        try:
            radio_id = int(row[0].strip())
        except ValueError:
            skipped += 1
            continue
        if not 1 <= radio_id <= MAX_RADIO_ID or radio_id in seen:
            skipped += 1
            continue
        seen.add(radio_id)
        users.append(RadioIdUser(radio_id, *(_clean(v) for v in row[1:])))
    return ParseResult(tuple(users), skipped)


@dataclass(frozen=True)
class Query:
    """All given criteria must match; text terms are AND-ed and case-insensitive."""

    country: str = ""
    id_prefix: str = ""
    text: str = ""

    def is_empty(self) -> bool:
        return not (self.country or self.id_prefix.strip() or self.text.strip())


class UserDb:
    def __init__(self, users: Iterable[RadioIdUser]) -> None:
        self.users: tuple[RadioIdUser, ...] = tuple(users)
        self._ids = [str(u.radio_id) for u in self.users]
        self._hay = [
            f"{u.radio_id} {u.callsign} {u.full_name} {u.city} {u.state}".casefold()
            for u in self.users
        ]

    def __len__(self) -> int:
        return len(self.users)

    def countries(self) -> list[tuple[str, int]]:
        counts = Counter(u.country for u in self.users if u.country)
        return sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))

    def search(self, query: Query) -> list[RadioIdUser]:
        prefix = query.id_prefix.strip()
        terms = query.text.casefold().split()
        out: list[RadioIdUser] = []
        for i, user in enumerate(self.users):
            if query.country and user.country != query.country:
                continue
            if prefix and not self._ids[i].startswith(prefix):
                continue
            if terms and not all(t in self._hay[i] for t in terms):
                continue
            out.append(user)
        return out


# ------------------------------------------------------------------ local cache


@dataclass(frozen=True)
class CacheInfo:
    path: Path
    size: int
    downloaded_at: float
    last_modified: str


def _read_meta(directory: Path) -> dict:
    try:
        data = json.loads((directory / META_NAME).read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except (OSError, ValueError):
        return {}


def _write_meta(directory: Path, meta: dict) -> None:
    (directory / META_NAME).write_text(json.dumps(meta, ensure_ascii=False, indent=2),
                                       encoding="utf-8")


def cache_info(directory: Path) -> CacheInfo | None:
    path = directory / CSV_NAME
    if not path.is_file():
        return None
    meta = _read_meta(directory)
    return CacheInfo(path, path.stat().st_size,
                     float(meta.get("downloaded_at") or path.stat().st_mtime),
                     str(meta.get("last_modified", "")))


def load_cached(directory: Path) -> tuple[UserDb, ParseResult] | None:
    info = cache_info(directory)
    if info is None:
        return None
    result = parse_csv(info.path.read_bytes())
    return UserDb(result.users), result


def lookup_cached(directory: Path, radio_id: int) -> RadioIdUser | None:
    """Find one ID in the downloaded user.csv without parsing the whole file."""
    info = cache_info(directory)
    if info is None:
        return None
    data = info.path.read_bytes()
    start = data.find(b"\n%d," % radio_id)
    if start < 0:
        return None
    end = data.find(b"\n", start + 1)
    line = data[start + 1 : end if end >= 0 else len(data)].decode("utf-8", errors="replace")
    row = next(csv.reader([line]), [])
    if len(row) != len(EXPECTED_HEADER):
        return None
    return RadioIdUser(radio_id, *(_clean(v) for v in row[1:]))


def _check_header(path: Path) -> None:
    with path.open("rb") as fh:
        first = fh.readline(512).decode("utf-8-sig", errors="replace").strip()
    if tuple(h.strip().upper() for h in first.split(",")) != EXPECTED_HEADER:
        raise RadioIdError("下載內容不是 radioid.net user.csv（可能是錯誤頁面）")


def download(directory: Path, progress: Progress | None = None,
             opener: Callable[..., object] = urllib.request.urlopen,
             timeout: float = 60.0) -> bool:
    """Refresh the cache. Returns ``False`` if the server says it has not changed."""
    directory.mkdir(parents=True, exist_ok=True)
    target = directory / CSV_NAME
    meta = _read_meta(directory)
    headers = {"User-Agent": USER_AGENT}
    if meta.get("last_modified") and target.is_file():
        headers["If-Modified-Since"] = str(meta["last_modified"])
    request = urllib.request.Request(CSV_URL, headers=headers)
    try:
        response = opener(request, timeout=timeout)
    except HTTPError as exc:
        if exc.code == 304:
            return False
        raise RadioIdError(f"radioid.net 回應 HTTP {exc.code}") from exc
    except OSError as exc:
        raise RadioIdError(f"無法連線 radioid.net：{exc}") from exc

    with response:
        total = int(response.headers.get("Content-Length") or 0)
        if total > MAX_DOWNLOAD:
            raise RadioIdError(f"檔案過大（{total:,} bytes）")
        fd, tmp_name = tempfile.mkstemp(dir=directory, suffix=".part")
        tmp = Path(tmp_name)
        try:
            got = 0
            with os.fdopen(fd, "wb") as fh:
                while chunk := response.read(CHUNK):
                    got += len(chunk)
                    if got > MAX_DOWNLOAD:
                        raise RadioIdError("檔案超過大小上限，已中止")
                    fh.write(chunk)
                    if progress:
                        progress(got, total)
            if total and got != total:
                raise RadioIdError(f"下載不完整（{got:,} / {total:,} bytes）")
            _check_header(tmp)
            os.replace(tmp, target)
        except BaseException:
            tmp.unlink(missing_ok=True)
            raise
        last_modified = response.headers.get("Last-Modified", "")
    _write_meta(directory, {"last_modified": last_modified, "downloaded_at": time.time(),
                            "size": got, "source": CSV_URL})
    return True
