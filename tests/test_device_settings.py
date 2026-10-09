from __future__ import annotations

from pathlib import Path

from opendrs.device import BAND_TABLE, DeviceInfo, frequency_problem
from opendrs.settings import MAX_RECENT, Settings, load_settings, save_settings
from opendrs.textcodec import NameCodec

MHZ = 1_000_000


def test_dual_band_label() -> None:
    info = DeviceInfo("F136-480", "", "DRS-8500", "")
    assert info.bands == BAND_TABLE["136-480"]
    assert frequency_problem(145 * MHZ, info.bands) is None
    assert frequency_problem(440 * MHZ, info.bands) is None
    assert frequency_problem(250 * MHZ, info.bands) is not None


def test_zero_and_unknown_band_are_not_flagged() -> None:
    assert frequency_problem(0, BAND_TABLE["400-480"]) is None
    assert DeviceInfo("F999", "", "", "").bands is None
    assert frequency_problem(1, None) is None


def test_device_fields_stop_at_padding() -> None:
    image = bytearray(b"\xff" * 0x90)
    image[0x50:0x58] = b"F400-480"
    image[0x70:0x76] = b"DM-850"
    info = DeviceInfo.from_image(bytes(image))
    assert (info.band_label, info.model, info.write_date) == ("F400-480", "DM-850", "")


def test_settings_round_trip_utf8(tmp_path: Path) -> None:
    s = Settings(name_codec=NameCodec.GBK).with_recent("C:/資料/甲.F136")
    save_settings(s, tmp_path)
    raw = (tmp_path / "settings.json").read_bytes()
    assert "甲".encode("utf-8") in raw
    assert load_settings(tmp_path) == s


def test_recent_list_dedups_and_caps() -> None:
    s = Settings()
    for i in range(MAX_RECENT + 3):
        s = s.with_recent(f"f{i}")
    s = s.with_recent("f5")
    assert s.recent_files[0] == "f5"
    assert len(s.recent_files) == MAX_RECENT
    assert len(set(s.recent_files)) == MAX_RECENT


def test_missing_or_corrupt_settings_give_defaults(tmp_path: Path) -> None:
    assert load_settings(tmp_path) == Settings()
    (tmp_path / "settings.json").write_text("{not json", encoding="utf-8")
    assert load_settings(tmp_path) == Settings()


def test_dmr_id_from_real_project_and_radio_stream() -> None:
    from opendrs import f136

    fixtures = Path(__file__).parent / "fixtures"
    dev = f136.load(fixtures / "dh98vu_20250811.F136").device
    assert dev.alias == "QA0AFK" and dev.dmr_id == 9998765
    # The same bytes arrive in the stream read from the radio itself.
    stream = (fixtures / "capture" / "read_stream.bin").read_bytes()
    assert DeviceInfo.from_image(stream).dmr_id == 9998765


def test_dmr_id_unset_or_short_image() -> None:
    from opendrs.device import DMR_ID_OFFSET

    base = bytearray(0x100)
    assert DeviceInfo.from_image(bytes(base)).dmr_id is None
    base[DMR_ID_OFFSET:DMR_ID_OFFSET + 3] = b"\xff\xff\xff"
    assert DeviceInfo.from_image(bytes(base)).dmr_id is None
    base[DMR_ID_OFFSET:DMR_ID_OFFSET + 4] = b"\x09\x00\x00\x02"  # 4th byte is another field
    assert DeviceInfo.from_image(bytes(base)).dmr_id == 9
    assert DeviceInfo.from_image(bytes(base[:DMR_ID_OFFSET + 2])).dmr_id is None


def test_edit_dmr_id_and_alias_touch_only_their_bytes() -> None:
    from opendrs import f136
    from opendrs.device import ALIAS_OFFSET, DMR_ID_OFFSET

    old = f136.load(Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136")
    new = old.with_dmr_id(9990999).with_alias("QA0AJS")
    assert new.device.dmr_id == 9990999 and new.device.alias == "QA0AJS"
    a, b = old.image, new.image
    changed = [i for i in range(len(a)) if a[i] != b[i]]
    assert all(ALIAS_OFFSET <= i < DMR_ID_OFFSET + 3 for i in changed)
    assert b[DMR_ID_OFFSET + 3] == a[DMR_ID_OFFSET + 3] == 0x44  # neighbouring setting kept
    assert b[ALIAS_OFFSET:DMR_ID_OFFSET] == b"QA0AJS".ljust(16, b"\0")
    assert new.contacts == old.contacts and len(b) == len(a)
    # Round-trips through the file format and back to the original bytes.
    assert f136.parse(f136.serialize(new)).device.dmr_id == 9990999
    assert new.with_dmr_id(9998765).with_alias("QA0AFK").image == old.image


def test_edit_validation() -> None:
    import pytest

    from opendrs.device import MAX_DMR_ID, parse_dmr_id, with_alias, with_dmr_id

    image = bytes(0x100)
    for bad in (0, MAX_DMR_ID + 1):
        with pytest.raises(ValueError, match="DMR ID"):
            with_dmr_id(image, bad)
    for text in ("abc", "", "12.5"):
        with pytest.raises(ValueError):
            parse_dmr_id(text)
    assert parse_dmr_id(" 9998765 ") == 9998765
    with pytest.raises(ValueError, match="最多 15"):
        with_alias(image, "A" * 16)
    with pytest.raises(ValueError, match="ASCII"):
        with_alias(image, "王小明")
    assert with_alias(image, "A" * 15)[0x80:0x90] == b"A" * 15 + b"\0"
    assert with_alias(image, "")[0x80:0x90] == bytes(16)
    with pytest.raises(ValueError, match="設定區太短"):
        with_dmr_id(bytes(0x92), 1)


def test_firmware_version_from_image() -> None:
    from opendrs import f136

    fix = Path(__file__).parent / "fixtures"
    dev = f136.load(fix / "dh98vu_20250811.F136").device
    assert (dev.firmware, dev.firmware_date) == ("308E.BBKTARSCB.003", "Mar 9 2024")
    assert f136.load(fix / "mmdvm_20241127.F136").device.firmware_date == "Dec 19 2023"
    old = f136.load(fix / "mmdvm_20241124.F136").device  # older layout: no build date
    assert (old.firmware, old.firmware_date) == ("V100MZSR", "")
    assert (DeviceInfo.from_image(b"").firmware, DeviceInfo.from_image(b"").firmware_date) == ("", "")
