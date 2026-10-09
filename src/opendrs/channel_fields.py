"""Channel record bit fields, as the legacy CPS channel grid edits them.

Offsets are within the 32-byte channel record. Mapped by driving the legacy CPS grid
(legacy_probe/probe_grid.py) on a real project, one option at a time:

* +0x14: bits 7-6 channel type, bit 5 power (高), bit 2 call indicator, bit 1
  private-call type (Oacsu), bit 0 together with +0x1D bits 0-2 the 時隙 choice;
* +0x15: bits 0-3 color code (repeated in +0x1D bits 4-7), bit 4 confirmed data,
  bits 6-5 TX policy;
* +0x16 encryption key number, +0x17 RX group list, +0x18 scan list, +0x19 emergency
  alarm list (緊急系統), +0x1E TX contact (records.py);
* +0x1A squelch types (bits 0-1 RX, bits 2-3 TX: 0 off, 1 CTCSS, 2 DCS normal,
  3 DCS inverted), +0x1B RX code, +0x1C TX code — an index into the legacy CPS's
  CTCSS (51 tones from 62.5 Hz) or DCS (108 codes from 017) list.
"""

from __future__ import annotations

from .radio_settings import OFF_ON, Setting, choices
from .records import Channel

PAGE = "頻道"


def _field(key: str, label: str, offset: int, mask: int, opts: tuple[tuple[str, int], ...]) -> Setting:
    return Setting(key, PAGE, label, offset, mask, opts)


POWER = _field("power", "功率", 0x14, 0x20, choices("低", "高"))
CALL_INDICATOR = _field("call_indicator", "呼叫指示", 0x14, 0x04, OFF_ON)
PRIVATE_CALL_TYPE = _field("private_call_type", "個呼類型", 0x14, 0x02, choices("Patcs", "Oacsu"))
MESSAGE_TYPE = _field("message_type", "短信類型", 0x15, 0x10, choices("非確認數據", "確認數據"))
# Raw 0 appears in projects written by older tools; the legacy grid shows it as Impolite.
TX_POLICY = _field("tx_policy", "發射權限", 0x15, 0x60,
                   (("Impolite", 1), ("Polite to CC", 2), ("Polite to ALL", 3), ("Impolite（舊值 0）", 0)))

FIELDS = (POWER, CALL_INDICATOR, PRIVATE_CALL_TYPE, MESSAGE_TYPE, TX_POLICY)

# 時隙: (bit 0 of +0x14, bits 0-2 of +0x1D) for each legacy option.
SLOT_CHOICES = (("On", (0, 0)), ("時隙1", (0, 5)), ("時隙2", (1, 7)))
SLOT_LABELS = tuple(label for label, _ in SLOT_CHOICES)


def get(channel: Channel, field: Setting) -> int:
    return (channel.raw[field.offset] & field.mask) >> field.shift


def put(channel: Channel, field: Setting, raw: int) -> Channel:
    return Channel(field.with_raw(channel.raw, raw))


def slot(channel: Channel) -> int | None:
    """Index into SLOT_CHOICES, or None for a combination the legacy CPS never writes."""
    state = (channel.raw[0x14] & 1, channel.raw[0x1D] & 7)
    return next((i for i, (_, bits) in enumerate(SLOT_CHOICES) if bits == state), None)


def slot_label(channel: Channel) -> str:
    index = slot(channel)
    return SLOT_LABELS[index] if index is not None else f"未知（{channel.raw[0x14] & 1}/{channel.raw[0x1D] & 7}）"


def with_slot(channel: Channel, index: int) -> Channel:
    if not 0 <= index < len(SLOT_CHOICES):
        raise ValueError("時隙選項無效")
    bit0, low3 = SLOT_CHOICES[index][1]
    raw = bytearray(channel.raw)
    raw[0x14] = (raw[0x14] & 0xFE) | bit0
    raw[0x1D] = (raw[0x1D] & 0xF8) | low3
    return Channel(bytes(raw))


def with_color_code(channel: Channel, cc: int) -> Channel:
    """The legacy CPS writes the color code twice: +0x15 low and +0x1D high nibble."""
    updated = channel.with_color_code(cc)
    raw = bytearray(updated.raw)
    raw[0x1D] = (raw[0x1D] & 0x0F) | (cc << 4)
    return Channel(bytes(raw))


def encryption_key(channel: Channel) -> int:
    return channel.raw[0x16]


def with_encryption_key(channel: Channel, number: int) -> Channel:
    if not 0 <= number <= 10:
        raise ValueError("加密列表編號必須介於 0 與 10")
    raw = bytearray(channel.raw)
    raw[0x16] = number
    return Channel(bytes(raw))


# ---------------------------------------------------------------- lists
SCAN_LIST = Setting("scan_list", PAGE, "掃描列表", 0x18, 0x3F, maximum=16)
EMERGENCY = Setting("emergency", PAGE, "緊急系統", 0x19, 0xFF, maximum=4)

# ---------------------------------------------------------------- CTCSS / DCS
CTCSS = ('62.5', '67.0', '69.3', '71.9', '74.4', '77.0', '79.7', '82.5', '85.4', '88.5', '91.5', '94.8', '97.4', '100.0', '103.5', '107.2', '110.9', '114.8', '118.8', '123.0', '127.3', '131.8', '136.5', '141.3', '146.2', '151.4', '156.7', '159.8', '162.2', '165.5', '167.9', '171.3', '173.8', '177.3', '179.9', '183.5', '186.2', '189.9', '192.8', '196.6', '199.5', '203.5', '206.5', '210.7', '218.1', '225.7', '229.1', '233.6', '241.8', '250.3', '254.1')
DCS = ('017', '023', '025', '026', '031', '032', '036', '043', '047', '050', '051', '053', '054', '065', '071', '072', '073', '074', '114', '115', '116', '122', '125', '131', '132', '134', '143', '145', '152', '155', '156', '162', '165', '172', '174', '205', '212', '223', '225', '226', '243', '244', '245', '246', '251', '252', '255', '261', '263', '265', '266', '271', '274', '306', '311', '315', '325', '331', '332', '343', '346', '351', '356', '364', '365', '371', '411', '412', '413', '423', '431', '432', '445', '446', '452', '454', '455', '462', '464', '465', '466', '503', '506', '516', '523', '526', '532', '546', '565', '606', '612', '624', '627', '631', '632', '645', '646', '654', '662', '664', '703', '712', '723', '731', '732', '734', '743', '754')
TONE_TYPE_OFFSET, RX_CODE_OFFSET, TX_CODE_OFFSET = 0x1A, 0x1B, 0x1C
OFF = 0


def tone_choices() -> list[tuple[str, int]]:
    """All legacy options as (label, value) with value = type << 8 | code."""
    out = [("Off", 0)]
    out += [(f"{hz} Hz", 1 << 8 | i) for i, hz in enumerate(CTCSS)]
    out += [(f"D{code}N", 2 << 8 | i) for i, code in enumerate(DCS)]
    out += [(f"D{code}I", 3 << 8 | i) for i, code in enumerate(DCS)]
    return out


def tone_label(value: int) -> str:
    kind, code = value >> 8, value & 0xFF
    table = {1: CTCSS, 2: DCS, 3: DCS}.get(kind)
    if kind == 0:
        return "Off"
    if table is None or code >= len(table):
        return f"未知（{kind}/{code}）"
    return f"{table[code]} Hz" if kind == 1 else f"D{table[code]}{'N' if kind == 2 else 'I'}"


def tone(channel: Channel, tx: bool) -> int:
    kind = (channel.raw[TONE_TYPE_OFFSET] >> (2 if tx else 0)) & 3
    code = channel.raw[TX_CODE_OFFSET if tx else RX_CODE_OFFSET]
    return kind << 8 | (code if kind else 0)


def with_tone(channel: Channel, tx: bool, value: int) -> Channel:
    kind, code = value >> 8, value & 0xFF
    if value not in {v for _, v in tone_choices()}:
        raise ValueError("亞音選項無效")
    raw = bytearray(channel.raw)
    shift = 2 if tx else 0
    raw[TONE_TYPE_OFFSET] = (raw[TONE_TYPE_OFFSET] & ~(3 << shift) & 0xFF) | (kind << shift)
    raw[TX_CODE_OFFSET if tx else RX_CODE_OFFSET] = code
    return Channel(bytes(raw))
