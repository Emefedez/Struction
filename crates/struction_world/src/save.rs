//! Saves: the save-data layer only, as versioned JSON.
//!
//! A save holds what definitions and spawners cannot rebuild: the value of persistent components
//! (types registered with [`ReflectPersist`]), stable ids, each instance's definition and path,
//! `masterIs` relations (by path or id), whether an instance is unloaded, and per spawner which
//! spawns it created and which were removed. Grants are derived from the relation and are not
//! saved; nor are `Definition`, `Reactions` or anything else the definition provides.
//!
//! Death, removal and unload stay distinct: death is ordinary persistent state (whatever the
//! `die` action left, such as a `Dead` component), removal is a spawner's `removed` entry (the
//! instance is not saved and never respawns), and an unloaded instance is saved with its state
//! and `unloaded: true`.
//!
//! Loading rebuilds the world from definitions and spawners, then applies the save: the save is
//! authoritative for persistent components (one the save lacks is removed) and for relations.
//! Path aliases recorded by renames are applied first.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::reflect::{FromType, ReflectFromReflect, TypeRegistry};
use serde::de::DeserializeSeed;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use struction_core::{Definition, GrantRecord, MasterIs, StableId, StableIdGenerator};
use struction_data::{DataError, DefinitionStore};
use thiserror::Error;
use uuid::Uuid;

use crate::cells::CellSize;
use crate::identity::{EntityPath, EntityRef, Index};
use crate::rename::PathAliases;
use crate::spawn::{
    PendingMaster, RuntimeCreated, Spawned, Spawner, WorldEntity, WorldErrors, build_world,
    link_masters, run_spawner, spawn_instance,
};

/// Marks a component type as persistent: saves keep its value. Derive with
/// `#[reflect(Component, Persist)]`, or for a foreign type
/// `app.register_type_data::<Transform, ReflectPersist>()`.
#[derive(Clone, Copy, Debug)]
pub struct ReflectPersist;

impl<T: Component> FromType<T> for ReflectPersist {
    fn from_type() -> Self {
        Self
    }
}

pub const SAVE_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SaveData {
    pub version: u32,
    /// Seeds the id generator after loading, so new ids continue a fresh sequence instead of
    /// repeating the ones already in the save.
    pub id_seed: u64,
    /// Spawners that have run, by path.
    pub spawners: BTreeMap<String, SpawnerSave>,
    /// Live and unloaded instances, sorted by id. Removed ones are not here.
    pub entities: Vec<EntitySave>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct SpawnerSave {
    /// Spawn name to instance id.
    pub created: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub removed: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub unloaded: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EntitySave {
    pub id: String,
    /// Path of a named spawn; `None` for runtime-created entities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub definition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub master: Option<SavedRef>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub unloaded: bool,
    /// Persistent components by type path.
    pub components: BTreeMap<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SavedRef {
    Path(String),
    Id(String),
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SaveError {
    #[error("not a readable save: {0}")]
    Malformed(String),
    #[error("save format version {found} is not supported; this build reads version {supported}")]
    UnsupportedVersion { found: u64, supported: u32 },
    #[error("invalid stable id \"{0}\"")]
    InvalidId(String),
    #[error("entity {entity}: definition \"{definition}\" no longer exists")]
    MissingDefinition { entity: String, definition: String },
    #[error("entity {entity}: `{component}` is not a registered persistent component")]
    UnknownComponent { entity: String, component: String },
    #[error("entity {entity}: saved `{component}` does not match the current type: {message}")]
    IncompatibleComponent {
        entity: String,
        component: String,
        message: String,
    },
}

/// Non-fatal differences between a save and the current project.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct LoadReport {
    /// Saved spawners and instances whose spawner or spawn no longer exists; they are dropped.
    pub dropped: Vec<String>,
    /// Instances that could not be rebuilt from their definition.
    pub errors: Vec<DataError>,
}

impl SaveData {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("save data is plain JSON")
    }

    /// Parses a save, checking the version before the layout so an old or newer save fails with
    /// a version error rather than a field error.
    pub fn from_json(text: &str) -> Result<Self, SaveError> {
        let value: Value =
            serde_json::from_str(text).map_err(|e| SaveError::Malformed(e.to_string()))?;
        let version = value
            .get("version")
            .ok_or_else(|| SaveError::Malformed("missing \"version\"".into()))?;
        let found = version
            .as_u64()
            .ok_or_else(|| SaveError::Malformed(format!("\"version\" is {version}")))?;
        if found != u64::from(SAVE_VERSION) {
            return Err(SaveError::UnsupportedVersion {
                found,
                supported: SAVE_VERSION,
            });
        }
        serde_json::from_value(value).map_err(|e| SaveError::Malformed(e.to_string()))
    }
}

fn parse_id(text: &str) -> Result<StableId, SaveError> {
    Uuid::parse_str(text)
        .map(StableId)
        .map_err(|_| SaveError::InvalidId(text.into()))
}

fn is_persistent(types: &TypeRegistry, type_id: TypeId) -> bool {
    types.get_type_data::<ReflectPersist>(type_id).is_some()
        && types.get_type_data::<ReflectComponent>(type_id).is_some()
}

fn persistent_components(world: &World, entity: Entity, types: &TypeRegistry) -> Vec<TypeId> {
    world
        .inspect_entity(entity)
        .into_iter()
        .flatten()
        .filter_map(|info| info.type_id())
        .filter(|&type_id| is_persistent(types, type_id))
        .collect()
}

/// Snapshots the save-data layer. Draws the next id seed, so it needs the world mutably.
pub fn save_world(world: &mut World) -> SaveData {
    let id_seed = {
        let id = world.resource_mut::<StableIdGenerator>().next_id();
        u64::from_le_bytes(id.0.as_bytes()[..8].try_into().expect("16-byte uuid"))
    };

    let mut spawners = BTreeMap::new();
    let mut query =
        world.query_filtered::<(&EntityPath, &Spawner, Has<Disabled>), Allow<Disabled>>();
    for (path, spawner, unloaded) in query.iter(world) {
        if spawner.has_run() {
            spawners.insert(
                path.to_string(),
                SpawnerSave {
                    created: spawner
                        .created()
                        .map(|(name, id)| (name.to_owned(), id.to_string()))
                        .collect(),
                    removed: spawner.removed().map(str::to_owned).collect(),
                    unloaded,
                },
            );
        }
    }

    let types = world.resource::<AppTypeRegistry>().clone();
    let types = types.read();
    let mut query = world.query_filtered::<(
        Entity,
        &StableId,
        &Definition,
        Option<&EntityPath>,
        Option<&MasterIs>,
        Option<&PendingMaster>,
        Has<Disabled>,
        Option<&GrantRecord>,
    ), (Or<(With<Spawned>, With<RuntimeCreated>)>, Allow<Disabled>)>();
    let mut refs =
        world.query_filtered::<(Option<&EntityPath>, Option<&StableId>), Allow<Disabled>>();
    let mut entities = Vec::new();
    for (entity, id, definition, path, master, pending, unloaded, granted) in query.iter(world) {
        let master = match (master, pending) {
            (Some(MasterIs(master)), _) => match refs.get(world, *master) {
                Ok((Some(path), _)) => Some(SavedRef::Path(path.to_string())),
                Ok((None, Some(id))) => Some(SavedRef::Id(id.to_string())),
                _ => {
                    warn!(
                        "{id}: its master {master} is not a world entity; the relation is not saved"
                    );
                    None
                }
            },
            (None, Some(PendingMaster(EntityRef::Path(path)))) => {
                Some(SavedRef::Path(path.to_string()))
            }
            (None, Some(PendingMaster(EntityRef::Id(id)))) => Some(SavedRef::Id(id.to_string())),
            (None, None) => None,
        };
        let entity_ref = world.entity(entity);
        let mut components = BTreeMap::new();
        for type_id in persistent_components(world, entity, &types) {
            // Granted components are derived from the relation.
            if granted.is_some_and(|record| record.components.contains(&type_id)) {
                continue;
            }
            let registration = types.get(type_id).expect("persistent types are registered");
            let reflect = registration.data::<ReflectComponent>().expect("checked");
            let Some(value) = reflect.reflect(entity_ref) else {
                continue;
            };
            let serializer = TypedReflectSerializer::new(value.as_partial_reflect(), &types);
            match serde_json::to_value(serializer) {
                Ok(json) => {
                    components.insert(registration.type_info().type_path().to_owned(), json);
                }
                Err(e) => error!(
                    "{id}: cannot save {}: {e}",
                    registration.type_info().type_path()
                ),
            }
        }
        entities.push(EntitySave {
            id: id.to_string(),
            path: path.map(ToString::to_string),
            definition: definition.path.to_string(),
            master,
            unloaded,
            components,
        });
    }
    entities.sort_by(|a, b| a.id.cmp(&b.id));

    SaveData {
        version: SAVE_VERSION,
        id_seed,
        spawners,
        entities,
    }
}

/// What a save says about one instance, decoded and checked against the current types.
pub(crate) struct SavedState {
    components: Vec<Box<dyn Reflect>>,
    master: Option<EntityRef>,
    unloaded: bool,
}

/// States waiting for their instance to be spawned during a load.
#[derive(Resource, Default)]
pub(crate) struct PendingRestore(HashMap<StableId, SavedState>);

fn decode_ref(saved: &SavedRef) -> Result<EntityRef, SaveError> {
    Ok(match saved {
        SavedRef::Path(path) => EntityRef::Path(EntityPath::new(path)),
        SavedRef::Id(id) => EntityRef::Id(parse_id(id)?),
    })
}

fn decode_state(entity: &EntitySave, types: &TypeRegistry) -> Result<SavedState, SaveError> {
    let mut components = Vec::new();
    for (type_path, json) in &entity.components {
        let unknown = || SaveError::UnknownComponent {
            entity: entity.id.clone(),
            component: type_path.clone(),
        };
        let registration = types.get_with_type_path(type_path).ok_or_else(unknown)?;
        if !is_persistent(types, registration.type_id()) {
            return Err(unknown());
        }
        let incompatible = |message: String| SaveError::IncompatibleComponent {
            entity: entity.id.clone(),
            component: type_path.clone(),
            message,
        };
        let partial = TypedReflectDeserializer::new(registration, types)
            .deserialize(json)
            .map_err(|e| incompatible(e.to_string()))?;
        let value = registration
            .data::<ReflectFromReflect>()
            .and_then(|from| from.from_reflect(&*partial))
            .ok_or_else(|| incompatible("the value cannot be rebuilt".into()))?;
        components.push(value);
    }
    Ok(SavedState {
        components,
        master: entity.master.as_ref().map(decode_ref).transpose()?,
        unloaded: entity.unloaded,
    })
}

/// Applies the saved state of instance `id`, if a load is waiting for it.
pub(crate) fn restore_saved_state(world: &mut World, entity: Entity, id: StableId) {
    let Some(state) = world
        .get_resource_mut::<PendingRestore>()
        .and_then(|mut pending| pending.0.remove(&id))
    else {
        return;
    };
    let types = world.resource::<AppTypeRegistry>().clone();
    let types = types.read();
    let saved: Vec<TypeId> = state
        .components
        .iter()
        .map(|c| c.reflect_type_info().type_id())
        .collect();
    let stale: Vec<TypeId> = persistent_components(world, entity, &types)
        .into_iter()
        .filter(|type_id| !saved.contains(type_id))
        .collect();
    let size = *world.resource::<CellSize>();
    let mut entity_mut = world.entity_mut(entity);
    for type_id in stale {
        types
            .get_type_data::<ReflectComponent>(type_id)
            .expect("persistent types are components")
            .remove(&mut entity_mut);
    }
    for component in &state.components {
        let type_id = component.reflect_type_info().type_id();
        types
            .get_type_data::<ReflectComponent>(type_id)
            .expect("checked when decoding")
            .insert(&mut entity_mut, component.as_partial_reflect(), &types);
    }
    entity_mut.remove::<PendingMaster>();
    if let Some(master) = state.master {
        entity_mut.insert(PendingMaster(master));
    }
    if let Some(transform) = entity_mut.get::<Transform>().copied() {
        entity_mut.insert(size.cell_of(transform.translation));
    }
    if state.unloaded {
        entity_mut.insert(Disabled);
    }
}

/// Despawns every world entity: zones, spawners and instances, loaded or not.
pub fn clear_world(world: &mut World) {
    let mut query = world.query_filtered::<Entity, (With<WorldEntity>, Allow<Disabled>)>();
    let entities: Vec<Entity> = query.iter(world).collect();
    for entity in entities {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
    world.flush();
}

/// Replaces the world entities with the ones a save describes. Nothing changes on error.
pub fn load_world(world: &mut World, save: &SaveData) -> Result<LoadReport, SaveError> {
    if save.version != SAVE_VERSION {
        return Err(SaveError::UnsupportedVersion {
            found: save.version.into(),
            supported: SAVE_VERSION,
        });
    }
    let save = world
        .get_resource::<PathAliases>()
        .map_or_else(|| save.clone(), |aliases| aliases.apply_to_save(save));

    // Decode everything before touching the world.
    let mut states = HashMap::new();
    let mut runtime = Vec::new();
    {
        let types = world.resource::<AppTypeRegistry>().clone();
        let types = types.read();
        let store = world.resource::<DefinitionStore>();
        for entity in &save.entities {
            let id = parse_id(&entity.id)?;
            if states.contains_key(&id) {
                return Err(SaveError::Malformed(format!("duplicate stable id {id}")));
            }
            if store.get(&entity.definition).is_none() {
                return Err(SaveError::MissingDefinition {
                    entity: entity.id.clone(),
                    definition: entity.definition.clone(),
                });
            }
            states.insert(id, decode_state(entity, &types)?);
            if entity.path.is_none() {
                runtime.push((id, entity.definition.clone()));
            }
        }
    }
    let mut spawners = Vec::new();
    let mut authored = HashMap::new();
    for (path, spawner) in &save.spawners {
        let catalog = world.resource::<crate::SceneCatalog>();
        if catalog.spawner(&EntityPath::new(path)).is_none()
            && (catalog.zones().any(|zone| zone.path.as_str() == path)
                || catalog
                    .spawners()
                    .any(|s| s.spawns.iter().any(|spawn| spawn.path.as_str() == path)))
        {
            return Err(SaveError::Malformed(format!("{path} is not a spawner")));
        }
        let created = spawner
            .created
            .iter()
            .map(|(name, id)| Ok((name.clone(), parse_id(id)?)))
            .collect::<Result<BTreeMap<_, _>, SaveError>>()?;
        for (name, id) in &created {
            if spawner.removed.contains(name)
                || authored.insert(*id, format!("{path}/{name}")).is_some()
            {
                return Err(SaveError::Malformed(format!(
                    "conflicting spawn record {path}/{name}"
                )));
            }
        }
        spawners.push((EntityPath::new(path), created, spawner));
    }
    for entity in &save.entities {
        let id = parse_id(&entity.id)?;
        if authored.get(&id) != entity.path.as_ref() {
            return Err(SaveError::Malformed(format!(
                "entity {id} does not match its spawn record"
            )));
        }
    }
    if let Some(id) = authored.keys().find(|id| !states.contains_key(id)) {
        return Err(SaveError::Malformed(format!(
            "spawn record has no entity {id}"
        )));
    }

    clear_world(world);
    world.insert_resource(StableIdGenerator::new(save.id_seed));
    build_world(world);
    world.insert_resource(PendingRestore(states));

    let mut report = LoadReport::default();
    let index = Index::build(world);
    for (path, created, saved) in spawners {
        let Some(entity) = index.get(&EntityRef::Path(path.clone())) else {
            report.dropped.push(format!("spawner {path}"));
            continue;
        };
        world
            .get_mut::<Spawner>(entity)
            .expect("spawner paths index spawners")
            .restore(created, saved.removed.clone());
        report.errors.extend(run_spawner(world, entity));
        if saved.unloaded {
            world.entity_mut(entity).insert(Disabled);
        }
    }
    for (id, definition) in runtime {
        match spawn_instance(world, &definition, None, Transform::IDENTITY) {
            Ok(entity) => {
                world.entity_mut(entity).insert((id, RuntimeCreated));
                restore_saved_state(world, entity, id);
            }
            Err(errors) => report.errors.extend(errors),
        }
    }
    let leftover = world
        .remove_resource::<PendingRestore>()
        .expect("inserted above");
    let mut dropped: Vec<String> = leftover
        .0
        .keys()
        .map(|id| format!("instance {id}"))
        .collect();
    dropped.sort();
    report.dropped.extend(dropped);

    link_masters(world);
    world
        .resource_mut::<WorldErrors>()
        .record(report.errors.iter().cloned());
    Ok(report)
}
