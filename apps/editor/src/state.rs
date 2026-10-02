//! Editor state over one `AuthoringProject`. Every change is a `Command` mapped onto the same
//! project operations the JSONL protocol exposes to AI tools; the GUI holds no second copy of
//! the data, only a snapshot refreshed after each operation.
use std::collections::BTreeSet;
use std::path::PathBuf;

use bevy::prelude::*;
use serde_json::Value;
use struction_data::parse_jsonc;
use struction_editor::{
    AuthoringProject, Diagnostic, DroppedEntry, EditRequest, EntityEntry, ExtensorEntry, Field,
    HierarchyNode, SessionError, SuggestedExtensor,
};

use crate::game;

/// Authored paths (or StableIds for runtime entities), never ECS entities: previews are rebuilt.
#[derive(Clone, Debug, PartialEq)]
pub enum Selected {
    Entity(String),
    Definition(String),
    /// A mesh source, by project-relative path.
    Asset(String),
}

pub enum Command {
    Open(PathBuf),
    OpenIde(struction_editor::SourceTarget),
    Refresh,
    Select(Option<Selected>),
    Edit(EditRequest),
    EditField {
        file: String,
        path: Vec<Field>,
        value: Value,
        group: Option<String>,
    },
    RemoveEntry {
        file: String,
        path: Vec<Field>,
        index: usize,
    },
    AddField {
        file: String,
        path: Vec<Field>,
        key: String,
        value: Value,
    },
    AddEntry {
        file: String,
        path: Vec<Field>,
        value: Value,
    },
    Move {
        path: String,
        position: Vec3,
        group: Option<String>,
    },
    EndGroup,
    Undo,
    Redo,
    CreateDefinition {
        path: String,
        parent: String,
    },
    SetMaster {
        path: String,
        master: Option<String>,
    },
    AddExtensor {
        definition: String,
        extensor: String,
    },
    RemoveExtensor {
        definition: String,
        extensor: String,
    },
    CreateSpawn {
        spawner: String,
        name: String,
        definition: String,
        master: Option<String>,
    },
    StartPlay,
    StopPlay,
    TogglePause,
    Step,
}

/// A failed operation, shown in Problems until the next one succeeds.
pub struct Rejection {
    pub action: &'static str,
    pub message: String,
    pub diagnostics: Vec<Diagnostic>,
}

pub struct Play {
    pub running: bool,
    pub ticks: u64,
    accumulated: f32,
}

pub enum Inspection {
    Entity {
        entry: Box<EntityEntry>,
        /// Short type name and reflected value, authored components first.
        components: Vec<(String, Value)>,
        authored: BTreeSet<String>,
        unavailable: Vec<String>,
        /// Scene file and field path of a named spawn, where overrides and moves are written.
        spawn: Option<(String, Vec<Field>)>,
        overrides: Value,
    },
    Definition {
        path: String,
        lineage: Vec<String>,
        components: Vec<(String, Value)>,
        /// The definition's own file, to tell fields set here from inherited ones.
        local: Value,
        extensors: Extensors,
        /// The read-only library defining it, and whether the project overrides it.
        library: Option<String>,
        overridden: bool,
    },
    Missing(String),
    InvalidDefinition {
        path: String,
        message: String,
        source: String,
    },
    /// Drawn from the toolbox, not the project.
    Asset,
}

/// What a definition's inspection says about its extensors.
pub struct Extensors {
    pub used: Vec<ExtensorEntry>,
    pub dropped: Vec<DroppedEntry>,
    pub suggested: Vec<SuggestedExtensor>,
    pub available: Vec<String>,
}

#[derive(Default)]
pub struct Editor {
    pub project: Option<AuthoringProject>,
    pub root: Option<PathBuf>,
    pub entities: Vec<EntityEntry>,
    pub definitions: Vec<String>,
    pub masters: Vec<HierarchyNode>,
    pub lineages: Vec<HierarchyNode>,
    pub diagnostics: Vec<Diagnostic>,
    pub rejection: Option<Rejection>,
    pub status: Option<String>,
    pub selected: Option<Selected>,
    pub play: Option<Play>,
    /// Bumped whenever the snapshot changes, so dependents rebuild.
    pub generation: u64,
    pub schema: std::sync::Arc<Value>,
    inspection: Option<(u64, Selected, Inspection)>,
}

impl Editor {
    pub fn apply(&mut self, command: Command) {
        match command {
            Command::Open(root) => self.open(root),
            Command::Select(selected) => self.selected = selected,
            command => self.apply_to_project(command),
        }
    }

    fn apply_to_project(&mut self, command: Command) {
        let Some(project) = self.project.as_mut() else {
            return;
        };
        let (action, result) = match command {
            Command::Open(_) | Command::Select(_) | Command::OpenIde(_) => return,
            Command::Refresh => ("Refresh", project.refresh().map(|()| None)),
            Command::AddField {
                file,
                path,
                key,
                value,
            } => (
                "Add field",
                project
                    .add_field(&file, &path, &key, Some(value))
                    .map(|a| Some(a.label)),
            ),
            Command::AddEntry { file, path, value } => (
                "Add entry",
                project
                    .add_entry(&file, &path, Some(value))
                    .map(|a| Some(a.label)),
            ),
            Command::EditField {
                file,
                path,
                value,
                group,
            } => (
                "Edit field",
                project
                    .edit_field(&file, &path, value, group)
                    .map(|a| Some(a.label)),
            ),
            Command::RemoveEntry { file, path, index } => (
                "Remove entry",
                project
                    .remove_entry(&file, &path, index)
                    .map(|a| Some(a.label)),
            ),
            Command::Edit(request) => ("Edit", project.edit(request).map(|a| Some(a.label))),
            Command::Move {
                path,
                position,
                group,
            } => (
                "Move",
                project
                    .move_spawn(&path, position, group)
                    .map(|a| Some(a.label)),
            ),
            Command::EndGroup => {
                project.end_group();
                return;
            }
            Command::Undo => (
                "Undo",
                project
                    .undo()
                    .map(|a| a.map(|a| format!("Undid {}", a.label))),
            ),
            Command::Redo => (
                "Redo",
                project
                    .redo()
                    .map(|a| a.map(|a| format!("Redid {}", a.label))),
            ),
            Command::CreateDefinition { path, parent } => (
                "Create definition",
                project
                    .create_definition(&path, &parent)
                    .map(|()| Some(format!("Created {path}"))),
            ),
            Command::SetMaster { path, master } => (
                "Set master",
                project
                    .set_master(&path, master.as_deref())
                    .map(|a| Some(a.label)),
            ),
            Command::AddExtensor {
                definition,
                extensor,
            } => (
                "Add extensor",
                project
                    .add_extensor(&definition, &extensor)
                    .map(|a| Some(a.label)),
            ),
            Command::RemoveExtensor {
                definition,
                extensor,
            } => (
                "Remove extensor",
                project
                    .remove_extensor(&definition, &extensor)
                    .map(|a| Some(a.label)),
            ),
            Command::CreateSpawn {
                spawner,
                name,
                definition,
                master,
            } => (
                "Create actor",
                project
                    .create_spawn(&spawner, &name, &definition, master.as_deref(), Vec3::ZERO)
                    .map(|a| Some(a.label)),
            ),
            Command::StartPlay => {
                let started = project.start_play();
                if started.is_ok() {
                    self.play = Some(Play {
                        running: true,
                        ticks: 0,
                        accumulated: 0.0,
                    });
                }
                ("Play", started.map(|()| None))
            }
            Command::StopPlay => {
                project.stop_play();
                self.play = None;
                ("Stop", Ok(None))
            }
            Command::TogglePause => {
                project.release_play_input();
                if let Some(play) = &mut self.play {
                    play.running = !play.running;
                }
                return;
            }
            Command::Step => {
                project.release_play_input();
                if let Some(play) = &mut self.play {
                    play.running = false;
                }
                return self.step(1);
            }
        };
        match result {
            Ok(status) => {
                self.rejection = None;
                if status.is_some() {
                    self.status = status;
                }
            }
            Err(error) => self.rejection = Some(rejection(action, error)),
        }
        self.reload(true);
    }

    fn open(&mut self, root: PathBuf) {
        let root = project_root(root);
        self.project = None;
        self.play = None;
        self.selected = None;
        match AuthoringProject::open(&root, game::factory) {
            Ok(project) => {
                self.root = Some(project.session().root().to_owned());
                self.project = Some(project);
                self.rejection = None;
                self.status = None;
            }
            Err(error) => {
                self.root = None;
                self.rejection = Some(rejection("Open", error));
            }
        }
        self.reload(true);
    }

    /// Re-reads the snapshot; validation only when sources may have changed.
    fn reload(&mut self, validate: bool) {
        self.generation += 1;
        let Some(project) = &self.project else {
            self.entities.clear();
            self.definitions.clear();
            self.masters.clear();
            self.lineages.clear();
            self.diagnostics.clear();
            return;
        };
        self.entities = project.entities(self.play.is_some()).unwrap_or_default();
        self.masters = project
            .master_hierarchy(self.play.is_some())
            .unwrap_or_default();
        if validate {
            self.schema = std::sync::Arc::new(project.schema());
            self.definitions = project.definitions();
            self.definitions.sort();
            self.lineages = project.definition_hierarchy();
            // Errors reached through several paths can repeat once reduced to diagnostics.
            self.diagnostics = project.validate();
            self.diagnostics.sort_by(|a, b| {
                (&a.file, a.line, a.column, &a.message)
                    .cmp(&(&b.file, b.line, b.column, &b.message))
            });
            self.diagnostics.dedup_by(|a, b| {
                (&a.file, a.line, a.column, &a.message) == (&b.file, b.line, b.column, &b.message)
            });
        }
    }

    /// Advances running play by whole fixed ticks of the game's own timestep.
    pub fn advance(&mut self, delta: f32) {
        let (Some(play), Some(project)) = (&mut self.play, &self.project) else {
            return;
        };
        if !play.running {
            return;
        }
        let step = project
            .play_world()
            .map_or(1.0 / 64.0, |world| {
                world.resource::<Time<Fixed>>().timestep().as_secs_f32()
            })
            .max(1e-4);
        play.accumulated += delta;
        // Drop time after a hitch rather than spiral: at most a few ticks per frame.
        let ticks = ((play.accumulated / step) as u64).min(4);
        play.accumulated = (play.accumulated - ticks as f32 * step).min(step);
        if ticks > 0 {
            self.step(ticks);
        }
    }

    fn step(&mut self, ticks: u64) {
        let (Some(play), Some(project)) = (&mut self.play, &mut self.project) else {
            return;
        };
        if let Err(error) = project.step_play(ticks as usize) {
            self.rejection = Some(rejection("Step", error));
            return;
        }
        play.ticks += ticks;
        self.reload(false);
    }

    pub fn playing(&self) -> bool {
        self.play.is_some()
    }

    pub fn entity(&self, target: &str) -> Option<&EntityEntry> {
        self.entities.iter().find(|entity| entity.key() == target)
    }

    pub fn inspection(&mut self) -> Option<&Inspection> {
        let selected = self.selected.clone()?;
        let fresh = self
            .inspection
            .as_ref()
            .is_some_and(|(generation, cached, _)| {
                *generation == self.generation && *cached == selected
            });
        if !fresh {
            let inspection = self.inspect(&selected)?;
            self.inspection = Some((self.generation, selected, inspection));
        }
        self.inspection
            .as_ref()
            .map(|(_, _, inspection)| inspection)
    }

    fn inspect(&self, selected: &Selected) -> Option<Inspection> {
        let project = self.project.as_ref()?;
        let source = |file: &str| {
            project
                .session()
                .read(file)
                .ok()
                .and_then(|text| parse_jsonc(file, &text).ok())
                .map_or(Value::Null, |node| node.to_value())
        };
        Some(match selected {
            Selected::Entity(target) => {
                let Ok(inspected) = project.inspect_entity(target, self.playing()) else {
                    return Some(Inspection::Missing(target.clone()));
                };
                let entry = inspected.entity;
                let authored: BTreeSet<String> = entry
                    .definition
                    .as_deref()
                    .and_then(|definition| project.inspect_definition(definition).ok())
                    .map(|definition| {
                        definition
                            .components
                            .keys()
                            .map(|key| short_name(key))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut components: Vec<_> = inspected
                    .components
                    .into_iter()
                    .map(|(key, value)| (short_name(&key), value))
                    .collect();
                components.sort_by_key(|(name, _)| (!authored.contains(name), name.clone()));
                let spawn = entry
                    .source
                    .as_ref()
                    .filter(|_| entry.is_named_spawn())
                    .map(|source| {
                        let path: Vec<_> = source.path.iter().cloned().map(Field::Key).collect();
                        (source.file.clone(), path)
                    });
                let overrides = spawn.as_ref().map_or(Value::Null, |(file, path)| {
                    let mut at = path.clone();
                    at.push(Field::Key("overrides".into()));
                    lookup(&source(file), &at).cloned().unwrap_or(Value::Null)
                });
                let unavailable = inspected
                    .unavailable
                    .iter()
                    .map(|entry| short_name(&entry.component))
                    .collect();
                Inspection::Entity {
                    entry: Box::new(entry),
                    components,
                    authored,
                    unavailable,
                    spawn,
                    overrides,
                }
            }
            Selected::Asset(_) => Inspection::Asset,
            Selected::Definition(path) => {
                let inspected = match project.inspect_definition(path) {
                    Ok(inspected) => inspected,
                    Err(error) => {
                        return Some(Inspection::InvalidDefinition {
                            path: path.clone(),
                            message: error.to_string(),
                            source: project
                                .session()
                                .read(&definition_file(path))
                                .unwrap_or_default(),
                        });
                    }
                };
                let components = inspected
                    .components
                    .into_iter()
                    .map(|(key, value)| (short_name(&key), value))
                    .collect();
                Inspection::Definition {
                    path: path.clone(),
                    lineage: inspected.lineage,
                    components,
                    local: source(&definition_file(path)),
                    extensors: Extensors {
                        used: inspected.extensors,
                        dropped: inspected.dropped_extensors,
                        suggested: inspected.suggested_extensors,
                        available: inspected.available_extensors,
                    },
                    library: inspected.library,
                    overridden: inspected.overridden,
                }
            }
        })
    }
}

/// Accept the data directory, the playground application, or this repository checkout.
fn project_root(root: PathBuf) -> PathBuf {
    if root.join("scenes").is_dir() {
        return root;
    }
    for child in ["project", "apps/playground/project"] {
        let candidate = root.join(child);
        if candidate.join("scenes").is_dir() {
            return candidate;
        }
    }
    root
}

fn rejection(action: &'static str, error: SessionError) -> Rejection {
    match error {
        SessionError::Validation(errors) => Rejection {
            action,
            message: if action == "Refresh" {
                "sources contain errors; showing the last valid preview"
            } else {
                "the change would make the project invalid"
            }
            .into(),
            diagnostics: errors.iter().map(Diagnostic::from).collect(),
        },
        error => Rejection {
            action,
            message: error.to_string(),
            diagnostics: Vec::new(),
        },
    }
}

pub fn definition_file(path: &str) -> String {
    format!("{path}/entity.jsonc")
}

/// Authored data names components by their short type path.
fn short_name(type_path: &str) -> String {
    type_path
        .rsplit("::")
        .next()
        .unwrap_or(type_path)
        .to_owned()
}

pub fn lookup<'a>(value: &'a Value, path: &[Field]) -> Option<&'a Value> {
    path.iter().try_fold(value, |value, field| match field {
        Field::Key(key) => value.get(key),
        Field::Index(index) => value.get(index),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn copy(from: &Path, to: &Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    const OGRE: &str = "Court/guards/ogre";

    fn open() -> (tempfile::TempDir, Editor) {
        let dir = tempfile::tempdir().unwrap();
        let example = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/authoring");
        copy(&example, dir.path());
        let mut editor = Editor::default();
        editor.apply(Command::Open(dir.path().to_owned()));
        assert!(editor.rejection.is_none());
        (dir, editor)
    }

    fn keys(path: &[&str]) -> Vec<Field> {
        path.iter().map(|key| Field::Key((*key).into())).collect()
    }

    fn health(editor: &mut Editor, field: &str) -> f64 {
        editor.apply(Command::Select(Some(Selected::Entity(OGRE.into()))));
        let Some(Inspection::Entity { components, .. }) = editor.inspection() else {
            panic!("the ogre is inspectable");
        };
        let (_, health) = components
            .iter()
            .find(|(name, _)| name == "Health")
            .unwrap();
        health[field].as_f64().unwrap()
    }

    fn ogre_position(editor: &Editor) -> Vec3 {
        editor.entity(OGRE).unwrap().position.unwrap()
    }

    #[test]
    fn playground_uses_its_registered_game_types_and_plays_headlessly() {
        let mut editor = Editor::default();
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        editor.apply(Command::Open(repository));
        assert!(editor.rejection.is_none());
        assert!(editor.diagnostics.is_empty(), "{:?}", editor.diagnostics);
        let player = "Playground/start/player";
        assert!(editor.entity(player).is_some());
        editor.apply(Command::Select(Some(Selected::Entity(player.into()))));
        let Some(Inspection::Entity { authored, .. }) = editor.inspection() else {
            panic!("player must be inspectable")
        };
        assert!(authored.contains("CharacterController"));
        editor.apply(Command::StartPlay);
        assert!(editor.rejection.is_none());
        editor.apply(Command::Step);
        assert_eq!(editor.play.as_ref().unwrap().ticks, 1);
        editor.apply(Command::StopPlay);
    }

    fn extensors(editor: &mut Editor, definition: &str) -> (Vec<String>, Vec<String>) {
        editor.apply(Command::Select(Some(Selected::Definition(
            definition.into(),
        ))));
        let Some(Inspection::Definition { extensors, .. }) = editor.inspection() else {
            panic!("{definition} is inspectable");
        };
        (
            extensors.used.iter().map(|e| e.name.clone()).collect(),
            extensors.suggested.iter().map(|e| e.name.clone()).collect(),
        )
    }

    #[test]
    fn extensors_are_added_and_removed_from_the_inspector_commands() {
        let dir = tempfile::tempdir().unwrap();
        let playground = Path::new(env!("CARGO_MANIFEST_DIR")).join("../playground/project");
        copy(&playground, dir.path());
        let mut editor = Editor::default();
        editor.apply(Command::Open(dir.path().to_owned()));
        assert!(editor.rejection.is_none());

        // The project names `dodge` on the sentry, so only `combat` is still suggested.
        let (used, suggested) = extensors(&mut editor, "characters/sentry");
        assert_eq!(used, ["dodge", "character", "physics"]);
        assert_eq!(suggested, ["combat"]);
        editor.apply(Command::AddExtensor {
            definition: "characters/sentry".into(),
            extensor: "combat".into(),
        });
        assert!(editor.rejection.is_none());
        let (used, suggested) = extensors(&mut editor, "characters/sentry");
        assert!(used.contains(&"combat".to_owned()));
        assert_eq!(suggested, Vec::<String>::new());

        editor.apply(Command::RemoveExtensor {
            definition: "characters/player".into(),
            extensor: "combat".into(),
        });
        assert!(editor.rejection.is_none());
        let (used, _) = extensors(&mut editor, "characters/player");
        assert!(!used.contains(&"combat".to_owned()));
        editor.apply(Command::Undo);
        let (used, _) = extensors(&mut editor, "characters/player");
        assert!(used.contains(&"combat".to_owned()));
    }

    #[test]
    fn opens_a_project_into_a_valid_snapshot() {
        let (_dir, mut editor) = open();
        assert!(editor.diagnostics.is_empty());
        for definition in ["Creature", "guards/ogre", "Actor"] {
            assert!(
                editor.definitions.iter().any(|d| d == definition),
                "{definition}"
            );
        }
        assert!(editor.entity(OGRE).is_some());

        editor.apply(Command::Select(Some(Selected::Entity(OGRE.into()))));
        let Some(Inspection::Entity {
            authored, spawn, ..
        }) = editor.inspection()
        else {
            panic!("the ogre is inspectable");
        };
        assert!(authored.contains("Health"));
        let (file, path) = spawn.clone().unwrap();
        assert_eq!(file, "scenes/courtyard.jsonc");
        assert_eq!(path.len(), 4);
    }

    #[test]
    fn instance_overrides_write_the_scene_and_undo() {
        let (dir, mut editor) = open();
        let scene = dir.path().join("scenes/courtyard.jsonc");
        let original = fs::read_to_string(&scene).unwrap();
        let path = keys(&[
            "spawnerList",
            "guards",
            "spawns",
            "ogre",
            "overrides",
            "components",
            "Health",
            "current",
        ]);
        editor.apply(Command::Edit(EditRequest::Set {
            file: "scenes/courtyard.jsonc".into(),
            path,
            value: 30.into(),
            label: "Set Health.current on ogre".into(),
            group: None,
            revision: None,
        }));
        assert!(editor.rejection.is_none());
        assert_eq!(health(&mut editor, "current"), 30.0);
        let Some(Inspection::Entity { overrides, .. }) = editor.inspection() else {
            unreachable!()
        };
        assert_eq!(overrides["components"]["Health"]["current"], 30);

        editor.apply(Command::Undo);
        assert_eq!(health(&mut editor, "current"), 50.0);
        assert_eq!(fs::read_to_string(&scene).unwrap(), original);
        editor.apply(Command::Redo);
        assert_eq!(health(&mut editor, "current"), 30.0);
    }

    #[test]
    fn invalid_edits_are_rejected_with_diagnostics_and_leave_sources() {
        let (dir, mut editor) = open();
        let file = dir.path().join("guards/ogre/entity.jsonc");
        let original = fs::read_to_string(&file).unwrap();
        editor.apply(Command::Edit(EditRequest::Set {
            file: "guards/ogre/entity.jsonc".into(),
            path: keys(&["components", "Health", "max"]),
            value: "lots".into(),
            label: "Set Health.max".into(),
            group: None,
            revision: None,
        }));
        let rejection = editor.rejection.as_ref().expect("the edit is rejected");
        assert!(!rejection.diagnostics.is_empty());
        assert_eq!(fs::read_to_string(&file).unwrap(), original);
        assert!(
            !editor
                .project
                .as_ref()
                .unwrap()
                .session()
                .history()
                .can_undo()
        );
    }

    #[test]
    fn definition_fields_set_and_reset() {
        let (_dir, mut editor) = open();
        let file = definition_file("guards/ogre");
        let local = |editor: &mut Editor| {
            editor.apply(Command::Select(Some(Selected::Definition(
                "guards/ogre".into(),
            ))));
            match editor.inspection() {
                Some(Inspection::Definition { local, .. }) => local.clone(),
                _ => panic!("the definition is inspectable"),
            }
        };
        editor.apply(Command::Edit(EditRequest::Set {
            file: file.clone(),
            path: keys(&["components", "Health", "current"]),
            value: 45.into(),
            label: "Set Health.current".into(),
            group: None,
            revision: None,
        }));
        assert_eq!(local(&mut editor)["components"]["Health"]["current"], 45);
        assert_eq!(health(&mut editor, "current"), 45.0);

        // Resetting removes the field here, so the ogre inherits Actor's value again.
        editor.apply(Command::Edit(EditRequest::Remove {
            file,
            path: keys(&["components", "Health", "current"]),
            label: "Reset Health.current".into(),
            revision: None,
        }));
        assert!(
            local(&mut editor)["components"]["Health"]
                .get("current")
                .is_none()
        );
        assert_eq!(health(&mut editor, "current"), 50.0);
    }

    #[test]
    fn a_drag_is_one_undoable_move() {
        let (_dir, mut editor) = open();
        let start = ogre_position(&editor);
        for step in 1..=3 {
            editor.apply(Command::Move {
                path: OGRE.into(),
                position: start + Vec3::X * step as f32,
                group: Some("drag".into()),
            });
        }
        editor.apply(Command::EndGroup);
        assert!(ogre_position(&editor).abs_diff_eq(start + Vec3::X * 3.0, 1e-4));
        editor.apply(Command::Undo);
        assert!(ogre_position(&editor).abs_diff_eq(start, 1e-4));
        assert!(
            !editor
                .project
                .as_ref()
                .unwrap()
                .session()
                .history()
                .can_undo()
        );
    }

    #[test]
    fn play_simulates_a_separate_world_and_stops_back_to_sources() {
        let (_dir, mut editor) = open();
        editor.apply(Command::StartPlay);
        assert!(editor.playing());
        editor.advance(0.5);
        let ticks = editor.play.as_ref().unwrap().ticks;
        assert!(ticks > 0);
        // Health regenerates in play only.
        assert!(health(&mut editor, "current") > 50.0);

        editor.apply(Command::Move {
            path: OGRE.into(),
            position: Vec3::ZERO,
            group: None,
        });
        assert!(
            editor.rejection.is_some(),
            "edits are blocked while playing"
        );

        editor.apply(Command::TogglePause);
        editor.advance(0.5);
        assert_eq!(editor.play.as_ref().unwrap().ticks, ticks);
        editor.apply(Command::Step);
        assert_eq!(editor.play.as_ref().unwrap().ticks, ticks + 1);

        editor.apply(Command::StopPlay);
        assert!(!editor.playing());
        assert_eq!(health(&mut editor, "current"), 50.0);
    }
}
