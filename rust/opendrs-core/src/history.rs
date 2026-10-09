//! Undo / redo over immutable `Project` snapshots (port of `history.py`). Projects are values, so
//! a history is just a list of snapshots — undo cannot drift out of sync with what was edited.

use crate::f136::Project;

pub const DEFAULT_LIMIT: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    project: Project,
    label: String,
}

#[derive(Clone, Debug)]
pub struct History {
    limit: usize,
    past: Vec<Step>,
    current: Step,
    future: Vec<Step>,
}

impl History {
    pub fn new(initial: Project, limit: usize) -> Self {
        History {
            limit: limit.max(1),
            past: Vec::new(),
            current: Step {
                project: initial,
                label: String::new(),
            },
            future: Vec::new(),
        }
    }

    pub fn reset(&mut self, project: Project) {
        *self = History::new(project, self.limit);
    }

    pub fn current(&self) -> &Project {
        &self.current.project
    }

    /// Record `project` as the new state; ignored (false) if nothing changed.
    pub fn push(&mut self, project: Project, label: impl Into<String>) -> bool {
        if project == self.current.project {
            return false;
        }
        let previous = std::mem::replace(
            &mut self.current,
            Step {
                project,
                label: label.into(),
            },
        );
        self.past.push(previous);
        if self.past.len() > self.limit {
            self.past.remove(0);
        }
        self.future.clear();
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }
    pub fn undo_label(&self) -> &str {
        if self.past.is_empty() {
            ""
        } else {
            &self.current.label
        }
    }
    pub fn redo_label(&self) -> &str {
        self.future.last().map_or("", |s| &s.label)
    }

    pub fn undo(&mut self) -> Option<&Project> {
        let previous = self.past.pop()?;
        let current = std::mem::replace(&mut self.current, previous);
        self.future.push(current);
        Some(&self.current.project)
    }

    pub fn redo(&mut self) -> Option<&Project> {
        let next = self.future.pop()?;
        let current = std::mem::replace(&mut self.current, next);
        self.past.push(current);
        Some(&self.current.project)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(byte: u8) -> Project {
        Project {
            image: vec![byte],
            ..Project::default()
        }
    }

    #[test]
    fn undo_redo_and_limit() {
        let mut h = History::new(project(0), 2);
        assert!(!h.push(project(0), "same"));
        for i in 1..=3 {
            assert!(h.push(project(i), format!("edit {i}")));
        }
        assert_eq!(h.undo_label(), "edit 3");
        assert_eq!(h.undo().unwrap().image, vec![2]);
        assert_eq!(h.undo().unwrap().image, vec![1]);
        assert!(h.undo().is_none()); // only 2 steps kept
        assert_eq!(h.redo_label(), "edit 2");
        assert_eq!(h.redo().unwrap().image, vec![2]);
        h.push(project(9), "branch");
        assert!(!h.can_redo());
    }
}
