from __future__ import annotations

from pathlib import Path

import pytest

from opendrs import f136
from opendrs.records import Channel, Contact, Zone, hz_to_mhz, mhz_to_hz
from opendrs.textcodec import NameCodec, NameEncodingError


def test_contact_fields_from_real_file(small_path: Path) -> None:
    contacts = f136.load(small_path).contacts
    tg = contacts[0]
    assert tg.index == 1
    assert tg.name(NameCodec.BIG5) == "TG46001_CN"
    assert tg.call_id == 46001
    assert tg.kind == 4 and tg.kind_label == "組呼"
    all_call = next(c for c in contacts if c.name(NameCodec.ASCII).startswith("ALL"))
    assert all_call.kind_label == "全呼"
    assert all_call.call_id >= 16777184


def test_channel_frequencies_from_real_file(small_path: Path) -> None:
    ch = f136.load(small_path).channels[0]
    assert ch.name(NameCodec.ASCII) == "MMDVM"
    assert ch.freq_a_hz == 438_800_000 == ch.freq_b_hz


def test_contact_call_id_bounds() -> None:
    c = Contact(bytes(16))
    assert c.with_call_id(46001).call_id == 46001
    assert c.kind_label == "類型 0"
    for bad in (0, 0x1000000):
        with pytest.raises(ValueError):
            c.with_call_id(bad)


def test_record_sizes_are_enforced() -> None:
    for cls, size in ((Contact, 16), (Zone, 32), (Channel, 32)):
        with pytest.raises(ValueError):
            cls(bytes(size - 1))


def test_zone_rename_round_trip() -> None:
    z = Zone(bytes(32)).with_name("高山", NameCodec.GBK)
    assert z.name(NameCodec.GBK) == "高山"
    assert z.index == 0


def test_name_too_long_is_rejected_not_truncated() -> None:
    with pytest.raises(NameEncodingError, match="超過"):
        Channel(bytes(32)).with_name("一二三四五六", NameCodec.BIG5)


def test_frequency_must_fit_u32() -> None:
    with pytest.raises(ValueError):
        Channel(bytes(32)).with_freq_a(-1)


@pytest.mark.parametrize(
    "text, hz", [("438.8", 438_800_000), (" 145.5000 ", 145_500_000), ("436.0125", 436_012_500)]
)
def test_mhz_parse_is_exact(text: str, hz: int) -> None:
    assert mhz_to_hz(text) == hz


@pytest.mark.parametrize("text", ["abc", "438.8000001"])
def test_mhz_parse_rejects_bad_input(text: str) -> None:
    with pytest.raises(ValueError):
        mhz_to_hz(text)


def test_hz_to_mhz_format() -> None:
    assert hz_to_mhz(438_800_000) == "438.80000"


def test_rx_tx_aliases_match_confirmed_offsets() -> None:
    ch = Channel(bytes(32)).with_rx(439_137_500).with_tx(431_537_500)
    assert ch.rx_hz == ch.freq_a_hz == 439_137_500
    assert ch.tx_hz == ch.freq_b_hz == 431_537_500
    assert ch.raw[0x0C:0x10] == (439_137_500).to_bytes(4, "little")
