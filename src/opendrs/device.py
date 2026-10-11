"""Device information held in the settings image, and frequency-band rules."""

from __future__ import annotations

import re
from dataclasses import dataclass

BAND_LABEL_OFFSET = 0x50
DATE_OFFSET = 0x60
MODEL_OFFSET = 0x70
ALIAS_OFFSET = 0x80
FIELD_SIZE = 16
# The radio's own DMR ID: u24 little-endian right after the alias. Located 2026-10-08:
# the DH-98VU project (alias QA0AFK) holds AD 91 98 = 9998765 here, the contact list
# names that ID "QA0AFK", and the stream read from the radio carries the same bytes.
DMR_ID_OFFSET = 0x90
DMR_ID_SIZE = 3
_DMR_ID_UNSET = (0, 0xFFFFFF)

Band = tuple[int, int]

_MHZ = 1_000_000

# Factory band variants of this product family. "136-480" denotes the dual-band
# VHF + UHF variant rather than one continuous range.
BAND_TABLE: dict[str, tuple[Band, ...]] = {
    "136-480": ((136 * _MHZ, 174 * _MHZ), (400 * _MHZ, 480 * _MHZ)),
    # Not a factory variant: the band-widened test firmware (mkpatch_band_record.py) reports this.
    "136-520": ((136 * _MHZ, 180 * _MHZ), (400 * _MHZ, 520 * _MHZ)),
    "136-174": ((136 * _MHZ, 174 * _MHZ),),
    "140-174": ((140 * _MHZ, 174 * _MHZ),),
    "150-170": ((150 * _MHZ, 170 * _MHZ),),
    "240-250": ((240 * _MHZ, 250 * _MHZ),),
    "350-400": ((350 * _MHZ, 400 * _MHZ),),
    "400-470": ((400 * _MHZ, 470 * _MHZ),),
    "400-480": ((400 * _MHZ, 480 * _MHZ),),
    "410-440": ((410 * _MHZ, 440 * _MHZ),),
    "430-450": ((430 * _MHZ, 450 * _MHZ),),
    "430-480": ((430 * _MHZ, 480 * _MHZ),),
    "450-520": ((450 * _MHZ, 520 * _MHZ),),
    "30-32": ((30 * _MHZ, 32 * _MHZ),),
    "66-68": ((66 * _MHZ, 68 * _MHZ),),
    "66-88": ((66 * _MHZ, 88 * _MHZ),),
}


# Firmware ID and build date, e.g. "308E.BBKTARSCB.003.Mar  9 2024", NUL-terminated;
# it runs past 16 bytes, into the next field's slot (the radio writes it on a read).
FIRMWARE_OFFSET = 0x30
FIRMWARE_SIZE = 0x20
_BUILD_DATE = re.compile(r"^(?P<version>.+?)\.(?P<date>[A-Z][a-z]{2} +\d{1,2} +\d{4})$")


def _ascii_field(image: bytes, offset: int, size: int = FIELD_SIZE) -> str:
    field = image[offset : offset + size]
    out = []
    for b in field:
        if b in (0x00, 0xFF):
            break
        out.append(chr(b) if 0x20 <= b < 0x7F else "?")
    return "".join(out).strip()


def _dmr_id(image: bytes) -> int | None:
    field = image[DMR_ID_OFFSET : DMR_ID_OFFSET + DMR_ID_SIZE]
    if len(field) != DMR_ID_SIZE:
        return None
    value = int.from_bytes(field, "little")
    return None if value in _DMR_ID_UNSET else value


def _clean_id(text: str) -> str:
    """Older projects hold binary junk here; show nothing rather than '?'s."""
    return "" if "?" in text else text


def _firmware(image: bytes) -> dict[str, str]:
    text = _ascii_field(image, FIRMWARE_OFFSET, FIRMWARE_SIZE)
    match = _BUILD_DATE.match(text)
    if match:
        return {"firmware": match["version"], "firmware_date": " ".join(match["date"].split())}
    return {"firmware": text, "firmware_date": ""}


@dataclass(frozen=True)
class DeviceInfo:
    band_label: str
    write_date: str
    model: str
    alias: str
    dmr_id: int | None = None  # None: not set (all 00 / all FF) or image too short
    firmware: str = ""  # e.g. 308E.BBKTARSCB.003 (as the radio reported it on its last read)
    factory_number: str = ""  # 0x00, legacy 設備信息 "出廠編號"
    serial_number: str = ""  # 0x10, "序號"
    model_number: str = ""  # 0x20, "型號" (e.g. DR8600)
    firmware_date: str = ""  # build date, e.g. Mar  9 2024

    @classmethod
    def from_image(cls, image: bytes) -> DeviceInfo:
        return cls(
            band_label=_ascii_field(image, BAND_LABEL_OFFSET),
            write_date=_ascii_field(image, DATE_OFFSET),
            model=_ascii_field(image, MODEL_OFFSET),
            alias=_ascii_field(image, ALIAS_OFFSET),
            dmr_id=_dmr_id(image),
            **_firmware(image),
            factory_number=_clean_id(_ascii_field(image, 0x00)),
            serial_number=_clean_id(_ascii_field(image, 0x10)),
            model_number=_clean_id(_ascii_field(image, 0x20)),
        )

    @property
    def bands(self) -> tuple[Band, ...] | None:
        key = self.band_label.lstrip("F")
        return BAND_TABLE.get(key)


def frequency_problem(hz: int, bands: tuple[Band, ...] | None) -> str | None:
    """Return a reason the frequency is unusable on this device, or ``None``."""
    if hz == 0:
        return None
    if bands is None:
        return None
    if any(lo <= hz <= hi for lo, hi in bands):
        return None
    ranges = "、".join(f"{lo // _MHZ}–{hi // _MHZ}" for lo, hi in bands)
    return f"{hz / _MHZ:.5f} MHz 超出本機頻段（{ranges} MHz）"


MAX_DMR_ID = 16_776_415  # top of the DMR individual/group ID range
# The alias field is 16 bytes and the DMR ID follows immediately; keep one NUL so the
# radio always finds the end of the string (both real projects are NUL-padded).
MAX_ALIAS_LEN = FIELD_SIZE - 1


def _patch(image: bytes, offset: int, value: bytes) -> bytes:
    if len(image) < offset + len(value):
        raise ValueError("設定區太短，無法寫入此欄位（請先開啟專案或從對講機讀取）")
    return image[:offset] + value + image[offset + len(value):]


def with_dmr_id(image: bytes, dmr_id: int) -> bytes:
    """Store the radio's own DMR ID; the byte after it (another setting) is untouched."""
    if not 1 <= dmr_id <= MAX_DMR_ID:
        raise ValueError(f"DMR ID 必須介於 1 與 {MAX_DMR_ID}")
    return _patch(image, DMR_ID_OFFSET, dmr_id.to_bytes(DMR_ID_SIZE, "little"))


def with_alias(image: bytes, alias: str) -> bytes:
    text = alias.strip()
    if len(text) > MAX_ALIAS_LEN:
        raise ValueError(f"別名最多 {MAX_ALIAS_LEN} 個字元")
    if any(not 0x20 <= ord(ch) < 0x7F for ch in text):
        raise ValueError("別名只能使用英數字與一般符號（ASCII）")
    return _patch(image, ALIAS_OFFSET, text.encode("ascii").ljust(FIELD_SIZE, b"\0"))


def parse_dmr_id(text: str) -> int:
    try:
        value = int(text.strip())
    except ValueError as exc:
        raise ValueError(f"「{text.strip()}」不是有效的 DMR ID（只能是數字）") from exc
    if not 1 <= value <= MAX_DMR_ID:
        raise ValueError(f"DMR ID 必須介於 1 與 {MAX_DMR_ID}")
    return value
