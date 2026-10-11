//! The structural commands that appear both in the menus and as buttons above the channel, zone
//! and contact tables (port of the QActions `main_window.py` shares through `_page_actions`):
//! one menu text, button text, icon and tooltip per command, so the two stay in step.

use crate::app::OpenDrsApp;
use crate::icons::Icon;
use crate::table::model::Table;

pub struct Command {
    /// The menu item's text.
    pub text: &'static str,
    /// The button's text (`short` in the Python version).
    pub short: &'static str,
    pub icon: Icon,
    pub tip: &'static str,
    pub run: fn(&mut OpenDrsApp),
}

pub const ADD_CHANNEL: Command = Command {
    text: "新增頻道（複製選取的頻道）",
    short: "新增",
    icon: Icon::Plus,
    tip: "複製目前選取的頻道，加在同一分區的最後面；掃描清單與其他頻道的編號不受影響",
    run: OpenDrsApp::add_channel,
};
pub const BATCH_ADD_CHANNELS: Command = Command {
    text: "批次新增頻道…",
    short: "批次新增",
    icon: Icon::Plus,
    tip: "以選取的頻道為範本，一次新增多台（頻率依間隔遞增、名稱依序編號）",
    run: OpenDrsApp::batch_add_channels,
};
pub const DELETE_CHANNEL: Command = Command {
    text: "刪除頻道",
    short: "刪除",
    icon: Icon::Trash,
    tip: "刪除選取的頻道；同分區之後的頻道編號與掃描清單會自動調整",
    run: OpenDrsApp::delete_channel,
};
pub const CHANNEL_UP: Command = Command {
    text: "上移頻道",
    short: "上移",
    icon: Icon::Up,
    tip: "與同分區的上一台交換位置；掃描清單等引用跟著調整",
    run: |app| app.shift_channel(-1),
};
pub const CHANNEL_DOWN: Command = Command {
    text: "下移頻道",
    short: "下移",
    icon: Icon::Down,
    tip: "與同分區的下一台交換位置；掃描清單等引用跟著調整",
    run: |app| app.shift_channel(1),
};
pub const ADD_ZONE: Command = Command {
    text: "新增分區…",
    short: "新增",
    icon: Icon::Plus,
    tip: "在最後新增一個分區，第一台頻道複製目前選取的頻道",
    run: OpenDrsApp::add_zone,
};
pub const DELETE_ZONE: Command = Command {
    text: "刪除分區",
    short: "刪除",
    icon: Icon::Trash,
    tip: "刪除選取的分區與其中的頻道；之後的分區與掃描清單自動調整",
    run: OpenDrsApp::delete_zone,
};
pub const ADD_CONTACT: Command = Command {
    text: "新增聯絡人…",
    short: "新增",
    icon: Icon::Plus,
    tip: "輸入類型、名稱與 ID，新增一筆聯絡人（加在清單最後面，其他編號不變）",
    run: OpenDrsApp::add_contact,
};
pub const IMPORT_RADIOID: Command = Command {
    text: "從 RadioID 匯入聯絡人…",
    short: "RadioID 匯入",
    icon: Icon::Users,
    tip: "從 RadioID 匯入聯絡人…",
    run: OpenDrsApp::import_from_radioid,
};
pub const DELETE_CONTACTS: Command = Command {
    text: "刪除選取的聯絡人",
    short: "刪除",
    icon: Icon::Trash,
    tip: "刪除選取的聯絡人；編號往前遞補，頻道與接收群組自動調整",
    run: OpenDrsApp::delete_contacts,
};
pub const CONTACT_UP: Command = Command {
    text: "上移聯絡人",
    short: "上移",
    icon: Icon::Up,
    tip: "與上一筆交換位置；頻道發話對象與接收群組跟著調整",
    run: |app| app.shift_contact(-1),
};
pub const CONTACT_DOWN: Command = Command {
    text: "下移聯絡人",
    short: "下移",
    icon: Icon::Down,
    tip: "與下一筆交換位置；頻道發話對象與接收群組跟著調整",
    run: |app| app.shift_contact(1),
};

/// The buttons above each table, in the Python version's order (`None`: a separator).
pub fn page_actions(table: Table) -> &'static [Option<&'static Command>] {
    match table {
        Table::Channels => &[
            Some(&ADD_CHANNEL),
            Some(&BATCH_ADD_CHANNELS),
            None,
            Some(&DELETE_CHANNEL),
            Some(&CHANNEL_UP),
            Some(&CHANNEL_DOWN),
        ],
        Table::Zones => &[Some(&ADD_ZONE), None, Some(&DELETE_ZONE)],
        Table::Contacts => &[
            Some(&ADD_CONTACT),
            Some(&IMPORT_RADIOID),
            None,
            Some(&DELETE_CONTACTS),
            Some(&CONTACT_UP),
            Some(&CONTACT_DOWN),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_table_starts_with_its_add_command() {
        for (table, first) in [
            (Table::Channels, ADD_CHANNEL.text),
            (Table::Zones, ADD_ZONE.text),
            (Table::Contacts, ADD_CONTACT.text),
        ] {
            let actions = page_actions(table);
            assert_eq!(actions[0].map(|c| c.text), Some(first));
            assert!(
                actions
                    .iter()
                    .flatten()
                    .all(|c| c.short.chars().count() <= 10)
            );
        }
        assert_eq!(page_actions(Table::Channels).len(), 6);
        assert_eq!(page_actions(Table::Zones).len(), 3);
        assert_eq!(page_actions(Table::Contacts).len(), 6);
    }
}
