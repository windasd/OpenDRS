"""Export / import the channel list as CSV (one row per channel, zone by zone).

Export: UTF-8 with a BOM and CRLF line ends (opens cleanly in Excel). Every value is
written the way the channel table shows it; references (TX contact, RX group, encryption
key, scan list, alarm list) as "number name".

Import updates the channels the CSV names and appends new ones; nothing is deleted:
* a row is located by 分區 (zone number) + 頻道 (position in that zone);
* 頻道 empty or past the end of the zone -> a new channel at the end of that zone,
  copied from the zone's last channel and then given the row's values;
* only columns present in the file are applied; blank cells keep the current value
  (a new channel needs a name);
* a row with any invalid cell is skipped as a whole, with the reason reported, so a
  channel is never left half-updated. Frequencies outside the radio's bands are rejected.
References may be given as a number ("12", "12 TG46621") or by exact name.
"""

from __future__ import annotations

import csv
import io
import re
from collections.abc import Callable
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation

from . import channel_fields as cf
from .alarm_enc import alarm_lists, encryption_keys
from .device import frequency_problem
from .f136 import Project
from .layout import LayoutError, add_channel, check_layout, zone_channel_rows
from .records import Channel, hz_to_mhz, mhz_to_hz
from .rxgroup import rx_groups
from .scanlist import scan_lists
from .textcodec import NameCodec, read_legacy_text

_NONE = {"", "0", "無", "（無）", "(無)", "none", "off"}


class CellError(ValueError):
    pass


# ---------------------------------------------------------------- references
def _ref_options(project: Project, key: str, codec: NameCodec) -> list[tuple[int, str]]:
    if key == "contact":
        return [(c.index, c.name(codec)) for c in project.contacts]
    if key == "group":
        return [(i, g.name(codec)) for i, g in enumerate(rx_groups(project.image), start=1)]
    if key == "key":
        return [(i, k.alias(codec)) for i, k in enumerate(encryption_keys(project.image), start=1)]
    if key == "scan":
        return [(i, s.name(codec)) for i, s in enumerate(scan_lists(project.image), start=1)]
    return [(i, a.name(codec)) for i, a in enumerate(alarm_lists(project.image), start=1)]


def _ref_text(project: Project, key: str, number: int, codec: NameCodec) -> str:
    if number == 0:
        return ""
    name = dict(_ref_options(project, key, codec)).get(number)
    return f"{number} {name}" if name is not None else str(number)


def _ref_parse(project: Project, key: str, text: str, codec: NameCodec) -> int:
    t = text.strip()
    if t.casefold() in _NONE:
        return 0
    options = _ref_options(project, key, codec)
    names = dict(options)
    hits = [n for n, name in options if name.strip() == t]
    m = re.match(r"^#?(\d+)(?:\s+(.*))?$", t)
    if m and m.group(2) is None:  # a bare number
        if int(m.group(1)) not in names:
            raise CellError(f"編號 {m.group(1)} 不存在")
        return int(m.group(1))
    if m and int(m.group(1)) in names and names[int(m.group(1))].strip() == m.group(2).strip():
        return int(m.group(1))  # "number name", as exported
    if len(hits) == 1:
        return hits[0]
    if hits:
        raise CellError(f"「{t}」不只一個，請改用編號")
    if m and int(m.group(1)) in names:
        raise CellError(f"編號 {m.group(1)} 是「{names[int(m.group(1))].strip()}」，不是「{m.group(2).strip()}」")
    raise CellError(f"編號 {m.group(1)} 不存在" if m else f"找不到「{t}」")


# ---------------------------------------------------------------- simple values
def _choice(text: str, options: list[tuple[str, int]], extra: dict[str, int] | None = None) -> int:
    t = text.strip().casefold()
    for label, value in options:
        if label.casefold() == t:
            return value
    if extra and t in extra:
        return extra[t]
    if t.isdecimal() and int(t) in {v for _, v in options}:  # not isdigit: int("²") raises
        return int(t)
    raise CellError(f"無效的選項「{text.strip()}」（可用：{'、'.join(label for label, _ in options)}）")


_MODE_ALIASES = {"analog": 0, "fm": 0, "a": 0, "digital": 1, "dmr": 1, "d": 1}
_SLOT_ALIASES = {"ts1": 1, "slot1": 1, "1": 1, "ts2": 2, "slot2": 2, "2": 2}


def _tone_parse(text: str) -> int:
    t = text.strip().upper().replace(" ", "")
    if t.casefold() in _NONE:
        return 0
    m = re.match(r"^D?(\d{3})([NI]?)$", t)
    if m and (t.startswith("D") or m.group(2)):
        if m.group(1) not in cf.DCS:
            raise CellError(f"沒有 DCS {m.group(1)}")
        return (3 if m.group(2) == "I" else 2) << 8 | cf.DCS.index(m.group(1))
    try:
        hz = Decimal(t.removesuffix("HZ"))
    except InvalidOperation as exc:
        raise CellError(f"無效的亞音「{text.strip()}」") from exc
    if not hz.is_finite():
        raise CellError(f"無效的亞音「{text.strip()}」")
    for i, ctcss in enumerate(cf.CTCSS):
        if Decimal(ctcss) == hz:
            return 1 << 8 | i
    raise CellError(f"沒有 CTCSS {text.strip()}")


def _freq(project: Project, text: str) -> int:
    try:
        hz = mhz_to_hz(re.sub(r"\s*mhz$", "", text.strip(), flags=re.IGNORECASE))
    except ValueError as exc:
        raise CellError(str(exc)) from exc
    if hz == 0:
        raise CellError("頻率不可為 0")
    problem = frequency_problem(hz, project.device.bands)
    if problem:
        raise CellError(problem)
    return hz


# ---------------------------------------------------------------- columns
Get = Callable[[Project, Channel, NameCodec], str]
Put = Callable[[Project, Channel, str, NameCodec], Channel]


@dataclass(frozen=True)
class Field:
    key: str
    header: str
    get: Get
    put: Put
    aliases: tuple[str, ...] = ()


def _setting(field: cf.Setting, key: str, *aliases: str) -> Field:
    return Field(key, field.label, lambda p, ch, c: field.display(ch.raw),
                 lambda p, ch, t, c: cf.put(ch, field, _choice(t, list(field.choices))), aliases)


def _ref(key: str, header: str, get_number: Callable[[Channel], int], set_number: Callable[[Channel, int], Channel],
         *aliases: str) -> Field:
    return Field(key, header, lambda p, ch, c: _ref_text(p, key, get_number(ch), c),
                 lambda p, ch, t, c: set_number(ch, _ref_parse(p, key, t, c)), aliases)


def _name(project: Project, ch: Channel, text: str, codec: NameCodec) -> Channel:
    try:
        return ch.with_name(text.strip(), codec)
    except ValueError as exc:  # includes NameEncodingError
        raise CellError(str(exc)) from exc


def _value(fn: Callable[[], Channel]) -> Channel:
    try:
        return fn()
    except CellError:
        raise
    except ValueError as exc:
        raise CellError(str(exc)) from exc


FIELDS: tuple[Field, ...] = (
    Field("name", "名稱", lambda p, ch, c: ch.name(c), _name, ("name", "channelname", "頻道名稱")),
    Field("rx", "接收MHz", lambda p, ch, c: hz_to_mhz(ch.rx_hz),
          lambda p, ch, t, c: ch.with_rx(_freq(p, t)), ("rx", "rxfreq", "接收", "接收頻率")),
    Field("tx", "發射MHz", lambda p, ch, c: hz_to_mhz(ch.tx_hz),
          lambda p, ch, t, c: ch.with_tx(_freq(p, t)), ("tx", "txfreq", "發射", "發射頻率")),
    Field("mode", "模式", lambda p, ch, c: ch.mode_label,
          lambda p, ch, t, c: _value(lambda: ch.with_mode(
              _choice(t, [(v, k) for k, v in Channel.MODES.items()], _MODE_ALIASES))), ("mode",)),
    Field("cc", "色碼", lambda p, ch, c: str(ch.color_code),
          lambda p, ch, t, c: _value(lambda: cf.with_color_code(ch, _choice(t, [(str(i), i) for i in range(16)]))),
          ("cc", "colorcode", "色馬")),
    Field("slot", "時隙", lambda p, ch, c: cf.slot_label(ch),
          lambda p, ch, t, c: cf.with_slot(ch, _choice(t, [(label, i) for i, label in enumerate(cf.SLOT_LABELS)],
                                                       _SLOT_ALIASES)), ("slot", "timeslot", "ts")),
    _setting(cf.POWER, "power", "power"),
    _setting(cf.CALL_INDICATOR, "call_indicator", "callindicator"),
    _setting(cf.PRIVATE_CALL_TYPE, "private_call_type", "privatecalltype"),
    _setting(cf.MESSAGE_TYPE, "message_type", "messagetype"),
    _setting(cf.TX_POLICY, "tx_policy", "txpolicy", "admit"),
    _ref("contact", "發話對象", lambda ch: ch.tx_contact, lambda ch, n: ch.with_tx_contact(n), "contact", "txcontact"),
    _ref("group", "接收群組", lambda ch: ch.rx_group, lambda ch, n: ch.with_rx_group(n), "rxgroup", "grouplist"),
    _ref("key", "加密列表", cf.encryption_key, cf.with_encryption_key, "encryption", "key"),
    _ref("scan", "掃描列表", lambda ch: cf.get(ch, cf.SCAN_LIST), lambda ch, n: cf.put(ch, cf.SCAN_LIST, n),
         "scanlist", "scan"),
    _ref("alarm", "緊急系統", lambda ch: cf.get(ch, cf.EMERGENCY), lambda ch, n: cf.put(ch, cf.EMERGENCY, n),
         "emergency", "alarm"),
    Field("rx_tone", "接收亞音", lambda p, ch, c: _tone_out(cf.tone(ch, False)),
          lambda p, ch, t, c: cf.with_tone(ch, False, _tone_parse(t)), ("rxtone", "rxctcss", "接收亞音頻")),
    Field("tx_tone", "發射亞音", lambda p, ch, c: _tone_out(cf.tone(ch, True)),
          lambda p, ch, t, c: cf.with_tone(ch, True, _tone_parse(t)), ("txtone", "txctcss", "發射亞音頻")),
)
LOCATE = (("zone", "分區", ("zone", "zoneno", "分區編號", "区域")), ("zone_name", "分區名稱", ("zonename",)),
          ("ch", "頻道", ("ch", "channel", "no", "頻道編號", "位置")))
HEADER = tuple(h for _, h, _ in LOCATE) + tuple(f.header for f in FIELDS)


def _tone_out(value: int) -> str:
    return "" if value == 0 else cf.tone_label(value)


def _norm(header: str) -> str:
    h = header.strip().casefold().replace(" ", "").replace("_", "")
    for junk in ("(mhz)", "（mhz）"):
        h = h.replace(junk, "mhz")
    return h


def _column_map(header: list[str]) -> dict[str, int]:
    names: dict[str, str] = {}
    for key, title, aliases in LOCATE:
        for a in (title, *aliases):
            names[_norm(a)] = key
    for f in FIELDS:
        for a in (f.header, f.key, *f.aliases):
            names[_norm(a)] = f.key
    out: dict[str, int] = {}
    for i, h in enumerate(header):
        key = names.get(_norm(h)) or names.get(_norm(h).removesuffix("mhz"))
        if key and key not in out:
            out[key] = i
    return out


# ---------------------------------------------------------------- export
def export_channels(project: Project, codec: NameCodec) -> bytes:
    """Raises LayoutError if the zones do not tile the channel list."""
    check_layout(project)
    buf = io.StringIO(newline="")
    writer = csv.writer(buf, lineterminator="\r\n")
    writer.writerow(HEADER)
    for zone_row, zone in enumerate(project.zones):
        for position, row in enumerate(zone_channel_rows(project, zone_row), start=1):
            ch = project.channels[row]
            writer.writerow([zone_row + 1, zone.name(codec), position] + [f.get(project, ch, codec) for f in FIELDS])
    return buf.getvalue().encode("utf-8-sig")


# ---------------------------------------------------------------- import
@dataclass(frozen=True)
class ChannelImport:
    project: Project
    updated: int
    added: int
    unchanged: int
    errors: tuple[str, ...]
    encoding: str

    def summary(self) -> str:
        parts = [f"更新 {self.updated} 個頻道", f"新增 {self.added} 個", f"未變動 {self.unchanged} 個"]
        if self.errors:
            parts.append(f"{len(self.errors)} 列略過")
        return "，".join(parts)


def _apply_row(project: Project, ch: Channel, row: list[str], columns: dict[str, int], codec: NameCodec,
               line: int, new: bool) -> tuple[Channel, list[str]]:
    errors = []
    for f in FIELDS:
        i = columns.get(f.key)
        if i is None or i >= len(row) or not row[i].strip():
            if new and f.key == "name":
                errors.append(f"第 {line} 列：新頻道需要名稱")
            continue
        if not new and row[i].strip() == f.get(project, ch, codec).strip():
            continue
        try:
            ch = f.put(project, ch, row[i], codec)
        except (CellError, ValueError) as exc:
            errors.append(f"第 {line} 列「{f.header}」：{exc}")
    return ch, errors


def _cells(row: list[str], columns: dict[str, int]) -> Callable[[str], str]:
    return lambda key: row[columns[key]].strip() if key in columns and columns[key] < len(row) else ""


def _read_rows(text: str) -> list[list[str]]:
    """CSV rows; a file whose header is one cell holding ';' or tabs uses that delimiter."""
    rows = list(csv.reader(io.StringIO(text, newline="")))
    if rows and len(rows[0]) == 1:
        for delimiter in (";", "\t"):
            if delimiter in rows[0][0]:
                return list(csv.reader(io.StringIO(text, newline=""), delimiter=delimiter))
    return rows


def _same_channel(project: Project, rows: range, channel: Channel, codec: NameCodec) -> int:
    """Position (1-based) of a channel in ``rows`` with the same name and frequencies, or 0."""
    for position, row in enumerate(rows, start=1):
        other = project.channels[row]
        if (other.name(codec), other.rx_hz, other.tx_hz) == (channel.name(codec), channel.rx_hz, channel.tx_hz):
            return position
    return 0


def _failed(project: Project, message: str, encoding: str) -> ChannelImport:
    return ChannelImport(project, 0, 0, 0, (message,), encoding)


def import_channels(project: Project, data: bytes, codec: NameCodec) -> ChannelImport:
    """Apply a channel CSV to ``project`` (see the module docstring); never raises on bad content.

    Positions refer to the zones as they were before the import: rows naming an existing
    position update that channel (once per file), everything else is appended in file order.
    """
    text, encoding = read_legacy_text(data)
    try:
        check_layout(project)
        rows = _read_rows(text)
    except LayoutError as exc:
        return _failed(project, f"專案的分區配置有誤，無法匯入：{exc}", encoding)
    except csv.Error as exc:
        return _failed(project, f"無法解析 CSV：{exc}", encoding)
    if not rows:
        return _failed(project, "檔案是空的", encoding)
    columns = _column_map(rows[0])
    missing = [title for key, title, _ in LOCATE if key in ("zone", "ch") and key not in columns]
    if missing:
        return _failed(project, f"缺少欄位：{'、'.join(missing)}", encoding)
    original_len = [len(zone_channel_rows(project, z)) for z in range(len(project.zones))]
    seen: dict[tuple[int, int], int] = {}
    current, errors = project, []
    updated = added = unchanged = 0
    for line, row in enumerate(rows[1:], start=2):
        if not any(cell.strip() for cell in row):
            continue
        cell = _cells(row, columns)
        zone_text, ch_text = cell("zone"), cell("ch")
        if not (zone_text.isdecimal() and 1 <= int(zone_text) <= len(project.zones)):
            errors.append(f"第 {line} 列：分區「{zone_text}」不存在（1–{len(project.zones)}）")
            continue
        zone_row = int(zone_text) - 1
        zone_name = project.zones[zone_row].name(codec).strip()
        if cell("zone_name") and cell("zone_name") != zone_name:
            errors.append(f"第 {line} 列：分區名稱「{cell('zone_name')}」與分區 {zone_row + 1}「{zone_name}」不符"
                          "（匯入不會改分區名稱；若分區順序變了，請更正分區編號）")
            continue
        if ch_text and not ch_text.isdecimal():
            errors.append(f"第 {line} 列：頻道「{ch_text}」不是數字")
            continue
        rows_in_zone = zone_channel_rows(current, zone_row)
        position = int(ch_text) if ch_text else 0
        if 1 <= position <= original_len[zone_row]:
            if (zone_row, position) in seen:
                errors.append(f"第 {line} 列：分區 {zone_row + 1} 第 {position} 台已由第 {seen[zone_row, position]} 列"
                              "修改，同一頻道只能出現一次")
                continue
            seen[zone_row, position] = line
            target = rows_in_zone[position - 1]  # appends only go after the original channels
            original = current.channels[target]
            channel, row_errors = _apply_row(current, original, row, columns, codec, line, new=False)
            if row_errors:
                errors.extend(row_errors)
            elif channel == original:
                unchanged += 1
            else:
                current = current.with_channel(target, channel)
                updated += 1
            continue
        if not len(rows_in_zone):
            errors.append(f"第 {line} 列：分區 {zone_row + 1} 沒有頻道可當新頻道的範本")
            continue
        template = current.channels[rows_in_zone[-1]]
        channel, row_errors = _apply_row(current, template, row, columns, codec, line, new=True)
        if row_errors:
            errors.extend(row_errors)
            continue
        twin = _same_channel(current, rows_in_zone, channel, codec)
        if twin:
            errors.append(f"第 {line} 列：分區 {zone_row + 1} 第 {twin} 台已有相同名稱與頻率的頻道，"
                          "略過以免重複新增")
            continue
        try:
            current, new_row = add_channel(current, rows_in_zone[-1])
        except LayoutError as exc:
            errors.append(f"第 {line} 列：{exc}")
            continue
        current = current.with_channel(new_row, channel.with_index(current.channels[new_row].index))
        added += 1
    return ChannelImport(current, updated, added, unchanged, tuple(errors), encoding)
