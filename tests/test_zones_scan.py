from __future__ import annotations

from pathlib import Path

import pytest

from opendrs import f136
from opendrs.codeplug import Counts, build_stream, counts_in_image, parse_stream
from opendrs.layout import LayoutError, add_zone, check_layout, delete_zone, zone_rows
from opendrs.scanlist import (
    SCAN_AREA_END,
    SCAN_LISTS_OFFSET,
    ScanListError,
    references,
    remap_entries,
    scan_lists,
    with_scan_list,
)
from opendrs.textcodec import NameCodec

FIX = Path(__file__).parent / "fixtures"


@pytest.fixture
def project() -> f136.Project:
    return f136.load(FIX / "dh98vu_20250811.F136")


def _names(p: f136.Project) -> list[str]:
    return [z.name(NameCodec.BIG5) for z in p.zones]


# ------------------------------------------------------------------ scan lists
def test_scan_lists_of_real_projects(project: f136.Project) -> None:
    lists = scan_lists(project.image)
    assert len(lists) == 16
    in_use = [(s.number, s.name(NameCodec.BIG5), len(s.entries)) for s in lists if s.in_use]
    assert in_use == [(1, "scan1", 0), (2, "ScanFRS_TW", 14)]
    assert lists[1].entries[0] == (4, 1) and lists[1].raw[10:16] == bytes(6)
    other = f136.load(FIX / "mmdvm_20241127.F136")
    assert [(s.name(NameCodec.ASCII), s.entries) for s in scan_lists(other.image) if s.in_use] == [
        ("scan1", ((1, 1),)), ("scan2", ()), ("scan3", ())]


def test_with_scan_list_keeps_settings_bytes(project: f136.Project) -> None:
    image = with_scan_list(project.image, 1, "TGscan", [(1, 1), (1, 2)], NameCodec.BIG5)
    s = scan_lists(image)[0]
    assert s.name(NameCodec.BIG5) == "TGscan" and s.entries == ((1, 1), (1, 2))
    assert s.raw[10:16] == project.image[SCAN_LISTS_OFFSET + 10:SCAN_LISTS_OFFSET + 16]
    changed = [i for i in range(len(image)) if image[i] != project.image[i]]
    assert all(SCAN_LISTS_OFFSET <= i < SCAN_LISTS_OFFSET + 0x50 for i in changed)
    assert scan_lists(image)[1] == scan_lists(project.image)[1]


def test_with_scan_list_validation(project: f136.Project) -> None:
    with pytest.raises(ScanListError, match="最多 16"):
        with_scan_list(project.image, 1, "x", [(1, 1)] * 17, NameCodec.ASCII)
    with pytest.raises(ScanListError, match="1 與 16"):
        with_scan_list(project.image, 17, "x", [], NameCodec.ASCII)
    with pytest.raises(ScanListError, match="從 1 起算"):
        with_scan_list(project.image, 1, "x", [(0, 1)], NameCodec.ASCII)
    with pytest.raises(ScanListError, match="太短"):
        with_scan_list(bytes(SCAN_AREA_END - 1), 1, "x", [], NameCodec.ASCII)
    assert scan_lists(b"") == () and references(b"", 1) == 0


def test_remap_counts_and_compacts(project: f136.Project) -> None:
    image, edit = remap_entries(project.image, lambda e: None if e[1] % 2 else e)
    assert (edit.removed, edit.renumbered) == (7, 0)
    assert scan_lists(image)[1].entries == tuple((4, n) for n in range(2, 15, 2))
    same, edit = remap_entries(project.image, lambda e: e)
    assert same == project.image and (edit.removed, edit.renumbered) == (0, 0)


# ------------------------------------------------------------------ zones
def test_add_zone_appends_without_renumbering(project: f136.Project) -> None:
    updated, zone_row = add_zone(project, 2, "TESTZONE", NameCodec.BIG5)
    assert zone_row == 4 and _names(updated)[-1] == "TESTZONE"
    assert [z.index for z in updated.zones] == [1, 2, 3, 4, 5]
    assert [z.first_record for z in updated.zones] == [6, 13, 15, 16, 30]
    assert [z.channel_count for z in updated.zones] == [7, 2, 1, 14, 1]
    new = updated.channels[-1]
    assert new.index == 1 and new.raw[2:] == project.channels[2].raw[2:]
    assert zone_rows(updated)[-1] == 4
    assert scan_lists(updated.image) == scan_lists(project.image)
    assert counts_in_image(updated.image) == Counts(5, 25, len(project.contacts))
    # The zone record's other bytes come from an existing zone, not invented.
    assert updated.zones[-1].raw[0x11:] == project.zones[-1].raw[0x11:]
    stream = build_stream(updated, "10/8/2026 20:00")
    back = parse_stream(stream, Counts(5, 25, len(project.contacts)))
    assert back.zones == updated.zones and back.channels == updated.channels


def test_delete_zone_moves_later_zones_and_scan_entries(project: f136.Project) -> None:
    updated, edit = delete_zone(project, 1)  # HighWay (2 channels)
    assert _names(updated) == ["MMDVM", "HiKing", "FRS_TW"]
    assert [z.index for z in updated.zones] == [1, 2, 3]
    assert [z.first_record for z in updated.zones] == [4, 11, 12]
    assert len(updated.channels) == 22 and updated.channels[7:] == project.channels[9:]
    check_layout(updated)
    # ScanFRS_TW followed FRS_TW from zone 4 to zone 3.
    assert (edit.removed, edit.renumbered) == (0, 14)
    assert scan_lists(updated.image)[1].entries == tuple((3, n) for n in range(1, 15))
    assert counts_in_image(updated.image) == Counts(3, 22, len(project.contacts))


def test_delete_zone_drops_its_scan_entries(project: f136.Project) -> None:
    updated, edit = delete_zone(project, 3)  # FRS_TW itself
    assert (edit.removed, edit.renumbered) == (14, 0)
    assert scan_lists(updated.image)[1].entries == ()
    assert scan_lists(updated.image)[1].name(NameCodec.BIG5) == "ScanFRS_TW"  # list kept
    assert len(updated.channels) == 10


def test_add_then_delete_zone_round_trip(project: f136.Project) -> None:
    added, row = add_zone(project, 0, "TMP", NameCodec.ASCII)
    back, _ = delete_zone(added, row)
    assert back == project


def test_zone_errors(project: f136.Project) -> None:
    with pytest.raises(LayoutError, match="選擇要放進"):
        add_zone(project, 99, "x", NameCodec.ASCII)
    with pytest.raises(LayoutError, match="選擇分區"):
        delete_zone(project, 9)
    only = f136.Project(zones=(project.zones[0].with_layout(2, 7),), channels=project.channels[:7],
                        contacts=project.contacts, image=project.image)
    with pytest.raises(LayoutError, match="至少要保留"):
        delete_zone(only, 0)


def test_move_channel_to_another_zone(project: f136.Project) -> None:
    from opendrs.layout import move_channel

    # FRS_TW_3 (zone 4, ch 3) -> HighWay (zone 2), whose last channel is HW_3.
    updated, new_row, edit = move_channel(project, 12, 1)
    names = [c.name(NameCodec.BIG5) for c in updated.channels]
    assert names[7:10] == ["HW_1", "HW_3", "FRS_TW_3"] and new_row == 9
    assert [z.channel_count for z in updated.zones] == [7, 3, 1, 13]
    assert [z.first_record for z in updated.zones] == [5, 12, 15, 16]
    assert updated.channels[9].index == 3
    assert [c.index for c in updated.channels[11:]] == list(range(1, 14))
    check_layout(updated)
    # ScanFRS_TW: the moved channel's entry now names (2, 3); later FRS entries close up.
    entries = scan_lists(updated.image)[1].entries
    assert entries[:3] == ((4, 1), (4, 2), (2, 3)) and entries[3:] == tuple((4, n) for n in range(3, 14))
    assert (edit.removed, edit.renumbered) == (0, 12)
    assert updated.channels[9].raw[2:] == project.channels[12].raw[2:]  # record otherwise unchanged
    assert counts_in_image(updated.image) == Counts(4, 24, len(project.contacts))
    same, row, edit = move_channel(project, 12, 3)
    assert same is project and row == 12
    with pytest.raises(LayoutError, match="只剩"):
        move_channel(project, 9, 0)  # CHT_1 is the only channel of HiKing
    with pytest.raises(LayoutError, match="不存在"):
        move_channel(project, 0, 9)


def test_shift_channel_swaps_references(project: f136.Project) -> None:
    from opendrs.layout import shift_channel

    updated, row = shift_channel(project, 12, -1)  # FRS_TW_3 up
    assert row == 11 and [c.name(NameCodec.BIG5) for c in updated.channels[10:14]] == [
        "FRS_TW_1", "FRS_TW_3", "FRS_TW_2", "FRS_TW_4"]
    assert [c.index for c in updated.channels[10:14]] == [1, 2, 3, 4]
    # The scan list still names the same physical channels, in the same scan order.
    assert scan_lists(updated.image)[1].entries[:4] == ((4, 1), (4, 3), (4, 2), (4, 4))
    assert updated.zones == project.zones
    back, _ = shift_channel(updated, 11, 1)
    assert back == project
    with pytest.raises(LayoutError, match="第一台"):
        shift_channel(project, 10, -1)
    with pytest.raises(LayoutError, match="最後一台"):
        shift_channel(project, 6, 1)  # last of MMDVM
    with pytest.raises(LayoutError, match="一格"):
        shift_channel(project, 0, 2)


def test_add_channels_batch(project: f136.Project) -> None:
    from opendrs.layout import add_channels
    from opendrs.records import hz_to_mhz

    updated, rows = add_channels(project, 23, 3, 467_687_500, 12_500, 0, "FRS_TW_", 15, NameCodec.BIG5)
    assert list(rows) == [24, 25, 26]
    new = updated.channels[24:]
    assert [(c.index, c.name(NameCodec.BIG5), hz_to_mhz(c.freq_a_hz)) for c in new] == [
        (15, "FRS_TW_15", "467.68750"), (16, "FRS_TW_16", "467.70000"), (17, "FRS_TW_17", "467.71250")]
    assert all(c.raw[0x14:] == project.channels[23].raw[0x14:] for c in new)  # other settings copied
    check_layout(updated)
    assert counts_in_image(updated.image) == Counts(4, 27, len(project.contacts))
    zone1, rows = add_channels(project, 2, 2, 439_500_000, 25_000, -5_000_000, "LONGPREFIX", 1, NameCodec.BIG5)
    assert list(rows) == [7, 8] and zone1.channels[7].freq_b_hz == 434_500_000
    assert zone1.channels[8].name(NameCodec.BIG5) == "LONGPREFIX"  # cut to the 10-byte field
    with pytest.raises(LayoutError, match="1 到 64"):
        add_channels(project, 0, 65, 1, 1, 0, "x", 1, NameCodec.ASCII)


def test_shift_contact_swaps_numbers(project: f136.Project) -> None:
    from opendrs.contact_edit import ContactEditError, shift_contact
    from opendrs.rxgroup import rx_groups

    updated, row = shift_contact(project, 1, 1)  # TG46600_TW (#2) down to #3
    assert row == 2 and [c.index for c in updated.contacts[:4]] == [1, 2, 3, 4]
    assert updated.contacts[2].raw[2:] == project.contacts[1].raw[2:]
    assert updated.channels[0].tx_contact == 3  # TG46600 still calls TG46600_TW
    assert rx_groups(updated.image)[0].members[:3] == (1, 3, 2)
    assert updated.image[0x1400:0x1400 + 16 * len(updated.contacts)] == b"".join(c.raw for c in updated.contacts)
    back, _ = shift_contact(updated, 2, -1)
    assert back.contacts == project.contacts and back.channels == project.channels
    with pytest.raises(ContactEditError, match="第一筆"):
        shift_contact(project, 0, -1)


def test_scan_list_settings(project: f136.Project) -> None:
    from opendrs.scanlist import SCAN_LISTS_OFFSET, with_scan_settings

    scan1 = scan_lists(project.image)[0]
    assert (scan1.talkback, scan1.tx_mode, scan1.designated) == (False, 0, (0x105, 6))  # stale -> shown Off
    image = with_scan_settings(project.image, 1, True, 2, (4, 3))
    s = scan_lists(image)[0]
    assert (s.talkback, s.tx_mode, s.designated) == (True, 2, (4, 3))
    assert image[SCAN_LISTS_OFFSET + 0x0B] == (project.image[SCAN_LISTS_OFFSET + 0x0B] & ~0x2C & 0xFF) | 0x28
    assert image[SCAN_LISTS_OFFSET + 0x0A] == project.image[SCAN_LISTS_OFFSET + 0x0A]
    with pytest.raises(ScanListError):
        with_scan_settings(project.image, 1, False, 3, (0, 0))
    # The designated channel follows deletions; stale values are left alone.
    from opendrs.layout import delete_channel, delete_zone

    p = f136.Project(**{**project.__dict__, "image": image})
    after, edit = delete_channel(p, 11)  # FRS_TW_2: (4,3) -> (4,2)
    assert scan_lists(after.image)[0].designated == (4, 2) and edit.refs_changed == 1
    gone, _ = delete_zone(p, 3)
    assert scan_lists(gone.image)[0].designated == (0, 0)
    untouched, _ = delete_zone(project, 0)
    assert scan_lists(untouched.image)[0].designated == (0x105, 6)


def test_scan_dialog_settings(project: f136.Project) -> None:
    import os

    os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
    from PySide6.QtWidgets import QApplication

    from opendrs.gui.scan_page import ScanListDialog

    QApplication.instance() or QApplication([])
    dlg = ScanListDialog(project, scan_lists(project.image)[0], NameCodec.BIG5)
    assert dlg.designated.currentText() == "Off" and not dlg.designated.isEnabled()
    assert dlg.settings() == (False, 0, (0x105, 6))  # untouched stale value is kept
    dlg.tx_mode.setCurrentIndex(2)
    assert dlg.designated.isEnabled()
    dlg.designated.setCurrentIndex(dlg.designated.findText("FRS_TW／FRS_TW_3"))
    dlg.talkback.setCurrentIndex(1)
    assert dlg.settings() == (True, 2, (4, 3))
