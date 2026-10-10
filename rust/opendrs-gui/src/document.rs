//! The project being edited: its file, the undo history, and whether it differs from what was
//! last saved (port of the project-state part of `main_window.py`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use opendrs_core::f136::{self, Project};
use opendrs_core::history::{DEFAULT_LIMIT, History};

/// Revisions are unique across documents, so a cache keyed by one never matches another project.
static NEXT_REVISION: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, Ordering::Relaxed)
}

pub struct Document {
    path: Option<PathBuf>,
    /// Changes whenever the current project does (edit, undo, redo, open).
    revision: u64,
    /// Where an unsaved project came from (e.g. read from the radio), shown instead of a name.
    source: String,
    history: History,
    saved: Project,
}

impl Default for Document {
    fn default() -> Self {
        Document {
            path: None,
            revision: next_revision(),
            source: String::new(),
            history: History::new(Project::default(), DEFAULT_LIMIT),
            saved: Project::default(),
        }
    }
}

impl Document {
    /// Open a project file; the error text names the problem.
    pub fn open(path: &Path) -> Result<Self, String> {
        let project = f136::load(path).map_err(|e| e.to_string())?;
        Ok(Document {
            path: Some(path.to_path_buf()),
            revision: next_revision(),
            source: String::new(),
            history: History::new(project.clone(), DEFAULT_LIMIT),
            saved: project,
        })
    }

    /// A project that did not come from a file (it counts as unsaved).
    pub fn unsaved(project: Project, source: impl Into<String>) -> Self {
        Document {
            path: None,
            revision: next_revision(),
            source: source.into(),
            history: History::new(project, DEFAULT_LIMIT),
            saved: Project::default(),
        }
    }

    /// A project opened from somewhere it must not be saved back to (a backup): clean, but
    /// without a path, so saving asks for a new file.
    pub fn detached(project: Project, source: impl Into<String>) -> Self {
        Document {
            path: None,
            revision: next_revision(),
            source: source.into(),
            history: History::new(project.clone(), DEFAULT_LIMIT),
            saved: project,
        }
    }

    pub fn project(&self) -> &Project {
        self.history.current()
    }

    /// A number that changes whenever [`Self::project`] does (for caches of derived views).
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Where an unsaved project came from (e.g. 從對講機讀取…), or "".
    pub fn source(&self) -> &str {
        &self.source
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Something to edit: a file was opened or the radio was read.
    pub fn has_project(&self) -> bool {
        let p = self.project();
        !(p.image.is_empty()
            && p.contacts.is_empty()
            && p.zones.is_empty()
            && p.channels.is_empty())
    }

    pub fn is_dirty(&self) -> bool {
        self.project() != &self.saved
    }

    /// Record an edit; false when it changed nothing.
    pub fn commit(&mut self, project: Project, label: impl Into<String>) -> bool {
        let pushed = self.history.push(project, label);
        self.changed(pushed)
    }

    pub fn undo(&mut self) -> bool {
        let done = self.history.undo().is_some();
        self.changed(done)
    }

    pub fn redo(&mut self) -> bool {
        let done = self.history.redo().is_some();
        self.changed(done)
    }

    fn changed(&mut self, changed: bool) -> bool {
        if changed {
            self.revision = next_revision();
        }
        changed
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    /// 「復原 <label>」 as the menu shows it.
    pub fn undo_text(&self) -> String {
        format!("復原 {}", self.history.undo_label())
            .trim()
            .to_string()
    }

    pub fn redo_text(&self) -> String {
        format!("重做 {}", self.history.redo_label())
            .trim()
            .to_string()
    }

    /// Save to `path` (a `.bak` of the previous file is kept); afterwards the project is clean.
    pub fn save_to(&mut self, path: &Path) -> Result<(), String> {
        f136::save(self.project(), path, true).map_err(|e| e.to_string())?;
        self.saved = self.project().clone();
        self.path = Some(path.to_path_buf());
        self.source.clear();
        Ok(())
    }

    /// `*name — OpenDRS 0.1.0`, with the asterisk while there are unsaved changes.
    pub fn window_title(&self) -> String {
        let name = match &self.path {
            Some(path) => path.file_name().map_or_else(
                || path.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            None if !self.source.is_empty() => self.source.clone(),
            None => "未開啟檔案".into(),
        };
        let dirty = if self.is_dirty() { "*" } else { "" };
        format!("{dirty}{name} — OpenDRS {}", env!("CARGO_PKG_VERSION"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/dh98vu_20250811.F136")
    }

    #[test]
    fn edits_mark_the_project_dirty_until_saved() {
        let mut doc = Document::open(&fixture()).unwrap();
        assert!(doc.has_project() && !doc.is_dirty());
        assert_eq!(
            doc.window_title(),
            format!(
                "dh98vu_20250811.F136 — OpenDRS {}",
                env!("CARGO_PKG_VERSION")
            )
        );
        let mut edited = doc.project().clone();
        edited.channels.remove(0);
        assert!(doc.commit(edited.clone(), "刪除頻道"));
        assert!(!doc.commit(edited, "同樣的內容"));
        assert!(doc.is_dirty() && doc.window_title().starts_with('*'));
        assert_eq!(doc.undo_text(), "復原 刪除頻道");
        assert!(doc.undo() && !doc.is_dirty());
        assert_eq!(doc.redo_text(), "重做 刪除頻道");
        assert!(doc.redo() && doc.is_dirty());
    }

    #[test]
    fn saving_keeps_a_backup_and_clears_the_mark() {
        let dir = std::env::temp_dir().join(format!("opendrs-gui-doc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.F136");
        std::fs::copy(fixture(), &path).unwrap();
        let mut doc = Document::open(&path).unwrap();
        let mut edited = doc.project().clone();
        edited.channels[0] = edited.channels[0].with_rx(439_500_000);
        doc.commit(edited, "編輯");
        doc.save_to(&path).unwrap();
        assert!(!doc.is_dirty() && dir.join("p.F136.bak").is_file());
        assert_eq!(Document::open(&path).unwrap().project(), doc.project());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_project_from_the_radio_is_unsaved() {
        let project = Document::open(&fixture()).unwrap().project().clone();
        let doc = Document::unsaved(project, "從對講機讀取");
        assert!(doc.is_dirty() && doc.window_title().starts_with("*從對講機讀取"));
        assert!(Document::open(Path::new("no-such.F136")).is_err());
        assert!(!Document::default().has_project());
    }
}
