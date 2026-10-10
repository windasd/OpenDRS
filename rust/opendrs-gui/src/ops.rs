//! Structural edits from the menus (port of `channel_ops.py` and `contact_ops.py`): add, delete
//! and move channels, zones and contacts, the CSV imports and exports, and the name clean-up.
//! Messages, confirmations and undo labels are the Python version's.

use std::path::PathBuf;

use opendrs_contacts::contact_import::{DEFAULT_CONTACT_CAPACITY, append_contacts};
use opendrs_contacts::contacts_csv::{export_contacts, parse_contacts_csv};
use opendrs_core::channels_csv::{export_channels, import_channels};
use opendrs_core::cleanup::{clean_names, needs_cleaning, scan};
use opendrs_core::contact_edit::{delete_contacts, shift_contact};
use opendrs_core::layout::{
    add_channel as append_channel_to_zone, add_zone as layout_add_zone, channel_position,
    delete_channel as layout_delete_channel, delete_zone as layout_delete_zone,
    shift_channel as layout_shift_channel, zone_rows,
};
use opendrs_core::pystr::strip;

use crate::app::{After, Dialog, OpenDrsApp};
use crate::pages::Tab;
use crate::table::model::Table;

fn csv_dialog(app: &OpenDrsApp, title: &str) -> rfd::FileDialog {
    let dialog = rfd::FileDialog::new()
        .set_title(title)
        .add_filter("CSV", &["csv"])
        .add_filter("所有檔案", &["*"]);
    match app.last_dir() {
        Some(dir) => dialog.set_directory(dir),
        None => dialog,
    }
}

/// `{stem}_{suffix}.csv` beside the project, else in the last folder.
fn default_csv_name(app: &OpenDrsApp, suffix: &str) -> String {
    app.doc.path().and_then(|p| p.file_stem()).map_or_else(
        || format!("{suffix}.csv"),
        |stem| format!("{}_{suffix}.csv", stem.to_string_lossy()),
    )
}

impl OpenDrsApp {
    /// 「請先開啟專案檔，或從對講機讀取。」 when nothing is loaded.
    pub fn require_project(&mut self, title: &str) -> bool {
        if self.doc.has_project() {
            return true;
        }
        self.warn(title, "請先開啟專案檔，或從對講機讀取。");
        false
    }

    fn selected(&self, table: Table) -> Option<usize> {
        let state = match table {
            Table::Channels => &self.pages.tables.channels,
            Table::Zones => &self.pages.tables.zones,
            Table::Contacts => &self.pages.tables.contacts,
        };
        state.single_selection()
    }

    /// Show `tab` and select `row` in its table.
    pub fn select_row(&mut self, table: Table, row: usize) {
        self.tab = match table {
            Table::Channels => Tab::Channels,
            Table::Zones => Tab::Zones,
            Table::Contacts => Tab::Contacts,
        };
        self.pages.tables.get_mut(table).select_only(row);
    }

    // ---------------------------------------------------------------- channels and zones
    /// Duplicate the selected channel (else the last one) at the end of its zone.
    pub fn add_channel(&mut self) {
        if !self.require_project("新增頻道") {
            return;
        }
        let row = match self.selected(Table::Channels) {
            Some(row) => row,
            None if self.project().channels.is_empty() => {
                self.warn("新增頻道", "專案中沒有可複製的頻道。");
                return;
            }
            None => self.project().channels.len() - 1,
        };
        match append_channel_to_zone(self.project(), row) {
            Ok((project, new_row)) => {
                self.commit(project, "新增頻道");
                self.select_row(Table::Channels, new_row);
            }
            Err(e) => self.warn("新增頻道", e.0),
        }
    }

    pub fn delete_channel(&mut self) {
        let Some(row) = self.selected(Table::Channels) else {
            self.warn("刪除頻道", "請先在「頻道」分頁選擇要刪除的頻道。");
            return;
        };
        let outcome = channel_position(self.project(), row).and_then(|position| {
            layout_delete_channel(self.project(), row).map(|done| (position, done))
        });
        let ((zone_row, position), (project, edit)) = match outcome {
            Ok(done) => done,
            Err(e) => {
                self.warn("刪除頻道", e.0);
                return;
            }
        };
        let name = self.project().channels[row].name(self.codec);
        let zone = self.project().zones[zone_row].name(self.codec);
        let mut lines = vec![format!(
            "刪除頻道「{name}」（分區 {}「{zone}」第 {} 台）？",
            zone_row + 1,
            position + 1
        )];
        if edit.removed > 0 {
            lines.push(format!(
                "掃描清單中有 {} 筆指向此頻道，會一併移除。",
                edit.removed
            ));
        }
        if edit.renumbered > 0 {
            lines.push(format!(
                "同分區之後的頻道在掃描清單中的 {} 筆編號會自動往前調整。",
                edit.renumbered
            ));
        }
        if edit.refs_changed > 0 {
            lines.push(format!(
                "GPS上報信道／報警信道有 {} 處引用會自動調整（被刪的改為 Off）。",
                edit.refs_changed
            ));
        }
        lines.push("可用「復原」退回。".into());
        let select = (!project.channels.is_empty())
            .then(|| (Table::Channels, row.min(project.channels.len() - 1)));
        self.confirm(
            "刪除頻道",
            lines.join("\n"),
            project,
            format!("刪除頻道 {name}"),
            After {
                select,
                ..After::default()
            },
        );
    }

    pub fn shift_channel(&mut self, delta: i32) {
        let Some(row) = self.selected(Table::Channels) else {
            return;
        };
        match layout_shift_channel(self.project(), row, delta) {
            Ok((project, new_row)) => {
                self.commit(
                    project,
                    if delta < 0 {
                        "上移頻道"
                    } else {
                        "下移頻道"
                    },
                );
                self.select_row(Table::Channels, new_row);
            }
            Err(e) => self.status = e.0,
        }
    }

    /// Ask for the new zone's name (its first channel copies the selected one).
    pub fn add_zone(&mut self) {
        if !self.require_project("新增分區") {
            return;
        }
        self.dialog = Some(Dialog::AddZone {
            name: format!("ZONE{}", self.project().zones.len() + 1),
        });
    }

    pub fn add_zone_named(&mut self, name: &str) {
        let row = self
            .selected(Table::Channels)
            .unwrap_or(if self.project().channels.is_empty() {
                usize::MAX
            } else {
                0
            });
        let name = strip(name).to_string();
        match layout_add_zone(self.project(), row, &name, self.codec) {
            Ok((project, zone_row)) => {
                self.commit(project, format!("新增分區 {name}"));
                self.select_row(Table::Zones, zone_row);
            }
            Err(e) => self.warn("新增分區", e.0),
        }
    }

    /// The zone selected on the zones tab, else the zone of the selected channel.
    fn zone_for_action(&self) -> Option<usize> {
        if self.tab == Tab::Zones {
            return self.selected(Table::Zones);
        }
        let row = self.selected(Table::Channels)?;
        zone_rows(self.project()).get(row).copied().flatten()
    }

    pub fn delete_zone(&mut self) {
        let Some(zone_row) = self.zone_for_action() else {
            self.warn("刪除分區", "請先在「分區」分頁選擇要刪除的分區。");
            return;
        };
        let (project, edit) = match layout_delete_zone(self.project(), zone_row) {
            Ok(done) => done,
            Err(e) => {
                self.warn("刪除分區", e.0);
                return;
            }
        };
        let zone = self.project().zones[zone_row];
        let name = zone.name(self.codec);
        let mut lines = vec![format!(
            "刪除分區 {}「{name}」以及其中的 {} 台頻道？",
            zone_row + 1,
            zone.channel_count()
        )];
        if edit.removed > 0 {
            lines.push(format!(
                "掃描清單中有 {} 筆指向這個分區的頻道，會一併移除。",
                edit.removed
            ));
        }
        if edit.renumbered > 0 {
            lines.push(format!(
                "之後的分區往前遞補，掃描清單中 {} 筆的分區編號會自動調整。",
                edit.renumbered
            ));
        }
        if edit.refs_changed > 0 {
            lines.push(format!(
                "GPS上報信道／報警信道有 {} 處引用會自動調整（被刪的改為 Off）。",
                edit.refs_changed
            ));
        }
        lines.push("可用「復原」退回。".into());
        self.confirm(
            "刪除分區",
            lines.join("\n"),
            project,
            format!("刪除分區 {name}"),
            After::default(),
        );
    }

    pub fn export_channels_csv(&mut self) {
        if !self.require_project("匯出頻道") {
            return;
        }
        let Some(path) = csv_dialog(self, "匯出頻道")
            .set_file_name(default_csv_name(self, "channels"))
            .save_file()
        else {
            return;
        };
        self.write_channels_csv(&path);
    }

    pub fn write_channels_csv(&mut self, path: &std::path::Path) -> bool {
        let data = match export_channels(self.project(), self.codec) {
            Ok(data) => data,
            Err(e) => {
                self.warn("匯出失敗", e.0);
                return false;
            }
        };
        match std::fs::write(path, data) {
            Ok(()) => {
                self.status = format!("已匯出頻道到 {}", path.display());
                true
            }
            Err(e) => {
                self.warn("匯出失敗", e.to_string());
                false
            }
        }
    }

    pub fn import_channels_csv(&mut self) {
        if !self.require_project("匯入頻道") {
            return;
        }
        if let Some(path) = csv_dialog(self, "從 CSV 匯入頻道").pick_file() {
            self.read_channels_csv(&path);
        }
    }

    pub fn read_channels_csv(&mut self, path: &std::path::Path) {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) => {
                self.warn("讀取失敗", e.to_string());
                return;
            }
        };
        let result = import_channels(self.project(), &data, self.codec);
        let mut problems = String::new();
        if !result.errors.is_empty() {
            problems = format!(
                "\n\n下列 {} 項無法套用（整列略過）：\n{}",
                result.errors.len(),
                result
                    .errors
                    .iter()
                    .take(12)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            if result.errors.len() > 12 {
                problems.push_str("\n…");
            }
        }
        if result.updated == 0 && result.added == 0 {
            self.warn(
                "從 CSV 匯入頻道",
                format!("沒有需要變更的頻道（{}）。{problems}", result.summary()),
            );
            return;
        }
        let text = format!(
            "{}（檔案編碼 {}）。{problems}\n\n要套用嗎？可用「復原」退回。",
            result.summary(),
            result.encoding
        );
        let after = After {
            status: Some(result.summary()),
            ..After::default()
        };
        self.confirm(
            "從 CSV 匯入頻道",
            text,
            result.project,
            "CSV 匯入頻道".into(),
            after,
        );
    }

    // ---------------------------------------------------------------- contacts
    fn selected_contacts(&self) -> Vec<usize> {
        self.pages
            .tables
            .contacts
            .selected
            .iter()
            .copied()
            .collect()
    }

    pub fn delete_contacts(&mut self) {
        let rows = self.selected_contacts();
        if rows.is_empty() {
            self.warn(
                "刪除聯絡人",
                "請先在「聯絡人」分頁選擇要刪除的聯絡人（可多選）。",
            );
            return;
        }
        let (project, report) = match delete_contacts(self.project(), &rows, self.codec) {
            Ok(done) => done,
            Err(e) => {
                self.warn("刪除聯絡人", e.0);
                return;
            }
        };
        let names: Vec<String> = rows
            .iter()
            .take(5)
            .map(|r| self.project().contacts[*r].name(self.codec))
            .collect();
        let more = if rows.len() > 5 { "…" } else { "" };
        let mut lines = vec![format!(
            "刪除 {} 筆聯絡人（{}{more}）？",
            rows.len(),
            names.join("、")
        )];
        if report.group_members_dropped > 0 {
            lines.push(format!(
                "接收群組中有 {} 筆成員會一併移除。",
                report.group_members_dropped
            ));
        }
        if report.renumbered > 0 {
            lines.push(format!(
                "之後的 {} 筆聯絡人編號會往前遞補，頻道發話對象與接收群組會自動跟著調整。",
                report.renumbered
            ));
        }
        lines.push("可用「復原」退回。".into());
        self.pages.tables.contacts.selected.clear();
        self.confirm(
            "刪除聯絡人",
            lines.join("\n"),
            project,
            format!("刪除聯絡人 {} 筆", rows.len()),
            After::default(),
        );
    }

    pub fn shift_contact(&mut self, delta: i32) {
        let rows = self.selected_contacts();
        let [row] = rows.as_slice() else {
            self.status = "請只選一筆聯絡人再移動".into();
            return;
        };
        match shift_contact(self.project(), *row, delta) {
            Ok((project, new_row)) => {
                self.commit(
                    project,
                    if delta < 0 {
                        "上移聯絡人"
                    } else {
                        "下移聯絡人"
                    },
                );
                self.select_row(Table::Contacts, new_row);
            }
            Err(e) => self.status = e.0,
        }
    }

    pub fn export_contacts_csv(&mut self) {
        if !self.require_project("匯出聯絡人") {
            return;
        }
        let Some(path) = csv_dialog(self, "匯出聯絡人")
            .set_file_name(default_csv_name(self, "contacts"))
            .save_file()
        else {
            return;
        };
        if let Err(e) = std::fs::write(&path, export_contacts(self.project(), self.codec)) {
            self.warn("匯出失敗", e.to_string());
        }
    }

    pub fn import_contacts_csv(&mut self) {
        if !self.require_project("匯入聯絡人") {
            return;
        }
        if let Some(path) = csv_dialog(self, "從 CSV 匯入聯絡人").pick_file() {
            self.read_contacts_csv(&path);
        }
    }

    pub fn read_contacts_csv(&mut self, path: &std::path::Path) {
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) => {
                self.warn("讀取失敗", e.to_string());
                return;
            }
        };
        let result = match parse_contacts_csv(&data) {
            Ok(result) => result,
            Err(e) => {
                self.warn("沒有可匯入的資料", format!("無法解析 CSV：{e}"));
                return;
            }
        };
        if result.entries.is_empty() {
            let errors = result
                .errors
                .iter()
                .take(15)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n");
            self.warn(
                "沒有可匯入的資料",
                if errors.is_empty() {
                    "檔案中沒有聯絡人".into()
                } else {
                    errors
                },
            );
            return;
        }
        let mut message = format!(
            "讀到 {} 筆（檔案編碼 {}）。",
            result.entries.len(),
            result.encoding
        );
        if !result.errors.is_empty() {
            message.push_str(&format!(
                "\n另有 {} 列無法使用：\n{}",
                result.errors.len(),
                result
                    .errors
                    .iter()
                    .take(10)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
            if result.errors.len() > 10 {
                message.push_str("\n…");
            }
        }
        message.push_str("\n\n要匯入聯絡人清單嗎？（新的附加在最後；已存在相同類型與 ID 者以新名稱覆蓋，編號不變）");
        match append_contacts(
            self.project(),
            &result.entries,
            self.codec,
            DEFAULT_CONTACT_CAPACITY,
        ) {
            Ok((project, report)) => {
                let after = After {
                    info: Some(("匯入完成".into(), report.summary())),
                    ..After::default()
                };
                self.confirm("從 CSV 匯入", message, project, "CSV 匯入".into(), after);
            }
            Err(e) => self.warn("從 CSV 匯入", e.to_string()),
        }
    }

    /// Strip line breaks and other control characters from every name, after confirmation.
    pub fn clean_control_chars(&mut self) {
        let report = scan(self.project());
        if report.total() == 0 {
            self.warn("清除換行字元", "沒有名稱含換行或控制字元。");
            return;
        }
        let sample = self
            .project()
            .contacts
            .iter()
            .find(|c| needs_cleaning(c.name_field()))
            .map(|c| c.name(self.codec))
            .unwrap_or_default();
        let example = if sample.is_empty() {
            String::new()
        } else {
            let cleaned = strip(&sample.replace(['␍', '␊'], "")).to_string();
            format!("，例如「{sample}」→「{cleaned}」")
        };
        let text = format!(
            "將清除 {} 名稱中的換行／控制字元{example}。\n\n只修改名稱，其他資料不變；可用「復原」退回。要繼續嗎？",
            report.summary()
        );
        let (project, _) = clean_names(self.project());
        self.confirm(
            "清除換行字元",
            text,
            project,
            "清除換行字元".into(),
            After::default(),
        );
    }

    /// Where file dialogs start: the last folder used.
    pub fn last_dir(&self) -> Option<PathBuf> {
        let dir = PathBuf::from(&self.settings().last_dir);
        (!dir.as_os_str().is_empty() && dir.is_dir()).then_some(dir)
    }
}
