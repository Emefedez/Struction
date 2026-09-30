//! Game-aware authoring: validation, an authored preview and a separate simulation app.
//! The host supplies the game's plugin/component/action registration once through a factory.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::reflect::serde::TypedReflectSerializer;
use bevy::time::{TimePlugin, TimeUpdateStrategy};
use serde::Serialize;
use serde_json::{Value, json};
use struction_core::{ActionRegistry, Definition, MasterIs, StableId};
use struction_data::{DataError, DefinitionStore, ErrorKind};
use struction_world::{
    EntityPath, PathAliases, SceneCatalog, Spawner, WorldEntity, WorldErrors, link_masters,
    run_spawner,
};

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
        return Err(SessionError::Validation(vec![DataError::new(
            ErrorKind::Io("game factory requires CorePlugin, DataPlugin and WorldPlugin".into()),
            None,
        )]));
    }
    // Initializes Startup and the clock without advancing a simulation tick.
    app.update();
    let mut query = app.world_mut().query::<(Entity, &EntityPath, &Spawner)>();
    let mut spawners: Vec<_> = query
        .iter(app.world())
        .map(|(entity, path, _)| (path.clone(), entity))
        .collect();
    spawners.sort();
    for (_, entity) in spawners {
        let errors = run_spawner(app.world_mut(), entity);
        app.world_mut().resource_mut::<WorldErrors>().record(errors);
    }
    link_masters(app.world_mut());
    Ok(app)
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
    pub fn schema(&self) -> Value {
        let types = self.preview.world().resource::<AppTypeRegistry>().read();
        self.preview
            .world()
            .resource::<DefinitionStore>()
            .schema(&types)
    }
    pub fn actions(&self) -> Vec<struction_core::ActionDescriptor> {
        self.preview
            .world()
            .resource::<ActionRegistry>()
            .descriptors()
    }
    pub fn definitions(&self) -> Vec<String> {
        self.preview
            .world()
            .resource::<DefinitionStore>()
            .definitions()
            .map(str::to_owned)
            .collect()
    }
    pub fn inspect_definition(&self, path: &str) -> Result<Value, SessionError> {
        let world = self.preview.world();
        let store = world.resource::<DefinitionStore>();
        let resolved = store.get(path).ok_or_else(|| {
            SessionError::Validation(vec![DataError::new(
                ErrorKind::MissingDefinition(path.into()),
                None,
            )])
        })?;
        let types = world.resource::<AppTypeRegistry>().read();
        let mut components = BTreeMap::new();
        for component in &resolved.components {
            let value = serde_json::to_value(TypedReflectSerializer::new(
                component.value.as_partial_reflect(),
                &types,
            ))
            .map_err(|e| {
                SessionError::Validation(vec![DataError::new(
                    ErrorKind::UnsupportedType(e.to_string()),
                    None,
                )])
            })?;
            components.insert(component.type_path, value);
        }
        Ok(
            json!({"definition":path,"lineage":resolved.lineage,"resolved":resolved.data(),"components":components}),
        )
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
        let world = self.preview.world();
        let applied = self
            .session
            .apply_checked(request, |sources| validate(world, sources))?;
        if !applied.files.is_empty() {
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
    pub fn entities(&self, playing: bool) -> Result<Vec<Value>, SessionError> {
        let world = self.inspected_world(playing)?;
        let Some(mut query) =
            world.try_query_filtered::<Entity, (With<WorldEntity>, Allow<Disabled>)>()
        else {
            return Ok(vec![]);
        };
        let mut entities: Vec<_> = query.iter(world).map(|entity| {
            let transform = world.get::<Transform>(entity);
            let path = world.get::<EntityPath>(entity);
            let source = world.resource::<SceneCatalog>().spawners().find_map(|spawner| {
                if path == Some(&spawner.path) {
                    Some(json!({"file":spawner.source.file.as_ref(),"line":spawner.source.start.line,"path":["spawnerList",spawner.name]}))
                } else {
                    spawner.spawns.iter().find(|spawn| path == Some(&spawn.path)).map(|spawn| json!({"file":spawn.source.file.as_ref(),"line":spawn.source.start.line,"path":["spawnerList",spawner.name,"spawns",spawn.name]}))
                }
            });
            json!({
                "entity":entity.to_bits().to_string(), "path":path.map(EntityPath::as_str), "source":source,
                "definition":world.get::<Definition>(entity).map(|d| d.path.as_str()), "stable_id":world.get::<StableId>(entity).map(ToString::to_string),
                "position":transform.map(|t| t.translation.to_array()),
                "rotation":transform.map(|t| t.rotation.to_array()), "scale":transform.map(|t| t.scale.to_array()),
                "disabled":world.get::<Disabled>(entity).is_some(),
                "master":world.get::<MasterIs>(entity).and_then(|m| world.get::<EntityPath>(m.0)).map(EntityPath::as_str),
            })
        }).collect();
        entities.sort_by(|a, b| {
            a["path"]
                .as_str()
                .cmp(&b["path"].as_str())
                .then(a["stable_id"].as_str().cmp(&b["stable_id"].as_str()))
        });
        Ok(entities)
    }

    /// Reflected component values plus names of unregistered components. `target` is an
    /// authored path or StableId, never a transient ECS entity number.
    pub fn inspect_entity(&self, target: &str, playing: bool) -> Result<Value, SessionError> {
        let world = self.inspected_world(playing)?;
        let entry = self
            .entities(playing)?
            .into_iter()
            .find(|e| e["path"].as_str() == Some(target) || e["stable_id"].as_str() == Some(target))
            .ok_or_else(|| SessionError::InvalidOperation(format!("unknown entity: {target}")))?;
        let entity = Entity::from_bits(entry["entity"].as_str().unwrap().parse().unwrap());
        let types = world.resource::<AppTypeRegistry>().read();
        let mut components = BTreeMap::new();
        let mut unavailable = Vec::new();
        for info in world
            .inspect_entity(entity)
            .expect("entity from this world")
        {
            let registered = info.type_id().and_then(|id| types.get(id));
            let reflected = registered
                .and_then(|r| r.data::<ReflectComponent>())
                .and_then(|r| r.reflect(world.entity(entity)));
            if let Some(value) = reflected {
                match serde_json::to_value(TypedReflectSerializer::new(
                    value.as_partial_reflect(),
                    &types,
                )) {
                    Ok(value) => {
                        components.insert(registered.unwrap().type_info().type_path(), value);
                    }
                    Err(error) => unavailable.push(
                        json!({"component":info.name().to_string(),"reason":error.to_string()}),
                    ),
                }
            } else {
                unavailable.push(json!({"component":info.name().to_string(),"reason":"component has no registered reflection"}));
            }
        }
        Ok(json!({"entity":entry,"components":components,"unavailable":unavailable}))
    }
}
