"""Digital alarm lists and the encryption key list (settings image).

Both were mapped by driving the legacy CPS (legacy_probe/probe_grid.py):

* Alarm lists: 4 records of 0x12 bytes from 0xE3 — name[10] | +0x0A low nibble
  alarm type (常規 1, 禁止 2, 靜默 3, 靜默帶語音 4; high nibble kept) | +0x0B mode
  (緊急報警 1, 緊急報警和呼叫 2, 緊急報警和語音 3) | +0x0C zone u16 + +0x0E channel u16
  (alarm channel, 0 = Off) | +0x10 low nibble impolite attempts (1-15) | +0x11
  emergency-MIC duration in seconds (0-150, verified option by option).
* Encryption: level at 0x122E (無 0 … 增強 4), then 10 keys of 16 bytes from 0x122F:
  alias[10] + 6 key bytes (the legacy grid shows them as hex).
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

from .records import NAME_SIZE
from .textcodec import NameCodec, display_name, encode_name

ALARM_OFFSET = 0xE3
ALARM_SIZE = 0x12
ALARM_COUNT = 4
ALARM_TYPES = (("常規", 1), ("禁止", 2), ("靜默", 3), ("靜默帶語音", 4))
ALARM_MODES = (("緊急報警", 1), ("緊急報警和呼叫", 2), ("緊急報警和語音", 3))
MAX_MIC_SECONDS = 150  # the legacy CPS offers 0S-150S, stored as seconds at +0x11

ENC_LEVEL_OFFSET = 0x122E
ENC_KEYS_OFFSET = 0x122F
ENC_KEY_SIZE = 16
ENC_KEY_BYTES = 6
ENC_KEY_COUNT = 10
ENC_END = ENC_KEYS_OFFSET + ENC_KEY_SIZE * ENC_KEY_COUNT


class ListEditError(ValueError):
    pass


def _patch(image: bytes, offset: int, value: bytes) -> bytes:
    if len(image) < offset + len(value):
        raise ListEditError("設定區太短，無法寫入")
    return image[:offset] + value + image[offset + len(value):]


# ---------------------------------------------------------------- alarm lists
@dataclass(frozen=True)
class AlarmList:
    number: int
    raw: bytes

    def name(self, codec: NameCodec) -> str:
        return display_name(self.raw[:NAME_SIZE], codec)

    @property
    def alarm_type(self) -> int:
        return self.raw[0x0A] & 0x0F

    @property
    def mode(self) -> int:
        return self.raw[0x0B]

    @property
    def channel(self) -> tuple[int, int]:
        return struct.unpack_from("<HH", self.raw, 0x0C)

    @property
    def impolite_attempts(self) -> int:
        return self.raw[0x10] & 0x0F

    @property
    def mic_seconds(self) -> int:
        return self.raw[0x11]


def alarm_base(number: int) -> int:
    return ALARM_OFFSET + (number - 1) * ALARM_SIZE


def alarm_lists(image: bytes) -> tuple[AlarmList, ...]:
    if len(image) < ALARM_OFFSET + ALARM_COUNT * ALARM_SIZE:
        return ()
    return tuple(AlarmList(n, image[alarm_base(n):alarm_base(n) + ALARM_SIZE]) for n in range(1, ALARM_COUNT + 1))


def with_alarm(image: bytes, number: int, *, name: str | None = None, alarm_type: int | None = None,
               mode: int | None = None, channel: tuple[int, int] | None = None,
               impolite_attempts: int | None = None, mic_seconds: int | None = None,
               codec: NameCodec = NameCodec.BIG5) -> bytes:
    """Change some fields of alarm list ``number`` (1-4); others keep their bytes."""
    if not 1 <= number <= ALARM_COUNT:
        raise ListEditError(f"報警列表編號必須介於 1 與 {ALARM_COUNT}")
    base = alarm_base(number)
    rec = bytearray(alarm_lists(image)[number - 1].raw) if alarm_lists(image) else None
    if rec is None:
        raise ListEditError("設定區太短，沒有報警列表")
    if name is not None:
        rec[:NAME_SIZE] = encode_name(name, NAME_SIZE, codec)
    if alarm_type is not None:
        if alarm_type not in {v for _, v in ALARM_TYPES}:
            raise ListEditError("報警類型無效")
        rec[0x0A] = (rec[0x0A] & 0xF0) | alarm_type
    if mode is not None:
        if mode not in {v for _, v in ALARM_MODES}:
            raise ListEditError("報警模式無效")
        rec[0x0B] = mode
    if channel is not None:
        struct.pack_into("<HH", rec, 0x0C, *channel)
    if impolite_attempts is not None:
        if not 1 <= impolite_attempts <= 15:
            raise ListEditError("不禮貌嘗試次數必須介於 1 與 15")
        rec[0x10] = (rec[0x10] & 0xF0) | impolite_attempts
    if mic_seconds is not None:
        if not 0 <= mic_seconds <= MAX_MIC_SECONDS:
            raise ListEditError(f"緊急麥克風持續時間必須介於 0 與 {MAX_MIC_SECONDS} 秒")
        rec[0x11] = mic_seconds
    return _patch(image, base, bytes(rec))


# ---------------------------------------------------------------- encryption keys
@dataclass(frozen=True)
class EncryptionKey:
    number: int
    raw: bytes

    def alias(self, codec: NameCodec) -> str:
        return display_name(self.raw[:NAME_SIZE], codec)

    @property
    def key(self) -> bytes:
        return self.raw[NAME_SIZE:NAME_SIZE + ENC_KEY_BYTES]

    @property
    def in_use(self) -> bool:
        return self.raw[0] not in (0x00, 0xFF)


def encryption_keys(image: bytes) -> tuple[EncryptionKey, ...]:
    if len(image) < ENC_END:
        return ()
    return tuple(EncryptionKey(n, image[ENC_KEYS_OFFSET + (n - 1) * ENC_KEY_SIZE:ENC_KEYS_OFFSET + n * ENC_KEY_SIZE])
                 for n in range(1, ENC_KEY_COUNT + 1))


def parse_key_hex(text: str) -> bytes:
    digits = "".join(text.split())
    try:
        key = bytes.fromhex(digits)
    except ValueError as exc:
        raise ListEditError("金鑰必須是 12 個十六進位數字（例如 12 34 56 78 9A BC）") from exc
    if len(key) != ENC_KEY_BYTES:
        raise ListEditError("金鑰必須是 6 bytes（12 個十六進位數字）")
    return key


def with_encryption_key(image: bytes, number: int, alias: str, key: bytes, codec: NameCodec) -> bytes:
    """Set key ``number`` (1-10). An empty alias clears the slot (alias and key zeroed)."""
    if not 1 <= number <= ENC_KEY_COUNT:
        raise ListEditError(f"金鑰編號必須介於 1 與 {ENC_KEY_COUNT}")
    if len(key) != ENC_KEY_BYTES:
        raise ListEditError("金鑰必須是 6 bytes")
    record = bytes(ENC_KEY_SIZE) if not alias.strip() else encode_name(alias.strip(), NAME_SIZE, codec) + key
    return _patch(image, ENC_KEYS_OFFSET + (number - 1) * ENC_KEY_SIZE, record)


# ---------------------------------------------------------------- GPS report channel
GPS_ZONE_OFFSET = 0x1354
GPS_CHANNEL_OFFSET = 0x1356


def gps_channel(image: bytes) -> tuple[int, int]:
    """DMR service "GPS上報信道" as (zone, channel in zone); (0, 0) = Off."""
    if len(image) < GPS_CHANNEL_OFFSET + 2:
        return (0, 0)
    return (struct.unpack_from("<H", image, GPS_ZONE_OFFSET)[0],
            struct.unpack_from("<H", image, GPS_CHANNEL_OFFSET)[0])


def with_gps_channel(image: bytes, entry: tuple[int, int]) -> bytes:
    zone, channel = entry
    image = _patch(image, GPS_ZONE_OFFSET, struct.pack("<H", zone))
    return _patch(image, GPS_CHANNEL_OFFSET, struct.pack("<H", channel))
