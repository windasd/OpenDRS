"""Contact import / export and name clean-up for the main window."""

from __future__ import annotations

from pathlib import Path

from PySide6.QtWidgets import QFileDialog, QMessageBox

from ..cleanup import clean_names, needs_cleaning, scan
from ..codeplug import MAX_CONTACTS
from ..contact_edit import ContactEditError, add_contact, delete_contacts, shift_contact
from ..contact_import import ImportReport, NameFormat, append_contacts, import_users
from ..contacts_csv import export_contacts, parse_contacts_csv
from ..records import Contact
from ..settings import default_dir
from .constants import CSV_FILTER, TABS
from .contact_dialog import AddContactDialog
from .radioid_dialog import RadioIdDialog


class ContactOpsMixin:
    """RadioID and CSV contact import/export for MainWindow."""

    def import_from_radioid(self) -> None:
        if not self._require_project("匯入聯絡人"):
            return
        dialog = RadioIdDialog(self._project, default_dir() / "radioid", parent=self)
        if dialog.exec() != RadioIdDialog.DialogCode.Accepted:
            return
        self.apply_import(dialog.selected_users(), dialog.name_format())

    def apply_import(self, users: list, fmt: NameFormat) -> ImportReport:
        project, report = import_users(self._project, users, fmt, self.codec)
        self._commit(project, "匯入聯絡人")
        QMessageBox.information(self, "匯入完成", report.summary())
        return report

    def export_contacts_csv(self, path: Path | None = None) -> bool:
        if not self._require_project("匯出聯絡人"):
            return False
        if path is None:
            default = (self._path.with_name(self._path.stem + "_contacts.csv") if self._path
                       else Path(self._settings.last_dir or ".") / "contacts.csv")
            chosen, _ = QFileDialog.getSaveFileName(self, "匯出聯絡人", str(default), CSV_FILTER)
            if not chosen:
                return False
            path = Path(chosen)
        try:
            path.write_bytes(export_contacts(self._project, self.codec))
        except OSError as exc:
            QMessageBox.critical(self, "匯出失敗", str(exc))
            return False
        return True

    def import_contacts_csv(self, path: Path | None = None) -> ImportReport | None:
        if not self._require_project("匯入聯絡人"):
            return None
        if path is None:
            chosen, _ = QFileDialog.getOpenFileName(self, "從 CSV 匯入聯絡人",
                                                    self._settings.last_dir, CSV_FILTER)
            if not chosen:
                return None
            path = Path(chosen)
        try:
            result = parse_contacts_csv(path.read_bytes())
        except OSError as exc:
            QMessageBox.critical(self, "讀取失敗", str(exc))
            return None
        if not result.entries:
            QMessageBox.warning(self, "沒有可匯入的資料",
                                "\n".join(result.errors[:15]) or "檔案中沒有聯絡人")
            return None
        message = f"讀到 {len(result.entries)} 筆（檔案編碼 {result.encoding}）。"
        if result.errors:
            message += f"\n另有 {len(result.errors)} 列無法使用：\n" + "\n".join(result.errors[:10])
            if len(result.errors) > 10:
                message += "\n…"
        message += "\n\n要匯入聯絡人清單嗎？（新的附加在最後；已存在相同類型與 ID 者以新名稱覆蓋，編號不變）"
        if QMessageBox.question(self, "從 CSV 匯入", message) != QMessageBox.StandardButton.Yes:
            return None
        project, report = append_contacts(self._project, list(result.entries), self.codec)
        self._commit(project, "CSV 匯入")
        QMessageBox.information(self, "匯入完成", report.summary())
        return report

    def clean_control_chars(self) -> int:
        """Strip control characters from all names after confirmation; returns records changed."""
        report = scan(self._project)
        if report.total == 0:
            QMessageBox.information(self, "清除換行字元", "沒有名稱含換行或控制字元。")
            return 0
        sample = next((c.name(self.codec) for c in self._project.contacts if needs_cleaning(c)), "")
        answer = QMessageBox.question(
            self, "清除換行字元",
            f"將清除 {report.summary()} 名稱中的換行／控制字元"
            + (f"，例如「{sample}」→「{sample.replace('␍', '').replace('␊', '').strip()}」" if sample else "")
            + "。\n\n只修改名稱，其他資料不變；可用「復原」退回。要繼續嗎？",
        )
        if answer != QMessageBox.StandardButton.Yes:
            return 0
        project, report = clean_names(self._project)
        self._commit(project, "清除換行字元")
        return report.total

    def add_contact(self, values: tuple[int, str, int] | None = None) -> int | None:
        """Append one contact (asked for when ``values`` is None); returns its row."""
        if not self._require_project("新增聯絡人"):
            return None
        if values is None:
            if len(self._project.contacts) >= MAX_CONTACTS:  # say so before asking for anything
                QMessageBox.information(self, "新增聯絡人", f"聯絡人已達對講機上限 {MAX_CONTACTS} 筆。")
                return None
            row = self.selected_row("contacts")
            kind = self._project.contacts[row].kind if row is not None else Contact.GROUP_CALL

            def problem(kind: int, name: str, call_id: int) -> str | None:
                try:
                    add_contact(self._project, kind, name, call_id, self.codec)
                except ContactEditError as exc:
                    return str(exc)
                return None

            dialog = AddContactDialog(kind, problem, parent=self)
            if dialog.exec() != AddContactDialog.DialogCode.Accepted:
                return None
            values = dialog.values()
        kind, name, call_id = values
        try:
            project, new_row = add_contact(self._project, kind, name, call_id, self.codec)
        except ContactEditError as exc:
            QMessageBox.warning(self, "新增聯絡人", str(exc))
            return None
        self._commit(project, f"新增聯絡人 {name.strip()}")
        self._tabs.setCurrentIndex(1 + [k for k, _ in TABS].index("contacts"))
        self.select_row("contacts", new_row)
        if self.selected_row("contacts") != new_row:  # hidden by the search or a filter
            self._pages["contacts"].reset()
            self.select_row("contacts", new_row)
        return new_row

    def selected_rows(self, key: str) -> list[int]:
        """Source rows of every selected cell in a table page (sorted, unique)."""
        page = self._pages[key]
        indexes = page.view.selectionModel().selectedIndexes()
        return sorted({page.proxy.mapToSource(i).row() for i in indexes})

    def delete_contacts(self, confirm: bool = True) -> bool:
        """Delete the selected contacts; later contacts are renumbered and channel TX
        contacts and RX group members follow."""
        rows = self.selected_rows("contacts")
        if not rows:
            QMessageBox.information(self, "刪除聯絡人", "請先在「聯絡人」分頁選擇要刪除的聯絡人（可多選）。")
            return False
        try:
            project, report = delete_contacts(self._project, rows, self.codec)
        except ContactEditError as exc:
            QMessageBox.warning(self, "刪除聯絡人", str(exc))
            return False
        if confirm:
            names = "、".join(self._project.contacts[r].name(self.codec) for r in rows[:5])
            more = "…" if len(rows) > 5 else ""
            lines = [f"刪除 {len(rows)} 筆聯絡人（{names}{more}）？"]
            if report.group_members_dropped:
                lines.append(f"接收群組中有 {report.group_members_dropped} 筆成員會一併移除。")
            if report.renumbered:
                lines.append(f"之後的 {report.renumbered} 筆聯絡人編號會往前遞補，"
                             "頻道發話對象與接收群組會自動跟著調整。")
            lines.append("可用「復原」退回。")
            answer = QMessageBox.question(self, "刪除聯絡人", "\n".join(lines))
            if answer != QMessageBox.StandardButton.Yes:
                return False
        self._commit(project, f"刪除聯絡人 {len(rows)} 筆")
        return True

    def shift_contact(self, delta: int) -> bool:
        """Move the selected contact up (-1) or down (+1); numbers follow everywhere."""
        rows = self.selected_rows("contacts")
        if len(rows) != 1:
            self.statusBar().showMessage("請只選一筆聯絡人再移動", 4000)
            return False
        try:
            project, new_row = shift_contact(self._project, rows[0], delta)
        except ContactEditError as exc:
            self.statusBar().showMessage(str(exc), 4000)
            return False
        self._commit(project, "上移聯絡人" if delta < 0 else "下移聯絡人")
        self.select_row("contacts", new_row)
        return True

