"""Channel and zone structure edits (add / delete) for the main window."""

from __future__ import annotations

from pathlib import Path

from PySide6.QtWidgets import QFileDialog, QInputDialog, QMessageBox

from ..channels_csv import ChannelImport, export_channels, import_channels
from ..layout import LayoutError, add_channels, channel_position, zone_rows
from ..layout import add_channel as append_channel_to_zone
from ..layout import add_zone as layout_add_zone
from ..layout import delete_channel as layout_delete_channel
from ..layout import delete_zone as layout_delete_zone
from ..layout import shift_channel as layout_shift_channel
from ..textcodec import NameEncodingError
from .batch_dialog import BatchAddDialog, BatchPlan
from .constants import CSV_FILTER, TABS


class ChannelOpsMixin:
    """Row selection helpers and structural channel / zone edits for MainWindow."""

    def selected_row(self, key: str) -> int | None:
        page = self._pages[key]
        index = page.view.currentIndex()
        return page.proxy.mapToSource(index).row() if index.isValid() else None

    def select_row(self, key: str, row: int) -> None:
        page = self._pages[key]
        index = page.proxy.mapFromSource(self._models[key].index(row, 1))
        if index.isValid():
            page.view.setCurrentIndex(index)
            page.view.scrollTo(index)

    def add_channel(self) -> int | None:
        """Duplicate the selected channel at the end of its zone; returns the new row."""
        if not self._require_project("新增頻道"):
            return None
        row = self.selected_row("channels")
        if row is None:
            if not self._project.channels:
                QMessageBox.information(self, "新增頻道", "專案中沒有可複製的頻道。")
                return None
            row = len(self._project.channels) - 1
        try:
            project, new_row = append_channel_to_zone(self._project, row)
        except LayoutError as exc:
            QMessageBox.warning(self, "新增頻道", str(exc))
            return None
        self._commit(project, "新增頻道")
        self._tabs.setCurrentIndex(1 + [k for k, _ in TABS].index("channels"))
        self.select_row("channels", new_row)
        return new_row

    def delete_channel(self, confirm: bool = True) -> bool:
        """Delete the selected channel after confirmation; scan lists are fixed up."""
        row = self.selected_row("channels")
        if row is None:
            QMessageBox.information(self, "刪除頻道", "請先在「頻道」分頁選擇要刪除的頻道。")
            return False
        try:
            zone_row, position = channel_position(self._project, row)
            project, edit = layout_delete_channel(self._project, row)
        except LayoutError as exc:
            QMessageBox.warning(self, "刪除頻道", str(exc))
            return False
        name = self._project.channels[row].name(self.codec)
        zone = self._project.zones[zone_row].name(self.codec)
        if confirm:
            lines = [f"刪除頻道「{name}」（分區 {zone_row + 1}「{zone}」第 {position + 1} 台）？"]
            if edit.removed:
                lines.append(f"掃描清單中有 {edit.removed} 筆指向此頻道，會一併移除。")
            if edit.renumbered:
                lines.append(f"同分區之後的頻道在掃描清單中的 {edit.renumbered} 筆編號會自動往前調整。")
            if edit.refs_changed:
                lines.append(f"GPS上報信道／報警信道有 {edit.refs_changed} 處引用會自動調整（被刪的改為 Off）。")
            lines.append("可用「復原」退回。")
            if QMessageBox.question(self, "刪除頻道", "\n".join(lines)) != QMessageBox.StandardButton.Yes:
                return False
        self._commit(project, f"刪除頻道 {name}")
        if project.channels:
            self.select_row("channels", min(row, len(project.channels) - 1))
        return True

    def _zone_for_action(self) -> int | None:
        """The zone selected on the zones tab, else the zone of the selected channel."""
        zones_tab = 1 + [k for k, _ in TABS].index("zones")
        if self._tabs.currentIndex() == zones_tab:
            return self.selected_row("zones")
        row = self.selected_row("channels")
        if row is None:
            return None
        return zone_rows(self._project)[row]

    def add_zone(self, name: str | None = None) -> int | None:
        """Append a zone whose first channel copies the selected channel."""
        if not self._require_project("新增分區"):
            return None
        row = self.selected_row("channels")
        if row is None:
            row = 0 if self._project.channels else -1
        if name is None:
            name, ok = QInputDialog.getText(self, "新增分區", "分區名稱（新分區的第一台頻道會複製目前選取的頻道）：",
                                            text=f"ZONE{len(self._project.zones) + 1}")
            if not ok:
                return None
        try:
            project, zone_row = layout_add_zone(self._project, row, name.strip(), self.codec)
        except (LayoutError, NameEncodingError) as exc:
            QMessageBox.warning(self, "新增分區", str(exc))
            return None
        self._commit(project, f"新增分區 {name.strip()}")
        self._tabs.setCurrentIndex(1 + [k for k, _ in TABS].index("zones"))
        self.select_row("zones", zone_row)
        return zone_row

    def delete_zone(self, confirm: bool = True) -> bool:
        """Delete the selected zone and its channels; scan lists follow."""
        zone_row = self._zone_for_action()
        if zone_row is None:
            QMessageBox.information(self, "刪除分區", "請先在「分區」分頁選擇要刪除的分區。")
            return False
        try:
            project, edit = layout_delete_zone(self._project, zone_row)
        except LayoutError as exc:
            QMessageBox.warning(self, "刪除分區", str(exc))
            return False
        zone = self._project.zones[zone_row]
        name = zone.name(self.codec)
        if confirm:
            lines = [f"刪除分區 {zone_row + 1}「{name}」以及其中的 {zone.channel_count} 台頻道？"]
            if edit.removed:
                lines.append(f"掃描清單中有 {edit.removed} 筆指向這個分區的頻道，會一併移除。")
            if edit.renumbered:
                lines.append(f"之後的分區往前遞補，掃描清單中 {edit.renumbered} 筆的分區編號會自動調整。")
            if edit.refs_changed:
                lines.append(f"GPS上報信道／報警信道有 {edit.refs_changed} 處引用會自動調整（被刪的改為 Off）。")
            lines.append("可用「復原」退回。")
            if QMessageBox.question(self, "刪除分區", "\n".join(lines)) != QMessageBox.StandardButton.Yes:
                return False
        self._commit(project, f"刪除分區 {name}")
        return True

    def shift_channel(self, delta: int) -> bool:
        """Move the selected channel up (-1) or down (+1) inside its zone."""
        row = self.selected_row("channels")
        if row is None:
            return False
        try:
            project, new_row = layout_shift_channel(self._project, row, delta)
        except LayoutError as exc:
            self.statusBar().showMessage(str(exc), 4000)
            return False
        self._commit(project, "上移頻道" if delta < 0 else "下移頻道")
        self.select_row("channels", new_row)
        return True

    def batch_add_channels(self, plan: BatchPlan | None = None) -> bool:
        """ADD++: several copies of the selected channel with stepped frequencies."""
        if not self._require_project("批次新增頻道"):
            return False
        row = self.selected_row("channels")
        if row is None:
            if not self._project.channels:
                QMessageBox.information(self, "批次新增頻道", "專案中沒有可作為範本的頻道。")
                return False
            row = len(self._project.channels) - 1
        zone_row = zone_rows(self._project)[row]
        if plan is None:
            zone = self._project.zones[zone_row].name(self.codec) if zone_row is not None else "?"
            dialog = BatchAddDialog(self._project.channels[row], self.codec, self._project.device.bands,
                                    f"{(zone_row or 0) + 1}. {zone}", parent=self)
            if dialog.exec() != BatchAddDialog.DialogCode.Accepted:
                return False
            plan = dialog.plan()
        try:
            project, rows = add_channels(self._project, row, plan.count, plan.start_rx_hz, plan.step_hz,
                                         plan.tx_offset_hz, plan.name_prefix, plan.first_number, self.codec)
        except LayoutError as exc:
            QMessageBox.warning(self, "批次新增頻道", str(exc))
            return False
        self._commit(project, f"批次新增頻道 {plan.count} 台")
        self._tabs.setCurrentIndex(1 + [k for k, _ in TABS].index("channels"))
        self.select_row("channels", rows.stop - 1)
        return True

    # ------------------------------------------------------------- CSV
    def export_channels_csv(self, path: Path | None = None) -> bool:
        if not self._require_project("匯出頻道"):
            return False
        if path is None:
            default = (self._path.with_name(self._path.stem + "_channels.csv") if self._path
                       else Path(self._settings.last_dir or ".") / "channels.csv")
            chosen, _ = QFileDialog.getSaveFileName(self, "匯出頻道", str(default), CSV_FILTER)
            if not chosen:
                return False
            path = Path(chosen)
        try:
            path.write_bytes(export_channels(self._project, self.codec))
        except (OSError, LayoutError) as exc:
            QMessageBox.critical(self, "匯出失敗", str(exc))
            return False
        self.statusBar().showMessage(f"已匯出頻道到 {path}", 8000)
        return True

    def import_channels_csv(self, path: Path | None = None, confirm: bool = True) -> ChannelImport | None:
        """Update / add channels from a CSV (see channels_csv); one undo step."""
        if not self._require_project("匯入頻道"):
            return None
        if path is None:
            chosen, _ = QFileDialog.getOpenFileName(self, "從 CSV 匯入頻道", self._settings.last_dir, CSV_FILTER)
            if not chosen:
                return None
            path = Path(chosen)
        try:
            result = import_channels(self._project, path.read_bytes(), self.codec)
        except OSError as exc:
            QMessageBox.critical(self, "讀取失敗", str(exc))
            return None
        problems = ""
        if result.errors:
            problems = f"\n\n下列 {len(result.errors)} 項無法套用（整列略過）：\n" + "\n".join(result.errors[:12])
            if len(result.errors) > 12:
                problems += "\n…"
        if result.updated == 0 and result.added == 0:
            QMessageBox.information(self, "從 CSV 匯入頻道", f"沒有需要變更的頻道（{result.summary()}）。{problems}")
            return result
        if confirm:
            answer = QMessageBox.question(
                self, "從 CSV 匯入頻道",
                f"{result.summary()}（檔案編碼 {result.encoding}）。{problems}\n\n要套用嗎？可用「復原」退回。")
            if answer != QMessageBox.StandardButton.Yes:
                return None
        self._commit(result.project, "CSV 匯入頻道")
        self.statusBar().showMessage(result.summary(), 8000)
        return result

