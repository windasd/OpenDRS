"""RX group list tab: the 32 slots with their member contacts, and an editor dialog."""

from __future__ import annotations

from collections.abc import Callable

from PySide6.QtWidgets import QWidget

from ..f136 import Project
from ..rxgroup import MAX_RX_MEMBERS, RxGroup, has_rx_groups, rx_groups
from ..textcodec import NameCodec
from .models import contact_label
from .slot_page import MemberPickerDialog, SlotTablePage


def member_labels(project: Project, members: tuple[int, ...], codec: NameCodec) -> list[str]:
    by_number = {c.index: c for c in project.contacts}
    return [contact_label(by_number[m], codec) if m in by_number else f"#{m}（不存在）"
            for m in members]


class RxGroupDialog(MemberPickerDialog):
    """Rename one list and tick up to 32 contacts (kept in contact-list order)."""

    def __init__(self, project: Project, group: RxGroup, codec: NameCodec,
                 parent: QWidget | None = None) -> None:
        choices = [(contact_label(c, codec), c.index) for c in project.contacts]
        super().__init__(f"接收群組 {group.number}", group.name(codec), choices,
                         set(group.members), MAX_RX_MEMBERS, "聯絡人", parent)


class RxGroupPage(SlotTablePage):
    def __init__(self, on_edit: Callable[[int], None], parent: QWidget | None = None) -> None:
        super().__init__("雙擊一列可編輯。頻道的「接收群組」選擇這裡的清單；群組最多 32 個聯絡人。"
                         "刪除聯絡人時，群組成員會自動移除並重新編號。", on_edit, parent)

    def set_project(self, project: Project, codec: NameCodec) -> int:
        rows = [(g.name(codec), len(g.members),
                 "、".join(member_labels(project, g.members, codec)), g.in_use)
                for g in rx_groups(project.image)]
        return self.set_rows(rows, has_rx_groups(project.image))
