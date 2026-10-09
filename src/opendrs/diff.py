"""Byte-level comparison of two projects.

Main use: map unknown fields. Change one setting in the legacy CPS, save, and
compare with the previous save — the report pinpoints the bytes that moved.
"""

from __future__ import annotations

from dataclasses import dataclass, fields

from .f136 import Project
from .records import Channel, hz_to_mhz
from .textcodec import NameCodec

MERGE_GAP = 4


@dataclass(frozen=True)
class ByteRange:
    start: int
    old: bytes
    new: bytes


def byte_ranges(a: bytes, b: bytes, merge_gap: int = MERGE_GAP) -> list[ByteRange]:
    """Differing spans; spans separated by at most ``merge_gap`` equal bytes are merged."""
    n = max(len(a), len(b))

    def same(i: int) -> bool:
        return i < len(a) and i < len(b) and a[i] == b[i]

    out: list[ByteRange] = []
    i = 0
    while i < n:
        if same(i):
            i += 1
            continue
        start = last = i
        i += 1
        while i < n and (not same(i) or i - last <= merge_gap):
            if not same(i):
                last = i
            i += 1
        out.append(ByteRange(start, a[start:last + 1], b[start:last + 1]))
        i = last + 1
    return out


def _ascii(data: bytes) -> str:
    return "".join(chr(x) if 0x20 <= x < 0x7F else "." for x in data)


def _fmt_range(r: ByteRange, base: str) -> list[str]:
    return [
        f"  {base}+0x{r.start:04X}  ({len(max(r.old, r.new, key=len))} bytes)",
        f"      舊 {r.old.hex(' ') or '(無)':<48} |{_ascii(r.old)}|",
        f"      新 {r.new.hex(' ') or '(無)':<48} |{_ascii(r.new)}|",
    ]


def _records(title: str, old: tuple, new: tuple, codec: NameCodec) -> list[str]:
    lines: list[str] = []
    for row in range(max(len(old), len(new))):
        a = old[row] if row < len(old) else None
        b = new[row] if row < len(new) else None
        if a is not None and b is not None and a.raw == b.raw:
            continue
        if a is None:
            lines.append(f"  第 {row + 1} 列 新增：{b.name(codec)}")
            continue
        if b is None:
            lines.append(f"  第 {row + 1} 列 刪除：{a.name(codec)}")
            continue
        label = a.name(codec) if a.name(codec) == b.name(codec) else f"{a.name(codec)} → {b.name(codec)}"
        lines.append(f"  第 {row + 1} 列（{label}）")
        if isinstance(a, Channel):
            for attr, title_ in (("freq_a_hz", "接收"), ("freq_b_hz", "發射")):
                if getattr(a, attr) != getattr(b, attr):
                    lines.append(f"      {title_}：{hz_to_mhz(getattr(a, attr))} → "
                                 f"{hz_to_mhz(getattr(b, attr))} MHz")
        for r in byte_ranges(a.raw, b.raw):
            lines.append(f"      +0x{r.start:02X}: {r.old.hex(' ')} → {r.new.hex(' ')}")
    if lines or len(old) != len(new):
        return [f"■ {title}（{len(old)} → {len(new)} 筆）", *lines]
    return []


def diff_projects(old: Project, new: Project, codec: NameCodec) -> list[str]:
    report: list[str] = []
    dev_a, dev_b = old.device, new.device
    dev_lines = [f"  {f.name}: {getattr(dev_a, f.name)!r} → {getattr(dev_b, f.name)!r}"
                 for f in fields(dev_a) if getattr(dev_a, f.name) != getattr(dev_b, f.name)]
    if dev_lines:
        report += ["■ 裝置資訊", *dev_lines]
    report += _records("聯絡人", old.contacts, new.contacts, codec)
    report += _records("分區", old.zones, new.zones, codec)
    report += _records("頻道", old.channels, new.channels, codec)
    ranges = byte_ranges(old.image, new.image)
    if ranges:
        changed = sum(len(max(r.old, r.new, key=len)) for r in ranges)
        report.append(f"■ 設定區（{len(old.image):,} → {len(new.image):,} bytes，"
                      f"{len(ranges)} 處、約 {changed} bytes 不同）")
        for r in ranges:
            report += _fmt_range(r, "image")
    return report or ["兩個專案內容完全相同。"]
