//! The menu bar and the toolbar of the main window (port of `_build_actions` in
//! `main_window.py`); items whose Rust page or dialog is missing are shown greyed out.

use std::path::{Path, PathBuf};

use eframe::egui::{self, KeyboardShortcut, ViewportCommand};

use crate::app::{
    DELETE, FIND, MOVE_DOWN, MOVE_UP, OPEN, OpenDrsApp, PRINT, REDO, SAVE, SAVE_AS, ThemeMode, UNDO,
};
use crate::radio;

impl OpenDrsApp {
    pub(crate) fn shortcut_button(
        ui: &mut egui::Ui,
        text: impl Into<String>,
        shortcut: &KeyboardShortcut,
        enabled: bool,
    ) -> bool {
        let label = ui.ctx().format_shortcut(shortcut);
        ui.add_enabled(enabled, egui::Button::new(text.into()).shortcut_text(label))
            .clicked()
    }

    /// A menu item with its shortcut text and tooltip; true when clicked.
    pub(crate) fn item(
        ui: &mut egui::Ui,
        text: &str,
        shortcut: Option<&KeyboardShortcut>,
        tip: &str,
    ) -> bool {
        let mut button = egui::Button::new(text);
        if let Some(shortcut) = shortcut {
            button = button.shortcut_text(ui.ctx().format_shortcut(shortcut));
        }
        let response = ui.add(button);
        let response = if tip.is_empty() {
            response
        } else {
            response.on_hover_text(tip)
        };
        response.clicked()
    }

    pub(crate) fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("檔案", |ui| self.file_menu(ui));
            ui.menu_button("編輯", |ui| self.edit_menu(ui));
            ui.menu_button("聯絡人", |ui| {
                if ui.button("從 RadioID 匯入聯絡人…").clicked() {
                    self.import_from_radioid();
                }
                if ui.button("從 CSV 匯入聯絡人…").clicked() {
                    self.import_contacts_csv();
                }
                if ui.button("匯出聯絡人 CSV…").clicked() {
                    self.export_contacts_csv();
                }
                ui.separator();
                if Self::item(
                    ui,
                    "上移聯絡人",
                    Some(&MOVE_UP),
                    "與上一筆交換位置；頻道發話對象與接收群組跟著調整",
                ) {
                    self.shift_contact(-1);
                }
                if Self::item(
                    ui,
                    "下移聯絡人",
                    Some(&MOVE_DOWN),
                    "與下一筆交換位置；頻道發話對象與接收群組跟著調整",
                ) {
                    self.shift_contact(1);
                }
                if Self::item(
                    ui,
                    "刪除選取的聯絡人",
                    Some(&DELETE),
                    "刪除選取的聯絡人；編號往前遞補，頻道與接收群組自動調整",
                ) {
                    self.delete_contacts();
                }
            });
            ui.menu_button("頻道／分區", |ui| {
                if Self::item(
                    ui,
                    "新增頻道（複製選取的頻道）",
                    None,
                    "複製目前選取的頻道，加在同一分區的最後面；掃描清單與其他頻道的編號不受影響",
                ) {
                    self.add_channel();
                }
                if Self::item(
                    ui,
                    "批次新增頻道…",
                    None,
                    "以選取的頻道為範本，一次新增多台（頻率依間隔遞增、名稱依序編號）",
                ) {
                    self.batch_add_channels();
                }
                if Self::item(
                    ui,
                    "刪除頻道",
                    Some(&DELETE),
                    "刪除選取的頻道；同分區之後的頻道編號與掃描清單會自動調整",
                ) {
                    self.delete_channel();
                }
                if Self::item(
                    ui,
                    "上移頻道",
                    Some(&MOVE_UP),
                    "與同分區的上一台交換位置；掃描清單等引用跟著調整",
                ) {
                    self.shift_channel(-1);
                }
                if Self::item(
                    ui,
                    "下移頻道",
                    Some(&MOVE_DOWN),
                    "與同分區的下一台交換位置；掃描清單等引用跟著調整",
                ) {
                    self.shift_channel(1);
                }
                ui.separator();
                if Self::item(
                    ui,
                    "新增分區…",
                    None,
                    "在最後新增一個分區，第一台頻道複製目前選取的頻道",
                ) {
                    self.add_zone();
                }
                if Self::item(
                    ui,
                    "刪除分區",
                    None,
                    "刪除選取的分區與其中的頻道；之後的分區與掃描清單自動調整",
                ) {
                    self.delete_zone();
                }
                ui.separator();
                if Self::item(
                    ui,
                    "匯出頻道 CSV…",
                    None,
                    "把所有頻道匯出成 Excel 可開啟的 CSV（依分區排列）",
                ) {
                    self.export_channels_csv();
                }
                if Self::item(
                    ui,
                    "從 CSV 匯入頻道…",
                    None,
                    "依「分區＋頻道」更新既有頻道，頻道欄空白則新增到該分區最後；不會刪除頻道",
                ) {
                    self.import_channels_csv();
                }
            });
            ui.menu_button("對講機", |ui| {
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if Self::item(ui, "讀取對講機…", None, radio::READ_TIP) {
                        self.read_from_radio();
                    }
                    if Self::item(ui, "寫入對講機…", None, radio::WRITE_TIP) {
                        self.write_to_radio();
                    }
                    if Self::item(ui, "校正對講機時鐘…", None, radio::CLOCK_TIP) {
                        self.sync_radio_clock();
                    }
                });
                ui.separator();
                if Self::item(
                    ui,
                    "備份管理…",
                    None,
                    "瀏覽每次讀取／寫入前自動保留的備份：開啟、與目前專案比較、刪除",
                ) {
                    self.backup_manager();
                }
                if Self::item(
                    ui,
                    "韌體升級（IAP）…",
                    None,
                    "檢查韌體檔是否適用這台對講機，再透過 IAP 升級（內建或原廠工具）",
                ) {
                    self.firmware_update();
                }
                if Self::item(
                    ui,
                    "讀取錄音…",
                    None,
                    "列出並下載對講機的通話錄音，轉成 WAV（取代原廠 Wav.exe）",
                ) {
                    self.recordings();
                }
            });
            ui.menu_button("工具", |ui| {
                if Self::item(
                    ui,
                    "比較專案…",
                    None,
                    "與另一個專案逐位元組比較，用來找出設定存放的位置",
                ) {
                    self.compare_dialog();
                }
                if ui
                    .button("關於本軟體…")
                    .on_hover_text("版本、作者與執行環境")
                    .clicked()
                {
                    self.about();
                }
            });
            ui.menu_button("檢視", |ui| {
                ui.menu_button("外觀", |ui| {
                    let current = self.theme();
                    for mode in ThemeMode::ALL {
                        if ui.radio(current == mode, mode.label()).clicked() {
                            self.set_theme(ui.ctx(), mode);
                        }
                    }
                });
            });
        });
    }

    pub(crate) fn file_menu(&mut self, ui: &mut egui::Ui) {
        if Self::shortcut_button(ui, "開啟…", &OPEN, true) {
            self.request_open(None);
        }
        let recent: Vec<String> = self
            .settings()
            .recent_files
            .iter()
            .filter(|p| Path::new(p).is_file())
            .cloned()
            .collect();
        ui.add_enabled_ui(!recent.is_empty(), |ui| {
            ui.menu_button("最近開啟", |ui| {
                for path in recent {
                    if ui.button(&path).clicked() {
                        self.request_open(Some(PathBuf::from(path)));
                    }
                }
            });
        });
        let has = self.doc.has_project();
        if Self::shortcut_button(ui, "儲存", &SAVE, has) {
            self.save();
        }
        if Self::shortcut_button(ui, "另存新檔…", &SAVE_AS, has) {
            self.save_as();
        }
        ui.separator();
        if Self::item(
            ui,
            "列印…",
            Some(&PRINT),
            "列印裝置資訊、各分區頻道、聯絡人、接收群組與掃描清單（A4 橫式）",
        ) {
            self.print_project();
        }
        if Self::item(ui, "預覽列印…", None, "") {
            self.print_preview();
        }
        if Self::item(ui, "匯出 PDF…", None, "把同一份列印內容存成 PDF") {
            self.export_pdf();
        }
        ui.separator();
        if ui.button("結束").clicked() {
            ui.ctx().send_viewport_cmd(ViewportCommand::Close);
        }
    }

    pub(crate) fn edit_menu(&mut self, ui: &mut egui::Ui) {
        if Self::shortcut_button(ui, self.doc.undo_text(), &UNDO, self.doc.can_undo()) {
            self.undo();
        }
        if Self::shortcut_button(ui, self.doc.redo_text(), &REDO, self.doc.can_redo()) {
            self.redo();
        }
        ui.separator();
        if Self::item(ui, "搜尋", Some(&FIND), "") {
            self.focus_search(ui.ctx());
        }
        if Self::item(
            ui,
            "清除名稱中的換行字元",
            None,
            "移除舊版寫頻軟體寫進名稱的換行等控制字元",
        ) {
            self.clean_control_chars();
        }
    }

    pub(crate) fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("開啟").clicked() {
                self.request_open(None);
            }
            if ui
                .add_enabled(self.doc.has_project(), egui::Button::new("儲存"))
                .clicked()
            {
                self.save();
            }
            if ui
                .add_enabled(self.doc.can_undo(), egui::Button::new("復原"))
                .on_hover_text(self.doc.undo_text())
                .clicked()
            {
                self.undo();
            }
            if ui
                .add_enabled(self.doc.can_redo(), egui::Button::new("重做"))
                .on_hover_text(self.doc.redo_text())
                .clicked()
            {
                self.redo();
            }
            ui.separator();
            self.radio_toolbar(ui);
            ui.separator();
            if ui.button("RadioID 匯入").clicked() {
                self.import_from_radioid();
            }
            if ui
                .button("清除換行")
                .on_hover_text("移除舊版寫頻軟體寫進名稱的換行等控制字元")
                .clicked()
            {
                self.clean_control_chars();
            }
        });
    }
}
