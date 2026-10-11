//! 接收群組 page (port of `group_page.py` and the RX-group part of `list_ops.py`): the 32 list
//! slots with their member contacts, and the editor for one list.

use eframe::egui;
use opendrs_core::f136::Project;
use opendrs_core::pystr::strip;
use opendrs_core::rxgroup::{MAX_RX_MEMBERS, RxGroup, has_rx_groups, rx_groups, with_rx_group};
use opendrs_core::textcodec::NameCodec;

use super::slots::{self, MEMBER_SEPARATOR, MemberPicker, Outcome, SlotRow};
use crate::app::OpenDrsApp;
use crate::table::model::contact_label;

pub const NOTE: &str = "雙擊一列可編輯。頻道的「接收群組」選擇這裡的清單；群組最多 32 個聯絡人。\
                        刪除聯絡人時，群組成員會自動移除並重新編號。";
/// Error title of a refused edit.
pub const ERROR_TITLE: &str = "接收群組";

/// What this page keeps between frames.
#[derive(Default)]
pub struct State {
    /// Selected list number (1-32).
    pub selected: Option<usize>,
    pub editor: Option<MemberPicker<u16>>,
}

/// `#{i} {name}（{kind} {id}）` for each member, or `#{m}（不存在）`.
pub fn member_labels(project: &Project, members: &[u16], codec: NameCodec) -> Vec<String> {
    members
        .iter()
        .map(|m| {
            project
                .contacts
                .iter()
                .find(|c| c.index() == *m)
                .map_or_else(|| format!("#{m}（不存在）"), |c| contact_label(c, codec))
        })
        .collect()
}

/// The table rows: name, member count, members (`、`-joined), in use.
pub fn rows(project: &Project, codec: NameCodec) -> Vec<SlotRow> {
    rx_groups(&project.image)
        .iter()
        .map(|g| {
            let members = g.members();
            SlotRow {
                number: g.number,
                name: g.name(codec),
                count: members.len(),
                members: member_labels(project, &members, codec).join(MEMBER_SEPARATOR),
                in_use: g.in_use(),
            }
        })
        .collect()
}

/// The editor of one list: every contact, the list's members ticked.
pub fn editor_for(
    project: &Project,
    group: &RxGroup,
    codec: NameCodec,
    revision: u64,
) -> MemberPicker<u16> {
    let choices = project
        .contacts
        .iter()
        .map(|c| (contact_label(c, codec), c.index()))
        .collect();
    MemberPicker::new(
        group.number,
        format!("接收群組 {}", group.number),
        group.name(codec),
        choices,
        &group.members(),
        MAX_RX_MEMBERS,
        "聯絡人",
        revision,
    )
}

/// `編輯接收群組 {n}`.
pub fn undo_label(number: usize) -> String {
    format!("編輯接收群組 {number}")
}

/// Store list `number`'s name (stripped) and members; the error text is shown under
/// [`ERROR_TITLE`].
pub fn apply(
    project: &Project,
    number: usize,
    name: &str,
    members: &[u16],
    codec: NameCodec,
) -> Result<Project, String> {
    let image = with_rx_group(&project.image, number, strip(name), members, codec)
        .map_err(|e| e.to_string())?;
    Ok(Project {
        image,
        ..project.clone()
    })
}

/// Open the editor of list `number` (1-32); false when there is no such list.
pub fn open_editor(app: &mut OpenDrsApp, number: usize) -> bool {
    let groups = rx_groups(&app.project().image);
    let Some(group) = number.checked_sub(1).and_then(|i| groups.get(i)) else {
        return false;
    };
    let editor = editor_for(app.project(), group, app.codec, app.doc.revision());
    app.pages.groups.editor = Some(editor);
    true
}

pub fn show(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let rows = rows(app.project(), app.codec);
    let enabled = has_rx_groups(&app.project().image);
    let open = ui
        .add_enabled_ui(enabled, |ui| {
            slots::slot_table(ui, "rx-groups", NOTE, &rows, &mut app.pages.groups.selected)
        })
        .inner;
    match open {
        Some(slots::SlotRequest::Edit(number)) => {
            open_editor(app, number);
        }
        Some(slots::SlotRequest::AllInUse(total)) => {
            app.warn(slots::ADD_TITLE, slots::all_in_use_text(total));
        }
        None => {}
    }
    editor(app, ui);
}

fn editor(app: &mut OpenDrsApp, ui: &mut egui::Ui) {
    let revision = app.doc.revision();
    let Some(picker) = app.pages.groups.editor.as_mut() else {
        return;
    };
    if picker.revision != revision {
        app.pages.groups.editor = None; // the project changed underneath (undo): start again
        return;
    }
    match picker.show(ui.ctx(), |_| {}) {
        Outcome::Open => {}
        Outcome::Cancel => app.pages.groups.editor = None,
        Outcome::Accept => {
            let Some(done) = app.pages.groups.editor.take() else {
                return;
            };
            match apply(
                app.project(),
                done.number,
                &done.name,
                &done.values(),
                app.codec,
            ) {
                Ok(project) => app.commit(project, undo_label(done.number)),
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
    fn members_are_labelled_like_the_channel_table() {
        let p = fixture();
        assert_eq!(
            member_labels(&p, &[1, 9999], NameCodec::Big5),
            ["#1 TG46001_CN（組呼 46001）", "#9999（不存在）"]
        );
        let rows = rows(&p, NameCodec::Big5);
        assert_eq!(rows.len(), 32);
        assert_eq!((rows[0].name.as_str(), rows[0].count), ("MMDVM_TG", 9));
        assert_eq!(slots::in_use(&rows), 1);
    }

    #[test]
    fn editor_ticks_members_in_contact_order() {
        let p = fixture();
        let group = &rx_groups(&p.image)[0];
        let picker = editor_for(&p, group, NameCodec::Big5, 1);
        assert_eq!(picker.title, "接收群組 1");
        assert_eq!(picker.values(), [1, 2, 3, 4, 5, 6, 10, 12, 13]);
        assert_eq!(picker.count_text(), "已勾選 9 / 32 個聯絡人");
        let edited = apply(&p, 2, " TW ", &[2, 10], NameCodec::Big5).expect("edit");
        assert_eq!(rx_groups(&edited.image)[1].name(NameCodec::Big5), "TW");
        assert_eq!(undo_label(2), "編輯接收群組 2");
    }
}
