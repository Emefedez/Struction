//! Renaming a path: every reference in the project sources follows, and an alias `old -> new`
//! lets saves written before the rename load.
//!
//! A rename applies to the path and everything below it (`minions` -> `creatures` also renames
//! `minions/ogre`). References are rewritten with struction_data's comment-preserving edits:
//! `descendsFrom`, `presets`, reaction and granted action names, grant targets, spawner zones,
//! spawn definitions and `masterIs`, overrides included. Definition directories and preset files
//! move with their path, and spawner, spawn and zone keys are renamed in place.
//!
//! Action names are renamed with their definition's prefix (`minions/ogre/die` ->
//! `minions/brute/die`); the Rust registrations of those actions must be renamed to match, and
//! [`RenameReport::renamed_actions`] lists them.
//!
//! Aliases live in `<root>/aliases.jsonc` as `{ "aliases": { "old": "new" } }`.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jsonc_parser::ParseOptions;
use jsonc_parser::cst::{CstNode, CstRootNode, ObjectPropName};
use serde_json::Value;
use struction_data::edit::{EditError, PathSegment, remove_value, set_value};
use struction_data::{DataError, ErrorKind, Node, parse_jsonc};
use thiserror::Error;

use crate::save::{SaveData, SavedRef};
use crate::scene::SCENES_DIR;

pub const ALIASES_FILE: &str = "aliases.jsonc";

/// `path` after renaming `old` to `new`, if it is `old` or below it.
fn renamed(path: &str, old: &str, new: &str) -> Option<String> {
    if path == old {
        return Some(new.to_owned());
    }
    let rest = path.strip_prefix(old)?.strip_prefix('/')?;
    Some(format!("{new}/{rest}"))
}

/// Recorded renames, applied to saves when loading.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct PathAliases(BTreeMap<String, String>);

impl PathAliases {
    /// Reads `<root>/aliases.jsonc`; no file means no aliases.
    pub fn load(root: &Path) -> Result<Self, DataError> {
        let Ok(text) = fs::read_to_string(root.join(ALIASES_FILE)) else {
            return Ok(Self::default());
        };
        let node = parse_jsonc(ALIASES_FILE, &text)?;
        let mut aliases = Self::default();
        let Some(entries) = node.get("aliases") else {
            return Ok(aliases);
        };
        for member in entries.as_object().unwrap_or_default() {
            let Some(new) = member.value.as_str() else {
                return Err(DataError::at(
                    ErrorKind::TypeMismatch {
                        expected: "string".into(),
                        found: member.value.kind_name().into(),
                    },
                    &member.value.span,
                ));
            };
            aliases.insert(member.key.clone(), new);
        }
        Ok(aliases)
    }

    pub fn insert(&mut self, old: impl Into<String>, new: impl Into<String>) {
        self.0.insert(old.into(), new.into());
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.0.iter().map(|(old, new)| (old.as_str(), new.as_str()))
    }

    /// The current name of `path`, following chained renames. The most specific alias wins.
    pub fn resolve<'a>(&self, path: &'a str) -> Cow<'a, str> {
        let mut current = Cow::Borrowed(path);
        // Each step applies one alias; more steps than aliases means a cycle.
        for _ in 0..=self.0.len() {
            let best = self
                .0
                .iter()
                .filter_map(|(old, new)| renamed(&current, old, new).map(|r| (old.len(), r)))
                .max_by_key(|(len, _)| *len);
            match best {
                Some((_, next)) if next != *current => current = Cow::Owned(next),
                _ => break,
            }
        }
        current
    }

    /// The save with every definition and entity path renamed.
    pub fn apply_to_save(&self, save: &SaveData) -> SaveData {
        let mut save = save.clone();
        if self.0.is_empty() {
            return save;
        }
        let resolve = |path: &mut String| *path = self.resolve(path).into_owned();
        save.spawners = std::mem::take(&mut save.spawners)
            .into_iter()
            .map(|(path, mut spawner)| {
                let resolve_name = |name: String| {
                    self.resolve(&format!("{path}/{name}"))
                        .rsplit('/')
                        .next()
                        .expect("paths have a final segment")
                        .to_owned()
                };
                spawner.created = spawner
                    .created
                    .into_iter()
                    .map(|(name, id)| (resolve_name(name), id))
                    .collect();
                spawner.removed = spawner.removed.into_iter().map(resolve_name).collect();
                (self.resolve(&path).into_owned(), spawner)
            })
            .collect();
        for entity in &mut save.entities {
            resolve(&mut entity.definition);
            if let Some(path) = &mut entity.path {
                resolve(path);
            }
            if let Some(SavedRef::Path(path)) = &mut entity.master {
                resolve(path);
            }
        }
        save
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum RenameError {
    #[error("\"{0}\" is not a valid path")]
    InvalidPath(String),
    #[error("cannot rename \"{old}\" into itself as \"{new}\"")]
    IntoItself { old: String, new: String },
    #[error("{0} already exists")]
    Exists(String),
    #[error("{0}")]
    Io(String),
    #[error(transparent)]
    Data(#[from] DataError),
    #[error("{file}: {error}")]
    Edit { file: String, error: EditError },
    #[error(
        "{file}: renaming \"{path}\" to \"{new}\" would move it to another spawner or zone; move it by hand"
    )]
    Unsupported {
        file: String,
        path: String,
        new: String,
    },
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RenameReport {
    /// Project-relative files whose text changed, sorted (paths before any move).
    pub edited: Vec<String>,
    /// Project-relative files or directories moved, `(from, to)`.
    pub moved: Vec<(String, String)>,
    /// Action names rewritten in the sources, `(old, new)`: rename their registrations too.
    pub renamed_actions: Vec<(String, String)>,
}

/// Edits collected for one file.
#[derive(Default)]
struct FileEdits {
    values: Vec<(Vec<PathSegment>, String)>,
    /// Parent path, old key, new key.
    keys: Vec<(Vec<PathSegment>, String, String)>,
}

struct Renamer<'a> {
    old: &'a str,
    new: &'a str,
    file: String,
    edits: FileEdits,
    actions: BTreeSet<(String, String)>,
}

fn at(path: &[PathSegment], segment: impl Into<PathSegment>) -> Vec<PathSegment> {
    let mut path = path.to_vec();
    path.push(segment.into());
    path
}

impl Renamer<'_> {
    fn string(&mut self, node: Option<&Node>, path: Vec<PathSegment>) -> Option<String> {
        let value = node?.as_str()?;
        let new = renamed(value, self.old, self.new)?;
        self.edits.values.push((path, new.clone()));
        Some(value.to_owned())
    }

    fn action(&mut self, node: Option<&Node>, path: Vec<PathSegment>) {
        if let Some(old) = self.string(node, path) {
            let new = renamed(&old, self.old, self.new).expect("just renamed");
            self.actions.insert((old, new));
        }
    }

    /// Fields of a definition body (entity, preset or spawn overrides) that hold paths.
    fn body(&mut self, node: &Node, path: &[PathSegment]) {
        self.string(node.get("descendsFrom"), at(path, "descendsFrom"));
        for (i, preset) in node
            .get("presets")
            .and_then(Node::as_array)
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            self.string(Some(preset), at(&at(path, "presets"), i));
        }
        let reactions = node.get("reactions").and_then(Node::as_array);
        for (i, reaction) in reactions.unwrap_or_default().iter().enumerate() {
            let here = at(&at(path, "reactions"), i);
            for key in ["after", "before", "call"] {
                self.action(reaction.get(key), at(&here, key));
            }
        }
        let grants = node.get("grantsToWards").and_then(Node::as_array);
        for (i, grant) in grants.unwrap_or_default().iter().enumerate() {
            let here = at(&at(path, "grantsToWards"), i);
            self.string(grant.get("to"), at(&here, "to"));
            let actions = grant.get("actions").and_then(Node::as_array);
            for (j, action) in actions.unwrap_or_default().iter().enumerate() {
                self.action(Some(action), at(&at(&here, "actions"), j));
            }
        }
    }

    /// Renames the key of an entity whose path is `parent/key`, given its parent's new path.
    fn key(
        &mut self,
        container: Vec<PathSegment>,
        key: &str,
        path: &str,
        new_parent: &str,
    ) -> Result<String, RenameError> {
        let new_path = renamed(path, self.old, self.new).unwrap_or_else(|| path.to_owned());
        let Some((parent, new_key)) = new_path.rsplit_once('/') else {
            return Err(self.unsupported(path, &new_path));
        };
        if parent != new_parent {
            return Err(self.unsupported(path, &new_path));
        }
        if new_key != key {
            self.edits
                .keys
                .push((container, key.into(), new_key.into()));
        }
        Ok(new_path)
    }

    fn unsupported(&self, path: &str, new: &str) -> RenameError {
        RenameError::Unsupported {
            file: self.file.clone(),
            path: path.into(),
            new: new.into(),
        }
    }

    fn scene(&mut self, root: &Node) -> Result<(), RenameError> {
        for zone in root
            .get("zones")
            .and_then(Node::as_object)
            .unwrap_or_default()
        {
            if let Some(new) = renamed(&zone.key, self.old, self.new) {
                self.edits
                    .keys
                    .push((vec!["zones".into()], zone.key.clone(), new));
            }
        }
        let spawners = root.get("spawnerList").and_then(Node::as_object);
        for spawner in spawners.unwrap_or_default() {
            let here = vec![
                PathSegment::from("spawnerList"),
                spawner.key.as_str().into(),
            ];
            let Some(zone) = spawner.value.get("zone").and_then(Node::as_str) else {
                continue;
            };
            self.string(spawner.value.get("zone"), at(&here, "zone"));
            let new_zone = renamed(zone, self.old, self.new).unwrap_or_else(|| zone.to_owned());
            let path = format!("{zone}/{}", spawner.key);
            let new_spawner =
                self.key(vec!["spawnerList".into()], &spawner.key, &path, &new_zone)?;
            let spawns = spawner.value.get("spawns").and_then(Node::as_object);
            for spawn in spawns.unwrap_or_default() {
                let spawn_here = at(&at(&here, "spawns"), spawn.key.as_str());
                self.key(
                    at(&here, "spawns"),
                    &spawn.key,
                    &format!("{path}/{}", spawn.key),
                    &new_spawner,
                )?;
                self.string(spawn.value.get("definition"), at(&spawn_here, "definition"));
                self.string(spawn.value.get("masterIs"), at(&spawn_here, "masterIs"));
                if let Some(overrides) = spawn.value.get("overrides") {
                    self.body(overrides, &at(&spawn_here, "overrides"));
                }
            }
        }
        Ok(())
    }
}

fn io(context: &str, error: std::io::Error) -> RenameError {
    RenameError::Io(format!("{context}: {error}"))
}

fn valid(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && segment != "."
                && segment != ".."
                && !segment.contains(['\\', '\0'])
        })
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !name.starts_with('.') && name != "build" && name != "target" {
                walk(root, &path, out);
            }
        } else if name.ends_with(".jsonc")
            && let Ok(rel) = path.strip_prefix(root)
        {
            let rel: Vec<_> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            out.push(rel.join("/"));
        }
    }
}

/// Renames the key `old` of the object at `parent` through the CST, keeping comments.
fn rename_key(
    text: &str,
    parent: &[PathSegment],
    old: &str,
    new: &str,
) -> Result<String, EditError> {
    let root = CstRootNode::parse(text, &ParseOptions::default())
        .map_err(|e| EditError::Syntax(e.to_string()))?;
    let not_found = || EditError::NotFound(format!("{old} in {parent:?}"));
    let mut node: CstNode = root.object_value().ok_or(EditError::RootNotObject)?.into();
    for segment in parent {
        node = match segment {
            PathSegment::Key(key) => node.as_object().and_then(|o| o.get(key)?.value()),
            PathSegment::Index(i) => node.as_array().and_then(|a| a.elements().get(*i).cloned()),
        }
        .ok_or_else(not_found)?;
    }
    let prop = node
        .as_object()
        .and_then(|o| o.get(old))
        .ok_or_else(not_found)?;
    let quoted = serde_json::to_string(new).expect("strings serialize");
    match prop.name().ok_or_else(not_found)? {
        ObjectPropName::String(name) => name.set_raw_value(quoted),
        ObjectPropName::Word(name) => name.set_raw_value(quoted),
    }
    Ok(root.to_string())
}

fn apply(file: &str, mut text: String, mut edits: FileEdits) -> Result<String, RenameError> {
    let edit_error = |error| RenameError::Edit {
        file: file.into(),
        error,
    };
    for (path, value) in edits.values {
        text = set_value(&text, &path, Value::String(value))
            .map_err(edit_error)?
            .text;
    }
    // Deepest first, so outer keys are still the old ones when inner keys are looked up.
    edits
        .keys
        .sort_by_key(|(parent, ..)| std::cmp::Reverse(parent.len()));
    for (parent, old, new) in edits.keys {
        let node = parse_jsonc(file, &text)?;
        let mut container = &node;
        for segment in &parent {
            container = match segment {
                PathSegment::Key(key) => container.get(key),
                PathSegment::Index(i) => container.as_array().and_then(|items| items.get(*i)),
            }
            .expect("edit paths came from the source");
        }
        if old != new && container.get(&new).is_some() {
            return Err(RenameError::Exists(format!("{file}: {new}")));
        }
        text = rename_key(&text, &parent, &old, &new).map_err(edit_error)?;
    }
    Ok(text)
}

fn has_entity_file(dir: &Path) -> bool {
    let mut files = Vec::new();
    walk(dir, dir, &mut files);
    files
        .iter()
        .any(|f| f == "entity.jsonc" || f.ends_with("/entity.jsonc"))
}

/// Renames `old` to `new` across the project at `root`, writing the edited sources, moving
/// definition directories and preset files, and recording the alias. Every edit is computed
/// before anything is written, so validation errors leave the project untouched. Filesystem
/// failures during writing or moving are reported but are not rolled back.
pub fn rename_path(root: &Path, old: &str, new: &str) -> Result<RenameReport, RenameError> {
    for path in [old, new] {
        if !valid(path) {
            return Err(RenameError::InvalidPath(path.into()));
        }
    }
    if renamed(new, old, "").is_some() {
        return Err(RenameError::IntoItself {
            old: old.into(),
            new: new.into(),
        });
    }

    // Files and directories that move with the path.
    let mut moves: Vec<(String, String)> = Vec::new();
    if root.join(old).is_dir() && has_entity_file(&root.join(old)) {
        moves.push((old.into(), new.into()));
    }
    if root.join(format!("presets/{old}.jsonc")).is_file() {
        moves.push((
            format!("presets/{old}.jsonc"),
            format!("presets/{new}.jsonc"),
        ));
    }
    if root.join(format!("presets/{old}")).is_dir() {
        moves.push((format!("presets/{old}"), format!("presets/{new}")));
    }
    for (_, to) in &moves {
        if root.join(to).exists() {
            return Err(RenameError::Exists(to.clone()));
        }
    }

    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    let mut report = RenameReport::default();
    let mut writes: Vec<(PathBuf, String)> = Vec::new();
    let mut actions = BTreeSet::new();
    for rel in files.iter().filter(|f| *f != ALIASES_FILE) {
        let is_scene = rel.starts_with(&format!("{SCENES_DIR}/"));
        let is_definition =
            rel.starts_with("presets/") || rel == "entity.jsonc" || rel.ends_with("/entity.jsonc");
        if !is_scene && !is_definition {
            continue;
        }
        let full = root.join(rel);
        let text = fs::read_to_string(&full).map_err(|e| io(rel, e))?;
        let node = parse_jsonc(rel, &text)?;
        let mut renamer = Renamer {
            old,
            new,
            file: rel.clone(),
            edits: FileEdits::default(),
            actions: BTreeSet::new(),
        };
        if is_scene {
            renamer.scene(&node)?;
        } else {
            renamer.body(&node, &[]);
        }
        actions.extend(std::mem::take(&mut renamer.actions));
        if renamer.edits.values.is_empty() && renamer.edits.keys.is_empty() {
            continue;
        }
        let edited = apply(rel, text, renamer.edits)?;
        report.edited.push(rel.clone());
        writes.push((full, edited));
    }

    let aliases_path = root.join(ALIASES_FILE);
    let mut aliases_text = match fs::read_to_string(&aliases_path) {
        Ok(text) => text,
        Err(_) => {
            "// Renamed paths, old -> new. Saves written before a rename load through these.\n{}\n"
                .to_owned()
        }
    };
    let edit_error = |error| RenameError::Edit {
        file: ALIASES_FILE.into(),
        error,
    };
    // Earlier renames of what now moves point at the new name directly; one that would point
    // back at itself is dropped.
    for (alias_old, alias_new) in PathAliases::load(root)?.iter() {
        let Some(updated) = renamed(alias_new, old, new) else {
            continue;
        };
        let path = [PathSegment::from("aliases"), alias_old.into()];
        aliases_text = if updated == alias_old {
            remove_value(&aliases_text, &path)
        } else {
            set_value(&aliases_text, &path, Value::String(updated))
        }
        .map_err(edit_error)?
        .text;
    }
    aliases_text = set_value(
        &aliases_text,
        &[PathSegment::from("aliases"), old.into()],
        Value::String(new.into()),
    )
    .map_err(edit_error)?
    .text;
    report.edited.push(ALIASES_FILE.into());
    report.edited.sort();
    writes.push((aliases_path, aliases_text));

    for (path, text) in writes {
        fs::write(&path, text).map_err(|e| io(&path.display().to_string(), e))?;
    }
    for (from, to) in &moves {
        let target = root.join(to);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| io(to, e))?;
        }
        fs::rename(root.join(from), &target).map_err(|e| io(from, e))?;
    }
    report.moved = moves;
    report.renamed_actions = actions.into_iter().collect();
    Ok(report)
}
