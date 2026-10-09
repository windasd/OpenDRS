//! Turn RadioID users into private-call contact records (port of `contact_import.py`).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::sync::{Arc, LazyLock};

use opendrs_core::codeplug::with_synced_image;
use opendrs_core::f136::Project;
use opendrs_core::records::{Contact, NAME_SIZE, RecordError};
use opendrs_core::textcodec::{NameCodec, fit_name};
use regex::Regex;

use crate::pystr::{casefold, is_printable, is_space, strip};
use crate::radioid::RadioIdUser;

/// The radio firmware treats more than 1000 contacts as corrupt: FUN_030272fc checks
/// `count < 0x3E9` and otherwise zeroes the count. (The legacy CPS help text says 2000, which the
/// radio would reject.)
pub const DEFAULT_CONTACT_CAPACITY: usize = opendrs_core::codeplug::MAX_CONTACTS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum NameFormat {
    Callsign,
    CallsignName,
    Name,
}

impl NameFormat {
    pub const ALL: [NameFormat; 3] = [
        NameFormat::Callsign,
        NameFormat::CallsignName,
        NameFormat::Name,
    ];

    pub fn value(self) -> &'static str {
        match self {
            NameFormat::Callsign => "callsign",
            NameFormat::CallsignName => "callsign_name",
            NameFormat::Name => "name",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            NameFormat::Callsign => "呼號（N0CALL）",
            NameFormat::CallsignName => "呼號 名字（N0CALL James）",
            NameFormat::Name => "名字（James）",
        }
    }

    pub fn from_value(value: &str) -> Option<NameFormat> {
        NameFormat::ALL.into_iter().find(|f| f.value() == value)
    }
}

fn first_non_empty<'a>(parts: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    parts.into_iter().find(|p| !p.is_empty())
}

pub fn contact_text(user: &RadioIdUser, fmt: NameFormat) -> String {
    let fallback = || user.radio_id.to_string();
    match fmt {
        NameFormat::Callsign => {
            first_non_empty([user.callsign.as_str()]).map_or_else(fallback, str::to_string)
        }
        NameFormat::Name => first_non_empty([user.first_name.as_str(), user.callsign.as_str()])
            .map_or_else(fallback, str::to_string),
        NameFormat::CallsignName => {
            let joined = [user.callsign.as_str(), user.first_name.as_str()]
                .into_iter()
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if joined.is_empty() {
                fallback()
            } else {
                joined
            }
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub added: usize,
    /// Existing (type, ID): name overwritten.
    pub updated: usize,
    /// Existing (type, ID): name already identical.
    pub unchanged: usize,
    pub over_capacity: usize,
    /// (radio id, wanted, stored).
    pub shortened: Vec<(u32, String, String)>,
    /// Existing (type, ID) with a custom name: left untouched.
    pub kept: usize,
}

impl ImportReport {
    pub fn summary(&self) -> String {
        let mut lines = vec![format!("新增 {} 筆聯絡人。", self.added)];
        if self.updated > 0 {
            lines.push(format!(
                "{} 筆已存在，名稱已更新（編號不變）。",
                self.updated
            ));
        }
        if self.unchanged > 0 {
            lines.push(format!("{} 筆已存在且名稱相同，未變更。", self.unchanged));
        }
        if self.kept > 0 {
            lines.push(format!("{} 筆已存在且為自訂名稱，保留原名稱。", self.kept));
        }
        if self.over_capacity > 0 {
            lines.push(format!("{} 筆因超過容量上限未匯入。", self.over_capacity));
        }
        if !self.shortened.is_empty() {
            lines.push(format!(
                "{} 筆名稱超過 {NAME_SIZE} bytes 或含無法表示的字元，已截短。",
                self.shortened.len()
            ));
        }
        lines.join("\n")
    }
}

// ITU-style amateur callsign: 1–3 char prefix (with a letter), a digit, 1–4 letter suffix,
// optionally followed by a portable/mobile indicator (N0CALL/P, N0CALL-M). Python writes the
// "prefix has a letter" rule as a lookahead, which the regex crate lacks: it is a second pattern
// here. `\n?$` is Python's `$`, which also matches before a final newline.
static CALLSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Z0-9]{1,3}[0-9][A-Z]{1,4}(?:[/-][A-Z0-9]{1,4})?\n?$").expect("constant regex")
});
static CALLSIGN_LETTER_FIRST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z0-9]{0,2}[A-Z]").expect("constant regex"));
/// A truncated first name ("Christophe") still counts as a match.
const MIN_PREFIX: usize = 3;

pub fn looks_like_callsign(token: &str) -> bool {
    let upper = token.to_uppercase();
    CALLSIGN_LETTER_FIRST.is_match(&upper) && CALLSIGN.is_match(&upper)
}

/// `re.split(r"[\s,;/|_]+")` separators.
fn is_token_separator(ch: char) -> bool {
    is_space(ch) || matches!(ch, ',' | ';' | '/' | '|' | '_')
}

/// True when an existing contact name was evidently taken from RadioID data, so a fresh import
/// may replace it: empty, the bare ID, a callsign (also an outdated one — callsigns change while
/// the DMR ID stays), or the user's own callsign/first name. Anything else (老王, 台中中繼, Dad)
/// is a name the owner chose and is kept.
pub fn is_radioid_style_name(existing: &str, user: &RadioIdUser) -> bool {
    let visible: String = existing
        .chars()
        .map(|ch| {
            if is_printable(ch) && ch != '␍' && ch != '␊' {
                ch
            } else {
                ' '
            }
        })
        .collect();
    let text = strip(&visible);
    if text.is_empty() || text == user.radio_id.to_string() {
        return true;
    }
    let call = casefold(&user.callsign);
    let first = casefold(&user.first_name);
    text.split(is_token_separator)
        .filter(|token| !token.is_empty())
        .any(|token| {
            let low = casefold(token);
            looks_like_callsign(token)
                || low == call
                || low == first
                || (low.chars().count() >= MIN_PREFIX && first.starts_with(&low))
        })
}

pub fn existing_private_ids(project: &Project) -> HashSet<u32> {
    project
        .contacts
        .iter()
        .filter(|c| c.kind() == Contact::PRIVATE_CALL)
        .map(Contact::call_id)
        .collect()
}

/// For an existing (type, ID): decides from its current name whether to overwrite it.
pub type ReplaceIf = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct NewContact {
    pub kind: u8,
    pub text: String,
    pub call_id: u32,
    /// `None` = always overwrite (CSV: the file is the user's own data). Ignored by `==`.
    pub replace_if: Option<ReplaceIf>,
}

impl NewContact {
    pub fn new(kind: u8, text: impl Into<String>, call_id: u32) -> Self {
        NewContact {
            kind,
            text: text.into(),
            call_id,
            replace_if: None,
        }
    }

    pub fn with_replace_if(self, replace_if: ReplaceIf) -> Self {
        NewContact {
            replace_if: Some(replace_if),
            ..self
        }
    }
}

impl PartialEq for NewContact {
    fn eq(&self, other: &Self) -> bool {
        (self.kind, &self.text, self.call_id) == (other.kind, &other.text, other.call_id)
    }
}

impl Eq for NewContact {}

impl fmt::Debug for NewContact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NewContact")
            .field("kind", &self.kind)
            .field("text", &self.text)
            .field("call_id", &self.call_id)
            .field("replace_if", &self.replace_if.as_ref().map(|_| "<fn>"))
            .finish()
    }
}

pub fn import_users(
    project: &Project,
    users: &[RadioIdUser],
    fmt: NameFormat,
    codec: NameCodec,
    capacity: usize,
) -> Result<(Project, ImportReport), RecordError> {
    let entries: Vec<NewContact> = users
        .iter()
        .map(|u| {
            let user = u.clone();
            NewContact::new(Contact::PRIVATE_CALL, contact_text(u, fmt), u.radio_id)
                .with_replace_if(Arc::new(move |name| is_radioid_style_name(name, &user)))
        })
        .collect();
    append_contacts(project, &entries, codec, capacity)
}

/// Merge contacts into the list: an existing (type, ID) gets its name overwritten (unless the
/// entry's `replace_if` rejects the current name), anything else is appended after the last
/// contact.
///
/// Existing records keep their index, type and ID — only the name bytes change — so anything in
/// the settings image that refers to a contact by number keeps pointing at the same one. If the
/// same (type, ID) occurs twice in `entries`, the later name wins.
pub fn append_contacts(
    project: &Project,
    entries: &[NewContact],
    codec: NameCodec,
    capacity: usize,
) -> Result<(Project, ImportReport), RecordError> {
    let mut merge = Merge::new(project, codec, capacity);
    for entry in entries {
        merge.apply(entry)?;
    }
    Ok(merge.finish())
}

/// The running state of [`append_contacts`].
struct Merge<'a> {
    project: &'a Project,
    codec: NameCodec,
    contacts: Vec<Contact>,
    position: HashMap<(u8, u32), usize>,
    next_index: u32,
    room: usize,
    added: usize,
    over: usize,
    shortened: Vec<(u32, String, String)>,
    touched: BTreeSet<usize>,
    kept: BTreeSet<usize>,
}

impl<'a> Merge<'a> {
    fn new(project: &'a Project, codec: NameCodec, capacity: usize) -> Self {
        let contacts = project.contacts.clone();
        // Later duplicates win, like the Python dict comprehension.
        let position = contacts
            .iter()
            .enumerate()
            .map(|(i, c)| ((c.kind(), c.call_id()), i))
            .collect();
        let next_index = contacts
            .iter()
            .map(|c| u32::from(c.index()))
            .max()
            .unwrap_or(0)
            + 1;
        Merge {
            project,
            codec,
            room: capacity.saturating_sub(contacts.len()),
            contacts,
            position,
            next_index,
            added: 0,
            over: 0,
            shortened: Vec::new(),
            touched: BTreeSet::new(),
            kept: BTreeSet::new(),
        }
    }

    fn original(&self) -> usize {
        self.project.contacts.len()
    }

    /// The existing record keeps its custom name.
    fn keeps_name(&self, entry: &NewContact, row: usize) -> bool {
        let Some(replace_if) = &entry.replace_if else {
            return false;
        };
        row < self.original() && !replace_if(&self.project.contacts[row].name(self.codec))
    }

    fn apply(&mut self, entry: &NewContact) -> Result<(), RecordError> {
        let key = (entry.kind, entry.call_id);
        let existing = self.position.get(&key).copied();
        if existing.is_none() && self.added >= self.room {
            self.over += 1;
            return Ok(());
        }
        if let Some(row) = existing.filter(|&row| self.keeps_name(entry, row)) {
            self.kept.insert(row);
            return Ok(());
        }
        let (field, stored) = fit_name(&entry.text, NAME_SIZE, self.codec);
        if stored != entry.text {
            self.shortened
                .push((entry.call_id, entry.text.clone(), stored));
        }
        match existing {
            Some(row) => {
                self.contacts[row] = self.contacts[row].with_name_field(&field)?;
                if row < self.original() {
                    self.touched.insert(row);
                }
            }
            None => self.append(entry, &field)?,
        }
        Ok(())
    }

    fn append(&mut self, entry: &NewContact, field: &[u8]) -> Result<(), RecordError> {
        let index = u16::try_from(self.next_index)
            .map_err(|_| RecordError::Invalid("索引或類型超出範圍".into()))?;
        self.contacts
            .push(Contact::build(index, entry.kind, field, entry.call_id)?);
        self.position
            .insert((entry.kind, entry.call_id), self.contacts.len() - 1);
        self.added += 1;
        self.next_index += 1;
        Ok(())
    }

    fn finish(self) -> (Project, ImportReport) {
        let project = self.project;
        let kept = self.kept.difference(&self.touched).count();
        let updated = self
            .touched
            .iter()
            .filter(|&&i| self.contacts[i].raw() != project.contacts[i].raw())
            .count();
        let mut section_order = project.section_order.clone();
        if self.added > 0 && !section_order.iter().any(|s| s == "Contacts") {
            section_order.insert(0, "Contacts".to_string());
        }
        let merged = with_synced_image(&Project {
            contacts: self.contacts,
            section_order,
            ..project.clone()
        });
        let report = ImportReport {
            added: self.added,
            updated,
            unchanged: self.touched.len() - updated,
            over_capacity: self.over,
            shortened: self.shortened,
            kept,
        };
        (merged, report)
    }
}
