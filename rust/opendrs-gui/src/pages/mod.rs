//! The pages of the main window, one tab each, in the Python version's order.

pub mod alarm_page;
pub mod device;
pub mod form;
pub mod group_page;
pub mod quickmsg_page;
pub mod records;
pub mod scan_page;
pub mod settings_page;
pub mod slots;

use eframe::egui;
use opendrs_core::f136::Project;
use opendrs_core::{rxgroup, scanlist};

use crate::app::OpenDrsApp;
use crate::table::model::Table;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Device,
    Channels,
    Zones,
    Contacts,
    ScanLists,
    RxGroups,
    Settings,
    QuickMessages,
    Alarms,
}

impl Tab {
    pub const ALL: [Tab; 9] = [
        Tab::Device,
        Tab::Channels,
        Tab::Zones,
        Tab::Contacts,
        Tab::ScanLists,
        Tab::RxGroups,
        Tab::Settings,
        Tab::QuickMessages,
        Tab::Alarms,
    ];

    /// The tab title, with the record count where the Python version shows one.
    pub fn title(self, project: &Project) -> String {
        let count = |n: usize| format!("（{n}）");
        match self {
            Tab::Device => "裝置資訊".into(),
            Tab::Channels => format!("頻道{}", count(project.channels.len())),
            Tab::Zones => format!("分區{}", count(project.zones.len())),
            Tab::Contacts => format!("聯絡人{}", count(project.contacts.len())),
            Tab::ScanLists => {
                let used = scanlist::scan_lists(&project.image)
                    .iter()
                    .filter(|s| s.in_use())
                    .count();
                format!("掃描清單{}", count(used))
            }
            Tab::RxGroups => {
                let used = rxgroup::rx_groups(&project.image)
                    .iter()
                    .filter(|g| g.in_use())
                    .count();
                format!("接收群組{}", count(used))
            }
            Tab::Settings => "對講機設定".into(),
            Tab::QuickMessages => "常用短信".into(),
            Tab::Alarms => "報警與加密".into(),
        }
    }
}

/// What the pages keep between frames (editor text, selections, filters).
#[derive(Default)]
pub struct PageState {
    pub device: device::DeviceState,
    pub tables: records::RecordTables,
    pub settings: settings_page::State,
    pub scan: scan_page::State,
    pub groups: group_page::State,
    pub quick_messages: quickmsg_page::State,
    pub alarms: alarm_page::State,
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    ui.horizontal_wrapped(|ui| {
        for tab in Tab::ALL {
            let title = tab.title(app.project());
            ui.selectable_value(&mut app.tab, tab, title);
        }
    });
    ui.separator();
    if !app.doc.has_project() {
        ui.centered_and_justified(|ui| {
            ui.label("請先開啟專案檔（檔案 › 開啟…）、從對講機讀取，或把 .F136 檔拖進視窗。");
        });
        return;
    }
    match app.tab {
        Tab::Device => {
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| device::show(app, ui));
        }
        Tab::Channels => records::show(app, ui, Table::Channels),
        Tab::Zones => records::show(app, ui, Table::Zones),
        Tab::Contacts => records::show(app, ui, Table::Contacts),
        Tab::ScanLists => scan_page::show(app, ui),
        Tab::RxGroups => group_page::show(app, ui),
        Tab::Settings => settings_page::show(app, ui),
        Tab::QuickMessages => quickmsg_page::show(app, ui),
        Tab::Alarms => alarm_page::show(app, ui),
    }
}
