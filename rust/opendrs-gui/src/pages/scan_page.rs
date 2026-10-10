//! 掃描清單 page (port of `scan_page.py` and the scan part of `list_ops.py`): the 16 list slots
//! with their members, and the editor for one list.

use eframe::egui;
use opendrs_core::f136::Project;
use opendrs_core::layout::zone_rows;
use opendrs_core::pystr::strip;
use opendrs_core::scanlist::{
    Entry, SCAN_ENTRY_COUNT, ScanList, TX_MODES, has_scan_lists, scan_lists, with_scan_list,
    with_scan_settings,
};
use opendrs_core::textcodec::NameCodec;

use super::form::{choice_combo, combo_width};
use super::slots::{self, MEMBER_SEPARATOR, MemberPicker, Outcome, SlotRow};
use crate::app::OpenDrsApp;

pub const NOTE: &str = "雙擊一列可編輯。清單最多 16 台頻道，依頻道清單順序掃描；\
                        刪除頻道或分區時，掃描清單會自動跟著調整。";
/// Error title of a refused edit.
pub const ERROR_TITLE: &str = "掃描清單";
const TALKBACK: [&str; 2] = ["Off", "On"];
const DESIGNATED_TX_MODE: u8 = 2;

/// What this page keeps between frames.
#[derive(Default)]
pub struct State {
    /// Selected list number (1-16).
    pub selected: Option<usize>,
    pub editor: Option<ScanEditor>,
}

/// Every channel as its scan entry (zone number, number in zone), in channel-list order.
pub fn channel_entries(project: &Project) -> Vec<Entry> {
    let mut seen = vec![0u16; project.zones.len()];
    zone_rows(project)
        .into_iter()
        .flatten()
        .map(|zone_row| {
            seen[zone_row] += 1;
            (zone_row as u16 + 1, seen[zone_row])
        })
        .collect()
}

/// `{zone}／{channel}`, `{zone}／第 {c} 台（不存在）` or `分區 {z}／第 {c} 台（不存在）`.
pub fn entry_label(project: &Project, entry: Entry, codec: NameCodec) -> String {
    let (zone_no, channel_no) = entry;
    let zone_count = project.zones.len();
    if !(1..=zone_count).contains(&usize::from(zone_no)) {
        return format!("分區 {zone_no}／第 {channel_no} 台（不存在）");
    }
    let zone = &project.zones[usize::from(zone_no) - 1];
    let missing = || format!("{}／第 {channel_no} 台（不存在）", zone.name(codec));
    if !(1..=zone.channel_count()).contains(&channel_no) {
        return missing();
    }
    // A zone table that claims channels past the end of the list (Python raises IndexError).
    let row = i64::from(zone.first_record()) - zone_count as i64 - 1 + i64::from(channel_no) - 1;
    match usize::try_from(row)
        .ok()
        .and_then(|r| project.channels.get(r))
    {
        Some(channel) => format!("{}／{}", zone.name(codec), channel.name(codec)),
        None => missing(),
    }
}

/// The table rows: name, member count, members (`、`-joined), in use.
pub fn rows(project: &Project, codec: NameCodec) -> Vec<SlotRow> {
    scan_lists(&project.image)
        .iter()
        .map(|s| {
            let entries = s.entries();
            SlotRow {
                number: s.number,
                name: s.name(codec),
                count: entries.len(),
                members: entries
                    .iter()
                    .map(|e| entry_label(project, *e, codec))
                    .collect::<Vec<_>>()
                    .join(MEMBER_SEPARATOR),
                in_use: s.in_use(),
            }
        })
        .collect()
}

/// The editor of one list: name and channels ([`MemberPicker`]) plus the list's settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanEditor {
    pub picker: MemberPicker<Entry>,
    pub options: ScanOptions,
}

/// 回講, 掃描發射模式 and 指定信道 as the editor shows them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    /// Index into Off / On.
    pub talkback: usize,
    /// Index into [`TX_MODES`].
    pub tx_mode: usize,
    /// `Off` (0, 0) and every channel.
    pub designated_items: Vec<(String, Entry)>,
    pub designated: usize,
    /// A designated channel the list does not offer (shown as Off), kept unless another is
    /// picked.
    pub stale: Option<Entry>,
}

impl ScanEditor {
    pub fn new(project: &Project, scan: &ScanList, codec: NameCodec, revision: u64) -> Self {
        let labelled: Vec<(String, Entry)> = channel_entries(project)
            .into_iter()
            .map(|e| (entry_label(project, e, codec), e))
            .collect();
        let designated_items: Vec<(String, Entry)> = std::iter::once(("Off".to_string(), (0, 0)))
            .chain(labelled.iter().cloned())
            .collect();
        let picker = MemberPicker::new(
            scan.number,
            format!("掃描清單 {}", scan.number),
            scan.name(codec),
            labelled,
            &scan.entries(),
            SCAN_ENTRY_COUNT,
            "頻道",
            revision,
        );
        let current = scan.designated();
        let found = designated_items.iter().position(|(_, e)| *e == current);
        let options = ScanOptions {
            talkback: usize::from(scan.talkback()),
            tx_mode: TX_MODES
                .iter()
                .position(|(_, v)| *v == scan.tx_mode())
                .unwrap_or(0),
            designated: found.unwrap_or(0),
            stale: (found.is_none() && current != (0, 0)).then_some(current),
            designated_items,
        };
        ScanEditor { picker, options }
    }

    /// (talkback, scan TX mode, designated channel) to store.
    pub fn settings(&self) -> (bool, u8, Entry) {
        self.options.settings()
    }
}

impl ScanOptions {
    /// 指定信道 can be chosen only in the designated-channel mode.
    pub fn designated_enabled(&self) -> bool {
        TX_MODES[self.tx_mode].1 == DESIGNATED_TX_MODE
    }

    /// Pick another designated channel; picking forgets a stale value.
    pub fn pick_designated(&mut self, index: usize) {
        if index != self.designated && index < self.designated_items.len() {
            self.designated = index;
            self.stale = None;
        }
    }

    pub fn settings(&self) -> (bool, u8, Entry) {
        let designated = self
            .stale
            .unwrap_or(self.designated_items[self.designated].1);
        (self.talkback == 1, TX_MODES[self.tx_mode].1, designated)
    }

    fn form_rows(&mut self, ui: &mut egui::Ui) {
        let talkback: Vec<(String, ())> = TALKBACK.iter().map(|t| (t.to_string(), ())).collect();
        let modes: Vec<(String, u8)> = TX_MODES.iter().map(|(l, v)| (l.to_string(), *v)).collect();
        ui.label("回講：");
        if let (Some(i), _) = choice_combo(ui, "scan-talkback", &talkback, self.talkback, 80.0) {
            self.talkback = i;
        }
        ui.end_row();
        ui.label("掃描發射模式：");
        let width = combo_width(ui, modes.iter().map(|(l, _)| l.as_str()));
        if let (Some(i), _) = choice_combo(ui, "scan-tx-mode", &modes, self.tx_mode, width) {
            self.tx_mode = i;
        }
        ui.end_row();
        ui.label("指定信道：");
        ui.add_enabled_ui(self.designated_enabled(), |ui| {
            let width = combo_width(ui, self.designated_items.iter().map(|(l, _)| l.as_str()));
            let items = &self.designated_items;
            if let (Some(i), _) = choice_combo(ui, "scan-designated", items, self.designated, width)
            {
                self.pick_designated(i);
            }
        });
        ui.end_row();
    }
}

/// `編輯掃描清單 {n}`.
pub fn undo_label(number: usize) -> String {
    format!("編輯掃描清單 {number}")
}

/// Store list `number`'s name (stripped), entries and settings; the error text is shown under
/// [`ERROR_TITLE`].
pub fn apply(
    project: &Project,
    number: usize,
    name: &str,
    entries: &[Entry],
    settings: (bool, u8, Entry),
    codec: NameCodec,
) -> Result<Project, String> {
    let image = with_scan_list(&project.image, number, strip(name), entries, codec)
        .map_err(|e| e.to_string())?;
    let (talkback, tx_mode, designated) = settings;
    let image = with_scan_settings(&image, number, talkback, tx_mode, designated)
        .map_err(|e| e.to_string())?;
    Ok(Project {
        image,
        ..project.clone()
    })
}

/// Open the editor of list `number` (1-16); false when there is no such list.
pub fn open_editor(app: &mut OpenDrsApp, number: usize) -> bool {
    let lists = scan_lists(&app.project().image);
    let Some(scan) = number.checked_sub(1).and_then(|i| lists.get(i)) else {
        return false;
    };
    let editor = ScanEditor::new(app.project(), scan, app.codec, app.doc.revision());
    app.pages.scan.editor = Some(editor);
    true
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let rows = rows(app.project(), app.codec);
    let enabled = has_scan_lists(&app.project().image);
    let open = ui
        .add_enabled_ui(enabled, |ui| {
            slots::slot_table(ui, "scan-lists", NOTE, &rows, &mut app.pages.scan.selected)
        })
        .inner;
    if let Some(number) = open {
        open_editor(app, number);
    }
    editor(app, ui);
}

fn editor(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let revision = app.doc.revision();
    let Some(editor) = app.pages.scan.editor.as_mut() else {
        return;
    };
    if editor.picker.revision != revision {
        app.pages.scan.editor = None; // the project changed underneath (undo): start again
        return;
    }
    let ScanEditor { picker, options } = editor;
    let outcome = picker.show(ui.ctx(), |ui| options.form_rows(ui));
    match outcome {
        Outcome::Open => {}
        Outcome::Cancel => app.pages.scan.editor = None,
        Outcome::Accept => {
            let Some(done) = app.pages.scan.editor.take() else {
                return;
            };
            let picker = &done.picker;
            match apply(
                app.project(),
                picker.number,
                &picker.name,
                &picker.values(),
                done.settings(),
                app.codec,
            ) {
                Ok(project) => app.commit(project, undo_label(picker.number)),
                Err(message) => app.warn(ERROR_TITLE, message),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opendrs_core::f136;
    use std::path::Path;

    fn fixture() -> Project {
        f136::load(Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/dh98vu_20250811.F136"))
            .expect("fixture")
    }

    #[test]
    fn entry_labels_name_missing_zones_and_channels() {
        let p = fixture();
        assert_eq!(entry_label(&p, (1, 1), NameCodec::Big5), "MMDVM／TG46600");
        assert_eq!(
            entry_label(&p, (1, 99), NameCodec::Big5),
            "MMDVM／第 99 台（不存在）"
        );
        assert_eq!(
            entry_label(&p, (9, 1), NameCodec::Big5),
            "分區 9／第 1 台（不存在）"
        );
        assert_eq!(channel_entries(&p).len(), 24);
    }

    #[test]
    fn stale_designated_channel_is_kept_until_another_is_picked() {
        let p = fixture();
        let scan = &scan_lists(&p.image)[0];
        let mut editor = ScanEditor::new(&p, scan, NameCodec::Big5, 1);
        assert_eq!(editor.options.designated, 0);
        assert!(!editor.options.designated_enabled());
        assert_eq!(editor.settings(), (false, 0, (0x105, 6)));
        let options = &mut editor.options;
        options.pick_designated(0); // the shown entry again: still stale
        assert_eq!(options.settings().2, (0x105, 6));
        options.tx_mode = 2;
        assert!(options.designated_enabled());
        let target = options
            .designated_items
            .iter()
            .position(|(l, _)| l == "FRS_TW／FRS_TW_3")
            .expect("entry");
        options.pick_designated(target);
        options.talkback = 1;
        assert_eq!(editor.settings(), (true, 2, (4, 3)));
    }

    #[test]
    fn apply_strips_the_name_and_reports_errors() {
        let p = fixture();
        let edited = apply(
            &p,
            1,
            " New ",
            &[(4, 1)],
            (false, 0, (0, 0)),
            NameCodec::Big5,
        )
        .expect("edit");
        assert_eq!(scan_lists(&edited.image)[0].name(NameCodec::Big5), "New");
        let err = apply(
            &p,
            1,
            "x",
            &[(1, 1); 17],
            (false, 0, (0, 0)),
            NameCodec::Big5,
        );
        assert_eq!(err.unwrap_err(), "每個掃描清單最多 16 個頻道");
        assert_eq!(undo_label(3), "編輯掃描清單 3");
    }
}
