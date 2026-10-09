from __future__ import annotations

import pytest

from opendrs.textcodec import (
    NameCodec,
    NameEncodingError,
    decode_name,
    display_name,
    encode_name,
    read_legacy_text,
    suggest_codec,
)


@pytest.mark.parametrize("codec, text", [(NameCodec.BIG5, "臺北中繼"), (NameCodec.GBK, "台北中继")])
def test_cjk_round_trip(codec: NameCodec, text: str) -> None:
    field = encode_name(text, 10, codec)
    assert len(field) == 10
    assert decode_name(field, codec) == text


def test_padding_stops_at_zero_or_ff() -> None:
    assert decode_name(b"AB\x00CD", NameCodec.ASCII) == "AB"
    assert decode_name(b"AB\xff\xff", NameCodec.ASCII) == "AB"


def test_unrepresentable_character_raises_instead_of_question_mark() -> None:
    with pytest.raises(NameEncodingError, match="無法以"):
        encode_name("中", 10, NameCodec.ASCII)


def test_nul_is_rejected() -> None:
    with pytest.raises(NameEncodingError):
        encode_name("a\x00b", 10, NameCodec.ASCII)


def test_undecodable_bytes_display_as_hex() -> None:
    raw = bytes([0x81, 0x30, 0, 0])
    assert decode_name(raw, NameCodec.BIG5) is None
    assert display_name(raw, NameCodec.BIG5) == "‹81 30›"


def test_codec_labels() -> None:
    assert {c.label for c in NameCodec} == {"Big5（繁體）", "GBK（簡體）", "ASCII"}


@pytest.mark.parametrize(
    "data, enc",
    [
        ("設定".encode("utf-8-sig"), "utf-8-sig"),
        ("設備機型".encode("big5"), "big5"),
        ("设备机型菜单".encode("gbk"), "gbk"),
        ("abc".encode("utf-16"), "utf-16"),
    ],
)
def test_legacy_text_detection(data: bytes, enc: str) -> None:
    text, used = read_legacy_text(data)
    assert used == enc
    assert text == data.decode(enc)


@pytest.mark.parametrize(
    "text, enc",
    [
        ("接收頻率起始", "big5"),
        ("接收频率起始", "gbk"),
        ("讀寫頻密碼", "big5"),
        ("读写频密码", "gbk"),
        ("中繼台設定", "big5"),
        ("中继台设置", "gbk"),
    ],
)
def test_cross_decodable_text_picks_the_right_codec(text: str, enc: str) -> None:
    other = "gbk" if enc == "big5" else "big5"
    data = text.encode(enc)
    try:
        data.decode(other)
    except UnicodeDecodeError:
        pass  # not ambiguous for this sample; detection must still be right
    decoded, used = read_legacy_text(data)
    assert (decoded, used) == (text, enc)


def test_suggest_codec_for_name_fields() -> None:
    big5_fields = [encode_name(t, 10, NameCodec.BIG5) for t in ("台北中繼", "高雄")]
    gbk_fields = [encode_name(t, 10, NameCodec.GBK) for t in ("台北中继", "高雄")]
    assert suggest_codec(big5_fields) is NameCodec.BIG5
    assert suggest_codec(gbk_fields) is NameCodec.GBK
    assert suggest_codec([b"MMDVM\x00\x00\x00\x00\x00"]) is None


def test_legacy_text_falls_back_without_raising() -> None:
    text, used = read_legacy_text(bytes([0x81, 0xFF, 0x81]))
    assert used in ("cp1252", "latin-1") and text


def test_bad_utf16_falls_back_to_other_encodings() -> None:
    assert read_legacy_text(b"\xff\xfe" + "中".encode("utf-16-le")) == ("中", "utf-16")
    assert read_legacy_text(b"\xff\xfe\x00\x00odd") == ("ÿþ\x00\x00odd", "cp1252")  # odd length
    assert read_legacy_text(b"\xfe\xff\xd8\x00") == ("þÿØ\x00", "cp1252")  # lone surrogate
