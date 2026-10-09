from __future__ import annotations

import struct
from pathlib import Path

import pytest

from opendrs import f136
from opendrs.alarm_enc import (
    ENC_KEYS_OFFSET,
    ENC_LEVEL_OFFSET,
    ListEditError,
    alarm_base,
    alarm_lists,
    encryption_keys,
    parse_key_hex,
    with_alarm,
    with_encryption_key,
)
from opendrs.layout import delete_channel, delete_zone
from opendrs.textcodec import NameCodec

FIX = Path(__file__).parent / "fixtures"


@pytest.fixture
def project() -> f136.Project:
    return f136.load(FIX / "dh98vu_20250811.F136")


def test_alarm_lists_of_real_project(project: f136.Project) -> None:
    lists = alarm_lists(project.image)
    assert [a.name(NameCodec.BIG5) for a in lists] == ["1", "2", "3", "4"]
    a = lists[0]
    assert (a.alarm_type, a.mode, a.channel, a.impolite_attempts, a.mic_seconds) == (1, 1, (0, 0), 8, 6)


def test_with_alarm_touches_only_its_fields(project: f136.Project) -> None:
    image = with_alarm(project.image, 2, alarm_type=3, mode=2, channel=(4, 3), impolite_attempts=15,
                       mic_seconds=20, name="SOS", codec=NameCodec.BIG5)
    a = alarm_lists(image)[1]
    assert (a.name(NameCodec.BIG5), a.alarm_type, a.mode, a.channel, a.impolite_attempts, a.mic_seconds) == (
        "SOS", 3, 2, (4, 3), 15, 20)
    base = alarm_base(2)
    assert image[base + 0x0A] & 0xF0 == project.image[base + 0x0A] & 0xF0  # high nibble kept
    assert image[base + 0x10] & 0xF0 == project.image[base + 0x10] & 0xF0
    changed = [i for i in range(len(image)) if image[i] != project.image[i]]
    assert all(base <= i < base + 0x12 for i in changed)
    for bad in ({"alarm_type": 9}, {"mode": 0}, {"impolite_attempts": 0}, {"mic_seconds": 300}):
        with pytest.raises(ListEditError):
            with_alarm(project.image, 1, **bad)
    with pytest.raises(ListEditError):
        with_alarm(project.image, 5, mode=1)


def test_encryption_keys(project: f136.Project) -> None:
    assert project.image[ENC_LEVEL_OFFSET] == 4  # 增強
    keys = encryption_keys(project.image)
    assert len(keys) == 10 and keys[0].alias(NameCodec.BIG5) == "windasd" and keys[0].key == bytes.fromhex("123456789ABC")
    assert [k.in_use for k in keys] == [True] + [False] * 9
    image = with_encryption_key(project.image, 2, "TW", parse_key_hex("01 02 03 04 05 06"), NameCodec.BIG5)
    assert encryption_keys(image)[1].key == bytes(range(1, 7))
    cleared = with_encryption_key(image, 2, "", bytes(6), NameCodec.BIG5)
    assert cleared == project.image
    assert all(ENC_KEYS_OFFSET + 16 <= i < ENC_KEYS_OFFSET + 32
               for i in range(len(image)) if image[i] != project.image[i])
    for bad in ("12345", "zz0102030405", "01020304050607"):
        with pytest.raises(ListEditError):
            parse_key_hex(bad)


def test_channel_references_follow_deletions(project: f136.Project) -> None:
    img = bytearray(project.image)
    struct.pack_into("<HH", img, 0x1354, 4, 5)      # GPS channel: FRS_TW_5
    struct.pack_into("<HH", img, alarm_base(1) + 0x0C, 4, 3)   # alarm 1: FRS_TW_3
    struct.pack_into("<HH", img, alarm_base(2) + 0x0C, 2, 1)   # alarm 2: HW_1
    p = f136.Project(**{**project.__dict__, "image": bytes(img)})
    updated, edit = delete_channel(p, 12)  # FRS_TW_3
    assert edit.refs_changed == 2
    assert struct.unpack_from("<HH", updated.image, 0x1354) == (4, 4)
    assert alarm_lists(updated.image)[0].channel == (0, 0)  # pointed at the deleted channel
    assert alarm_lists(updated.image)[1].channel == (2, 1)
    zoned, edit = delete_zone(p, 1)  # HighWay
    assert struct.unpack_from("<HH", zoned.image, 0x1354) == (3, 5)
    assert alarm_lists(zoned.image)[1].channel == (0, 0)
    assert edit.refs_changed == 3
