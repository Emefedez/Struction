//! Zones, spawners and the instances they create.
//!
//! [`build_world`] creates one entity per zone and per spawner from the [`SceneCatalog`]. A
//! spawner runs once, the first `FixedUpdate` it is loaded ([`WorldSet::Spawn`]), creating its
//! spawns through `DefinitionStore::instantiate`. Instances are placed in world space without a
//! transform parent, so they move independently of the spawner and of each other.
//!
//! Every instance gets a [`StableId`] from core's seeded generator; named spawns also get their
//! [`EntityPath`]. A spawner records which ids it created, and which spawns were removed
//! (despawned), so a save restores the former and never brings back the latter.
//!
//! `masterIs` becomes a [`PendingMaster`] until its target exists: the master may spawn later,
//! from another spawner or file. [`link_masters`] turns it into core's `MasterIs`, which applies
//! the master's grants.
//!
//! [`WorldSet::Spawn`]: crate::WorldSet::Spawn

use std::collections::{BTreeMap, BTreeSet};

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::lifecycle::Despawn;
use bevy::ecs::query::Allow;
use bevy::prelude::*;
use struction_core::{ActionRegistry, MasterIs, StableId, StableIdGenerator};
use struction_data::{DataError, DefinitionStore, Node};

use crate::cells::CellSize;
use crate::identity::{EntityPath, EntityRef, Index};
use crate::scene::SceneCatalog;

/// Marks everything the world builds or restores: zones, spawners, instances. Loading a save
/// replaces exactly these entities.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct WorldEntity;

/// A named zone. Its [`EntityPath`] is the zone path.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Zone;

/// A spawner and its bookkeeping. Internal to saves: gameplay does not care who created an
/// entity (if it matters, the spawner assigns `masterIs`).
#[derive(Component, Clone, Debug, Default)]
pub struct Spawner {
    ran: bool,
    created: BTreeMap<String, StableId>,
    removed: BTreeSet<String>,
}

impl Spawner {
    pub fn has_run(&self) -> bool {
        self.ran
    }

    /// Spawn name to the id of the live (or unloaded) instance it created.
    pub fn created(&self) -> impl Iterator<Item = (&str, StableId)> {
        self.created.iter().map(|(name, id)| (name.as_str(), *id))
    }

    /// Spawns whose instance was removed; they are not spawned again.
    pub fn removed(&self) -> impl Iterator<Item = &str> {
        self.removed.iter().map(String::as_str)
    }

    pub(crate) fn restore(
        &mut self,
        created: BTreeMap<String, StableId>,
        removed: BTreeSet<String>,
    ) {
        self.created = created;
        self.removed = removed;
    }
}

/// An instance created by a spawner.
#[derive(Component, Clone, Debug)]
pub struct Spawned {
    pub spawner: Entity,
    /// Key of the spawn in the spawner.
    pub spawn: String,
}

/// An instance created at runtime (not by a spawner), identified only by its [`StableId`].
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct RuntimeCreated;

/// A `masterIs` waiting for its target to exist.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct PendingMaster(pub EntityRef);

/// Problems found while building, spawning or loading. They are also logged.
#[derive(Resource, Default, Debug)]
pub struct WorldErrors(Vec<DataError>);

impl WorldErrors {
    pub fn record(&mut self, errors: impl IntoIterator<Item = DataError>) {
        for error in errors {
            error!("{error}");
            self.0.push(error);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &DataError> {
        self.0.iter()
    }

    pub fn drain(&mut self) -> Vec<DataError> {
        std::mem::take(&mut self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Spawns the zone and spawner entities of the [`SceneCatalog`]. Spawners start pending.
pub fn build_world(world: &mut World) {
    let size = *world.resource::<CellSize>();
    let catalog = world.resource::<SceneCatalog>();
    let zones: Vec<_> = catalog
        .zones()
        .map(|zone| (zone.path.clone(), zone.transform()))
        .collect();
    let spawners: Vec<_> = catalog
        .spawners()
        .map(|spawner| {
            let zone = catalog.zone_transform(&spawner.zone);
            (spawner.path.clone(), spawner.world_transform(&zone))
        })
        .collect();
    for (path, transform) in zones {
        world.spawn((
            WorldEntity,
            Zone,
            Name::new(path.to_string()),
            path,
            transform,
        ));
    }
    for (path, transform) in spawners {
        world.spawn((
            WorldEntity,
            Spawner::default(),
            Name::new(path.to_string()),
            path,
            transform,
            size.cell_of(transform.translation),
        ));
    }
}

/// Creates an instance of `definition` with scene `overrides`, placed at `placement` (the
/// definition's own transform, such as its scale, is applied under it). Adds the definition's
/// `GrantsToWards` and `Reactions`. The caller gives it an identity.
pub fn spawn_instance(
    world: &mut World,
    definition: &str,
    overrides: Option<&Node>,
    placement: Transform,
) -> Result<Entity, Vec<DataError>> {
    let types = world.resource::<AppTypeRegistry>().clone();
    let types = types.read();
    let resolved = world
        .resource::<DefinitionStore>()
        .instantiate(definition, overrides, &types)?;
    let actions = world.resource::<ActionRegistry>();
    let grants = resolved.grants_to_wards(&types, actions)?;
    let reactions = resolved.reactions(actions)?;
    let transform = placement.mul_transform(
        resolved
            .component::<Transform>()
            .copied()
            .unwrap_or_default(),
    );
    let cell = world.resource::<CellSize>().cell_of(transform.translation);

    let mut entity = world.spawn((WorldEntity, cell));
    resolved.insert_into(&mut entity, &types);
    entity.insert(transform);
    if let Some(grants) = grants {
        entity.insert(grants);
    }
    if let Some(reactions) = reactions {
        entity.insert(reactions);
    }
    Ok(entity.id())
}

/// Creates a runtime entity, identified by a fresh [`StableId`]. Saves restore it until it is
/// despawned.
pub fn spawn_runtime(
    world: &mut World,
    definition: &str,
    placement: Transform,
) -> Result<Entity, Vec<DataError>> {
    let entity = spawn_instance(world, definition, None, placement)?;
    let id = world.resource_mut::<StableIdGenerator>().next_id();
    world.entity_mut(entity).insert((id, RuntimeCreated));
    Ok(entity)
}

/// Runs a spawner that has not run yet: creates its spawns, except removed ones, reusing the
/// ids it already recorded (after a load) or drawing new ones.
pub fn run_spawner(world: &mut World, spawner: Entity) -> Vec<DataError> {
    let (Some(path), Some(state)) = (
        world.get::<EntityPath>(spawner).cloned(),
        world.get::<Spawner>(spawner).cloned(),
    ) else {
        return Vec::new();
    };
    if state.ran {
        return Vec::new();
    }
    let catalog = world.resource::<SceneCatalog>();
    let Some(def) = catalog.spawner(&path).cloned() else {
        return Vec::new();
    };
    let zone = catalog.zone_transform(&def.zone);

    let mut errors = Vec::new();
    let mut created = BTreeMap::new();
    for spawn in def
        .spawns
        .iter()
        .filter(|s| !state.removed.contains(&s.name))
    {
        let placement = def.spawn_transform(&zone, spawn);
        let entity = match spawn_instance(
            world,
            &spawn.definition,
            spawn.overrides.as_ref(),
            placement,
        ) {
            Ok(entity) => entity,
            Err(e) => {
                errors.extend(e);
                continue;
            }
        };
        let id = match state.created.get(&spawn.name) {
            Some(id) => *id,
            None => world.resource_mut::<StableIdGenerator>().next_id(),
        };
        created.insert(spawn.name.clone(), id);
        let mut instance = world.entity_mut(entity);
        instance.insert((
            id,
            Name::new(spawn.path.to_string()),
            spawn.path.clone(),
            Spawned {
                spawner,
                spawn: spawn.name.clone(),
            },
        ));
        if let Some(master) = &spawn.master_is {
            instance.insert(PendingMaster(EntityRef::Path(master.clone())));
        }
        crate::save::restore_saved_state(world, entity, id);
    }
    let mut state = world.get_mut::<Spawner>(spawner).expect("checked above");
    state.created = created;
    state.ran = true;
    errors
}

/// Runs loaded spawners that have not run yet, in path order so ids replay deterministically.
pub fn run_pending_spawners(world: &mut World) {
    let mut query = world.query::<(Entity, &EntityPath, &Spawner)>();
    let mut pending: Vec<_> = query
        .iter(world)
        .filter(|(_, _, spawner)| !spawner.ran)
        .map(|(entity, path, _)| (path.clone(), entity))
        .collect();
    pending.sort();
    for (_, spawner) in pending {
        let errors = run_spawner(world, spawner);
        world.resource_mut::<WorldErrors>().record(errors);
    }
}

/// Turns every resolvable [`PendingMaster`] into core's `MasterIs`.
pub fn link_masters(world: &mut World) {
    let mut query = world.query_filtered::<(Entity, &PendingMaster), Allow<Disabled>>();
    let pending: Vec<_> = query
        .iter(world)
        .map(|(entity, pending)| (entity, pending.0.clone()))
        .collect();
    if pending.is_empty() {
        return;
    }
    let index = Index::build(world);
    for (ward, target) in pending {
        if let Some(master) = index.get(&target)
            && master != ward
        {
            world
                .entity_mut(ward)
                .remove::<PendingMaster>()
                .insert(MasterIs(master));
        }
    }
    world.flush();
}

/// Despawning an instance is removal: its spawner remembers not to spawn it again.
pub(crate) fn record_removal(
    event: On<Despawn, Spawned>,
    spawned: Query<&Spawned, Allow<Disabled>>,
    mut spawners: Query<&mut Spawner, Allow<Disabled>>,
) {
    let Ok(spawned) = spawned.get(event.entity) else {
        return;
    };
    if let Ok(mut spawner) = spawners.get_mut(spawned.spawner) {
        spawner.created.remove(&spawned.spawn);
        spawner.removed.insert(spawned.spawn.clone());
    }
}
