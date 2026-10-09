from __future__ import annotations

from pathlib import Path

import pytest

from opendrs import f136
from opendrs.radio_settings import (
    SETTINGS,
    Setting,
    SettingError,
    TextSetting,
    by_key,
    by_page,
    choices,
    pages,
)

FIX = Path(__file__).parent / "fixtures"

BACKLIGHT = Setting("t_backlight", "T", "背光", 0x10, 0x28, (("Off", 0), ("On", 1), ("Auto", 4)))
TOT = Setting("t_tot", "T", "限時發射", 0x11, 0xFF, minimum=0, maximum=500, scale=10, unit="S")
WORD = Setting("t_word", "T", "數值", 0x12, 0x0FF0, minimum=0, maximum=255, size=2)


def test_masked_choice_keeps_neighbouring_bits() -> None:
    image = bytes(0x10) + bytes([0b0101_0111]) + bytes(8)
    assert BACKLIGHT.display(image) == "Off"
    on = BACKLIGHT.with_raw(image, 1)
    assert on[0x10] == 0b0101_1111 and BACKLIGHT.display(on) == "On"
    auto = BACKLIGHT.with_raw(on, 4)
    assert auto[0x10] == 0b0111_0111 and BACKLIGHT.display(auto) == "Auto"
    assert auto[:0x10] == image[:0x10] and auto[0x11:] == image[0x11:]
    with pytest.raises(SettingError, match="有效"):
        BACKLIGHT.with_raw(image, 2)
    with pytest.raises(SettingError, match="範圍"):
        BACKLIGHT.with_raw(image, 99)


def test_scaled_numbers_and_multibyte() -> None:
    image = bytes(0x20)
    tot = TOT.with_value(image, 60)
    assert tot[0x11] == 6 and TOT.display(tot) == "60S"
    with pytest.raises(SettingError, match="倍數"):
        TOT.with_value(image, 65)
    with pytest.raises(SettingError, match="介於"):
        TOT.with_value(image, 510)
    word = WORD.with_raw(bytes(0x12) + b"\x0f\xf0" + bytes(10), 0xAB)
    assert word[0x12:0x14] == b"\xbf\xfa" and WORD.raw(word) == 0xAB


def test_short_image_is_reported() -> None:
    assert TOT.raw(b"") is None and TOT.display(b"") == "—"
    with pytest.raises(SettingError, match="太短"):
        TOT.with_raw(b"", 1)


def test_choices_helper() -> None:
    assert choices("a", "b", start=1) == (("a", 1), ("b", 2))


def test_declared_settings_are_consistent() -> None:
    keys = [s.key for s in SETTINGS]
    assert len(keys) == len(set(keys))
    project = f136.load(FIX / "dh98vu_20250811.F136")
    for s in SETTINGS:
        assert s.page in pages() and s in by_page(s.page) and by_key(s.key) is s
        if isinstance(s, TextSetting):
            text = s.text(project.image)
            assert text is not None and s.with_text(project.image, text) == project.image, s.key
            continue
        raw = s.raw(project.image)
        assert raw is not None, s.key
        if s.is_choice:
            values = [v for _, v in s.choices]
            assert len(values) == len(set(values)), s.key
            assert all(0 <= v <= s.mask >> s.shift for v in values), s.key
        # Writing the current value back changes nothing.
        assert s.with_raw(project.image, raw) == project.image or not s.is_choice or raw not in {
            v for _, v in s.choices}
    with pytest.raises(KeyError):
        by_key("nope")


def test_quick_messages() -> None:
    from opendrs.quickmsg import QUICK_MSG_OFFSET, quick_messages, with_quick_message
    from opendrs.textcodec import NameCodec, NameEncodingError

    image = bytes(QUICK_MSG_OFFSET) + b"CHECK!" + bytes(640 - 6) + b"\xaa"
    assert quick_messages(image, NameCodec.BIG5)[:2] == ["CHECK!", ""]
    out = with_quick_message(image, 2, "收到請回", NameCodec.BIG5)
    assert quick_messages(out, NameCodec.BIG5)[1] == "收到請回"
    changed = [i for i in range(len(out)) if out[i] != image[i]]
    assert all(QUICK_MSG_OFFSET + 40 <= i < QUICK_MSG_OFFSET + 80 for i in changed)
    assert out[-1] == 0xAA
    with pytest.raises(NameEncodingError):
        with_quick_message(image, 1, "x" * 41, NameCodec.ASCII)
    with pytest.raises(ValueError):
        with_quick_message(image, 17, "x", NameCodec.ASCII)
    assert quick_messages(b"", NameCodec.ASCII) == []


def test_dtmf_code_text_setting() -> None:
    project = f136.load(FIX / "dh98vu_20250811.F136")
    code = by_key("dtmf_code")
    assert isinstance(code, TextSetting) and code.text(project.image) == "1234"
    image = code.with_text(project.image, "56789*#")
    assert image[0x1364:0x136F] == b"56789*#\0\0\0\0" and code.text(image) == "56789*#"
    assert [i for i in range(len(image)) if image[i] != project.image[i]] == list(range(0x1364, 0x136B))
    with pytest.raises(SettingError, match="最多 11"):
        code.with_text(project.image, "1" * 12)
    with pytest.raises(SettingError, match="不可使用"):
        code.with_text(project.image, "12x")
    assert code.text(b"") is None
