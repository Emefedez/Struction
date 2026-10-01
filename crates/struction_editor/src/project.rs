//! Game-aware authoring: validation, an authored preview and a separate simulation app.
//! The host supplies the game's plugin/component/action registration once through a factory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use bevy::reflect::serde::TypedReflectSerializer;
use bevy::time::{TimePlugin, TimeUpdateStrategy};
use serde::Serialize;
use serde_json::{Value, json};
use struction_core::{ActionRegistry, Definition, MasterIs, StableId};
use struction_data::{DataError, DefinitionStore, ErrorKind};
use struction_world::{
    EntityPath, PathAliases, SceneCatalog, WorldEntity, link_masters, run_pending_spawners,
};

use crate::extensors::{DroppedEntry, ExtensorEntry, SuggestedExtensor};
use crate::session::{Applied, EditRequest, EditSession, Field, SessionError};

#[derive(Clone, Debug, Serialize)]
pub struct Diagnostic {
    pub message: String,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

impl From<&DataError> for Diagnostic {
    fn from(error: &DataError) -> Self {
        Self {
            message: error.kind.to_string(),
            file: error.location.as_ref().map(|at| at.file.to_string()),
            line: error.location.as_ref().map(|at| at.line),
            column: error.location.as_ref().map(|at| at.column),
        }
    }
}

type GameFactory = dyn Fn(&Path) -> App;

pub struct AuthoringProject {
    session: EditSession,
    factory: Box<GameFactory>,
    preview: App,
    play: Option<App>,
}

fn build(root: &Path, factory: &GameFactory) -> Result<App, SessionError> {
    let mut app = factory(root);
    if !app.is_plugin_added::<TimePlugin>() {
        app.add_plugins(MinimalPlugins);
    }
    let step = app.world().resource::<Time<Fixed>>().timestep();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(step));
    app.finish();
    app.cleanup();
    if !app.world().contains_resource::<DefinitionStore>()
        || !app.world().contains_resource::<SceneCatalog>()
        || !app.world().contains_resource::<ActionRegistry>()
    {
        return Err(invalid(ErrorKind::Io(
            "game factory requires CorePlugin, DataPlugin and WorldPlugin".into(),
        )));
    }
    // Initializes Startup and the clock without advancing a simulation tick.
    app.update();
    run_pending_spawners(app.world_mut());
    link_masters(app.world_mut());
    Ok(app)
}

/// A validation failure without a source location.
fn invalid(kind: ErrorKind) -> SessionError {
    SessionError::Validation(vec![DataError::new(kind, None)])
}

fn validate(world: &World, sources: &BTreeMap<String, String>) -> Result<(), Vec<DataError>> {
    let types = world.resource::<AppTypeRegistry>().read();
    let store = world
        .resource::<DefinitionStore>()
        .preview_sources(sources, &types);
    let actions = world.resource::<ActionRegistry>();
    let mut errors = store.errors();
    if let Err(error) = PathAliases::load(store.root()) {
        errors.push(error);
    }
    errors.extend(store.check_references(&types, actions));
    let scenes = SceneCatalog::load_with_sources(store.root(), &store, &types, sources);
    errors.extend(scenes.errors().iter().cloned());
    let spawns: BTreeMap<_, _> = scenes
        .spawners()
        .flat_map(|s| &s.spawns)
        .map(|s| (s.path.as_str(), s))
        .collect();
    for spawn in spawns.values() {
        let mut seen = BTreeSet::new();
        let mut at = Some(spawn.path.as_str());
        while let Some(path) = at {
            if !seen.insert(path) {
                errors.push(DataError::at(
                    ErrorKind::InvalidValue {
                        ty: "masterIs".into(),
                        message: format!("master relationship cycle through {path}"),
                    },
                    &spawn.source,
                ));
                break;
            }
            at = spawns
                .get(path)
                .and_then(|s| s.master_is.as_ref().map(|p| p.as_str()));
        }
    }
    // Scene overrides can carry grants/reactions as well as ordinary components.
    for spawner in scenes.spawners() {
        for spawn in &spawner.spawns {
            if let Ok(resolved) =
                store.instantiate(&spawn.definition, spawn.overrides.as_ref(), &types)
            {
                if let Err(found) = resolved.grants_to_wards(&types, actions) {
                    errors.extend(found);
                }
                if let Err(found) = resolved.reactions(actions) {
                    errors.extend(found);
                }
            }
        }
    }
    errors.sort_by_key(ToString::to_string);
    errors.dedup();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

impl AuthoringProject {
    /// `factory` returns a configured, unstarted headless app with the game's registrations.
    /// Preview and play use the same factory. No application/world is shared between them.
    pub fn open(
        root: impl AsRef<Path>,
        factory: impl Fn(&Path) -> App + 'static,
    ) -> Result<Self, SessionError> {
        let root = std::fs::canonicalize(root).map_err(|source| SessionError::Io {
            file: "project root".into(),
            source,
        })?;
        if !root.is_dir() {
            return Err(SessionError::InvalidOperation("open the project directory containing definitions and scenes/, not an individual source file".into()));
        }
        let preview = build(&root, &factory)?;
        Ok(Self {
            session: EditSession::new(root),
            factory: Box::new(factory),
            preview,
            play: None,
        })
    }
    pub fn session(&self) -> &EditSession {
        &self.session
    }
    pub fn preview(&self) -> &World {
        self.preview.world()
    }
    pub fn play_world(&self) -> Option<&World> {
        self.play.as_ref().map(App::world)
    }

    /// Current source diagnostics, including edits made outside this process.
    pub fn validate(&self) -> Vec<Diagnostic> {
        validate(self.preview.world(), &BTreeMap::new())
            .err()
            .unwrap_or_default()
            .iter()
            .map(Diagnostic::from)
            .collect()
    }
    /// Validates complete unsaved buffers through the same checks as edits, without changing
    /// sources, history, the preview or play state. Paths are relative to the project root.
    pub fn validate_sources(
        &self,
        sources: &BTreeMap<String, String>,
    ) -> Result<Vec<Diagnostic>, SessionError> {
        for file in sources.keys() {
            self.session.path_of(file)?;
        }
        Ok(validate(self.preview.world(), sources)
            .err()
            .unwrap_or_default()
            .iter()
            .map(Diagnostic::from)
            .collect())
    }

    pub fn schema(&self) -> Value {
        let world = self.preview.world();
        let types = world.resource::<AppTypeRegistry>().read();
        let actions = world.resource::<ActionRegistry>();
        world.resource::<DefinitionStore>().schema(&types, actions)
    }
    pub fn actions(&self) -> Vec<struction_core::ActionDescriptor> {
        self.preview
            .world()
            .resource::<ActionRegistry>()
            .descriptors()
    }
    /// Includes broken source definitions so authoring clients can still find and repair them.
    pub fn definitions(&self) -> Vec<String> {
        fn sources(root: &Path, directory: &Path, paths: &mut BTreeSet<String>) {
            let Ok(entries) = std::fs::read_dir(directory) else {
                return;
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                let Ok(kind) = entry.file_type() else {
                    continue;
                };
                if kind.is_dir()
                    && !name.starts_with('.')
                    && !matches!(name.as_ref(), "target" | "presets" | "scenes")
                {
                    sources(root, &entry.path(), paths);
                } else if kind.is_file()
                    && name == "entity.jsonc"
                    && let Ok(path) = directory.strip_prefix(root)
                    && !path.as_os_str().is_empty()
                {
                    paths.insert(path.to_string_lossy().replace('\\', "/"));
                }
            }
        }
        let mut paths = self
            .preview
            .world()
            .resource::<DefinitionStore>()
            .definitions()
            .map(str::to_owned)
            .collect();
        sources(self.session.root(), self.session.root(), &mut paths);
        paths.into_iter().collect()
    }
    pub fn inspect_definition(&self, path: &str) -> Result<DefinitionInspection, SessionError> {
        let world = self.preview.world();
        let resolved = world
            .resource::<DefinitionStore>()
            .get(path)
            .ok_or_else(|| {
                let store = world.resource::<DefinitionStore>();
                let file = format!("{path}/entity.jsonc");
                let errors: Vec<_> = store
                    .errors()
                    .into_iter()
                    .filter(|e| {
                        e.location
                            .as_ref()
                            .is_some_and(|at| at.file.as_ref() == file)
                    })
                    .collect();
                if errors.is_empty() {
                    invalid(ErrorKind::MissingDefinition(path.into()))
                } else {
                    SessionError::Validation(errors)
                }
            })?;
        let types = world.resource::<AppTypeRegistry>().read();
        let components = resolved
            .components
            .iter()
            .map(|component| {
                reflect_value(component.value.as_partial_reflect(), &types)
                    .map(|value| (component.type_path.to_owned(), value))
                    .map_err(|e| invalid(ErrorKind::UnsupportedType(e.to_string())))
            })
            .collect::<Result<_, _>>()?;
        let registry = world.resource::<DefinitionStore>().extensors();
        let in_use = |name: &str| resolved.extensors.iter().any(|used| used.name == name);
        let store = world.resource::<DefinitionStore>();
        Ok(DefinitionInspection {
            definition: path.into(),
            library: store.library_of(path).map(str::to_owned),
            overridden: store.library_of(path).is_some() && store.in_project(path),
            lineage: resolved.lineage.clone(),
            extensors: resolved.extensors.iter().map(ExtensorEntry::from).collect(),
            dropped_extensors: resolved.dropped.iter().map(DroppedEntry::from).collect(),
            suggested_extensors: resolved
                .suggested_extensors(registry)
                .into_iter()
                .map(SuggestedExtensor::from)
                .collect(),
            available_extensors: registry
                .names()
                .filter(|name| !in_use(name))
                .map(str::to_owned)
                .collect(),
            resolved: resolved.data().clone(),
            components,
        })
    }

    /// Rebuilds the authored preview only after all sources validate. A bad outside edit keeps
    /// the last working preview, while invalidating affected undo history.
    pub fn refresh(&mut self) -> Result<(), SessionError> {
        self.session.check_external_changes();
        validate(self.preview.world(), &BTreeMap::new()).map_err(SessionError::Validation)?;
        self.preview = build(self.session.root(), &*self.factory)?;
        Ok(())
    }
    pub fn edit(&mut self, request: EditRequest) -> Result<Applied, SessionError> {
        let file = match &request {
            EditRequest::Set { file, .. } | EditRequest::Remove { file, .. } => file,
        };
        if !(file.ends_with("/entity.jsonc")
            || ((file.starts_with("scenes/") || file.starts_with("presets/"))
                && file.ends_with(".jsonc")))
        {
            return Err(SessionError::InvalidOperation(
                "edits require an entity, preset or scene JSONC source".into(),
            ));
        }
        let created = match file.strip_suffix("/entity.jsonc") {
            Some(id) => self.ensure_override(id)?,
            None => None,
        };
        let world = self.preview.world();
        let applied = self
            .session
            .apply_checked(request, |sources| validate(world, sources));
        self.finish_override(created, applied)
    }

    /// Creates the project's override file of a library (engine) definition, so editing it changes
    /// the project instead of the engine. Returns the file when it had to be created.
    pub(crate) fn ensure_override(&mut self, id: &str) -> Result<Option<String>, SessionError> {
        let file = format!("{id}/entity.jsonc");
        let store = self.preview.world().resource::<DefinitionStore>();
        let Some(library) = store.library_of(id) else {
            return Ok(None);
        };
        if store.in_project(id) || self.session.read(&file).is_ok() {
            return Ok(None);
        }
        let text = format!(
            "// The project's changes to the {library} definition {id}; the rest comes from it.\n{{}}\n"
        );
        self.session.create_file(&file, &text)?;
        Ok(Some(file))
    }

    /// Refreshes after an applied edit. An override file created for an edit that failed or
    /// changed nothing is removed again, so only real changes leave one.
    pub(crate) fn finish_override(
        &mut self,
        created: Option<String>,
        applied: Result<Applied, SessionError>,
    ) -> Result<Applied, SessionError> {
        let wrote = applied.as_ref().is_ok_and(|a| !a.files.is_empty());
        if !wrote
            && let Some(file) = created
            && let Ok(path) = self.session.path_of(&file)
        {
            let _ = std::fs::remove_file(path);
        }
        let applied = applied?;
        if wrote {
            self.refresh()?;
        }
        Ok(applied)
    }
    pub fn undo(&mut self) -> Result<Option<Applied>, SessionError> {
        let world = self.preview.world();
        let applied = self
            .session
            .undo_checked(|sources| validate(world, sources))?;
        if applied.is_some() {
            self.refresh()?;
        }
        Ok(applied)
    }
    pub fn redo(&mut self) -> Result<Option<Applied>, SessionError> {
        let world = self.preview.world();
        let applied = self
            .session
            .redo_checked(|sources| validate(world, sources))?;
        if applied.is_some() {
            self.refresh()?;
        }
        Ok(applied)
    }
    pub fn end_group(&mut self) {
        self.session.end_group();
    }

    /// Several field edits of one entity source as one validated, undoable step. `created` is the
    /// override file made for it, removed again if the edit fails.
    pub(crate) fn edit_fields(
        &mut self,
        file: &str,
        label: &str,
        edits: Vec<(Vec<struction_data::edit::PathSegment>, Option<Value>)>,
        created: Option<String>,
    ) -> Result<Applied, SessionError> {
        let world = self.preview.world();
        let applied = self
            .session
            .apply_fields_checked(file, label, edits, |sources| validate(world, sources));
        self.finish_override(created, applied)
    }

    /// Templates become user-owned files; existing files are never overwritten.
    pub fn create_definition(&mut self, path: &str, parent: &str) -> Result<(), SessionError> {
        if self.session.is_playing() {
            return Err(SessionError::Playing);
        }
        if path == "presets"
            || path.starts_with("presets/")
            || path == "scenes"
            || path.starts_with("scenes/")
        {
            return Err(SessionError::InvalidOperation(
                "definitions cannot use the reserved presets or scenes directories".into(),
            ));
        }
        let file = format!("{path}/entity.jsonc");
        self.session.path_of(&file)?;
        let text = format!(
            "// Entity definition.\n{}\n",
            serde_json::to_string_pretty(&json!({"descendsFrom":parent,"components":{}}))
                .expect("plain data")
        );
        validate(
            self.preview.world(),
            &BTreeMap::from([(file.clone(), text.clone())]),
        )
        .map_err(SessionError::Validation)?;
        self.session.create_file(&file, &text)?;
        self.refresh()
    }

    pub fn start_play(&mut self) -> Result<(), SessionError> {
        if self.play.is_some() {
            return Err(SessionError::Playing);
        }
        self.refresh()?;
        self.play = Some(build(self.session.root(), &*self.factory)?);
        self.session.set_playing(true);
        Ok(())
    }
    /// Queue input for the isolated simulation. Callers may submit faster than its tick rate.
    pub fn play_input(&mut self, input: crate::PlayInput) -> Result<(), SessionError> {
        if input
            .movement
            .iter()
            .chain(&input.look)
            .chain([&input.zoom])
            .any(|v| !v.is_finite())
        {
            return Err(SessionError::InvalidOperation(
                "play input must contain finite numbers".into(),
            ));
        }
        let world = self
            .play
            .as_mut()
            .ok_or_else(|| SessionError::InvalidOperation("play mode is not running".into()))?
            .world_mut();
        let mut pending = world
            .get_resource_mut::<crate::play::PendingInput>()
            .ok_or_else(|| {
                SessionError::InvalidOperation(
                    "game factory does not install PlayInputPlugin".into(),
                )
            })?;
        pending.push(input);
        Ok(())
    }

    /// Release held and unconsumed input when focus is lost or playback is paused.
    pub fn release_play_input(&mut self) {
        if let Some(app) = &mut self.play {
            let world = app.world_mut();
            if let Some(mut pending) = world.get_resource_mut::<crate::play::PendingInput>() {
                *pending = default();
            }
            let mut players = world.query_filtered::<&mut struction_character::CharacterIntent, With<struction_character::PlayerControlled>>();
            for mut intent in players.iter_mut(world) {
                *intent = default();
            }
        }
    }

    pub fn step_play(&mut self, ticks: usize) -> Result<(), SessionError> {
        let app = self
            .play
            .as_mut()
            .ok_or_else(|| SessionError::InvalidOperation("play mode is not running".into()))?;
        for _ in 0..ticks {
            app.update();
        }
        Ok(())
    }
    pub fn stop_play(&mut self) {
        self.play = None;
        self.session.set_playing(false);
    }

    /// Moves an authored instance in world space, accounting for its zone, spawner,
    /// spawn rotation and inherited definition transform. Drags share a history group.
    pub fn move_spawn(
        &mut self,
        path: &str,
        position: Vec3,
        group: Option<String>,
    ) -> Result<Applied, SessionError> {
        if self.session.is_playing() {
            return Err(SessionError::Playing);
        }
        if !position.is_finite() {
            return Err(SessionError::InvalidOperation(
                "position must be finite".into(),
            ));
        }
        self.refresh()?;
        let world = self.preview();
        let scenes = world.resource::<SceneCatalog>();
        let (spawner, spawn) = scenes
            .spawners()
            .find_map(|spawner| {
                spawner
                    .spawns
                    .iter()
                    .find(|spawn| spawn.path.as_str() == path)
                    .map(|spawn| (spawner, spawn))
            })
            .ok_or_else(|| {
                SessionError::InvalidOperation(format!("unknown authored spawn: {path}"))
            })?;
        let types = world.resource::<AppTypeRegistry>().read();
        let resolved = world
            .resource::<DefinitionStore>()
            .instantiate(&spawn.definition, spawn.overrides.as_ref(), &types)
            .map_err(SessionError::Validation)?;
        let definition_offset = resolved
            .component::<Transform>()
            .map_or(Vec3::ZERO, |t| t.translation);
        let parent = spawner.world_transform(&scenes.zone_transform(&spawner.zone));
        let offset = parent.compute_affine().inverse().transform_point3(position)
            - spawn.rotation * definition_offset;
        let request = EditRequest::Set {
            file: spawn.source.file.to_string(),
            path: [
                "spawnerList",
                &spawner.name,
                "spawns",
                &spawn.name,
                "offset",
            ]
            .into_iter()
            .map(|key| Field::Key(key.into()))
            .collect(),
            value: json!(offset.to_array()),
            label: "Move spawn".into(),
            group,
            revision: Some(crate::session::revision(
                &self.session.read(&spawn.source.file)?,
            )),
        };
        drop(types);
        self.edit(request)
    }

    fn inspected_world(&self, playing: bool) -> Result<&World, SessionError> {
        if playing {
            self.play_world()
                .ok_or_else(|| SessionError::InvalidOperation("play mode is not running".into()))
        } else {
            Ok(self.preview())
        }
    }

    /// Includes disabled and runtime-created instances. Paths express authoring hierarchy;
    /// master relations are reported independently.
    pub fn entities(&self, playing: bool) -> Result<Vec<EntityEntry>, SessionError> {
        let world = self.inspected_world(playing)?;
        let Some(mut query) =
            world.try_query_filtered::<Entity, (With<WorldEntity>, Allow<Disabled>)>()
        else {
            return Ok(vec![]);
        };
        let mut entities: Vec<_> = query
            .iter(world)
            .map(|entity| EntityEntry::new(world, entity))
            .collect();
        entities.sort_by(|a, b| {
            (a.path.as_deref(), a.stable_id.as_deref())
                .cmp(&(b.path.as_deref(), b.stable_id.as_deref()))
        });
        Ok(entities)
    }

    /// Reflected component values plus names of unregistered components. `target` is an
    /// authored path or StableId, never a transient ECS entity number.
    pub fn inspect_entity(
        &self,
        target: &str,
        playing: bool,
    ) -> Result<EntityInspection, SessionError> {
        let world = self.inspected_world(playing)?;
        let entry = self
            .entities(playing)?
            .into_iter()
            .find(|entry| entry.key() == target)
            .ok_or_else(|| SessionError::InvalidOperation(format!("unknown entity: {target}")))?;
        let types = world.resource::<AppTypeRegistry>().read();
        let mut components = BTreeMap::new();
        let mut unavailable = Vec::new();
        for info in world
            .inspect_entity(entry.entity)
            .expect("entity from this world")
        {
            let registered = info.type_id().and_then(|id| types.get(id));
            let reflected = registered
                .and_then(|r| r.data::<ReflectComponent>())
                .and_then(|r| r.reflect(world.entity(entry.entity)));
            let reason = match (registered, reflected) {
                (Some(registered), Some(value)) => match reflect_value(value, &types) {
                    Ok(value) => {
                        components.insert(registered.type_info().type_path().to_owned(), value);
                        continue;
                    }
                    Err(error) => error.to_string(),
                },
                _ => "component has no registered reflection".into(),
            };
            unavailable.push(Unavailable {
                component: info.name().to_string(),
                reason,
            });
        }
        Ok(EntityInspection {
            entity: entry,
            components,
            unavailable,
        })
    }
}

fn reflect_value(
    value: &dyn PartialReflect,
    types: &TypeRegistry,
) -> Result<Value, serde_json::Error> {
    serde_json::to_value(TypedReflectSerializer::new(value, types))
}

/// A world entity as the hierarchy lists it.
#[derive(Clone, Debug, Serialize)]
pub struct EntityEntry {
    /// Valid only in the world it was listed from; select by [`Self::key`] instead.
    #[serde(serialize_with = "entity_bits")]
    pub entity: Entity,
    pub path: Option<String>,
    pub source: Option<SpawnSource>,
    pub definition: Option<String>,
    pub stable_id: Option<String>,
    pub position: Option<Vec3>,
    pub rotation: Option<Quat>,
    pub scale: Option<Vec3>,
    pub disabled: bool,
    /// Authored path or stable ID of the master, using the same key as entity inspection.
    pub master: Option<String>,
}

/// Where a zone's spawner or a spawn is authored.
#[derive(Clone, Debug, Serialize)]
pub struct SpawnSource {
    pub file: String,
    pub line: u32,
    /// Field path in the scene file: `spawnerList.<spawner>` or
    /// `spawnerList.<spawner>.spawns.<spawn>`.
    pub path: Vec<String>,
}

fn entity_bits<S: serde::Serializer>(entity: &Entity, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_str(&entity.to_bits())
}

impl EntityEntry {
    fn new(world: &World, entity: Entity) -> Self {
        let transform = world.get::<Transform>(entity);
        let path = world.get::<EntityPath>(entity);
        let source = path.and_then(|path| {
            world
                .resource::<SceneCatalog>()
                .spawners()
                .find_map(|spawner| {
                    let (span, mut fields) = if *path == spawner.path {
                        (&spawner.source, vec![])
                    } else {
                        let spawn = spawner.spawns.iter().find(|spawn| spawn.path == *path)?;
                        (&spawn.source, vec!["spawns".into(), spawn.name.clone()])
                    };
                    fields.splice(0..0, ["spawnerList".into(), spawner.name.clone()]);
                    Some(SpawnSource {
                        file: span.file.to_string(),
                        line: span.start.line,
                        path: fields,
                    })
                })
        });
        Self {
            entity,
            path: path.map(ToString::to_string),
            source,
            definition: world.get::<Definition>(entity).map(|d| d.path.to_string()),
            stable_id: world.get::<StableId>(entity).map(ToString::to_string),
            position: transform.map(|t| t.translation),
            rotation: transform.map(|t| t.rotation),
            scale: transform.map(|t| t.scale),
            disabled: world.get::<Disabled>(entity).is_some(),
            master: world.get::<MasterIs>(entity).and_then(|m| {
                world
                    .get::<EntityPath>(m.0)
                    .map(ToString::to_string)
                    .or_else(|| world.get::<StableId>(m.0).map(ToString::to_string))
            }),
        }
    }

    /// How tools select it: its authored path, or its StableId if it was created at runtime.
    pub fn key(&self) -> &str {
        self.path
            .as_deref()
            .or(self.stable_id.as_deref())
            .unwrap_or_default()
    }

    /// Definition instances, as opposed to zones and spawners.
    pub fn is_instance(&self) -> bool {
        self.definition.is_some()
    }

    /// Named spawns are the entities with an authored offset and overrides.
    pub fn is_named_spawn(&self) -> bool {
        self.source.as_ref().is_some_and(|s| s.path.len() == 4)
    }

    pub fn transform(&self) -> Option<Transform> {
        Some(Transform {
            translation: self.position?,
            rotation: self.rotation?,
            scale: self.scale?,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct EntityInspection {
    pub entity: EntityEntry,
    /// Reflected values by full type path.
    pub components: BTreeMap<String, Value>,
    pub unavailable: Vec<Unavailable>,
}

/// A component present on the entity whose value cannot be shown.
#[derive(Clone, Debug, Serialize)]
pub struct Unavailable {
    pub component: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DefinitionInspection {
    pub definition: String,
    /// The read-only library (`engine`) defining it, if one does. Edits then go to a project
    /// override file, created on the first edit.
    pub library: Option<String>,
    /// The project has its own override of the library definition.
    pub overridden: bool,
    /// Ancestors, nearest first.
    pub lineage: Vec<String>,
    /// Extensors in use: named ones first, then inferred ones.
    pub extensors: Vec<ExtensorEntry>,
    /// Inherited extensors this definition (or an ancestor or preset) dropped.
    pub dropped_extensors: Vec<DroppedEntry>,
    /// Opt-in extensors whose requirements are met, for the author to consider.
    pub suggested_extensors: Vec<SuggestedExtensor>,
    /// Registered extensors the definition does not use.
    pub available_extensors: Vec<String>,
    /// Merged data after inheritance, presets and overrides.
    pub resolved: Value,
    /// Reflected values by full type path.
    pub components: BTreeMap<String, Value>,
}
