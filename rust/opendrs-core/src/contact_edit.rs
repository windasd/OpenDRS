//! Deleting and reordering contacts without leaving dangling references (port of
//! `contact_edit.py`).
//!
//! Only two places store contact numbers (`Contact::index`, 1-based, equal to the position): a
//! channel's TX contact (+0x1E) and the RX group member lists. After removing contacts the rest
//! are renumbered and those two kinds of reference are rewritten.

use std::collections::{BTreeSet, HashMap};

use crate::codeplug::{CONTACT_SIZE, CONTACTS_OFFSET, image_length_for, with_synced_image};
use crate::f136::Project;
use crate::pystr;
use crate::records::{Contact, NAME_SIZE};
use crate::rxgroup::remap_members;
use crate::textcodec::{NameCodec, encode_name};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ContactEditError(pub String);

fn error(message: impl Into<String>) -> ContactEditError {
    ContactEditError(message.into())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContactDeletion {
    pub removed: usize,
    pub group_members_dropped: usize,
    /// Remaining contacts whose number changed.
    pub renumbered: usize,
}

/// `"row. name"` of every channel whose TX contact is one of `numbers`.
pub fn channels_calling(
    project: &Project,
    numbers: &BTreeSet<u16>,
    codec: NameCodec,
) -> Vec<String> {
    project
        .channels
        .iter()
        .enumerate()
        .filter(|(_, ch)| numbers.contains(&ch.tx_contact()))
        .map(|(row, ch)| format!("{}. {}", row + 1, ch.name(codec)))
        .collect()
}

/// Clear the freed contact slots and shrink to the legacy CPS's image length.
fn trim_image(image: &[u8], contacts: usize) -> Vec<u8> {
    let end = CONTACTS_OFFSET + contacts * CONTACT_SIZE;
    if image.len() < end {
        return image.to_vec();
    }
    let mut kept = image[..end].to_vec();
    kept.resize(image_length_for(contacts).max(end), 0);
    kept
}

pub fn delete_contacts(
    project: &Project,
    rows: &[usize],
    codec: NameCodec,
) -> Result<(Project, ContactDeletion), ContactEditError> {
    let doomed_rows: BTreeSet<usize> = rows.iter().copied().collect();
    let (Some(first), Some(last)) = (doomed_rows.first(), doomed_rows.last()) else {
        return Err(error("請先選擇要刪除的聯絡人"));
    };
    if *first >= project.contacts.len() || *last >= project.contacts.len() {
        return Err(error("選取的聯絡人不存在"));
    }
    let doomed: BTreeSet<u16> = doomed_rows
        .iter()
        .map(|r| project.contacts[*r].index())
        .collect();
    let users = channels_calling(project, &doomed, codec);
    if !users.is_empty() {
        let mut listed = users.iter().take(8).cloned().collect::<Vec<_>>().join("、");
        if users.len() > 8 {
            listed.push('…');
        }
        return Err(error(format!(
            "下列頻道的發話對象是要刪除的聯絡人，請先改掉：{listed}"
        )));
    }
    let mut renumber: HashMap<u16, u16> = HashMap::new();
    let mut contacts = Vec::with_capacity(project.contacts.len());
    for (row, contact) in project.contacts.iter().enumerate() {
        if doomed_rows.contains(&row) {
            continue;
        }
        let new_number =
            u16::try_from(contacts.len() + 1).map_err(|_| error("聯絡人編號超出範圍"))?;
        renumber.insert(contact.index(), new_number);
        contacts.push(contact.with_index(new_number));
    }
    let channels = project
        .channels
        .iter()
        .map(|ch| {
            renumber
                .get(&ch.tx_contact())
                .map_or(*ch, |n| ch.with_tx_contact(*n))
        })
        .collect();
    let (image, dropped) = remap_members(&project.image, |m| renumber.get(&m).copied())
        .map_err(|e| error(e.to_string()))?;
    let image = if image.is_empty() {
        image
    } else {
        trim_image(&image, contacts.len())
    };
    let changed = renumber.iter().filter(|(old, new)| old != new).count();
    let updated = with_synced_image(&Project {
        contacts,
        channels,
        image,
        ..project.clone()
    });
    Ok((
        updated,
        ContactDeletion {
            removed: doomed_rows.len(),
            group_members_dropped: dropped,
            renumbered: changed,
        },
    ))
}

/// Swap contact `row` with its neighbour above (-1) or below (+1); returns `(project, new row)`.
/// The two numbers are exchanged in every channel TX contact and RX group member list as well.
pub fn shift_contact(
    project: &Project,
    row: usize,
    delta: i32,
) -> Result<(Project, usize), ContactEditError> {
    if delta != -1 && delta != 1 {
        return Err(error("一次只能移動一格"));
    }
    if row >= project.contacts.len() {
        return Err(error("請先選擇聯絡人"));
    }
    let target = row
        .checked_add_signed(delta as isize)
        .filter(|t| *t < project.contacts.len());
    let Some(target) = target else {
        return Err(error(format!(
            "已經在清單的{}",
            if delta < 0 {
                "第一筆"
            } else {
                "最後一筆"
            }
        )));
    };
    let mut contacts = project.contacts.clone();
    let (a, b) = (contacts[row].index(), contacts[target].index());
    let (at_row, at_target) = (contacts[target].with_index(a), contacts[row].with_index(b));
    contacts[row] = at_row;
    contacts[target] = at_target;
    let swap = |m: u16| {
        if m == a {
            b
        } else if m == b {
            a
        } else {
            m
        }
    };
    let channels = project
        .channels
        .iter()
        .map(|ch| {
            if ch.tx_contact() == a || ch.tx_contact() == b {
                ch.with_tx_contact(swap(ch.tx_contact()))
            } else {
                *ch
            }
        })
        .collect();
    let (image, _dropped) =
        remap_members(&project.image, |m| Some(swap(m))).map_err(|e| error(e.to_string()))?;
    Ok((
        with_synced_image(&Project {
            contacts,
            channels,
            image,
            ..project.clone()
        }),
        target,
    ))
}

/// Append one contact after the last; returns `(project, new row)`.
///
/// Nothing else is renumbered, so every reference stays valid. A contact with the same type and
/// ID is refused rather than renamed (an import overwrites its name instead). `capacity` is the
/// radio's limit ([`crate::codeplug::MAX_CONTACTS`]).
pub fn add_contact(
    project: &Project,
    kind: u8,
    name: &str,
    call_id: u32,
    codec: NameCodec,
    capacity: usize,
) -> Result<(Project, usize), ContactEditError> {
    if !(4..=7).contains(&kind) {
        return Err(error(format!("不支援的聯絡人類型 {kind}")));
    }
    if !(1..=Contact::MAX_ID).contains(&call_id) {
        return Err(error(format!(
            "ID 必須介於 1 與 {}",
            pystr::thousands(u64::from(Contact::MAX_ID))
        )));
    }
    if project.contacts.len() >= capacity {
        return Err(error(format!("聯絡人已達對講機上限 {capacity} 筆")));
    }
    let text = pystr::strip(name);
    if text.is_empty() {
        return Err(error("請輸入名稱"));
    }
    if text.chars().any(|ch| (ch as u32) < 0x20 || ch == '\x7f') {
        return Err(error("名稱不可包含換行等控制字元"));
    }
    if let Some(same) = project
        .contacts
        .iter()
        .find(|c| c.kind() == kind && c.call_id() == call_id)
    {
        return Err(error(format!(
            "已有相同類型與 ID 的聯絡人：#{} {}",
            same.index(),
            same.name(codec)
        )));
    }
    let field = encode_name(text, NAME_SIZE, codec).map_err(|e| error(e.to_string()))?;
    let number = project
        .contacts
        .iter()
        .map(Contact::index)
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| error("聯絡人編號已用完"))?;
    let contact =
        Contact::build(number, kind, &field, call_id).map_err(|e| error(e.to_string()))?;
    let mut contacts = project.contacts.clone();
    contacts.push(contact);
    let mut section_order = project.section_order.clone();
    if !section_order.iter().any(|s| s == "Contacts") {
        section_order.insert(0, "Contacts".to_owned());
    }
    let row = contacts.len() - 1;
    let updated = with_synced_image(&Project {
        contacts,
        section_order,
        ..project.clone()
    });
    Ok((updated, row))
}
