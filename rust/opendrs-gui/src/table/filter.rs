//! Searching, quick filters and sorting of a record table (port of `record_filter.py` and the
//! controls of `filter_bar.py`). They only change what is shown: the project's own record order
//! is never touched.

use std::collections::HashMap;

use opendrs_core::cleanup::needs_cleaning;
use opendrs_core::f136::Project;
use opendrs_core::pystr::{casefold, split, strip, thousands};
use opendrs_core::textcodec::NameCodec;

use super::model::{self, SortKey, Table};

/// 「顯示：」 quick filters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    All,
    ControlChars,
    DuplicateNames,
    /// Contacts only.
    DuplicateIds,
}

impl Show {
    pub fn label(self) -> &'static str {
        match self {
            Show::All => "全部",
            Show::ControlChars => "名稱含換行字元",
            Show::DuplicateNames => "名稱重複",
            Show::DuplicateIds => "類型與 ID 重複",
        }
    }

    pub fn choices(table: Table) -> &'static [Show] {
        match table {
            Table::Contacts => &[
                Show::All,
                Show::ControlChars,
                Show::DuplicateNames,
                Show::DuplicateIds,
            ],
            Table::Zones | Table::Channels => {
                &[Show::All, Show::ControlChars, Show::DuplicateNames]
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Ascending,
    Descending,
}

/// The search box, its column scope, the contact type and the quick filter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filter {
    pub text: String,
    /// Search only this column (`None`: 全部欄位).
    pub scope: Option<usize>,
    /// Contacts of this kind only.
    pub kind: Option<u8>,
    pub show: Show,
}

impl Filter {
    pub fn is_filtered(&self) -> bool {
        !self.terms().is_empty() || self.kind.is_some() || self.show != Show::All
    }

    fn terms(&self) -> Vec<String> {
        split(&self.text).map(casefold).collect()
    }
}

fn name_field(project: &Project, table: Table, row: usize) -> &[u8] {
    match table {
        Table::Channels => project.channels[row].name_field(),
        Table::Zones => project.zones[row].name_field(),
        Table::Contacts => project.contacts[row].name_field(),
    }
}

fn name(project: &Project, table: Table, row: usize, codec: NameCodec) -> String {
    match table {
        Table::Channels => project.channels[row].name(codec),
        Table::Zones => project.zones[row].name(codec),
        Table::Contacts => project.contacts[row].name(codec),
    }
}

/// Rows whose key occurs more than once.
fn duplicated<K: std::hash::Hash + Eq>(keys: Vec<K>) -> Vec<bool> {
    let mut seen: HashMap<&K, usize> = HashMap::new();
    for key in &keys {
        *seen.entry(key).or_default() += 1;
    }
    keys.iter().map(|k| seen[k] > 1).collect()
}

fn show_rows(project: &Project, table: Table, codec: NameCodec, show: Show) -> Vec<bool> {
    let rows = 0..model::row_count(project, table);
    match show {
        Show::All => rows.map(|_| true).collect(),
        Show::ControlChars => rows
            .map(|r| needs_cleaning(name_field(project, table, r)))
            .collect(),
        Show::DuplicateNames => duplicated(
            rows.map(|r| casefold(strip(&name(project, table, r, codec))))
                .collect(),
        ),
        Show::DuplicateIds => match table {
            Table::Contacts => duplicated(
                project
                    .contacts
                    .iter()
                    .map(|c| (c.kind(), c.call_id()))
                    .collect(),
            ),
            _ => rows.map(|_| true).collect(),
        },
    }
}

/// The rows to show, in display order: filtered, then sorted (stable, so equal keys keep the
/// record order) when `sort` is given.
pub fn visible_rows(
    project: &Project,
    table: Table,
    codec: NameCodec,
    filter: &Filter,
    sort: Option<(usize, Order)>,
) -> Vec<usize> {
    let terms = filter.terms();
    let shown = show_rows(project, table, codec, filter.show);
    let columns: Vec<usize> = match filter.scope {
        Some(col) => vec![col],
        None => (0..model::columns(table).len()).collect(),
    };
    let mut rows: Vec<usize> = (0..model::row_count(project, table))
        .filter(|&r| shown[r])
        .filter(|&r| match (table, filter.kind) {
            (Table::Contacts, Some(kind)) => project.contacts[r].kind() == kind,
            _ => true,
        })
        .filter(|&r| {
            if terms.is_empty() {
                return true;
            }
            let texts: Vec<String> = columns
                .iter()
                .map(|&c| casefold(&model::text(project, table, r, c, codec)))
                .collect();
            terms
                .iter()
                .all(|term| texts.iter().any(|t| t.contains(term.as_str())))
        })
        .collect();
    if let Some((col, order)) = sort {
        let keys: HashMap<usize, SortKey> = rows
            .iter()
            .map(|&r| (r, model::sort_key(project, table, r, col, codec)))
            .collect();
        match order {
            Order::Ascending => rows.sort_by(|a, b| keys[a].cmp(&keys[b])),
            Order::Descending => rows.sort_by(|a, b| keys[b].cmp(&keys[a])),
        }
    }
    rows
}

/// 「共 {n} 筆」, or 「顯示 {shown} / {total} 筆」 while a filter is on.
pub fn count_text(shown: usize, total: usize, filtered: bool) -> String {
    if filtered {
        format!(
            "顯示 {} / {} 筆",
            thousands(shown as u64),
            thousands(total as u64)
        )
    } else {
        format!("共 {} 筆", thousands(total as u64))
    }
}

/// A header click: ascending, then descending, then back to the record order.
pub fn next_sort(current: Option<(usize, Order)>, col: usize) -> Option<(usize, Order)> {
    match current {
        Some((c, Order::Ascending)) if c == col => Some((col, Order::Descending)),
        Some((c, Order::Descending)) if c == col => None,
        _ => Some((col, Order::Ascending)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_clicks_cycle_through_three_states() {
        let asc = next_sort(None, 2);
        assert_eq!(asc, Some((2, Order::Ascending)));
        let desc = next_sort(asc, 2);
        assert_eq!(desc, Some((2, Order::Descending)));
        assert_eq!(next_sort(desc, 2), None);
        assert_eq!(next_sort(desc, 3), Some((3, Order::Ascending)));
    }

    #[test]
    fn counts() {
        assert_eq!(count_text(3, 1359, true), "顯示 3 / 1,359 筆");
        assert_eq!(count_text(1359, 1359, false), "共 1,359 筆");
    }
}
