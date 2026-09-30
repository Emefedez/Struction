//! The write path: edits go to JSONC sources through the comment-preserving API and into the
//! history; undo and redo write the inverse edits back.
//!
//! The session remembers a hash of every file as it last read or wrote it. A file whose content
//! differs from that on the next access was changed outside the editor ("Open in…", a text
//! editor): its history entries are dropped before anything else happens.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::Value;
use struction_data::edit::{self, EditError, PathSegment};
use thiserror::Error;

use crate::history::{Change, History, Transaction};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("play mode is running: stop it to edit")]
    Playing,
    #[error("{file}: {source}")]
    Io {
        file: String,
        source: std::io::Error,
    },
    #[error("{file}: {source}")]
    Edit { file: String, source: EditError },
}

/// What an edit, undo or redo wrote.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    pub label: String,
    /// Project-relative files that were written, to reload.
    pub files: Vec<String>,
}

#[derive(Debug)]
pub struct EditSession {
    root: PathBuf,
    history: History,
    known: BTreeMap<String, u64>,
    playing: bool,
}

fn hash(text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

impl EditSession {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            history: History::default(),
            known: BTreeMap::new(),
            playing: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn history(&self) -> &History {
        &self.history
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// Play mode runs on a copy of the world: nothing is written and nothing enters history
    /// until it stops.
    pub fn set_playing(&mut self, playing: bool) {
        self.history.close_group();
        self.playing = playing;
    }

    pub fn path_of(&self, file: &str) -> PathBuf {
        self.root.join(file)
    }

    pub fn read(&self, file: &str) -> Result<String, SessionError> {
        fs::read_to_string(self.path_of(file)).map_err(|source| SessionError::Io {
            file: file.into(),
            source,
        })
    }

    fn write(&mut self, file: &str, text: &str) -> Result<(), SessionError> {
        fs::write(self.path_of(file), text).map_err(|source| SessionError::Io {
            file: file.into(),
            source,
        })?;
        self.known.insert(file.to_owned(), hash(text));
        Ok(())
    }

    /// Reads `file` for an edit, first dropping its history if it changed outside the editor.
    fn read_checked(&mut self, file: &str) -> Result<String, SessionError> {
        let text = self.read(file)?;
        let digest = hash(&text);
        if self.known.get(file).is_some_and(|known| *known != digest) {
            self.history.invalidate_file(file);
        }
        self.known.insert(file.to_owned(), digest);
        Ok(text)
    }

    /// Sets the value at `path` in `file`. Edits sharing a `group` key merge into one undo step
    /// until [`Self::end_group`] (a gizmo drag, a slider).
    pub fn set(
        &mut self,
        file: &str,
        path: &[PathSegment],
        value: Value,
        label: &str,
        group: Option<&str>,
    ) -> Result<Applied, SessionError> {
        self.change(file, label, group, |text| edit::set_value(text, path, value))
    }

    /// Removes the value at `path` in `file` (the field falls back to what it inherits).
    pub fn remove(
        &mut self,
        file: &str,
        path: &[PathSegment],
        label: &str,
    ) -> Result<Applied, SessionError> {
        self.change(file, label, None, |text| edit::remove_value(text, path))
    }

    fn change(
        &mut self,
        file: &str,
        label: &str,
        group: Option<&str>,
        apply: impl FnOnce(&str) -> Result<edit::Edited, EditError>,
    ) -> Result<Applied, SessionError> {
        if self.playing {
            return Err(SessionError::Playing);
        }
        let text = self.read_checked(file)?;
        let edited = apply(&text).map_err(|source| SessionError::Edit {
            file: file.into(),
            source,
        })?;
        let unchanged = edited.edit.previous == edited.edit.next;
        if unchanged && group.is_none() {
            return Ok(Applied {
                label: label.into(),
                files: Vec::new(),
            });
        }
        if !unchanged {
            self.write(file, &edited.text)?;
        }
        self.history.record(
            label,
            Change {
                file: file.into(),
                edit: edited.edit,
            },
            group,
        );
        Ok(Applied {
            label: label.into(),
            files: if unchanged { vec![] } else { vec![file.into()] },
        })
    }

    /// Closes the open edit group: the next edit starts a new undo step.
    pub fn end_group(&mut self) {
        self.history.close_group();
    }

    /// Reverts the last transaction. `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Result<Option<Applied>, SessionError> {
        if self.playing {
            return Err(SessionError::Playing);
        }
        self.check_external_changes();
        let Some(transaction) = self.history.undo() else {
            return Ok(None);
        };
        let result = self.apply_all(&transaction, &transaction.inverse());
        if result.is_err() {
            self.history.discard_redo();
        }
        result.map(Some)
    }

    /// Re-applies the last undone transaction.
    pub fn redo(&mut self) -> Result<Option<Applied>, SessionError> {
        if self.playing {
            return Err(SessionError::Playing);
        }
        self.check_external_changes();
        let Some(transaction) = self.history.redo() else {
            return Ok(None);
        };
        let result = self.apply_all(&transaction, &transaction.changes);
        if result.is_err() {
            self.history.discard_undo();
        }
        result.map(Some)
    }

    fn apply_all(
        &mut self,
        transaction: &Transaction,
        changes: &[Change],
    ) -> Result<Applied, SessionError> {
        let mut texts: BTreeMap<String, String> = BTreeMap::new();
        for change in changes {
            let text = match texts.remove(&change.file) {
                Some(text) => text,
                None => self.read(&change.file)?,
            };
            let edited = edit::apply_edit(&text, &change.edit).map_err(|source| {
                SessionError::Edit {
                    file: change.file.clone(),
                    source,
                }
            })?;
            texts.insert(change.file.clone(), edited.text);
        }
        for (file, text) in &texts {
            self.write(file, text)?;
        }
        Ok(Applied {
            label: transaction.label.clone(),
            files: texts.into_keys().collect(),
        })
    }

    /// Whether `file`'s content on disk is what the editor last read or wrote. Files the editor
    /// never touched count as external.
    pub fn is_own_content(&self, file: &str, text: &str) -> bool {
        self.known.get(file) == Some(&hash(text))
    }

    /// Notes that `file` changed on disk. If the editor did not write that content, the file's
    /// history is dropped; returns whether it was an outside change.
    pub fn file_changed(&mut self, file: &str) -> bool {
        let text = self.read(file).unwrap_or_default();
        if self.is_own_content(file, &text) {
            return false;
        }
        if self.known.contains_key(file) || self.history.files().iter().any(|f| f == file) {
            self.history.invalidate_file(file);
        }
        self.known.insert(file.to_owned(), hash(&text));
        true
    }

    /// Checks every file the editor has touched against disk; drops the history of those changed
    /// outside and returns them.
    pub fn check_external_changes(&mut self) -> Vec<String> {
        let files: Vec<String> = self.known.keys().cloned().collect();
        files
            .into_iter()
            .filter(|file| {
                let text = self.read(file).unwrap_or_default();
                !self.is_own_content(file, &text) && self.file_changed(file)
            })
            .collect()
    }

    /// Creates a new file (templates). Not undoable: a scaffolded file belongs to the user.
    pub fn create_file(&mut self, file: &str, text: &str) -> Result<(), SessionError> {
        let path = self.path_of(file);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|source| SessionError::Io {
                file: file.into(),
                source,
            })?;
        }
        self.write(file, text)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use struction_data::edit::parse_path;

    use super::*;

    const OGRE: &str = r#"// A big dumb brute.
{
  "descendsFrom": "Actor",
  "components": {
    // Bigger than the Actor default.
    "Health": { "current": 60.0, "max": 60.0 }
  }
}
"#;

    fn project() -> (tempfile::TempDir, EditSession) {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("minions/ogre")).unwrap();
        fs::write(dir.path().join("minions/ogre/entity.jsonc"), OGRE).unwrap();
        let session = EditSession::new(dir.path());
        (dir, session)
    }

    const FILE: &str = "minions/ogre/entity.jsonc";

    #[test]
    fn edits_write_through_and_undo_restores_the_exact_text() {
        let (_dir, mut session) = project();
        let applied = session
            .set(FILE, &parse_path("components.Health.max"), json!(80.0), "Max", None)
            .unwrap();
        assert_eq!(applied.files, vec![FILE.to_owned()]);
        let text = session.read(FILE).unwrap();
        assert!(text.contains("\"max\": 80.0"));
        assert!(text.contains("// Bigger than the Actor default."));

        session.undo().unwrap().unwrap();
        assert_eq!(session.read(FILE).unwrap(), OGRE);
        session.redo().unwrap().unwrap();
        assert!(session.read(FILE).unwrap().contains("\"max\": 80.0"));
    }

    #[test]
    fn adding_and_removing_fields_is_undoable() {
        let (_dir, mut session) = project();
        session
            .set(FILE, &parse_path("components.Stats.agility"), json!(6), "Add", None)
            .unwrap();
        session
            .remove(FILE, &parse_path("components.Health.current"), "Revert")
            .unwrap();
        let text = session.read(FILE).unwrap();
        assert!(text.contains("\"Stats\""));
        assert!(!text.contains("\"current\""));
        session.undo().unwrap();
        session.undo().unwrap();
        assert_eq!(session.read(FILE).unwrap(), OGRE);
        assert!(session.undo().unwrap().is_none());
    }

    #[test]
    fn a_grouped_drag_is_one_undo_step() {
        let (_dir, mut session) = project();
        let path = parse_path("transform.translation");
        for x in 1..=20 {
            session
                .set(FILE, &path, json!([x as f64 * 0.1, 0.0, 0.0]), "Move", Some("gizmo"))
                .unwrap();
        }
        session.end_group();
        assert_eq!(session.history().undo_stack().len(), 1);
        session.undo().unwrap();
        assert_eq!(session.read(FILE).unwrap(), OGRE);
    }

    #[test]
    fn play_mode_blocks_edits_and_keeps_history_untouched() {
        let (_dir, mut session) = project();
        session
            .set(FILE, &parse_path("components.Health.max"), json!(70.0), "Max", None)
            .unwrap();
        session.set_playing(true);
        assert!(matches!(
            session.set(FILE, &parse_path("components.Health.max"), json!(1.0), "x", None),
            Err(SessionError::Playing)
        ));
        assert!(matches!(session.undo(), Err(SessionError::Playing)));
        session.set_playing(false);
        assert_eq!(session.history().undo_stack().len(), 1);
        assert!(session.read(FILE).unwrap().contains("70.0"));
    }

    #[test]
    fn an_outside_change_invalidates_that_files_history() {
        let (dir, mut session) = project();
        fs::write(dir.path().join("other.jsonc"), "{}").unwrap();
        session
            .set(FILE, &parse_path("components.Health.max"), json!(70.0), "Max", None)
            .unwrap();
        session
            .set("other.jsonc", &parse_path("a"), json!(1), "A", None)
            .unwrap();

        // "Open in…": another program rewrites the ogre.
        let outside = OGRE.replace("60.0", "65.0");
        fs::write(dir.path().join(FILE), &outside).unwrap();
        assert_eq!(session.check_external_changes(), vec![FILE.to_owned()]);
        assert_eq!(session.history().files(), vec!["other.jsonc".to_owned()]);

        // Undo only reaches the other file; the outside text stays.
        session.undo().unwrap().unwrap();
        assert!(session.undo().unwrap().is_none());
        assert_eq!(session.read(FILE).unwrap(), outside);
    }

    #[test]
    fn an_outside_change_is_noticed_on_the_next_edit_too() {
        let (dir, mut session) = project();
        session
            .set(FILE, &parse_path("components.Health.max"), json!(70.0), "Max", None)
            .unwrap();
        fs::write(dir.path().join(FILE), OGRE).unwrap();
        session
            .set(FILE, &parse_path("components.Health.current"), json!(1.0), "Cur", None)
            .unwrap();
        assert_eq!(session.history().undo_stack().len(), 1);
        assert_eq!(session.history().undo_label(), Some("Cur"));
    }

    #[test]
    fn own_writes_are_not_outside_changes() {
        let (_dir, mut session) = project();
        session
            .set(FILE, &parse_path("components.Health.max"), json!(70.0), "Max", None)
            .unwrap();
        assert!(!session.file_changed(FILE));
        assert!(session.check_external_changes().is_empty());
        assert!(session.history().can_undo());
    }
}
