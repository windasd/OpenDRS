from __future__ import annotations

import struct
from dataclasses import replace
from pathlib import Path

import pytest

from opendrs import f136
from opendrs.codeplug import (
    COUNTS_OFFSET,
    Counts,
    build_stream,
    counts_in_image,
    parse_stream,
)
from opendrs.layout import LayoutError, add_channel, check_layout, zone_rows
from opendrs.records import Channel
from opendrs.textcodec import NameCodec

DH98 = Path(__file__).parent / "fixtures" / "dh98vu_20250811.F136"


@pytest.fixture
def project() -> f136.Project:
    return f136.load(DH98)


def test_zone_rows_follow_zone_table(project: f136.Project) -> None:
    rows = zone_rows(project)
    assert rows == [0] * 7 + [1] * 2 + [2] + [3] * 14
    check_layout(project)


def test_tx_contact_field_matches_contact_names(project: f136.Project) -> None:
    by_number = {c.index: c.name(NameCodec.BIG5) for c in project.contacts}
    for ch in project.channels:
        name = ch.name(NameCodec.BIG5)
        if name.startswith("TG"):
            assert by_number[ch.tx_contact].startswith(name)


def test_add_channel_appends_to_its_zone(project: f136.Project) -> None:
    updated, new_row = add_channel(project, 2)  # TG9 in zone 1 "MMDVM"
    assert new_row == 7
    new = updated.channels[7]
    assert new.index == 8 and new.raw[2:] == project.channels[2].raw[2:]
    assert [z.channel_count for z in updated.zones] == [8, 2, 1, 14]
    assert [z.first_record for z in updated.zones] == [5, 13, 15, 16]
    assert updated.channels[:7] == project.channels[:7]
    assert updated.channels[8:] == project.channels[7:]
    check_layout(updated)
    assert zone_rows(updated)[7] == 0
    # Only the channel count changes in the settings image: scan lists stay valid.
    assert counts_in_image(updated.image) == Counts(4, 25, len(project.contacts))
    changed = [i for i in range(len(project.image)) if project.image[i] != updated.image[i]]
    assert changed == [COUNTS_OFFSET + 2]
    assert updated.contacts == project.contacts


def test_add_channel_to_last_zone_and_round_trip(project: f136.Project) -> None:
    updated, new_row = add_channel(project, 10)
    assert new_row == len(updated.channels) - 1 == 24
    assert updated.channels[-1].index == 15
    assert [z.first_record for z in updated.zones] == [z.first_record for z in project.zones]
    reloaded = f136.parse(f136.serialize(updated))
    assert reloaded == updated
    stream = build_stream(updated, "10/8/2026 18:00")
    counts = Counts(4, 25, len(project.contacts))
    assert len(stream) == counts.stream_size
    assert parse_stream(stream, counts).channels == updated.channels


def test_add_channel_rejects_bad_input(project: f136.Project) -> None:
    with pytest.raises(LayoutError, match="選擇"):
        add_channel(project, 99)
    broken = replace(project, zones=(project.zones[0].with_layout(6, 7), *project.zones[1:]))
    with pytest.raises(LayoutError, match="起始位置"):
        add_channel(broken, 0)
    short = replace(project, channels=project.channels[:-1])
    with pytest.raises(LayoutError, match="合計"):
        check_layout(short)


def test_dangling_tx_contact_is_a_problem(project: f136.Project) -> None:
    assert project.problems() == []
    bad = project.with_channel(0, project.channels[0].with_tx_contact(999))
    assert any("聯絡人 #999 不存在" in p for p in bad.problems())
    assert project.with_channel(0, project.channels[0].with_tx_contact(0)).problems() == []


def test_channel_field_setters() -> None:
    ch = Channel(bytes(32)).with_tx_contact(513).with_index(7)
    assert ch.tx_contact == 513 and ch.index == 7
    assert struct.unpack_from("<H", ch.raw, 0x1E)[0] == 513
    with pytest.raises(ValueError):
        ch.with_tx_contact(0x10000)
    with pytest.raises(ValueError):
        ch.with_index(-1)


def _scan_list(image: bytes, n: int) -> list[tuple[int, int]]:
    from opendrs.scanlist import SCAN_LIST_SIZE, SCAN_LISTS_OFFSET

    base = SCAN_LISTS_OFFSET + n * SCAN_LIST_SIZE + 0x10
    return [struct.unpack_from("<HH", image, base + 4 * i) for i in range(16)]


def test_scan_list_layout_matches_real_project(project: f136.Project) -> None:
    from opendrs.scanlist import references as scan_references

    assert _scan_list(project.image, 1) == [(4, n) for n in range(1, 15)] + [(0, 0)] * 2
    assert project.image[0x17B:0x185] == b"ScanFRS_TW"
    assert scan_references(project.image, 4, 3) == 1 and scan_references(project.image, 1, 1) == 0


def test_delete_channel_renumbers_zone_and_scan_list(project: f136.Project) -> None:
    from opendrs.layout import delete_channel

    row = 12  # FRS_TW_3: zone 4, channel 3
    assert project.channels[row].name(NameCodec.BIG5) == "FRS_TW_3"
    updated, edit = delete_channel(project, row)
    assert (edit.removed, edit.renumbered) == (1, 11)
    frs = updated.channels[10:]
    assert [c.name(NameCodec.BIG5) for c in frs][:3] == ["FRS_TW_1", "FRS_TW_2", "FRS_TW_4"]
    assert [c.index for c in frs] == list(range(1, 14))
    assert [z.channel_count for z in updated.zones] == [7, 2, 1, 13]
    assert [z.first_record for z in updated.zones] == [5, 12, 14, 15]
    check_layout(updated)
    # Entries now point at the same physical channels as before (FRS_TW_4 is now (4,3)).
    assert _scan_list(updated.image, 1) == [(4, n) for n in range(1, 14)] + [(0, 0)] * 3
    assert counts_in_image(updated.image) == Counts(4, 23, len(project.contacts))
    changed = {i for i in range(len(project.image)) if project.image[i] != updated.image[i]}
    scan_area = range(0x17B + 0x10, 0x17B + 0x50)
    assert changed <= set(scan_area) | {COUNTS_OFFSET + 2}
    assert updated.contacts == project.contacts and updated.channels[:10] == project.channels[:10]


def test_delete_in_earlier_zone_leaves_other_zones_scan_entries(project: f136.Project) -> None:
    from opendrs.layout import delete_channel

    updated, edit = delete_channel(project, 3)  # TN6A_relay, zone 1
    assert (edit.removed, edit.renumbered) == (0, 0)
    assert [c.index for c in updated.channels[:6]] == [1, 2, 3, 4, 5, 6]
    assert [z.first_record for z in updated.zones] == [5, 11, 13, 14]
    assert updated.channels[6:] == project.channels[7:]  # later zones' records untouched
    assert _scan_list(updated.image, 1) == _scan_list(project.image, 1)
    reloaded = f136.parse(f136.serialize(updated))
    assert reloaded == updated
    stream = build_stream(updated, "10/8/2026 18:00")
    assert parse_stream(stream, Counts(4, 23, len(project.contacts))).zones == updated.zones


def test_delete_then_add_restores_counts(project: f136.Project) -> None:
    from opendrs.layout import delete_channel

    smaller, _ = delete_channel(project, 0)
    again, row = add_channel(smaller, 0)
    assert [z.channel_count for z in again.zones] == [7, 2, 1, 14]
    assert again.zones == project.zones and row == 6


def test_cannot_delete_a_zones_only_channel(project: f136.Project) -> None:
    from opendrs.layout import delete_channel

    assert project.channels[9].name(NameCodec.BIG5) == "CHT_1"  # zone 3 has one channel
    with pytest.raises(LayoutError, match="只剩這個頻道"):
        delete_channel(project, 9)
    with pytest.raises(LayoutError, match="選擇"):
        delete_channel(project, -1)


def test_scan_entries_in_other_lists_and_duplicates() -> None:
    from opendrs.scanlist import SCAN_LISTS_OFFSET

    img = bytearray(0x700)
    base0 = SCAN_LISTS_OFFSET + 0x10
    base5 = SCAN_LISTS_OFFSET + 5 * 0x50 + 0x10
    struct.pack_into("<8H", img, base0, 2, 5, 1, 5, 2, 7, 2, 5)
    struct.pack_into("<4H", img, base5, 2, 6, 3, 1)
    from opendrs.scanlist import remap_entries

    def drop_2_5(entry: tuple[int, int]) -> tuple[int, int] | None:
        zone, ch = entry
        if zone != 2 or ch < 5:
            return entry
        return None if ch == 5 else (zone, ch - 1)

    out, edit = remap_entries(bytes(img), drop_2_5)
    assert (edit.removed, edit.renumbered) == (2, 2)
    assert _scan_list(out, 0)[:4] == [(1, 5), (2, 6), (0, 0), (0, 0)]
    assert _scan_list(out, 5)[:2] == [(2, 5), (3, 1)]
    short, edit = remap_entries(b"\0" * 0x200, drop_2_5)
    assert short == b"\0" * 0x200 and edit.removed == 0


def test_channel_fields_match_legacy_grid(project: f136.Project) -> None:
    from opendrs import channel_fields as cf

    by_name = {c.name(NameCodec.BIG5): c for c in project.channels}
    # What the legacy CPS grid shows for these channels.
    expected = {
        "TG46600": ("數位", 1, "On", "低", "Off", "Patcs", "非確認數據", "Impolite"),
        "TG46621": ("數位", 1, "On", "低", "On", "Patcs", "非確認數據", "Polite to ALL"),
        "TN6A_relay": ("數位", 6, "時隙2", "高", "Off", "Patcs", "非確認數據", "Impolite（舊值 0）"),
        "TN_analogy": ("類比", 1, "時隙2", "高", "On", "Patcs", "非確認數據", "Impolite（舊值 0）"),
    }
    for name, row in expected.items():
        ch = by_name[name]
        got = (ch.mode_label, ch.color_code, cf.slot_label(ch),
               *(f.display(ch.raw) for f in (cf.POWER, cf.CALL_INDICATOR, cf.PRIVATE_CALL_TYPE,
                                             cf.MESSAGE_TYPE, cf.TX_POLICY)))
        assert got == row, name
    assert by_name["FRS_TW_1"].mode_label == "類比+數位（發射類比）"


def test_channel_field_edits_touch_only_their_bits(project: f136.Project) -> None:
    from opendrs import channel_fields as cf

    ch = project.channels[0]  # TG46600
    edited = cf.with_slot(cf.with_color_code(ch, 7), 2)
    assert edited.color_code == 7 and edited.raw[0x1D] >> 4 == 7  # both copies
    assert cf.slot_label(edited) == "時隙2" and edited.raw[0x14] & 1 == 1 and edited.raw[0x1D] & 7 == 7
    assert cf.slot_label(cf.with_slot(edited, 1)) == "時隙1"
    assert cf.with_slot(cf.with_color_code(edited, 1), 0) == ch
    powered = cf.put(ch, cf.POWER, 1)
    assert powered.raw[0x14] == ch.raw[0x14] | 0x20 and cf.POWER.display(powered.raw) == "高"
    policy = cf.put(ch, cf.TX_POLICY, 2)
    assert policy.raw[0x15] == (ch.raw[0x15] & 0x9F) | 0x40
    keyed = cf.with_encryption_key(ch, 1)
    assert cf.encryption_key(keyed) == 1 and keyed.raw[:0x16] == ch.raw[:0x16]
    for bad in (lambda: cf.with_slot(ch, 3), lambda: cf.with_encryption_key(ch, 11),
                lambda: ch.with_mode(4), lambda: ch.with_color_code(16)):
        with pytest.raises(ValueError):
            bad()


def test_tones_match_legacy_codes(project: f136.Project) -> None:
    from opendrs import channel_fields as cf

    analog = next(c for c in project.channels if c.name(NameCodec.BIG5) == "TN_analogy")
    assert cf.tone_label(cf.tone(analog, False)) == cf.tone_label(cf.tone(analog, True)) == "123.0 Hz"
    choices = dict(cf.tone_choices())
    assert len(choices) == 268 and choices["177.3 Hz"] == (1 << 8 | 33)
    assert choices["D072N"] == (2 << 8 | 15) and choices["D043I"] == (3 << 8 | 7)  # legacy raw values
    tx_dcs = cf.with_tone(analog, True, choices["D072N"])
    assert tx_dcs.raw[0x1A] == 0b1001 and tx_dcs.raw[0x1C] == 15 and tx_dcs.raw[0x1B] == analog.raw[0x1B]
    assert cf.tone_label(cf.tone(tx_dcs, True)) == "D072N"
    off = cf.with_tone(tx_dcs, False, 0)
    assert off.raw[0x1A] == 0b1000 and cf.tone(off, False) == 0
    with pytest.raises(ValueError):
        cf.with_tone(analog, False, 1 << 8 | 60)
    listed = cf.put(cf.put(analog, cf.SCAN_LIST, 2), cf.EMERGENCY, 3)
    assert (listed.raw[0x18], listed.raw[0x19]) == (2, 3)
