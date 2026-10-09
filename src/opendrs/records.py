"""Immutable views over the fixed-size records of a ``.F136`` project.

Every record keeps its complete raw bytes. Only the fields whose meaning has been
confirmed are exposed; everything else is carried through untouched, so editing a
name or frequency never disturbs bytes this program does not understand.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

from .textcodec import NameCodec, display_name, encode_name

NAME_SIZE = 10


def _replace(raw: bytes, offset: int, value: bytes) -> bytes:
    return raw[:offset] + value + raw[offset + len(value) :]


@dataclass(frozen=True)
class Contact:
    """16 bytes: index u16 | kind u8 | name[10] | call id u24 (little-endian)."""

    raw: bytes
    SIZE = 16
    NAME_OFFSET = 3
    ID_OFFSET = 13
    MAX_ID = 0xFFFFFF
    GROUP_CALL = 4
    PRIVATE_CALL = 5
    # Confirmed from a real 359-contact project: 4 = talkgroups (TG46001…),
    # 5 = 344 callsigns with 466xxxx IDs, 6 = ALL200/ALL215 (16777200–215),
    # 7 = ALL (16777184, the no-address range).
    KINDS = {4: "組呼", 5: "個呼", 6: "全呼", 7: "無地址呼"}

    def __post_init__(self) -> None:
        if len(self.raw) != self.SIZE:
            raise ValueError(f"Contact record must be {self.SIZE} bytes, got {len(self.raw)}")

    @classmethod
    def build(cls, index: int, kind: int, name_field: bytes, call_id: int) -> Contact:
        if not 0 <= index <= 0xFFFF or not 0 <= kind <= 0xFF:
            raise ValueError("索引或類型超出範圍")
        if len(name_field) != NAME_SIZE:
            raise ValueError(f"名稱欄必須是 {NAME_SIZE} bytes")
        if not 1 <= call_id <= cls.MAX_ID:
            raise ValueError(f"ID 必須介於 1 與 {cls.MAX_ID}")
        raw = struct.pack("<HB", index, kind) + name_field + call_id.to_bytes(3, "little")
        return cls(raw)

    @property
    def index(self) -> int:
        return int(struct.unpack_from("<H", self.raw, 0)[0])

    @property
    def kind(self) -> int:
        return self.raw[2]

    @property
    def kind_label(self) -> str:
        return self.KINDS.get(self.kind, f"類型 {self.kind}")

    @property
    def call_id(self) -> int:
        return int.from_bytes(self.raw[self.ID_OFFSET : self.ID_OFFSET + 3], "little")

    def name(self, codec: NameCodec) -> str:
        return display_name(self.raw[self.NAME_OFFSET : self.NAME_OFFSET + NAME_SIZE], codec)

    def with_name(self, text: str, codec: NameCodec) -> Contact:
        field = encode_name(text, NAME_SIZE, codec)
        return Contact(_replace(self.raw, self.NAME_OFFSET, field))

    def with_name_field(self, field: bytes) -> Contact:
        """Replace the raw 10-byte name field (already encoded and padded)."""
        if len(field) != NAME_SIZE:
            raise ValueError(f"名稱欄必須是 {NAME_SIZE} bytes")
        return Contact(_replace(self.raw, self.NAME_OFFSET, field))

    def with_index(self, index: int) -> Contact:
        if not 0 <= index <= 0xFFFF:
            raise ValueError("聯絡人編號超出範圍")
        return Contact(_replace(self.raw, 0, struct.pack("<H", index)))

    def with_call_id(self, call_id: int) -> Contact:
        if not 1 <= call_id <= self.MAX_ID:
            raise ValueError(f"ID 必須介於 1 與 {self.MAX_ID}")
        return Contact(_replace(self.raw, self.ID_OFFSET, call_id.to_bytes(3, "little")))


@dataclass(frozen=True)
class Zone:
    """32 bytes: index u16 | u8 | name[10] | first u16 | count u16 | remainder kept raw.

    ``first`` numbers the zone's first channel among all 32-byte records of part 2
    (zones first, then channels, counting from 1), so zone 1 starts at
    ``zone_count + 1`` and each zone's channels directly follow the previous zone's.
    """

    raw: bytes
    SIZE = 32
    NAME_OFFSET = 3
    FIRST_OFFSET = 0x0D
    COUNT_OFFSET = 0x0F

    def __post_init__(self) -> None:
        if len(self.raw) != self.SIZE:
            raise ValueError(f"Zone record must be {self.SIZE} bytes, got {len(self.raw)}")

    @property
    def index(self) -> int:
        return int(struct.unpack_from("<H", self.raw, 0)[0])

    def name(self, codec: NameCodec) -> str:
        return display_name(self.raw[self.NAME_OFFSET : self.NAME_OFFSET + NAME_SIZE], codec)

    def with_name(self, text: str, codec: NameCodec) -> Zone:
        field = encode_name(text, NAME_SIZE, codec)
        return Zone(_replace(self.raw, self.NAME_OFFSET, field))

    @property
    def first_record(self) -> int:
        return int(struct.unpack_from("<H", self.raw, self.FIRST_OFFSET)[0])

    @property
    def channel_count(self) -> int:
        return int(struct.unpack_from("<H", self.raw, self.COUNT_OFFSET)[0])

    def with_index(self, index: int) -> Zone:
        if not 0 <= index <= 0xFFFF:
            raise ValueError("分區編號超出範圍")
        return Zone(_replace(self.raw, 0, struct.pack("<H", index)))

    def with_layout(self, first_record: int, channel_count: int) -> Zone:
        if not 0 <= first_record <= 0xFFFF or not 0 <= channel_count <= 0xFFFF:
            raise ValueError("分區範圍超出 16-bit")
        packed = struct.pack("<HH", first_record, channel_count)
        return Zone(_replace(self.raw, self.FIRST_OFFSET, packed))


@dataclass(frozen=True)
class Channel:
    """32 bytes: index u16 | name[10] | frequency A u32 | frequency B u32 | 12 bytes raw.

    Both frequencies are little-endian Hz: A (+0x0C) is receive, B (+0x10) is
    transmit — confirmed 2026-10-08 by programming a channel with RX 439.1375 /
    TX 431.5375 in the legacy CPS and capturing the transfer.
    """

    raw: bytes
    SIZE = 32
    NAME_OFFSET = 2
    FREQ_A_OFFSET = 0x0C
    FREQ_B_OFFSET = 0x10
    # Contact number (Contact.index) called when transmitting. Confirmed 2026-10-08:
    # in all three real projects channel TG46600 -> contact #2 "TG46600_TW",
    # TG46621 -> #10 "TG46621_TW", TG9 -> #13 "TG9"; analog channels hold 1.
    TX_CONTACT_OFFSET = 0x1E

    def __post_init__(self) -> None:
        if len(self.raw) != self.SIZE:
            raise ValueError(f"Channel record must be {self.SIZE} bytes, got {len(self.raw)}")

    @property
    def index(self) -> int:
        return int(struct.unpack_from("<H", self.raw, 0)[0])

    def name(self, codec: NameCodec) -> str:
        return display_name(self.raw[self.NAME_OFFSET : self.NAME_OFFSET + NAME_SIZE], codec)

    @property
    def freq_a_hz(self) -> int:
        return int(struct.unpack_from("<I", self.raw, self.FREQ_A_OFFSET)[0])

    @property
    def freq_b_hz(self) -> int:
        return int(struct.unpack_from("<I", self.raw, self.FREQ_B_OFFSET)[0])

    def with_name(self, text: str, codec: NameCodec) -> Channel:
        field = encode_name(text, NAME_SIZE, codec)
        return Channel(_replace(self.raw, self.NAME_OFFSET, field))

    @property
    def tx_contact(self) -> int:
        return int(struct.unpack_from("<H", self.raw, self.TX_CONTACT_OFFSET)[0])

    def with_tx_contact(self, contact_index: int) -> Channel:
        if not 0 <= contact_index <= 0xFFFF:
            raise ValueError("聯絡人編號超出範圍")
        return Channel(_replace(self.raw, self.TX_CONTACT_OFFSET, struct.pack("<H", contact_index)))

    # Channel parser FUN_030315a8: +0x14 bits 7-6 = channel type (00 analog path,
    # 01/10/11 digital path with sub-mode 0/1/2), matching the legacy CPS's options
    # Analog / Digital / A&D TX-A / A&D TX-D; +0x15 bits 0-3 = color code. The other
    # bits are described in channel_fields.py (mapped against the legacy CPS grid).
    FLAGS_OFFSET = 0x14
    DIGITAL_OFFSET = 0x15
    MODES = {0: "類比", 1: "數位", 2: "類比+數位（發射類比）", 3: "類比+數位（發射數位）"}
    ANALOG = 0

    @property
    def mode(self) -> int:
        return self.raw[self.FLAGS_OFFSET] >> 6

    @property
    def mode_label(self) -> str:
        return self.MODES[self.mode]

    def with_mode(self, mode: int) -> Channel:
        if mode not in self.MODES:
            raise ValueError("頻道模式必須是 0–3")
        flags = (self.raw[self.FLAGS_OFFSET] & 0x3F) | (mode << 6)
        return Channel(_replace(self.raw, self.FLAGS_OFFSET, bytes([flags])))

    @property
    def color_code(self) -> int:
        return self.raw[self.DIGITAL_OFFSET] & 0x0F

    def with_color_code(self, cc: int) -> Channel:
        if not 0 <= cc <= 15:
            raise ValueError("色碼必須介於 0 與 15")
        byte = (self.raw[self.DIGITAL_OFFSET] & 0xF0) | cc
        return Channel(_replace(self.raw, self.DIGITAL_OFFSET, bytes([byte])))

    # RX group list number (0 = none): byte +0x17 is handed to the group loader
    # FUN_03031284; every digital channel in the real projects holds 1 = "MMDVM_TG".
    RX_GROUP_OFFSET = 0x17

    @property
    def rx_group(self) -> int:
        return self.raw[self.RX_GROUP_OFFSET]

    def with_rx_group(self, number: int) -> Channel:
        if not 0 <= number <= 32:
            raise ValueError("接收群組編號必須介於 0 與 32")
        return Channel(_replace(self.raw, self.RX_GROUP_OFFSET, bytes([number])))

    def with_index(self, index: int) -> Channel:
        if not 0 <= index <= 0xFFFF:
            raise ValueError("頻道編號超出範圍")
        return Channel(_replace(self.raw, 0, struct.pack("<H", index)))

    def with_freq_a(self, hz: int) -> Channel:
        return Channel(_replace(self.raw, self.FREQ_A_OFFSET, _u32(hz)))

    def with_freq_b(self, hz: int) -> Channel:
        return Channel(_replace(self.raw, self.FREQ_B_OFFSET, _u32(hz)))


def _u32(value: int) -> bytes:
    if not 0 <= value <= 0xFFFFFFFF:
        raise ValueError("頻率超出 32-bit 範圍")
    return struct.pack("<I", value)


def mhz_to_hz(text: str) -> int:
    """Parse a MHz string such as ``"438.8"`` into integer Hz, exactly."""
    from decimal import Decimal, InvalidOperation

    try:
        mhz = Decimal(text.strip())
        if not mhz.is_finite():
            raise InvalidOperation
        sign, digits, exponent = mhz.as_tuple()
        value = Decimal((sign, digits, exponent + 6))  # × 10^6, exact (`*` rounds to 28 digits)
    except InvalidOperation as exc:
        raise ValueError(f"無效頻率「{text}」") from exc
    if value != value.to_integral_value():
        raise ValueError("頻率解析度不可小於 1 Hz")
    if not -(2**63) <= value < 2**63:  # also keeps int() from expanding e.g. 1E999999999
        raise ValueError(f"無效頻率「{text}」")
    return int(value)


def hz_to_mhz(hz: int) -> str:
    return f"{hz / 1_000_000:.5f}"


# Readable aliases now that the field meaning is confirmed.
Channel.rx_hz = Channel.freq_a_hz  # type: ignore[attr-defined]
Channel.tx_hz = Channel.freq_b_hz  # type: ignore[attr-defined]
Channel.with_rx = Channel.with_freq_a  # type: ignore[attr-defined]
Channel.with_tx = Channel.with_freq_b  # type: ignore[attr-defined]
