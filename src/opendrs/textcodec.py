"""Fixed-width name fields and legacy text files, without silent character loss.

The legacy CPS wrote names through the Windows ANSI code page, so the same project
showed different (or ``?``-replaced) text depending on the PC's locale. Here every
conversion names its codec explicitly, refuses to drop characters, and keeps the
original bytes of names it could not decode so they survive a load/save cycle.
"""

from __future__ import annotations

import unicodedata
from enum import Enum

PAD_BYTES = (0x00, 0xFF)


class NameCodec(str, Enum):
    """Character set used for names stored inside the codeplug."""

    BIG5 = "big5"
    GBK = "gbk"
    ASCII = "ascii"

    @property
    def label(self) -> str:
        return {"big5": "Big5（繁體）", "gbk": "GBK（簡體）", "ascii": "ASCII"}[self.value]


class NameEncodingError(ValueError):
    """A name cannot be stored in the field with the chosen codec."""


def _strip_padding(raw: bytes) -> bytes:
    end = len(raw)
    for i, b in enumerate(raw):
        if b in PAD_BYTES:
            end = i
            break
    return raw[:end]


def decode_name(raw: bytes, codec: NameCodec) -> str | None:
    """Decode a padded name field; ``None`` when the bytes are not valid in ``codec``."""
    body = _strip_padding(raw)
    try:
        return body.decode(codec.value)
    except UnicodeDecodeError:
        return None


def encode_name(text: str, size: int, codec: NameCodec) -> bytes:
    """Encode ``text`` into a zero-padded field of ``size`` bytes, or raise."""
    if "\x00" in text:
        raise NameEncodingError("名稱不可包含 NUL 字元")
    try:
        body = text.encode(codec.value, errors="strict")
    except UnicodeEncodeError as exc:
        bad = text[exc.start : exc.end]
        raise NameEncodingError(f"「{bad}」無法以 {codec.label} 表示") from exc
    if len(body) > size:
        raise NameEncodingError(
            f"名稱需 {len(body)} bytes，超過欄位上限 {size} bytes（中文字每字 2 bytes）"
        )
    return body + bytes(size - len(body))


def display_name(raw: bytes, codec: NameCodec) -> str:
    """Human-readable form; undecodable bytes are shown as hex rather than ``?``.

    Control characters are rendered as visible symbols (CR → ␍, LF → ␊). The legacy
    CPS's CSV import wrote line breaks into contact names; they must be seen, not hidden.
    """
    text = decode_name(raw, codec)
    if text is not None:
        return "".join(chr(0x2400 + ord(ch)) if ord(ch) < 0x20 else ch for ch in text)
    return "‹" + _strip_padding(raw).hex(" ") + "›"


def fit_name(text: str, size: int, codec: NameCodec) -> tuple[bytes, str]:
    """Best-effort fit for imported names: returns ``(field, stored_text)``.

    Accents are folded (``José`` → ``Jose``), control and unrepresentable
    characters dropped, and the result cut at a character boundary so a CJK
    character is never split in half. Callers compare ``stored_text`` with the
    input to report what was changed.
    """
    folded = unicodedata.normalize("NFKD", text)
    out = bytearray()
    kept: list[str] = []
    for ch in folded:
        if unicodedata.combining(ch) or ord(ch) < 0x20 or ch == "\x7f":
            continue
        try:
            encoded = ch.encode(codec.value)
        except UnicodeEncodeError:
            continue
        if len(out) + len(encoded) > size:
            break
        out += encoded
        kept.append(ch)
    stored = "".join(kept).rstrip()
    body = stored.encode(codec.value)
    return body + bytes(size - len(body)), stored


# Big5 and GBK byte ranges overlap heavily: text in one usually decodes *without
# error* in the other, just as different characters. Picking "the first codec that
# decodes" is exactly how the legacy CPS produced garbled names. Instead, decode
# with both and keep the result made of more everyday characters.
_COMMON_HANZI = frozenset(
    "的一是不了人我在有他這这中大來来上國国個个到說说們们為为子和你地出道也時时年得就那要下以"
    "生會会自著着去之過过家學学對对可裡里後后小麼么心多天而能好都然沒没日於于起還还發发成事只作"
    "當当想看文無无開开手十用主行方又如前所本見见經经頭头面公同三已老從从動动兩两長长知民樣样現现"
    "分將将外但身些與与高意進进把法此實实回二理美點点月明其種种聲声全工己話话兒儿者向情部正名定女"
    "問问力機机給给等幾几很業业最間间新什打便位因重被走電电四第門门相次東东政海口使教西再平真聽听"
    "世氣气信北少關关並并內内加化由卻却代軍军產产入先山五太水萬万市眼體体別别處处總总才場场師师書书"
    "比住員员九笑性通目華华報报立馬马命張张活難难神數数件安表原車车白應应路期叫死常提感金何更反合"
    "放做系計计或司利受光王果親亲界及今京務务制解各任至清物台臺象記记邊边共風风戰战干接條条該该達达"
    "城半跟形認认音保收術术約约連连團团必流管設设建交且區区族友指義义科類类備备頻频率碼码號号道組组"
    "聯联絡络錄录掃扫描語语檔档線线講讲單单選选項项列表訊讯息呼叫鍵键鎖锁靜静噪功池量時区域網网版"
)


def _hanzi_score(text: str) -> float:
    cjk = [ch for ch in text if "㐀" <= ch <= "鿿"]
    if not cjk:
        return 0.0
    return sum(ch in _COMMON_HANZI for ch in cjk) / len(cjk)


def best_cjk_decoding(data: bytes) -> tuple[str, str] | None:
    """Decode ``data`` as Big5 or GBK, whichever yields more common characters."""
    results: list[tuple[float, str, str]] = []
    for enc in ("big5", "gbk"):
        try:
            text = data.decode(enc)
        except UnicodeDecodeError:
            continue
        results.append((_hanzi_score(text), enc, text))
    if not results:
        return None
    score, enc, text = max(results, key=lambda r: r[0])
    return text, enc


def suggest_codec(fields: list[bytes]) -> NameCodec | None:
    """Guess the codec of a set of name fields; ``None`` if all are plain ASCII."""
    blob = b" ".join(_strip_padding(f) for f in fields if any(b >= 0x80 for b in f))
    if not blob:
        return None
    best = best_cjk_decoding(blob)
    return NameCodec(best[1]) if best else None


def read_legacy_text(data: bytes) -> tuple[str, str]:
    """Decode a legacy config/help file, returning ``(text, encoding_used)``."""
    if data.startswith(b"\xff\xfe") or data.startswith(b"\xfe\xff"):
        try:
            return data.decode("utf-16"), "utf-16"
        except UnicodeDecodeError:
            pass  # not UTF-16 after all (e.g. a cp1252 file starting with "ÿþ"): try the others
    try:
        return data.decode("utf-8-sig"), "utf-8-sig"
    except UnicodeDecodeError:
        pass
    best = best_cjk_decoding(data)
    if best is not None:
        return best
    try:
        return data.decode("cp1252"), "cp1252"
    except UnicodeDecodeError:
        return data.decode("latin-1"), "latin-1"
