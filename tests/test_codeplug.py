"""Codeplug stream layout, verified against a real capture of the legacy CPS."""

from __future__ import annotations

import struct
from dataclasses import replace
from pathlib import Path

import pytest

from opendrs import f136
from opendrs.codeplug import (
    BLOCK,
    MAX_CONTACTS,
    CodeplugError,
    Counts,
    build_stream,
    check_writable,
    counts_in_image,
    encode_date,
    image_length_for,
    parse_stream,
    sync_image,
)
from opendrs.contact_import import NewContact, append_contacts
from opendrs.records import Contact
from opendrs.textcodec import NameCodec

CAP = Path(__file__).parent / "fixtures" / "capture"
FIX = Path(__file__).parent / "fixtures"


@pytest.fixture(scope="module")
def written() -> f136.Project:
    return f136.load(CAP / "written_project.F136")


def reply_counts(reply: bytes) -> tuple[Counts, int, int]:
    z, c, n, p1, p2 = struct.unpack_from("<5H", reply, 12)
    return Counts(z, c, n), p1, p2


def test_block_formulas_match_what_the_radio_reported() -> None:
    counts, p1, p2 = reply_counts((CAP / "read_reply.bin").read_bytes())
    assert counts == Counts(4, 27, 360)
    assert (counts.part1_blocks, counts.part2_blocks) == (p1, p2) == (6, 1)
    assert counts.stream_size == len((CAP / "read_stream.bin").read_bytes())
    assert (CAP / "write_cmd.bin").read_bytes()[15:19] == struct.pack(">I", counts.total_blocks)


def test_build_stream_reproduces_the_captured_write_byte_for_byte(written: f136.Project) -> None:
    captured = (CAP / "write_stream.bin").read_bytes()
    assert build_stream(written, date_text="10/8/2026 16:54") == captured


def test_without_date_only_the_date_field_differs(written: f136.Project) -> None:
    captured = (CAP / "write_stream.bin").read_bytes()
    ours = build_stream(written)
    diff = [i for i in range(len(captured)) if ours[i] != captured[i]]
    assert diff and all(0x60 <= i < 0x70 for i in diff)


def test_parse_captured_read_reproduces_the_project(written: f136.Project) -> None:
    counts, _, _ = reply_counts((CAP / "read_reply.bin").read_bytes())
    project = parse_stream((CAP / "read_stream.bin").read_bytes(), counts)
    assert project.contacts == written.contacts
    assert project.zones == written.zones
    assert project.channels == written.channels
    assert len(project.image) == len(written.image) == 11000
    diff = [i for i in range(len(project.image)) if project.image[i] != written.image[i]]
    assert all(0x60 <= i < 0x70 for i in diff)  # only the write timestamp
    assert project.channels[9].name(NameCodec.ASCII) == "test"
    assert project.channels[9].freq_a_hz == 439_137_500  # RX
    assert project.channels[9].freq_b_hz == 431_537_500  # TX


def test_first_read_has_the_pre_edit_channel_count() -> None:
    project = parse_stream((CAP / "read1_stream.bin").read_bytes(), Counts(4, 26, 360))
    assert len(project.channels) == 26
    assert all(c.name(NameCodec.ASCII) != "test" for c in project.channels)


def test_read_then_write_round_trip(written: f136.Project) -> None:
    counts, _, _ = reply_counts((CAP / "read_reply.bin").read_bytes())
    stream = (CAP / "read_stream.bin").read_bytes()
    assert build_stream(parse_stream(stream, counts)) == stream


@pytest.mark.parametrize("name", ["mmdvm_20241124", "mmdvm_20241127", "dh98vu_20250811"])
def test_legacy_projects_are_already_consistent(name: str) -> None:
    project = f136.load(FIX / f"{name}.F136")
    raw = b"".join(c.raw for c in project.contacts)
    assert sync_image(project.image, raw, len(project.zones), len(project.channels)) == project.image
    assert counts_in_image(project.image) == Counts.of(project)
    build_stream(project)  # every legacy project is transferable


@pytest.mark.parametrize("n", [0, 1, 8, 9, 359, 360, 448, 449, 729, 1000])
def test_layout_invariants(n: int) -> None:
    c = Counts(4, 27, n)
    assert c.part1_blocks >= 3
    assert c.part1_blocks * BLOCK >= 0x1400 + 16 * n
    assert (c.part1_blocks - 1) * BLOCK < 0x1400 + 16 * n or n == 0 and c.part1_blocks == 3
    assert image_length_for(n) >= 0x1400 + 16 * n


def test_more_contacts_grow_part1_and_keep_everything_consistent(written: f136.Project) -> None:
    extra = [NewContact(Contact.PRIVATE_CALL, f"C{i}", 4670000 + i) for i in range(369)]
    big, report = append_contacts(written, extra, NameCodec.ASCII)
    assert report.added == 369 and len(big.contacts) == 729
    stream = build_stream(big)
    counts = Counts.of(big)
    assert counts.part1_blocks == 9 and len(stream) == counts.stream_size
    assert parse_stream(stream, counts).contacts == big.contacts
    assert stream[9 * BLOCK:9 * BLOCK + 32] == big.zones[0].raw  # zones start right after part 1


def test_contact_limit_is_enforced(written: f136.Project) -> None:
    filler = (Contact.build(1, 5, bytes(10), 9990001),) * (MAX_CONTACTS + 1)
    too_many = replace(written, contacts=filler)
    assert any("上限" in r for r in check_writable(too_many))
    with pytest.raises(CodeplugError, match="上限"):
        build_stream(too_many)
    with pytest.raises(CodeplugError, match="上限"):
        parse_stream(bytes(BLOCK * 30), Counts(4, 1, MAX_CONTACTS + 1))


def test_projects_without_settings_cannot_be_written() -> None:
    with pytest.raises(CodeplugError, match="設定區"):
        build_stream(f136.Project(zones=(f136.Zone(bytes(32)),)))


def test_parse_rejects_wrong_size_and_inconsistent_counts() -> None:
    stream = (CAP / "read_stream.bin").read_bytes()
    with pytest.raises(CodeplugError, match="預期"):
        parse_stream(stream[:-1], Counts(4, 27, 360))
    with pytest.raises(CodeplugError, match="不一致"):
        parse_stream(stream, Counts(4, 26, 360))


def test_data_beyond_part1_is_refused(written: f136.Project) -> None:
    img = bytearray(written.image) + bytearray(2048)
    img[-1] = 1
    with pytest.raises(CodeplugError, match="超出"):
        build_stream(replace(written, image=bytes(img)))


@pytest.mark.parametrize("text", ["2026/10/08 16:54", "10/8/2026 16:54", ""])
def test_encode_date(text: str) -> None:
    raw = encode_date(text)
    assert len(raw) == 16 and raw.rstrip(b"\x00").decode() == text


@pytest.mark.parametrize("text, msg", [("2026年10月8日", "ASCII"), ("x" * 17, "超過")])
def test_encode_date_rejects(text: str, msg: str) -> None:
    with pytest.raises(CodeplugError, match=msg):
        encode_date(text)


def test_sync_image_skips_projects_without_settings() -> None:
    assert sync_image(b"abc", b"x" * 16, 1, 1) == b"abc"
    assert counts_in_image(b"abc") is None
