from __future__ import annotations

import io
from pathlib import Path
from urllib.error import HTTPError, URLError

import pytest

from opendrs import radioid
from opendrs.radioid import Query, RadioIdError, UserDb, parse_csv

CSV = (
    "RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\r\n"
    "9990001,QA0BD,James,,New Taipei,Northern Taiwan,Taiwan\r\n"
    "9990002,QA0ADN,Chen,Wei,Taichung,Central Taiwan,Taiwan\r\n"
    "9990010,QA0AHJ,Lin,,Taipei,Northern Taiwan,Taiwan\r\n"
    "1023007,QA0AKM,Hans Peter,,Cornwall,Ontario,Canada\r\n"
    "2141234,QA0AKK,José,Núñez,León,Castilla,Spain\r\n"
    "4600001,QA0AB,王,小明,北京,,China\r\n"
    '3100001,QA0CT,"Smith, Jr",,Boston,MA,United States\r\n'
    "bad,X,,,,,Nowhere\r\n"
    "9990001,DUP,dup,,,,Taiwan\r\n"
    "99999999,BIG,,,,,Taiwan\r\n"
    "short,row\r\n"
).encode("utf-8")


@pytest.fixture
def db() -> UserDb:
    return UserDb(parse_csv(CSV).users)


def test_parse_validates_rows() -> None:
    result = parse_csv(CSV)
    assert len(result.users) == 7
    assert result.skipped == 4  # non-numeric id, duplicate id, out of range, short row
    w1 = next(u for u in result.users if u.callsign == "QA0CT")
    assert w1.first_name == "Smith, Jr"


def test_parse_rejects_wrong_header_and_bad_utf8() -> None:
    with pytest.raises(RadioIdError, match="格式"):
        parse_csv(b"<html>error</html>")
    with pytest.raises(RadioIdError, match="UTF-8"):
        parse_csv(b"RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\n1,\xff,,,,,X\n")


def test_control_characters_are_stripped() -> None:
    data = b"RADIO_ID,CALLSIGN,FIRST_NAME,LAST_NAME,CITY,STATE,COUNTRY\n" \
           b'9990099,"BV2\r\nZZ",A\tB,,,,Taiwan\n'
    user = parse_csv(data).users[0]
    assert user.callsign == "BV2 ZZ" and user.first_name == "A B"


def test_countries_sorted_by_count(db: UserDb) -> None:
    assert db.countries()[0] == ("Taiwan", 3)
    assert len(db) == 7


@pytest.mark.parametrize(
    "query, expected",
    [
        (Query(id_prefix="999"), {"QA0BD", "QA0ADN", "QA0AHJ"}),
        (Query(country="Taiwan"), {"QA0BD", "QA0ADN", "QA0AHJ"}),
        (Query(text="qa0bd"), {"QA0BD"}),
        (Query(text="9990002"), {"QA0ADN"}),
        (Query(text="taipei"), {"QA0BD", "QA0AHJ"}),
        (Query(text="new taipei"), {"QA0BD"}),
        (Query(country="Taiwan", text="chen"), {"QA0ADN"}),
        (Query(id_prefix="999001"), {"QA0AHJ"}),
        (Query(text="小明"), {"QA0AB"}),
        (Query(country="Spain", id_prefix="999"), set()),
    ],
)
def test_search(db: UserDb, query: Query, expected: set[str]) -> None:
    assert {u.callsign for u in db.search(query)} == expected


def test_empty_query_flag() -> None:
    assert Query().is_empty()
    assert not Query(id_prefix=" 4").is_empty()


# ------------------------------------------------------------------ download


class FakeResponse(io.BytesIO):
    def __init__(self, body: bytes, headers: dict[str, str]) -> None:
        super().__init__(body)
        self.headers = headers


def opener_for(body: bytes, headers: dict[str, str] | None = None, seen: list | None = None):
    def opener(request, timeout):  # noqa: ANN001, ANN202
        if seen is not None:
            seen.append(request)
        hdrs = {"Content-Length": str(len(body)), "Last-Modified": "Thu, 08 Oct 2026 05:01:49 GMT"}
        hdrs.update(headers or {})
        return FakeResponse(body, hdrs)

    return opener


def test_download_caches_and_loads(tmp_path: Path) -> None:
    progress: list[tuple[int, int]] = []
    assert radioid.download(tmp_path, progress=lambda g, t: progress.append((g, t)),
                            opener=opener_for(CSV))
    assert progress[-1] == (len(CSV), len(CSV))
    info = radioid.cache_info(tmp_path)
    assert info and info.size == len(CSV) and info.last_modified.startswith("Thu")
    db, result = radioid.load_cached(tmp_path)
    assert len(db) == 7 and result.skipped == 4


def test_second_download_is_conditional_and_handles_304(tmp_path: Path) -> None:
    radioid.download(tmp_path, opener=opener_for(CSV))
    seen: list = []

    def not_modified(request, timeout):  # noqa: ANN001, ANN202
        seen.append(request)
        raise HTTPError(request.full_url, 304, "Not Modified", {}, None)

    assert radioid.download(tmp_path, opener=not_modified) is False
    assert seen[0].get_header("If-modified-since") == "Thu, 08 Oct 2026 05:01:49 GMT"
    assert seen[0].get_header("User-agent").startswith("OpenDRS")
    assert (tmp_path / "user.csv").read_bytes() == CSV


@pytest.mark.parametrize(
    "body, headers, message",
    [
        (b"<html>maintenance</html>", None, "錯誤頁面"),
        (CSV, {"Content-Length": str(len(CSV) + 10)}, "不完整"),
        (CSV, {"Content-Length": str(radioid.MAX_DOWNLOAD + 1)}, "過大"),
    ],
)
def test_bad_downloads_never_replace_cache(tmp_path: Path, body: bytes, headers, message: str) -> None:
    radioid.download(tmp_path, opener=opener_for(CSV))
    with pytest.raises(RadioIdError, match=message):
        radioid.download(tmp_path, opener=opener_for(body, headers))
    assert (tmp_path / "user.csv").read_bytes() == CSV
    assert not list(tmp_path.glob("*.part"))


def test_download_stops_past_size_cap(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(radioid, "MAX_DOWNLOAD", 100)
    with pytest.raises(RadioIdError, match="大小上限"):
        radioid.download(tmp_path, opener=opener_for(CSV, {"Content-Length": "0"}))
    assert radioid.cache_info(tmp_path) is None


@pytest.mark.parametrize("exc, message", [
    (HTTPError("u", 503, "x", {}, None), "HTTP 503"),
    (URLError("no route"), "無法連線"),
])
def test_network_errors_are_reported(tmp_path: Path, exc: Exception, message: str) -> None:
    def failing(request, timeout):  # noqa: ANN001, ANN202
        raise exc

    with pytest.raises(RadioIdError, match=message):
        radioid.download(tmp_path, opener=failing)


def test_cache_helpers_without_data(tmp_path: Path) -> None:
    assert radioid.cache_info(tmp_path) is None
    assert radioid.load_cached(tmp_path) is None
    (tmp_path / "user.csv").write_bytes(CSV)
    (tmp_path / "meta.json").write_text("[1,2]", encoding="utf-8")
    info = radioid.cache_info(tmp_path)
    assert info and info.last_modified == ""


def test_lookup_cached_finds_one_id(tmp_path: Path) -> None:
    assert radioid.lookup_cached(tmp_path, 9990002) is None  # nothing downloaded yet
    radioid.download(tmp_path, opener=opener_for(CSV))
    user = radioid.lookup_cached(tmp_path, 9990002)
    assert user is not None and (user.callsign, user.first_name, user.city) == (
        "QA0ADN", "Chen", "Taichung")
    assert radioid.lookup_cached(tmp_path, 3100001).first_name == "Smith, Jr"
    assert radioid.lookup_cached(tmp_path, 466000) is None  # prefix of a real ID
    assert radioid.lookup_cached(tmp_path, 1234567) is None
