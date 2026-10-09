from __future__ import annotations

from dataclasses import replace
from pathlib import Path

import pytest

from opendrs import f136
from opendrs.diff import ByteRange, byte_ranges, diff_projects
from opendrs.textcodec import NameCodec

FIX = Path(__file__).parent / "fixtures"


@pytest.mark.parametrize(
    "a, b, expected",
    [
        (b"abcdef", b"abcdef", []),
        (b"abcdef", b"aXcdef", [ByteRange(1, b"b", b"X")]),
        (b"abcdefghij", b"aXcdeYghij", [ByteRange(1, b"bcdef", b"XcdeY")]),          # gap 3: merged
        (b"abcdefghijkl", b"aXcdefgYijkl", [ByteRange(1, b"b", b"X"), ByteRange(7, b"h", b"Y")]),
        (b"abc", b"abcde", [ByteRange(3, b"", b"de")]),                                  # grown
        (b"abcde", b"ab", [ByteRange(2, b"cde", b"")]),                                  # shrunk
    ],
)
def test_byte_ranges(a: bytes, b: bytes, expected: list[ByteRange]) -> None:
    assert byte_ranges(a, b) == expected


def test_identical_projects() -> None:
    p = f136.load(FIX / "mmdvm_20241127.F136")
    assert diff_projects(p, p, NameCodec.BIG5) == ["兩個專案內容完全相同。"]


def test_real_files_diff_points_at_changes() -> None:
    old = f136.load(FIX / "mmdvm_20241124.F136")
    new = f136.load(FIX / "mmdvm_20241127.F136")
    report = "\n".join(diff_projects(old, new, NameCodec.BIG5))
    assert "■ 裝置資訊" in report and "'DRS-8600' → 'DRS-8500'" in report
    assert "■ 聯絡人（8 → 9 筆）" in report
    assert "■ 分區（1 → 2 筆）" in report and "新增：HighWay" in report
    assert "■ 頻道（1 → 3 筆）" in report and "新增：HW_1" in report
    assert "■ 設定區（5,300 → 5,400 bytes" in report
    assert "image+0x0000" in report


def test_channel_frequency_change_is_spelled_out() -> None:
    old = f136.load(FIX / "mmdvm_20241127.F136")
    ch = old.channels[0].with_freq_b(439_000_000)
    new = old.with_channel(0, ch)
    lines = diff_projects(old, new, NameCodec.BIG5)
    assert "■ 頻道（3 → 3 筆）" in lines
    assert any("發射：438.80000 → 439.00000 MHz" in ln for ln in lines)
    assert any(ln.strip().startswith("+0x10:") for ln in lines)
    assert not any("接收：" in ln for ln in lines)


def test_removed_rows_and_image_only_change() -> None:
    old = f136.load(FIX / "mmdvm_20241127.F136")
    fewer = replace(old, contacts=old.contacts[:-1])
    assert any("刪除：" in ln for ln in diff_projects(old, fewer, NameCodec.BIG5))
    img = bytearray(old.image)
    img[0xA0] ^= 0xFF
    lines = diff_projects(old, replace(old, image=bytes(img)), NameCodec.BIG5)
    assert lines[0].startswith("■ 設定區") and "image+0x00A0" in lines[1]


def test_dmr_id_change_is_reported() -> None:
    old = f136.load(FIX / "dh98vu_20250811.F136")
    img = bytearray(old.image)
    img[0x90:0x93] = (9990999).to_bytes(3, "little")
    lines = diff_projects(old, replace(old, image=bytes(img)), NameCodec.BIG5)
    assert "  dmr_id: 9998765 → 9990999" in lines
