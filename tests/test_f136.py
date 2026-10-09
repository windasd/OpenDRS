from __future__ import annotations

from pathlib import Path

import pytest

from opendrs import f136
from opendrs.f136 import F136FormatError
from opendrs.records import Channel
from opendrs.textcodec import NameCodec

EXPECTED = {
    "mmdvm_20241124.F136": (8, 1, 1, 5300, "DRS-8600", "windasd"),
    "mmdvm_20241127.F136": (9, 2, 3, 5400, "DRS-8500", "windasd"),
    "dh98vu_20250811.F136": (359, 4, 24, 11000, "DRS-8500", "QA0AFK"),
}


def test_unmodified_project_round_trips_byte_exact(sample_path: Path) -> None:
    original = sample_path.read_bytes()
    assert f136.serialize(f136.parse(original)) == original


def test_parses_expected_structure(sample_path: Path) -> None:
    contacts, zones, channels, image_len, model, alias = EXPECTED[sample_path.name]
    project = f136.load(sample_path)
    assert (len(project.contacts), len(project.zones), len(project.channels)) == (
        contacts,
        zones,
        channels,
    )
    assert len(project.image) == image_len
    assert project.device.band_label == "F136-480"
    assert project.device.model == model
    assert project.device.alias == alias
    assert project.section_order == ("Contacts", "Zone", "CH")


def test_editing_a_channel_name_changes_only_that_field(small_path: Path) -> None:
    project = f136.load(small_path)
    edited = project.with_channel(0, project.channels[0].with_name("中繼台", NameCodec.BIG5))
    reloaded = f136.parse(f136.serialize(edited))
    assert reloaded.channels[0].name(NameCodec.BIG5) == "中繼台"
    before, after = project.channels[0].raw, reloaded.channels[0].raw
    changed = [i for i in range(32) if before[i] != after[i]]
    assert changed and all(2 <= i < 12 for i in changed)
    assert reloaded.image == project.image
    assert reloaded.contacts == project.contacts


def test_out_of_band_frequency_blocks_save(small_path: Path, tmp_path: Path) -> None:
    project = f136.load(small_path)
    bad = project.with_channel(0, project.channels[0].with_freq_a(500_000_000))
    assert any("超出本機頻段" in p for p in bad.problems())
    with pytest.raises(F136FormatError):
        f136.save(bad, tmp_path / "x.F136")


def test_in_band_frequency_is_accepted(small_path: Path) -> None:
    project = f136.load(small_path)
    ok = project.with_channel(0, project.channels[0].with_freq_b(145_500_000))
    assert ok.problems() == []


def test_save_is_atomic_and_keeps_one_backup(small_path: Path, tmp_path: Path) -> None:
    target = tmp_path / "p.F136"
    target.write_bytes(small_path.read_bytes())
    project = f136.load(target)
    edited = project.with_channel(0, project.channels[0].with_name("A", NameCodec.ASCII))
    f136.save(edited, target)
    f136.save(edited, target)
    assert (tmp_path / "p.F136.bak").read_bytes() == small_path.read_bytes()
    assert f136.load(target).channels[0].name(NameCodec.ASCII) == "A"
    assert not list(tmp_path.glob(".opendrs-*"))


def test_bom_and_missing_final_newline_are_tolerated(small_path: Path) -> None:
    data = small_path.read_bytes()
    variant = b"\xef\xbb\xbf" + data[:-2]
    assert f136.serialize(f136.parse(variant)) == data


def test_unknown_labels_are_preserved() -> None:
    data = b"Scan\t1\t2\t3\r\nCH\t" + b"\t".join(b"0" for _ in range(32)) + b"\r\n1\t2\t3\r\n"
    project = f136.parse(data)
    assert project.unknown_rows == (("Scan", bytes([1, 2, 3])),)
    assert f136.serialize(project) == data


@pytest.mark.parametrize(
    "data, message",
    [
        (b"1\tx\r\n", "非數字"),
        (b"1\t256\r\n", "0–255"),
        (b"CH\t1\t2\r\n", "CH"),
        (b"1\t2\r\n\r\n3\r\n", "空行"),
    ],
)
def test_malformed_files_raise_with_line_number(data: bytes, message: str) -> None:
    with pytest.raises(F136FormatError, match=message):
        f136.parse(data)


def test_with_helpers_reject_bad_rows(small_path: Path) -> None:
    project = f136.load(small_path)
    with pytest.raises(IndexError):
        project.with_channel(99, Channel(bytes(32)))
    assert project.with_zone(0, project.zones[0]).zones == project.zones
    assert project.with_contact(0, project.contacts[0]).contacts == project.contacts


def test_names_decodable_flags_bad_bytes() -> None:
    raw = bytes([1, 0]) + bytes([0x81, 0x30]) + bytes(28)
    project = f136.Project(channels=(Channel(raw),))
    assert not project.names_decodable(NameCodec.BIG5)
    assert f136.Project(channels=(Channel(bytes(32)),)).names_decodable(NameCodec.BIG5)
