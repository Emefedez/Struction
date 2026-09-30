//! Source edits and history, shared by UI and tool clients. Undo restores exact source snapshots.
//! Writes are staged beside their destinations and atomically replace individual files. Grouped
//! multi-file writes roll back on ordinary errors; this is not a crash-recovery journal.

use std::collections::BTreeMap;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use struction_data::DataError;
use struction_data::edit::{self, EditError, PathSegment};
use thiserror::Error;

use crate::history::{Change, History, SourceChange};

#[derive(Debug, Error)]
pub enum SessionError {
    #[error("{0}")]
    InvalidOperation(String),
    #[error("play mode is running: stop it to edit")]
    Playing,
    #[error("invalid project-relative path: {0}")]
    InvalidPath(String),
    #[error("{0}: source changed since it was read")]
    Conflict(String),
    #[error("edit failed validation: {0:?}")]
    Validation(Vec<DataError>),
    #[error("{file}: {source}")]
    Io {
        file: String,
        source: std::io::Error,
    },
    #[error("{file}: {source}")]
    Edit { file: String, source: EditError },
    #[error("{0}")]
    Rollback(String),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Applied {
    pub label: String,
    pub files: Vec<String>,
}

/// Structured paths avoid ambiguities in names containing dots or slashes.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum Field {
    Key(String),
    Index(usize),
}

impl From<&Field> for PathSegment {
    fn from(field: &Field) -> Self {
        match field {
            Field::Key(key) => Self::Key(key.clone()),
            Field::Index(index) => Self::Index(*index),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum EditRequest {
    Set {
        file: String,
        path: Vec<Field>,
        value: Value,
        label: String,
        #[serde(default)]
        group: Option<String>,
        #[serde(default)]
        revision: Option<String>,
    },
    Remove {
        file: String,
        path: Vec<Field>,
        label: String,
        #[serde(default)]
        revision: Option<String>,
    },
}

#[derive(Debug)]
pub struct EditSession {
    root: PathBuf,
    history: History,
    known: BTreeMap<String, String>,
    playing: bool,
}

pub fn revision(text: &str) -> String {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// A synced temporary file holding `text` in the directory of `path`, ready to persist there.
fn stage(path: &Path, text: &str) -> std::io::Result<tempfile::NamedTempFile> {
    let mut staged =
        tempfile::NamedTempFile::new_in(path.parent().expect("project-relative file"))?;
    staged.write_all(text.as_bytes())?;
    staged.as_file().sync_all()?;
    Ok(staged)
}

/// What each file would contain once `sources` are written, for validation.
fn candidates(sources: &BTreeMap<String, SourceChange>) -> BTreeMap<String, String> {
    sources
        .iter()
        .map(|(file, change)| (file.clone(), change.after.clone()))
        .collect()
}

fn io(file: &str, source: std::io::Error) -> SessionError {
    SessionError::Io {
        file: file.into(),
        source,
    }
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
    pub fn set_playing(&mut self, playing: bool) {
        self.history.close_group();
        self.playing = playing;
    }

    /// Reject aliases and symlinks so two client paths cannot bypass conflict/history tracking.
    pub fn path_of(&self, file: &str) -> Result<PathBuf, SessionError> {
        if file.is_empty()
            || file.contains(['\\', '\0'])
            || file
                .split('/')
                .any(|p| p.is_empty() || p == "." || p == "..")
            || Path::new(file)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(SessionError::InvalidPath(file.into()));
        }
        let mut path = fs::canonicalize(&self.root).map_err(|e| io(file, e))?;
        for part in Path::new(file).components() {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(SessionError::InvalidPath(file.into()));
                }
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(file, e)),
            }
        }
        Ok(path)
    }

    pub fn read(&self, file: &str) -> Result<String, SessionError> {
        fs::read_to_string(self.path_of(file)?).map_err(|e| io(file, e))
    }
    fn read_checked(&mut self, file: &str) -> Result<String, SessionError> {
        let text = self.read(file)?;
        if self
            .known
            .get(file)
            .is_some_and(|known| *known != revision(&text))
        {
            self.history.invalidate_file(file);
        }
        self.known.insert(file.into(), revision(&text));
        Ok(text)
    }

    pub fn set(
        &mut self,
        file: &str,
        path: &[PathSegment],
        value: Value,
        label: &str,
        group: Option<&str>,
    ) -> Result<Applied, SessionError> {
        self.change(
            file,
            label,
            group,
            None,
            |text| edit::set_value(text, path, value),
            |_| Ok(()),
        )
    }
    pub fn remove(
        &mut self,
        file: &str,
        path: &[PathSegment],
        label: &str,
    ) -> Result<Applied, SessionError> {
        self.change(
            file,
            label,
            None,
            None,
            |text| edit::remove_value(text, path),
            |_| Ok(()),
        )
    }

    /// Validates candidate sources before writing or recording history.
    pub fn apply_checked(
        &mut self,
        request: EditRequest,
        validate: impl FnOnce(&BTreeMap<String, String>) -> Result<(), Vec<DataError>>,
    ) -> Result<Applied, SessionError> {
        let (file, path, label, group, revision, value) = match request {
            EditRequest::Set {
                file,
                path,
                value,
                label,
                group,
                revision,
            } => (file, path, label, group, revision, Some(value)),
            EditRequest::Remove {
                file,
                path,
                label,
                revision,
            } => (file, path, label, None, revision, None),
        };
        let path: Vec<_> = path.iter().map(PathSegment::from).collect();
        self.change(
            &file,
            &label,
            group.as_deref(),
            revision.as_deref(),
            |text| match value {
                Some(value) => edit::set_value(text, &path, value),
                None => edit::remove_value(text, &path),
            },
            validate,
        )
    }

    fn change(
        &mut self,
        file: &str,
        label: &str,
        group: Option<&str>,
        expected: Option<&str>,
        apply: impl FnOnce(&str) -> Result<edit::Edited, EditError>,
        validate: impl FnOnce(&BTreeMap<String, String>) -> Result<(), Vec<DataError>>,
    ) -> Result<Applied, SessionError> {
        if self.playing {
            return Err(SessionError::Playing);
        }
        let text = self.read_checked(file)?;
        if expected.is_some_and(|expected| expected != revision(&text)) {
            return Err(SessionError::Conflict(file.into()));
        }
        let edited = apply(&text).map_err(|source| SessionError::Edit {
            file: file.into(),
            source,
        })?;
        if edited.edit.previous == edited.edit.next {
            return Ok(Applied {
                label: label.into(),
                files: vec![],
            });
        }
        let sources = BTreeMap::from([(
            file.to_owned(),
            SourceChange {
                before: text,
                after: edited.text,
            },
        )]);
        validate(&candidates(&sources)).map_err(SessionError::Validation)?;
        self.write_sources(&sources)?;
        self.history.record_source(
            label,
            Change {
                file: file.into(),
                edit: edited.edit,
            },
            sources[file].clone(),
            group,
        );
        Ok(Applied {
            label: label.into(),
            files: vec![file.into()],
        })
    }

    /// `text` in a temporary file beside the existing `file`, with its permissions.
    fn staged(&self, file: &str, text: &str) -> Result<tempfile::NamedTempFile, SessionError> {
        let path = self.path_of(file)?;
        let permissions = fs::metadata(&path).map_err(|e| io(file, e))?.permissions();
        let staged = stage(&path, text).map_err(|e| io(file, e))?;
        staged
            .as_file()
            .set_permissions(permissions)
            .map_err(|e| io(file, e))?;
        Ok(staged)
    }

    fn write_sources(
        &mut self,
        sources: &BTreeMap<String, SourceChange>,
    ) -> Result<(), SessionError> {
        let mut staged = Vec::new();
        for (file, change) in sources {
            if self.read(file)? != change.before {
                return Err(SessionError::Conflict(file.clone()));
            }
            staged.push((
                file.clone(),
                self.path_of(file)?,
                self.staged(file, &change.after)?,
            ));
        }
        let mut written: Vec<String> = Vec::new();
        for (file, path, stage) in staged {
            let result = (|| {
                if self.read(&file)? != sources[&file].before {
                    Err(SessionError::Conflict(file.clone()))
                } else {
                    stage
                        .persist(path)
                        .map(|_| ())
                        .map_err(|e| io(&file, e.error))
                }
            })();
            if let Err(error) = result {
                for prior in written.iter().rev() {
                    let rollback = (|| {
                        if self.read(prior)? != sources[prior].after {
                            return Err(SessionError::Conflict(prior.clone()));
                        }
                        self.staged(prior, &sources[prior].before)
                    })()
                    .and_then(|stage| {
                        stage
                            .persist(self.path_of(prior)?)
                            .map(|_| ())
                            .map_err(|e| io(prior, e.error))
                    });
                    if let Err(rollback) = rollback {
                        return Err(SessionError::Rollback(format!(
                            "{error}; rollback failed: {rollback}"
                        )));
                    }
                }
                return Err(error);
            }
            written.push(file);
        }
        for (file, source) in sources {
            self.known.insert(file.clone(), revision(&source.after));
        }
        Ok(())
    }

    pub fn end_group(&mut self) {
        self.history.close_group();
    }
    pub fn undo(&mut self) -> Result<Option<Applied>, SessionError> {
        self.undo_checked(|_| Ok(()))
    }
    pub fn redo(&mut self) -> Result<Option<Applied>, SessionError> {
        self.redo_checked(|_| Ok(()))
    }
    pub fn undo_checked(
        &mut self,
        validate: impl FnOnce(&BTreeMap<String, String>) -> Result<(), Vec<DataError>>,
    ) -> Result<Option<Applied>, SessionError> {
        self.restore(false, validate)
    }
    pub fn redo_checked(
        &mut self,
        validate: impl FnOnce(&BTreeMap<String, String>) -> Result<(), Vec<DataError>>,
    ) -> Result<Option<Applied>, SessionError> {
        self.restore(true, validate)
    }
    fn restore(
        &mut self,
        redo: bool,
        validate: impl FnOnce(&BTreeMap<String, String>) -> Result<(), Vec<DataError>>,
    ) -> Result<Option<Applied>, SessionError> {
        if self.playing {
            return Err(SessionError::Playing);
        }
        self.check_external_changes();
        self.history.close_group();
        let next = if redo {
            self.history.next_redo()
        } else {
            self.history.next_undo()
        };
        let Some(transaction) = next else {
            return Ok(None);
        };
        let label = transaction.label.clone();
        let sources: BTreeMap<_, _> = transaction
            .sources
            .iter()
            .map(|(file, source)| {
                let change = if redo {
                    source.clone()
                } else {
                    source.reversed()
                };
                (file.clone(), change)
            })
            .collect();
        validate(&candidates(&sources)).map_err(SessionError::Validation)?;
        self.write_sources(&sources)?;
        if redo {
            self.history.redo();
        } else {
            self.history.undo();
        }
        Ok(Some(Applied {
            label,
            files: sources.into_keys().collect(),
        }))
    }

    pub fn is_own_content(&self, file: &str, text: &str) -> bool {
        self.known.get(file) == Some(&revision(text))
    }
    pub fn file_changed(&mut self, file: &str) -> bool {
        let text = self.read(file).unwrap_or_default();
        if self.is_own_content(file, &text) {
            return false;
        }
        self.history.invalidate_file(file);
        self.known.insert(file.into(), revision(&text));
        true
    }
    pub fn check_external_changes(&mut self) -> Vec<String> {
        let files: Vec<_> = self.known.keys().cloned().collect();
        files
            .into_iter()
            .filter(|file| self.file_changed(file))
            .collect()
    }

    /// Scaffolds a user-owned file without overwriting existing content.
    pub fn create_file(&mut self, file: &str, text: &str) -> Result<(), SessionError> {
        if self.playing {
            return Err(SessionError::Playing);
        }
        let path = self.path_of(file)?;
        fs::create_dir_all(path.parent().expect("project-relative file"))
            .map_err(|e| io(file, e))?;
        stage(&path, text)
            .map_err(|e| io(file, e))?
            .persist_noclobber(path)
            .map_err(|e| io(file, e.error))?;
        self.known.insert(file.into(), revision(text));
        Ok(())
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
            .set(
                FILE,
                &parse_path("components.Health.max"),
                json!(80.0),
                "Max",
                None,
            )
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
            .set(
                FILE,
                &parse_path("components.Stats.agility"),
                json!(6),
                "Add",
                None,
            )
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
                .set(
                    FILE,
                    &path,
                    json!([x as f64 * 0.1, 0.0, 0.0]),
                    "Move",
                    Some("gizmo"),
                )
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
            .set(
                FILE,
                &parse_path("components.Health.max"),
                json!(70.0),
                "Max",
                None,
            )
            .unwrap();
        session.set_playing(true);
        assert!(matches!(
            session.set(
                FILE,
                &parse_path("components.Health.max"),
                json!(1.0),
                "x",
                None
            ),
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
            .set(
                FILE,
                &parse_path("components.Health.max"),
                json!(70.0),
                "Max",
                None,
            )
            .unwrap();
        session
            .set("other.jsonc", &parse_path("a"), json!(1), "A", None)
            .unwrap();

        // "Open in…": another program rewrites the ogre.
        let outside = OGRE.replace("60.0", "65.0");
        fs::write(dir.path().join(FILE), &outside).unwrap();
        assert_eq!(session.check_external_changes(), vec![FILE.to_owned()]);
        let history = session.history();
        assert_eq!(history.undo_stack().len(), 1);
        assert_eq!(history.undo_stack()[0].files(), ["other.jsonc"]);

        // Undo only reaches the other file; the outside text stays.
        session.undo().unwrap().unwrap();
        assert!(session.undo().unwrap().is_none());
        assert_eq!(session.read(FILE).unwrap(), outside);
    }

    #[test]
    fn an_outside_change_is_noticed_on_the_next_edit_too() {
        let (dir, mut session) = project();
        session
            .set(
                FILE,
                &parse_path("components.Health.max"),
                json!(70.0),
                "Max",
                None,
            )
            .unwrap();
        fs::write(dir.path().join(FILE), OGRE).unwrap();
        session
            .set(
                FILE,
                &parse_path("components.Health.current"),
                json!(1.0),
                "Cur",
                None,
            )
            .unwrap();
        assert_eq!(session.history().undo_stack().len(), 1);
        assert_eq!(session.history().undo_label(), Some("Cur"));
    }

    #[test]
    fn own_writes_are_not_outside_changes() {
        let (_dir, mut session) = project();
        session
            .set(
                FILE,
                &parse_path("components.Health.max"),
                json!(70.0),
                "Max",
                None,
            )
            .unwrap();
        assert!(!session.file_changed(FILE));
        assert!(session.check_external_changes().is_empty());
        assert!(session.history().can_undo());
    }
}
