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

from .codeplug import (
    CONTACT_SIZE,
    CONTACTS_OFFSET,
    MAX_CONTACTS,
    image_length_for,
    with_synced_image,
)
from .f136 import Project
from .records import NAME_SIZE, Contact
from .rxgroup import remap_members
from .textcodec import NameCodec, NameEncodingError, encode_name


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


def add_contact(project: Project, kind: int, name: str, call_id: int, codec: NameCodec,
                capacity: int = MAX_CONTACTS) -> tuple[Project, int]:
    """Append one contact after the last; returns (project, new row).

    Nothing else is renumbered, so every reference stays valid. A contact with the same
    type and ID is refused rather than renamed (an import overwrites its name instead).
    """
    if kind not in Contact.KINDS:
        raise ContactEditError(f"不支援的聯絡人類型 {kind}")
    if not 1 <= call_id <= Contact.MAX_ID:
        raise ContactEditError(f"ID 必須介於 1 與 {Contact.MAX_ID:,}")
    if len(project.contacts) >= capacity:
        raise ContactEditError(f"聯絡人已達對講機上限 {capacity} 筆")
    text = name.strip()
    if not text:
        raise ContactEditError("請輸入名稱")
    if any(ord(ch) < 0x20 or ord(ch) == 0x7F for ch in text):
        raise ContactEditError("名稱不可包含換行等控制字元")
    for contact in project.contacts:
        if (contact.kind, contact.call_id) == (kind, call_id):
            raise ContactEditError(f"已有相同類型與 ID 的聯絡人：#{contact.index} {contact.name(codec)}")
    try:
        field = encode_name(text, NAME_SIZE, codec)
    except NameEncodingError as exc:
        raise ContactEditError(str(exc)) from exc
    number = max((c.index for c in project.contacts), default=0) + 1
    contacts = (*project.contacts, Contact.build(number, kind, field, call_id))
    order = project.section_order
    if "Contacts" not in order:
        order = ("Contacts", *order)
    updated = with_synced_image(replace(project, contacts=contacts, section_order=order))
    return updated, len(contacts) - 1
