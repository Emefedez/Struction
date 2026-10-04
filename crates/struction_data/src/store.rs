//! A project's definitions: loading, inheritance, presets, overrides and hot reload.
//!
//! Layout: `<root>/**/entity.jsonc` defines the entity `<dir>` (`minions/ogre/entity.jsonc` is
//! `minions/ogre`), `<root>/presets/**.jsonc` defines presets by their path below `presets/`.
//! Other files (scenes, assets) are ignored here. Primordial types (capitalized names) may be
//! files too, or be declared with [`DefinitionStore::declare_primordial`].
//!
//! Libraries (the engine's base definitions) are read-only roots with the same layout, loaded
//! before the project. A project file at a library definition's path overrides it: it is merged
//! over the library's version, keeps its parent, and every descendant sees the change.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use bevy::prelude::Resource;
use bevy::reflect::TypeRegistry;
use struction_core::{ActionRegistry, ExtensorRegistry};

use crate::build::{Builder, ComponentValue};
use crate::definition::{
    DEFAULT_EXTRA_SECTIONS, Layer, LayerKind, Resolved, is_primordial, parse_layer,
    strip_removed_components,
};
use crate::error::{DataError, ErrorKind, Location};
use crate::extensors::{self, Named};
use crate::source::{Node, NodeValue, Span, parse_jsonc};

const ENTITY_FILE: &str = "entity.jsonc";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Dep {
    Entity(String),
    Preset(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FileKind {
    Entity(String),
    Preset(String),
}

impl FileKind {
    fn dep(&self) -> Dep {
        match self {
            FileKind::Entity(id) => Dep::Entity(id.clone()),
            FileKind::Preset(name) => Dep::Preset(name.clone()),
        }
    }
}

fn classify(rel: &str) -> Option<FileKind> {
    if let Some(rest) = rel.strip_prefix("presets/") {
        let name = rest.strip_suffix(".jsonc")?;
        return (!name.is_empty()).then(|| FileKind::Preset(name.to_owned()));
    }
    let id = rel.strip_suffix(ENTITY_FILE)?.strip_suffix('/')?;
    (!id.is_empty()).then(|| FileKind::Entity(id.to_owned()))
}

/// What a load or reload changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ReloadReport {
    /// Definitions whose resolved data or lineage changed, or that are new. Includes descendants
    /// (and users of a changed preset), so a runtime refreshes exactly these instances.
    pub changed: Vec<String>,
    /// Definitions whose file is gone.
    pub removed: Vec<String>,
    /// Problems in the reloaded file and in the definitions that depend on it. A definition that
    /// fails to resolve keeps its last good version in the store.
    pub errors: Vec<DataError>,
}

struct Merged {
    body: Node,
    lineage: Vec<String>,
    extensors: Vec<Named>,
    /// `"-name"` entries that removed an extensor named earlier in the merge.
    dropped: Vec<Named>,
}

/// A read-only root of definitions, such as the engine's.
#[derive(Clone, Debug)]
struct Library {
    name: String,
    root: PathBuf,
    entities: BTreeMap<String, Layer>,
    presets: BTreeMap<String, Layer>,
}

#[derive(Resource)]
pub struct DefinitionStore {
    root: PathBuf,
    primordials: BTreeSet<String>,
    extra_sections: BTreeSet<String>,
    extensors: ExtensorRegistry,
    libraries: Vec<Library>,
    /// The project's own definitions, including overrides of library ones.
    entities: BTreeMap<String, Layer>,
    presets: BTreeMap<String, Layer>,
    /// Unreadable or malformed files, by project-relative path.
    file_errors: BTreeMap<String, DataError>,
    resolved: BTreeMap<String, Resolved>,
    /// Resolution errors by entity id.
    errors: BTreeMap<String, Vec<DataError>>,
    deps: BTreeMap<String, BTreeSet<Dep>>,
}

impl DefinitionStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            primordials: BTreeSet::new(),
            extra_sections: DEFAULT_EXTRA_SECTIONS.map(String::from).into(),
            extensors: ExtensorRegistry::default(),
            libraries: Vec::new(),
            entities: BTreeMap::new(),
            presets: BTreeMap::new(),
            file_errors: BTreeMap::new(),
            resolved: BTreeMap::new(),
            errors: BTreeMap::new(),
            deps: BTreeMap::new(),
        }
    }

    /// Creates a store and loads every definition under `root`. Inspect [`Self::errors`] for
    /// problems: bad definitions are left out, the rest still load.
    pub fn open(root: impl Into<PathBuf>, registry: &TypeRegistry) -> Self {
        let mut store = Self::new(root);
        store.load(registry);
        store
    }

    /// Declares an engine primordial type (`Actor`) that has no file. Call before loading.
    pub fn declare_primordial(&mut self, id: impl Into<String>) -> &mut Self {
        let id = id.into();
        assert!(is_primordial(&id), "primordial names are capitalized: {id}");
        self.primordials.insert(id);
        self
    }

    /// Allows a top-level section other crates interpret. Call before loading.
    pub fn allow_section(&mut self, name: impl Into<String>) -> &mut Self {
        self.extra_sections.insert(name.into());
        self
    }

    /// The extensors definitions may name. Call before loading.
    pub fn set_extensors(&mut self, extensors: ExtensorRegistry) -> &mut Self {
        self.extensors = extensors;
        self
    }

    pub fn extensors(&self) -> &ExtensorRegistry {
        &self.extensors
    }

    /// Adds a read-only root of definitions the project can descend from and override, such as
    /// the engine's base definitions. Its files are labeled `<name>:<path>`. Call before loading.
    pub fn add_library(&mut self, name: impl Into<String>, root: impl Into<PathBuf>) -> &mut Self {
        self.libraries.push(Library {
            name: name.into(),
            root: root.into(),
            entities: BTreeMap::new(),
            presets: BTreeMap::new(),
        });
        self
    }

    /// The library defining `id`, if a library does; the project may still override it.
    pub fn library_of(&self, id: &str) -> Option<&str> {
        self.library_entity(id)
            .map(|(library, _)| library.name.as_str())
    }

    /// Whether the project has its own file for `id`: a definition or a library override.
    pub fn in_project(&self, id: &str) -> bool {
        self.entities.contains_key(id)
    }

    /// The file of a library definition.
    pub fn library_file(&self, id: &str) -> Option<PathBuf> {
        self.library_entity(id)
            .map(|(library, _)| library.root.join(id).join(ENTITY_FILE))
    }

    /// Every library's name and root, so clients can resolve the files it reports.
    pub fn library_roots(&self) -> impl Iterator<Item = (&str, &Path)> {
        self.libraries
            .iter()
            .map(|library| (library.name.as_str(), library.root.as_path()))
    }

    fn library_entity(&self, id: &str) -> Option<(&Library, &Layer)> {
        self.libraries
            .iter()
            .find_map(|library| library.entities.get(id).map(|layer| (library, layer)))
    }

    fn library_preset(&self, name: &str) -> Option<(&Library, &Layer)> {
        self.libraries
            .iter()
            .find_map(|library| library.presets.get(name).map(|layer| (library, layer)))
    }

    /// Every definition id, from libraries and the project.
    fn ids(&self) -> BTreeSet<String> {
        self.libraries
            .iter()
            .flat_map(|library| library.entities.keys())
            .chain(self.entities.keys())
            .chain(&self.primordials)
            .cloned()
            .collect()
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn get(&self, id: &str) -> Option<&Resolved> {
        self.resolved.get(id)
    }

    /// Ids of every successfully resolved definition, sorted.
    pub fn definitions(&self) -> impl Iterator<Item = &str> {
        self.resolved.keys().map(String::as_str)
    }

    pub fn preset_names(&self) -> impl Iterator<Item = &str> {
        self.libraries
            .iter()
            .flat_map(|library| library.presets.keys())
            .chain(self.presets.keys())
            .map(String::as_str)
    }

    /// Definitions that have `ancestor` in their lineage.
    pub fn descendants(&self, ancestor: &str) -> Vec<&str> {
        self.resolved
            .values()
            .filter(|r| r.lineage.iter().any(|a| a == ancestor))
            .map(|r| r.id.as_str())
            .collect()
    }

    /// Every current problem: file errors and definitions that failed to resolve. Sorted and
    /// without repeats (a bad parent would otherwise be reported once per descendant).
    pub fn errors(&self) -> Vec<DataError> {
        let mut all: Vec<DataError> = self
            .file_errors
            .values()
            .chain(self.errors.values().flatten())
            .cloned()
            .collect();
        all.sort_by(|a, b| {
            let key = |e: &DataError| {
                e.location
                    .as_ref()
                    .map(|l| (l.file.clone(), l.line, l.column))
            };
            key(a)
                .cmp(&key(b))
                .then_with(|| a.kind.to_string().cmp(&b.kind.to_string()))
        });
        all.dedup();
        all
    }

    /// The entity-file schema for this project: component types from `registry`, this project's
    /// definitions and presets as completions for `descendsFrom` and `presets`, and the registered
    /// actions wherever a reaction or a grant names one.
    pub fn schema(&self, registry: &TypeRegistry, actions: &ActionRegistry) -> serde_json::Value {
        let definitions = self.ids().into_iter().collect();
        crate::schema::entity_schema(
            registry,
            &crate::schema::SchemaOptions {
                definitions,
                presets: self.preset_names().map(str::to_owned).collect(),
                extra_sections: self.extra_sections.iter().cloned().collect(),
                extensors: self
                    .extensors
                    .iter()
                    .map(|e| (e.name.clone(), e.doc.clone()))
                    .collect(),
                states: self
                    .extensors
                    .state_docs()
                    .map(|(name, doc)| (name.to_owned(), doc.to_owned()))
                    .collect(),
                actions: actions
                    .descriptors()
                    .iter()
                    .map(|action| (action.name.clone(), action.doc.clone()))
                    .collect(),
            },
        )
    }

    /// Writes [`Self::schema`] to `path`; entity files point at it with `"$schema"`.
    pub fn write_schema(
        &self,
        registry: &TypeRegistry,
        actions: &ActionRegistry,
        path: &Path,
    ) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.schema(registry, actions))?;
        fs::write(path, text + "\n")
    }

    /// Scans the project directory and resolves everything, replacing previous contents.
    pub fn load(&mut self, registry: &TypeRegistry) -> ReloadReport {
        self.load_with_sources(&BTreeMap::new(), registry)
    }

    /// Resolves candidate sources without writing files or changing this store, including
    /// their effect on inherited definitions and presets.
    pub fn preview_sources(
        &self,
        sources: &BTreeMap<String, String>,
        registry: &TypeRegistry,
    ) -> Self {
        let mut candidate = Self::new(self.root.clone());
        candidate.primordials = self.primordials.clone();
        candidate.extra_sections = self.extra_sections.clone();
        candidate.extensors = self.extensors.clone();
        candidate.libraries = self.libraries.clone();
        candidate.load_with_sources(sources, registry);
        candidate
    }

    fn load_with_sources(
        &mut self,
        sources: &BTreeMap<String, String>,
        registry: &TypeRegistry,
    ) -> ReloadReport {
        self.entities.clear();
        self.presets.clear();
        self.file_errors.clear();
        for index in 0..self.libraries.len() {
            let library = &mut self.libraries[index];
            library.entities.clear();
            library.presets.clear();
            let root = library.root.clone();
            let mut files = Vec::new();
            walk(&root, &root, &mut files);
            files.sort();
            for rel in files {
                if let Some(kind) = classify(&rel) {
                    self.read_library_file(index, &rel, &kind);
                }
            }
        }
        let mut files = Vec::new();
        walk(&self.root, &self.root, &mut files);
        files.extend(sources.keys().cloned());
        files.sort();
        files.dedup();
        for rel in files {
            if let Some(kind) = classify(&rel) {
                self.read_layer_source(&rel, &kind, sources.get(&rel));
            }
        }
        let old_ids: Vec<String> = self.resolved.keys().cloned().collect();
        let ids = self.ids();
        let mut report = self.resolve(ids.iter().cloned().collect(), registry);
        for id in old_ids {
            if !ids.contains(&id) {
                self.resolved.remove(&id);
                report.removed.push(id);
            }
        }
        report.errors = self.errors();
        report
    }

    /// Re-reads one changed file (absolute, or relative to the root) and re-resolves what depends
    /// on it. A deleted file removes its definition; a file that no longer parses keeps its
    /// previous contents and reports the error.
    pub fn reload_file(&mut self, path: &Path, registry: &TypeRegistry) -> ReloadReport {
        let rel = if path.is_absolute() {
            match path.strip_prefix(&self.root) {
                Ok(rel) => rel,
                Err(_) => return ReloadReport::default(),
            }
        } else {
            path
        };
        let rel = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let Some(kind) = classify(&rel) else {
            return ReloadReport::default();
        };

        let mut report = ReloadReport::default();
        if self.root.join(&rel).exists() {
            self.read_layer(&rel, &kind);
        } else {
            self.file_errors.remove(&rel);
            match &kind {
                // Deleting a library override falls back to the library's version.
                FileKind::Entity(id) if self.library_entity(id).is_some() => {
                    self.entities.remove(id);
                }
                FileKind::Entity(id) => {
                    self.entities.remove(id);
                    if self.resolved.remove(id).is_some() {
                        report.removed.push(id.clone());
                    }
                    self.errors.remove(id);
                    self.deps.remove(id);
                }
                FileKind::Preset(name) => {
                    self.presets.remove(name);
                }
            }
        }

        let dep = kind.dep();
        let mut affected: BTreeSet<String> = self
            .deps
            .iter()
            .filter(|(_, deps)| deps.contains(&dep))
            .map(|(id, _)| id.clone())
            .collect();
        if let FileKind::Entity(id) = &kind
            && (self.entities.contains_key(id) || self.library_entity(id).is_some())
        {
            affected.insert(id.clone());
        }
        let resolved = self.resolve(affected.into_iter().collect(), registry);
        report.changed = resolved.changed;
        report.errors = self.file_errors.get(&rel).cloned().into_iter().collect();
        report.errors.extend(resolved.errors);
        report
    }

    fn read_layer(&mut self, rel: &str, kind: &FileKind) {
        self.read_layer_source(rel, kind, None);
    }

    fn read_library_file(&mut self, index: usize, rel: &str, kind: &FileKind) {
        let library = &self.libraries[index];
        let label = format!("{}:{rel}", library.name);
        let text = fs::read_to_string(library.root.join(rel));
        match self.parse_layer_text(&label, kind, text) {
            Ok(layer) => {
                let library = &mut self.libraries[index];
                match kind {
                    FileKind::Entity(id) => library.entities.insert(id.clone(), layer),
                    FileKind::Preset(name) => library.presets.insert(name.clone(), layer),
                };
            }
            Err(error) => {
                self.file_errors.insert(label, error);
            }
        }
    }

    fn parse_layer_text(
        &self,
        label: &str,
        kind: &FileKind,
        text: std::io::Result<String>,
    ) -> Result<Layer, DataError> {
        text.map_err(|e| {
            DataError::new(
                ErrorKind::Io(format!("cannot read {label}: {e}")),
                Some(Location {
                    file: label.into(),
                    line: 1,
                    column: 1,
                }),
            )
        })
        .and_then(|text| parse_jsonc(label, &text))
        .and_then(|node| {
            let layer_kind = match kind {
                FileKind::Entity(_) => LayerKind::Entity,
                FileKind::Preset(_) => LayerKind::Preset,
            };
            parse_layer(node, layer_kind, &self.extra_sections)
        })
    }

    fn read_layer_source(&mut self, rel: &str, kind: &FileKind, source: Option<&String>) {
        let text = source.map_or_else(
            || fs::read_to_string(self.root.join(rel)),
            |source| Ok(source.clone()),
        );
        let result = self
            .parse_layer_text(rel, kind, text)
            .and_then(|layer| match kind {
                FileKind::Preset(name) => match self.library_preset(name) {
                    Some((library, _)) => Err(DataError::at(
                        ErrorKind::LibraryPreset {
                            name: name.clone(),
                            library: library.name.clone(),
                        },
                        &layer.root_span,
                    )),
                    None => Ok(layer),
                },
                FileKind::Entity(_) => Ok(layer),
            });
        match result {
            Ok(layer) => {
                self.file_errors.remove(rel);
                match kind {
                    FileKind::Entity(id) => self.entities.insert(id.clone(), layer),
                    FileKind::Preset(name) => self.presets.insert(name.clone(), layer),
                };
            }
            Err(e) => {
                self.file_errors.insert(rel.to_owned(), e);
            }
        }
    }

    /// Everything a definition's resolution reads, computed from the parsed files alone so it is
    /// right even when resolution fails.
    fn collect_deps(&self, id: &str) -> BTreeSet<Dep> {
        let mut deps = BTreeSet::new();
        let mut entity_queue = vec![id.to_owned()];
        let mut preset_queue = Vec::new();
        while let Some(current) = entity_queue.pop() {
            if !deps.insert(Dep::Entity(current.clone())) {
                continue;
            }
            let library = self.library_entity(&current).map(|(_, layer)| layer);
            for layer in library.into_iter().chain(self.entities.get(&current)) {
                entity_queue.extend(layer.descends_from.iter().map(|(p, _)| p.clone()));
                preset_queue.extend(layer.presets.iter().map(|(p, _)| p.clone()));
            }
        }
        while let Some(current) = preset_queue.pop() {
            if !deps.insert(Dep::Preset(current.clone())) {
                continue;
            }
            let library = self.library_preset(&current).map(|(_, layer)| layer);
            if let Some(layer) = library.or(self.presets.get(&current)) {
                preset_queue.extend(layer.presets.iter().map(|(p, _)| p.clone()));
            }
        }
        deps
    }

    fn resolve(&mut self, ids: Vec<String>, registry: &TypeRegistry) -> ReloadReport {
        let mut resolver = Resolver::new(self);
        let mut outcomes = Vec::new();
        for id in ids {
            let outcome = resolver.entity_resolved(&id, registry);
            outcomes.push((id, outcome));
        }
        drop(resolver);

        let mut report = ReloadReport::default();
        for (id, outcome) in outcomes {
            self.deps.insert(id.clone(), self.collect_deps(&id));
            match outcome {
                Ok(new) => {
                    self.errors.remove(&id);
                    let changed = self
                        .resolved
                        .get(&id)
                        .is_none_or(|old| !old.same_data(&new));
                    if changed {
                        report.changed.push(id.clone());
                    }
                    self.resolved.insert(id, new);
                }
                Err(errors) => {
                    report.errors.extend(errors.iter().cloned());
                    self.errors.insert(id, errors);
                }
            }
        }
        report.errors.sort_by_key(|e| e.to_string());
        report.errors.dedup();
        report
    }

    /// Resolves `id` and applies scene/spawn overrides on top, without storing anything.
    ///
    /// `overrides` has the shape of a definition minus `descendsFrom`: `presets`, `transform`,
    /// `components` (deep-merged over the definition's), extra sections. Parse it with
    /// [`parse_jsonc`], or take the node from a larger scene file so errors point there.
    pub fn instantiate(
        &self,
        id: &str,
        overrides: Option<&Node>,
        registry: &TypeRegistry,
    ) -> Result<Resolved, Vec<DataError>> {
        let base = self.resolved.get(id).ok_or_else(|| {
            vec![DataError::new(
                ErrorKind::MissingDefinition(id.into()),
                None,
            )]
        })?;
        let mut merged = Merged {
            body: base.body.clone(),
            lineage: base.lineage.clone(),
            extensors: base.named.clone(),
            dropped: base.dropped_entries.clone(),
        };
        if let Some(overrides) = overrides {
            let layer = parse_layer(overrides.clone(), LayerKind::Override, &self.extra_sections)
                .map_err(|e| vec![e])?;
            Resolver::new(self)
                .apply_layer(&mut merged, &layer, "a scene override", &mut Vec::new())
                .map_err(|e| vec![e])?;
            strip_removed_components(&mut merged.body);
        }
        self.finish(id, merged, registry)
    }

    /// Builds the components of a merged definition and resolves its extensors.
    fn finish(
        &self,
        id: &str,
        merged: Merged,
        registry: &TypeRegistry,
    ) -> Result<Resolved, Vec<DataError>> {
        let mut components = build_components(&merged.body, registry)?;
        let uses = extensors::resolve(
            &merged.extensors,
            &merged.body,
            &mut components,
            &self.extensors,
            registry,
        )?;
        let states = extensors::resolve_states(&merged.body, &uses, &self.extensors, registry)?;
        Ok(Resolved::new(
            id.into(),
            merged.lineage,
            (merged.extensors, merged.dropped, uses),
            components,
            states,
            merged.body,
        ))
    }
}

/// Memoizes merged bodies within one pass, so a shared ancestor merges once.
struct Resolver<'a> {
    store: &'a DefinitionStore,
    entities: HashMap<String, Result<Rc<Merged>, DataError>>,
    presets: HashMap<String, Result<Rc<Merged>, DataError>>,
}

impl<'a> Resolver<'a> {
    fn new(store: &'a DefinitionStore) -> Self {
        Self {
            store,
            entities: HashMap::new(),
            presets: HashMap::new(),
        }
    }

    fn entity_resolved(
        &mut self,
        id: &str,
        registry: &TypeRegistry,
    ) -> Result<Resolved, Vec<DataError>> {
        let merged = self.entity(id, &mut Vec::new()).map_err(|e| vec![e])?;
        let merged = Merged {
            body: merged.body.clone(),
            lineage: merged.lineage.clone(),
            extensors: merged.extensors.clone(),
            dropped: merged.dropped.clone(),
        };
        self.store.finish(id, merged, registry)
    }

    fn entity(&mut self, id: &str, stack: &mut Vec<String>) -> Result<Rc<Merged>, DataError> {
        if let Some(done) = self.entities.get(id) {
            return done.clone();
        }
        stack.push(id.to_owned());
        let result = self.compute_entity(id, stack).map(Rc::new);
        stack.pop();
        self.entities.insert(id.to_owned(), result.clone());
        result
    }

    fn compute_entity(&mut self, id: &str, stack: &mut Vec<String>) -> Result<Merged, DataError> {
        let store = self.store;
        let library = store.library_entity(id);
        // A project file over a library definition is its override, merged last.
        let (layer, by, project_override) = match (library, store.entities.get(id)) {
            (Some((library, layer)), project) => {
                (Some(layer), format!("{}:{id}", library.name), project)
            }
            (None, project) => (project, id.to_owned(), None),
        };
        let Some(layer) = layer else {
            if store.primordials.contains(id) {
                let file = format!("<primordial {id}>");
                let span = synthetic_span(&file);
                return Ok(Merged {
                    body: Node::empty_object(span),
                    lineage: Vec::new(),
                    extensors: Vec::new(),
                    dropped: Vec::new(),
                });
            }
            return Err(DataError::new(
                ErrorKind::MissingDefinition(id.into()),
                None,
            ));
        };

        let mut merged = match &layer.descends_from {
            Some((parent, span)) => {
                if let Some(start) = stack.iter().position(|s| s == parent) {
                    let mut chain = stack[start..].to_vec();
                    chain.push(parent.clone());
                    return Err(DataError::at(ErrorKind::DefinitionCycle(chain), span));
                }
                if !store.entities.contains_key(parent)
                    && store.library_entity(parent).is_none()
                    && !store.primordials.contains(parent)
                {
                    return Err(DataError::at(
                        ErrorKind::MissingDefinition(parent.clone()),
                        span,
                    ));
                }
                let parent_merged = self.entity(parent, stack)?;
                let mut lineage = parent_merged.lineage.clone();
                lineage.push(parent.clone());
                Merged {
                    body: parent_merged.body.clone(),
                    lineage,
                    extensors: parent_merged.extensors.clone(),
                    dropped: parent_merged.dropped.clone(),
                }
            }
            None => {
                if !is_primordial(id) {
                    return Err(DataError::at(
                        ErrorKind::NotPrimordial(id.into()),
                        &layer.root_span,
                    ));
                }
                Merged {
                    body: Node::empty_object(layer.root_span.clone()),
                    lineage: Vec::new(),
                    extensors: Vec::new(),
                    dropped: Vec::new(),
                }
            }
        };
        self.apply_layer(&mut merged, layer, &by, &mut Vec::new())?;
        if let Some(project) = project_override {
            if let Some((parent, span)) = &project.descends_from
                && Some(parent) != layer.descends_from.as_ref().map(|(p, _)| p)
            {
                return Err(DataError::at(
                    ErrorKind::OverrideParent {
                        id: id.into(),
                        library: by.split(':').next().unwrap_or_default().into(),
                    },
                    span,
                ));
            }
            self.apply_layer(&mut merged, project, id, &mut Vec::new())?;
        }
        strip_removed_components(&mut merged.body);
        Ok(merged)
    }

    /// Merges the layer's presets, then its own data, over `merged`: inherited < presets < own.
    /// Extensors accumulate; `by` is who named the layer's own. A `"-name"` entry drops an
    /// extensor named so far, with the components it owns, before the layer's own components
    /// merge in.
    fn apply_layer(
        &mut self,
        merged: &mut Merged,
        layer: &Layer,
        by: &str,
        preset_stack: &mut Vec<String>,
    ) -> Result<(), DataError> {
        for (name, span) in &layer.presets {
            let preset = self.preset(name, span, preset_stack)?;
            for entry in &preset.dropped {
                self.drop_extensor(merged, entry.clone())?;
            }
            merged.body.merge(preset.body.clone());
            merged
                .dropped
                .retain(|d| !preset.extensors.iter().any(|n| n.name == d.name));
            merged.extensors.extend(preset.extensors.iter().cloned());
        }
        for (name, span) in &layer.extensors {
            let entry = Named {
                name: name.strip_prefix('-').unwrap_or(name).to_owned(),
                span: span.clone(),
                by: by.into(),
            };
            if name.starts_with('-') {
                self.drop_extensor(merged, entry)?;
            } else {
                merged.dropped.retain(|d| d.name != entry.name);
                merged.extensors.push(entry);
            }
        }
        merged.body.merge(layer.body.clone());
        Ok(())
    }

    fn drop_extensor(&self, merged: &mut Merged, entry: Named) -> Result<(), DataError> {
        let Some(meta) = self.store.extensors.get(&entry.name) else {
            return Err(DataError::at(
                ErrorKind::UnknownExtensor {
                    name: entry.name,
                    known: self.store.extensors.names().map(str::to_owned).collect(),
                },
                &entry.span,
            ));
        };
        merged.extensors.retain(|n| n.name != entry.name);
        if let NodeValue::Object(members) = &mut merged.body.value
            && let Some(components) = members.iter_mut().find(|m| m.key == "components")
            && let NodeValue::Object(items) = &mut components.value.value
        {
            items.retain(|item| {
                !meta
                    .components
                    .iter()
                    .any(|c| item.key == c.name || item.key == c.type_path)
            });
        }
        merged.dropped.retain(|d| d.name != entry.name);
        merged.dropped.push(entry);
        Ok(())
    }

    fn preset(
        &mut self,
        name: &str,
        used_at: &Span,
        stack: &mut Vec<String>,
    ) -> Result<Rc<Merged>, DataError> {
        if let Some(start) = stack.iter().position(|s| s == name) {
            let mut chain = stack[start..].to_vec();
            chain.push(name.to_owned());
            return Err(DataError::at(ErrorKind::PresetCycle(chain), used_at));
        }
        if let Some(done) = self.presets.get(name) {
            return done.clone();
        }
        let store = self.store;
        let library = store.library_preset(name).map(|(_, layer)| layer);
        let Some(layer) = library.or(store.presets.get(name)) else {
            return Err(DataError::at(
                ErrorKind::MissingPreset(name.into()),
                used_at,
            ));
        };
        stack.push(name.to_owned());
        let mut merged = Merged {
            body: Node::empty_object(layer.root_span.clone()),
            lineage: Vec::new(),
            extensors: Vec::new(),
            dropped: Vec::new(),
        };
        let by = format!("preset {name}");
        let result = self
            .apply_layer(&mut merged, layer, &by, stack)
            .map(|()| Rc::new(merged));
        stack.pop();
        // A cycle error depends on where resolution entered it; only cache successes.
        if let Ok(ok) = &result {
            self.presets.insert(name.to_owned(), Ok(ok.clone()));
        }
        result
    }
}

fn synthetic_span(file: &str) -> Span {
    let pos = crate::source::Pos {
        offset: 0,
        line: 1,
        column: 1,
    };
    Span {
        file: file.into(),
        start: pos,
        end: pos,
    }
}

/// Builds every component of a merged body, collecting one error per bad component.
fn build_components(
    body: &Node,
    registry: &TypeRegistry,
) -> Result<Vec<ComponentValue>, Vec<DataError>> {
    match body.get("components") {
        Some(components) => build_component_map(components, registry),
        None => Ok(Vec::new()),
    }
}

/// Builds the components of a `{ "Name": value }` object, collecting one error per bad
/// component. Also used for component maps outside `components`, such as grants.
pub(crate) fn build_component_map(
    components: &Node,
    registry: &TypeRegistry,
) -> Result<Vec<ComponentValue>, Vec<DataError>> {
    let builder = Builder { registry };
    let mut out: Vec<ComponentValue> = Vec::new();
    let mut errors = Vec::new();
    let Some(components) = components.as_object() else {
        return Err(vec![DataError::at(
            ErrorKind::TypeMismatch {
                expected: "object of components".into(),
                found: components.kind_name().into(),
            },
            &components.span,
        )]);
    };
    for member in components {
        let built = builder
            .component_registration(&member.key, &member.key_span)
            .and_then(|registration| {
                if out.iter().any(|c| c.type_id == registration.type_id()) {
                    Err(DataError::at(
                        ErrorKind::DuplicateComponent(member.key.clone()),
                        &member.key_span,
                    ))
                } else {
                    builder.component(registration, &member.value)
                }
            });
        match built {
            Ok(component) => out.push(component),
            Err(e) => errors.push(e),
        }
    }
    if errors.is_empty() {
        Ok(out)
    } else {
        Err(errors)
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') && name != "build" && name != "target" {
                walk(root, &path, out);
            }
        } else if name.ends_with(".jsonc")
            && let Ok(rel) = path.strip_prefix(root)
        {
            out.push(
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
}
