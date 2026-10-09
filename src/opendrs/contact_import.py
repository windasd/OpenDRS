"""Turn RadioID users into private-call contact records."""

from __future__ import annotations

import re
from collections.abc import Callable
from dataclasses import dataclass, field, replace
from enum import Enum

# The radio firmware treats more than 1000 contacts as corrupt: FUN_030272fc checks
# ``count < 0x3E9`` and otherwise zeroes the count. (The legacy CPS help text says
# 2000, which the radio would reject.)
from .codeplug import MAX_CONTACTS as DEFAULT_CONTACT_CAPACITY  # noqa: E402
from .codeplug import with_synced_image
from .f136 import Project
from .radioid import RadioIdUser
from .records import NAME_SIZE, Contact
from .textcodec import NameCodec, fit_name


class NameFormat(str, Enum):
    CALLSIGN = "callsign"
    CALLSIGN_NAME = "callsign_name"
    NAME = "name"

    @property
    def label(self) -> str:
        return {"callsign": "呼號（N0CALL）", "callsign_name": "呼號 名字（N0CALL James）",
                "name": "名字（James）"}[self.value]


def contact_text(user: RadioIdUser, fmt: NameFormat) -> str:
    if fmt is NameFormat.CALLSIGN:
        return user.callsign or str(user.radio_id)
    if fmt is NameFormat.NAME:
        return user.first_name or user.callsign or str(user.radio_id)
    return " ".join(p for p in (user.callsign, user.first_name) if p) or str(user.radio_id)


@dataclass(frozen=True)
class ImportReport:
    added: int
    updated: int  # existing (type, ID): name overwritten
    unchanged: int  # existing (type, ID): name already identical
    over_capacity: int
    shortened: tuple[tuple[int, str, str], ...]  # (radio id, wanted, stored)
    kept: int = 0  # existing (type, ID) with a custom name: left untouched

    def summary(self) -> str:
        lines = [f"新增 {self.added} 筆聯絡人。"]
        if self.updated:
            lines.append(f"{self.updated} 筆已存在，名稱已更新（編號不變）。")
        if self.unchanged:
            lines.append(f"{self.unchanged} 筆已存在且名稱相同，未變更。")
        if self.kept:
            lines.append(f"{self.kept} 筆已存在且為自訂名稱，保留原名稱。")
        if self.over_capacity:
            lines.append(f"{self.over_capacity} 筆因超過容量上限未匯入。")
        if self.shortened:
            lines.append(f"{len(self.shortened)} 筆名稱超過 {NAME_SIZE} bytes 或含無法表示的字元，已截短。")
        return "\n".join(lines)


# ITU-style amateur callsign: 1–3 char prefix (with a letter), a digit, 1–4 letter suffix,
# optionally followed by a portable/mobile indicator (N0CALL/P, N0CALL-M).
_CALLSIGN = re.compile(r"^(?=[A-Z0-9]{0,2}[A-Z])[A-Z0-9]{1,3}[0-9][A-Z]{1,4}(?:[/-][A-Z0-9]{1,4})?$")
_TOKEN_SPLIT = re.compile(r"[\s,;/|_]+")
_MIN_PREFIX = 3  # a truncated first name ("Christophe") still counts as a match


def looks_like_callsign(token: str) -> bool:
    return bool(_CALLSIGN.match(token.upper()))


def is_radioid_style_name(existing: str, user: RadioIdUser) -> bool:
    """True when an existing contact name was evidently taken from RadioID data, so a
    fresh import may replace it: empty, the bare ID, a callsign (also an outdated one —
    callsigns change while the DMR ID stays), or the user's own callsign/first name.
    Anything else (老王, 台中中繼, Dad) is a name the owner chose and is kept.
    """
    text = "".join(ch if ch.isprintable() and ch not in "␍␊" else " " for ch in existing).strip()
    if not text or text == str(user.radio_id):
        return True
    call = user.callsign.casefold()
    first = user.first_name.casefold()
    for token in _TOKEN_SPLIT.split(text):
        if not token:
            continue
        low = token.casefold()
        if looks_like_callsign(token) or low == call or low == first:
            return True
        if len(low) >= _MIN_PREFIX and first.startswith(low):
            return True
    return False


def existing_private_ids(project: Project) -> set[int]:
    return {c.call_id for c in project.contacts if c.kind == Contact.PRIVATE_CALL}


@dataclass(frozen=True)
class NewContact:
    kind: int
    text: str
    call_id: int
    # For an existing (type, ID): decides from its current name whether to overwrite.
    # None = always overwrite (CSV: the file is the user's own data).
    replace_if: Callable[[str], bool] | None = field(default=None, compare=False)


def import_users(project: Project, users: list[RadioIdUser], fmt: NameFormat,
                 codec: NameCodec, capacity: int = DEFAULT_CONTACT_CAPACITY,
                 ) -> tuple[Project, ImportReport]:
    entries = [NewContact(Contact.PRIVATE_CALL, contact_text(u, fmt), u.radio_id,
                          replace_if=lambda name, u=u: is_radioid_style_name(name, u))
               for u in users]
    return append_contacts(project, entries, codec, capacity)


def append_contacts(project: Project, entries: list[NewContact], codec: NameCodec,
                    capacity: int = DEFAULT_CONTACT_CAPACITY) -> tuple[Project, ImportReport]:
    """Merge contacts into the list: an existing (type, ID) gets its name overwritten
    (unless the entry's ``replace_if`` rejects the current name), anything else is
    appended after the last contact.

    Existing records keep their index, type and ID — only the name bytes change — so
    anything in the settings image that refers to a contact by number keeps pointing
    at the same one. If the same (type, ID) occurs twice in ``entries``, the later
    name wins.
    """
    contacts = list(project.contacts)
    where = {(c.kind, c.call_id): i for i, c in enumerate(contacts)}
    original = len(contacts)
    next_index = max((c.index for c in contacts), default=0) + 1
    room = max(capacity - original, 0)
    shortened: list[tuple[int, str, str]] = []
    added = over = 0
    touched: set[int] = set()
    kept: set[int] = set()
    for entry in entries:
        key = (entry.kind, entry.call_id)
        if key not in where and added >= room:
            over += 1
            continue
        if key in where and where[key] < original and entry.replace_if is not None                 and not entry.replace_if(project.contacts[where[key]].name(codec)):
            kept.add(where[key])
            continue
        field, stored = fit_name(entry.text, NAME_SIZE, codec)
        if stored != entry.text:
            shortened.append((entry.call_id, entry.text, stored))
        if key in where:
            i = where[key]
            contacts[i] = contacts[i].with_name_field(field)
            if i < original:
                touched.add(i)
            continue
        contacts.append(Contact.build(next_index, entry.kind, field, entry.call_id))
        where[key] = len(contacts) - 1
        added += 1
        next_index += 1
    kept -= touched
    updated_n = sum(1 for i in touched if contacts[i].raw != project.contacts[i].raw)
    order = project.section_order
    if added and "Contacts" not in order:
        order = ("Contacts", *order)
    result = with_synced_image(replace(project, contacts=tuple(contacts), section_order=order))
    return result, ImportReport(added, updated_n, len(touched) - updated_n, over,
                                tuple(shortened), len(kept))
