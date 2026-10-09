"""Firmware patches the CPS can apply to a vendor DR-8500UV image.

RECORD_READ wires the firmware's own recording-download handlers to the PC receive loop
(they exist but are unreachable in the vendor builds — see DR8500UV_逆向分析_最終紀錄.md 6.7/6.8):

  0x03026E5C  bt16 0x03026E3C -> bt16 CAVE_R     ("not 'p'" used to drop the byte)
  0x03026EC6  bt16 0x03026E88 -> bt16 CAVE_H     ("not 0x88" used to drop the frame)
  CAVE_R 0x03027024: cmpnei r3,'R' ; bf 0x03026F04 (the 'F' command path) ; br 0x03026E3C
  CAVE_H 0x0302702C: cmpnei r3,0x58 ; bt 0x03026E88 ; mov r0,r4 ; bsr 0x030272C4 (list-frame
                     handler) ; bsr 0x03026DC0 ; br 0x03026E3C   -- mirrors the 0x88 path

The caves overwrite the start of a dead dispatcher at 0x03027024. The same 30 bytes apply to
both vendor builds seen so far (…_03_136174_400480 and …_05_144146_430440: identical code, they
differ only in band-limit constants and build date). Verified on a DR-8500UV (BK4819) on
2026-10-09: flashed with the built-in IAP upgrade, recordings listed and downloaded.

Safety: every patched byte must hold its known original value; for an image that is not one
of the known vendor builds the whole file is also scanned (raw C-SKY V2 encodings, both units)
to make sure nothing else branches, calls, loads or points into the overwritten dead code.
"""

from __future__ import annotations

import hashlib
import struct
from dataclasses import dataclass

A_BASE, CUT, B_BASE = 0x0300C000, 0x0A9F5C, 0x00010000
CAVE_START, CAVE_END = 0x03027024, 0x0302703E
DEAD_CODE = (0x03027024, 0x0302705E)  # instructions of the dead dispatcher (its pool follows)
BRANCH_SITES = (0x03026E5C, 0x03026EC6)

# Vendor builds the patch was checked against (whole-file scan done offline).
KNOWN_BASES = {
    "fd52d62d980c59039da869c05f2c5802fb60a9fc639f4cd6d73680c3b896cc36": "C7000_DR8500UV_4819_03_136174_400480.bin",
    "2267ee7a650490368bbf0de4197b542353864ef1ab82ea8cc54d9f20f0e526ab": "C7000_DR8500UV_4819_05_144146_430440.bin",
}


@dataclass(frozen=True)
class Site:
    va: int
    original: bytes
    patched: bytes
    why: str


# 32-bit instructions are stored high halfword first, each halfword little-endian.
RECORD_READ: tuple[Site, ...] = (
    Site(0x03026E5C, bytes.fromhex("f00b"), bytes.fromhex("e408"), "bt16 -> CAVE_R（原本：不是 'p' 就丟棄）"),
    Site(0x03026EC6, bytes.fromhex("e10b"), bytes.fromhex("b308"), "bt16 -> CAVE_H（原本：不是 0x88 就丟棄）"),
    Site(0x03027024, bytes.fromhex("d2142614"), bytes.fromhex("43eb5200"), "cmpnei r3,'R'"),
    Site(0x03027028, bytes.fromhex("c36c"), bytes.fromhex("6e0f"), "bf 0x03026F04（交給 'F' 指令路徑）"),
    Site(0x0302702A, bytes.fromhex("476d"), bytes.fromhex("0907"), "br 0x03026E3C（其他照舊丟棄）"),
    Site(0x0302702C, bytes.fromhex("3b6c4f6c"), bytes.fromhex("43eb5800"), "cmpnei r3,0x58"),
    Site(0x03027030, bytes.fromhex("976c"), bytes.fromhex("2c0b"), "bt 0x03026E88（不是 0x58 照舊丟棄）"),
    Site(0x03027032, bytes.fromhex("0033"), bytes.fromhex("136c"), "mov r0,r4（封包緩衝區）"),
    Site(0x03027034, bytes.fromhex("60b861b8"), bytes.fromhex("00e04801"), "bsr 0x030272C4（清單封包處理）"),
    Site(0x03027038, bytes.fromhex("62b863b8"), bytes.fromhex("ffe3c4fe"), "bsr 0x03026DC0"),
    Site(0x0302703C, bytes.fromhex("64b8"), bytes.fromhex("0007"), "br 0x03026E3C"),
)


class PatchError(ValueError):
    pass


@dataclass(frozen=True)
class PatchResult:
    data: bytes
    base_sha256: str
    sha256: str
    changed: int
    base_name: str  # known vendor file name, or "" for an image checked by scanning


def _fo(va: int) -> int:
    return va - A_BASE if A_BASE <= va < A_BASE + CUT else va - B_BASE + CUT


def _va(o: int) -> int:
    return A_BASE + o if o < CUT else B_BASE + (o - CUT)


def _sext(v: int, bits: int) -> int:
    return v - (1 << bits) if v & (1 << (bits - 1)) else v


def refs_into(data: bytes, lo: int, hi: int) -> list[str]:
    """Raw scan of both units for branches, calls, PC-relative loads and pointers into [lo, hi)."""
    out = []
    halves = struct.unpack_from(f"<{len(data) // 2}H", data)
    for i in range(len(halves) - 1):
        o = i * 2
        at = _va(o)
        h, n = halves[i], halves[i + 1]
        w = (h << 16) | n
        tgt = None
        if w >> 26 == 0b111000:                                   # bsr32
            tgt = at + _sext(w & 0x3FFFFFF, 26) * 2
        elif h >> 8 == 0xE8 and h & 0x1F == 0:                    # br32 / bt32 / bf32
            tgt = at + _sext(n, 16) * 2
        elif h >> 10 == 0b111010 and (h >> 5) & 0x1F in (0x14, 0x16, 0x17):  # lrw32 / jmpi32 / jsri32
            tgt = (at & ~3) + n * 4
        if tgt is not None and lo <= tgt < hi:
            out.append(f"{at:08X}->{tgt:08X}")
        if h >> 10 in (1, 2, 3):                                   # br16 / bt16 / bf16
            t16 = at + _sext(h & 0x3FF, 10) * 2
            if lo <= t16 < hi:
                out.append(f"{at:08X}->{t16:08X}(b16)")
        if h >> 10 == 0b000100:                                    # lrw16 (0x14xx = push/pop)
            t16 = (at & ~3) + ((((h >> 8) & 3) << 5) | (h & 0x1F)) * 4
            if lo <= t16 < hi:
                out.append(f"{at:08X}->{t16:08X}(lrw16)")
        if lo <= w < hi or lo <= ((n << 16) | h) < hi:            # pointer, either halfword order
            out.append(f"{at:08X}:ptr")
    return out


def _site_bytes(data: bytes, site: Site) -> bytes:
    o = _fo(site.va)
    return data[o:o + len(site.original)]


def is_record_read_patched(data: bytes) -> bool:
    return len(data) > CUT and all(_site_bytes(data, s) == s.patched for s in RECORD_READ)


def record_read_status(data: bytes) -> str:
    """"patched", "patchable" or a reason the patch does not fit (for the UI)."""
    if is_record_read_patched(data):
        return "patched"
    if len(data) <= CUT:
        return "檔案太小，不是 DR-8500UV／DH-98VU（C7000 平台）的完整韌體"
    wrong = [s for s in RECORD_READ if _site_bytes(data, s) != s.original]
    if wrong:
        return f"程式碼與已知的原廠版本不同（{len(wrong)} 個修補點不符），無法套用"
    return "patchable"


def patch_record_read(data: bytes) -> PatchResult:
    status = record_read_status(data)
    if status == "patched":
        raise PatchError("這個檔案已經是錄音讀取版")
    if status != "patchable":
        raise PatchError(status)
    base = hashlib.sha256(data).hexdigest()
    base_name = KNOWN_BASES.get(base, "")
    if not base_name:  # unknown build with matching bytes: prove the overwritten code is dead
        lo, hi = DEAD_CODE
        outside = [r for r in refs_into(data, lo, hi) if not lo <= int(r[:8], 16) < hi]
        if outside:
            raise PatchError("檢查到有程式會進入被覆寫的區域，不套用：" + "、".join(outside[:5]))
    out = bytearray(data)
    for s in RECORD_READ:
        o = _fo(s.va)
        out[o:o + len(s.patched)] = s.patched
    if not base_name:
        into = sorted(r for r in refs_into(bytes(out), CAVE_START, CAVE_END)
                      if not CAVE_START <= int(r[:8], 16) < CAVE_END)
        if into != ["03026E5C->03027024(b16)", "03026EC6->0302702C(b16)"]:
            raise PatchError("修補後的進入點不符預期，不套用：" + "、".join(into[:5]))
    changed = sum(1 for a, b in zip(data, out) if a != b)
    return PatchResult(bytes(out), base, hashlib.sha256(out).hexdigest(), changed, base_name)
