"""Deleting contacts without leaving dangling references.

Only two places in the codeplug store contact numbers (``Contact.index``, 1-based and
equal to the record's position): a channel's TX contact (+0x1E) and the RX group
member lists. The firmware reads contacts through the base 0x3C13F in exactly three
places — the TX-contact loader FUN_03030b70, the RX-group loader FUN_03031284 and the
"current channel contact" info screen FUN_0304503c, which only displays the channel's
TX contact. So after removing contacts the rest are renumbered and those two kinds of
reference are rewritten; nothing else needs to change.
"""

from __future__ import annotations

from collections.abc import Iterable
from dataclasses import dataclass, replace

from .codeplug import CONTACT_SIZE, CONTACTS_OFFSET, image_length_for, with_synced_image
from .f136 import Project
from .rxgroup import remap_members
from .textcodec import NameCodec


class ContactEditError(ValueError):
    pass


@dataclass(frozen=True)
class ContactDeletion:
    removed: int
    group_members_dropped: int
    renumbered: int  # remaining contacts whose number changed


def channels_calling(project: Project, numbers: set[int], codec: NameCodec) -> list[str]:
    """'row. name' of every channel whose TX contact is one of ``numbers``."""
    return [f"{row}. {ch.name(codec)}" for row, ch in enumerate(project.channels, start=1)
            if ch.tx_contact in numbers]


def _trim_image(image: bytes, contacts: int) -> bytes:
    """Clear the freed contact slots and shrink to the legacy CPS's image length."""
    end = CONTACTS_OFFSET + contacts * CONTACT_SIZE
    length = image_length_for(contacts)
    if len(image) < end:
        return image
    kept = image[:end]
    return kept + bytes(max(length, len(kept)) - len(kept))


def delete_contacts(project: Project, rows: Iterable[int], codec: NameCodec) -> tuple[Project, ContactDeletion]:
    doomed_rows = sorted(set(rows))
    if not doomed_rows:
        raise ContactEditError("請先選擇要刪除的聯絡人")
    if doomed_rows[0] < 0 or doomed_rows[-1] >= len(project.contacts):
        raise ContactEditError("選取的聯絡人不存在")
    doomed = {project.contacts[r].index for r in doomed_rows}
    users = channels_calling(project, doomed, codec)
    if users:
        listed = "、".join(users[:8]) + ("…" if len(users) > 8 else "")
        raise ContactEditError(f"下列頻道的發話對象是要刪除的聯絡人，請先改掉：{listed}")
    renumber: dict[int, int] = {}
    contacts = []
    for row, contact in enumerate(project.contacts):
        if row in doomed_rows:
            continue
        new_number = len(contacts) + 1
        renumber[contact.index] = new_number
        contacts.append(contact.with_index(new_number))
    channels = tuple(ch.with_tx_contact(renumber[ch.tx_contact]) if ch.tx_contact in renumber else ch
                     for ch in project.channels)
    image, dropped = remap_members(project.image, renumber.get)
    if image:
        image = _trim_image(image, len(contacts))
    updated = with_synced_image(replace(project, contacts=tuple(contacts), channels=channels, image=image))
    changed = sum(1 for old, new in renumber.items() if old != new)
    return updated, ContactDeletion(len(doomed_rows), dropped, changed)


def shift_contact(project: Project, row: int, delta: int) -> tuple[Project, int]:
    """Swap contact ``row`` with its neighbour above (-1) or below (+1); returns
    (project, new row). Contact numbers follow the list position, so the two numbers
    are exchanged in every channel TX contact and RX group member list as well."""
    if delta not in (-1, 1):
        raise ContactEditError("一次只能移動一格")
    target = row + delta
    if not 0 <= row < len(project.contacts):
        raise ContactEditError("請先選擇聯絡人")
    if not 0 <= target < len(project.contacts):
        raise ContactEditError("已經在清單的" + ("第一筆" if delta < 0 else "最後一筆"))
    contacts = list(project.contacts)
    a, b = contacts[row].index, contacts[target].index
    contacts[row], contacts[target] = contacts[target].with_index(a), contacts[row].with_index(b)
    swap = {a: b, b: a}
    channels = tuple(ch.with_tx_contact(swap[ch.tx_contact]) if ch.tx_contact in swap else ch
                     for ch in project.channels)
    image, _dropped = remap_members(project.image, lambda m: swap.get(m, m))
    updated = with_synced_image(replace(project, contacts=tuple(contacts), channels=channels, image=image))
    return updated, target
