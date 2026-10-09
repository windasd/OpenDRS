from __future__ import annotations

from pathlib import Path

import pytest

from opendrs import f136
from opendrs.codeplug import (
    Counts,
    build_stream,
    counts_in_image,
    image_length_for,
    parse_stream,
)
from opendrs.contact_edit import ContactEditError, channels_calling, delete_contacts
from opendrs.rxgroup import (
    RX_AREA_END,
    RX_GROUPS_OFFSET,
    RxGroupError,
    groups_containing,
    remap_members,
    rx_groups,
    with_rx_group,
)
from opendrs.scanlist import SCAN_AREA_END
from opendrs.textcodec import NameCodec

FIX = Path(__file__).parent / "fixtures"


@pytest.fixture
def project() -> f136.Project:
    return f136.load(FIX / "dh98vu_20250811.F136")


def _name(project: f136.Project, number: int) -> str:
    return next(c for c in project.contacts if c.index == number).name(NameCodec.BIG5).replace("␍␊", "")


# ------------------------------------------------------------------ RX groups
def test_rx_groups_follow_scan_lists_and_match_real_data(project: f136.Project) -> None:
    assert RX_GROUPS_OFFSET == SCAN_AREA_END  # firmware constants 0x3C062B / 0x3C0635
    groups = rx_groups(project.image)
    assert len(groups) == 32 and [g.number for g in groups if g.in_use] == [1]
    g = groups[0]
    assert g.name(NameCodec.BIG5) == "MMDVM_TG"
    assert g.members == (1, 2, 3, 4, 5, 6, 10, 12, 13)
    assert all(_name(project, m).startswith("TG") for m in g.members)
    # Every digital channel listens to group 1; analog ones to none.
    for ch in project.channels:
        assert ch.rx_group in (0, 1)
    assert {ch.rx_group for ch in project.channels if ch.mode == 0} <= {0, 1}
    other = f136.load(FIX / "mmdvm_20241127.F136")
    assert [(x.name(NameCodec.ASCII), x.members) for x in rx_groups(other.image) if x.in_use] == [
        ("Group1", (1, 2, 3, 4, 5, 6))]


def test_with_rx_group_only_touches_its_record(project: f136.Project) -> None:
    image = with_rx_group(project.image, 2, "TW", [2, 10], NameCodec.BIG5)
    assert rx_groups(image)[1].name(NameCodec.BIG5) == "TW" and rx_groups(image)[1].members == (2, 10)
    changed = [i for i in range(len(image)) if image[i] != project.image[i]]
    assert all(RX_GROUPS_OFFSET + 0x4C <= i < RX_GROUPS_OFFSET + 2 * 0x4C for i in changed)
    assert groups_containing(image, 10) == [1, 2]


def test_rx_group_validation(project: f136.Project) -> None:
    with pytest.raises(RxGroupError, match="最多 32"):
        with_rx_group(project.image, 1, "x", list(range(1, 34)), NameCodec.ASCII)
    with pytest.raises(RxGroupError, match="1 與 32"):
        with_rx_group(project.image, 33, "x", [], NameCodec.ASCII)
    with pytest.raises(RxGroupError, match="從 1 起算"):
        with_rx_group(project.image, 1, "x", [0], NameCodec.ASCII)
    with pytest.raises(RxGroupError, match="太短"):
        with_rx_group(bytes(RX_AREA_END - 1), 1, "x", [], NameCodec.ASCII)
    assert rx_groups(b"") == () and remap_members(b"", lambda m: m) == (b"", 0)


def test_channel_rx_group_setter(project: f136.Project) -> None:
    ch = project.channels[0].with_rx_group(5)
    assert ch.rx_group == 5 and ch.raw[0x17] == 5
    assert ch.raw[:0x17] == project.channels[0].raw[:0x17]
    with pytest.raises(ValueError):
        ch.with_rx_group(33)


# ------------------------------------------------------------------ deleting contacts
def test_delete_contacts_renumbers_everything(project: f136.Project) -> None:
    rows = [2, 299]  # #3 TG46610 (in the RX group) and #300
    updated, report = delete_contacts(project, rows, NameCodec.BIG5)
    assert (report.removed, report.group_members_dropped) == (2, 1)
    assert report.renumbered == len(project.contacts) - 3 - 1  # everything after #3 except the last one gone
    assert [c.index for c in updated.contacts] == list(range(1, len(project.contacts) - 1))
    # Channels still call the same contacts by their new numbers.
    for old, new in zip(project.channels, updated.channels, strict=True):
        assert _name(updated, new.tx_contact) == _name(project, old.tx_contact)
    assert [_name(updated, m) for m in rx_groups(updated.image)[0].members] == [
        _name(project, m) for m in (1, 2, 4, 5, 6, 10, 12, 13)]
    # Image copy of the contacts and the counts follow; freed slots are cleared.
    n = len(updated.contacts)
    assert counts_in_image(updated.image) == Counts(4, 24, n)
    assert updated.image[0x1400:0x1400 + 16 * n] == b"".join(c.raw for c in updated.contacts)
    assert len(updated.image) == image_length_for(n) and set(updated.image[0x1400 + 16 * n:]) == {0}
    stream = build_stream(updated, "10/8/2026 21:00")
    assert parse_stream(stream, Counts(4, 24, n)).contacts == updated.contacts
    assert f136.parse(f136.serialize(updated)) == updated


def test_delete_contact_used_by_a_channel_is_refused(project: f136.Project) -> None:
    assert channels_calling(project, {2}, NameCodec.BIG5) == ["1. TG46600"]
    with pytest.raises(ContactEditError, match="TG46600"):
        delete_contacts(project, [1], NameCodec.BIG5)
    with pytest.raises(ContactEditError, match="選擇"):
        delete_contacts(project, [], NameCodec.BIG5)
    with pytest.raises(ContactEditError, match="不存在"):
        delete_contacts(project, [9999], NameCodec.BIG5)


def test_delete_last_contacts_keeps_others_identical(project: f136.Project) -> None:
    last = len(project.contacts) - 1
    updated, report = delete_contacts(project, [last], NameCodec.BIG5)
    assert report.renumbered == 0 and updated.contacts == project.contacts[:-1]
    assert updated.channels == project.channels
