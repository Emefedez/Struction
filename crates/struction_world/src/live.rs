//! Live reload: applies saved sources to a running world without restarting it.
//!
//! [`reload_sources`] re-reads changed files and refreshes what depends on them in place. An
//! instance keeps its runtime state: [`Authored`] remembers what it was built from, and only the
//! fields whose authored value changed are written, so raising `Health.max` in a file does not
//! heal a wounded ogre. A spawn whose authored placement changed (the editor moved it) is placed
//! again; everything else keeps its runtime transform.
//!
//! [`LiveReloadPlugin`] polls the project for saved files, so an edit from the editor, an AI tool
//! or a text editor reaches a running game. Changes the running world cannot absorb (a new spawn
//! in a spawner that already ran) are reported, not guessed at.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;
use bevy::reflect::{ReflectMut, ReflectRef, TypeRegistry};
use struction_core::ActionRegistry;
use struction_data::{DefinitionStore, Node, Resolved, reload_definition_file};

use crate::cells::CellSize;
use crate::identity::EntityPath;
use crate::scene::{SCENES_DIR, SceneCatalog};
use crate::spawn::{RuntimeCreated, Spawned, Spawner, WorldEntity, WorldErrors, Zone};

/// Polls the project directory and applies saved sources through [`reload_sources`] in
/// [`WorldSet::Reload`](crate::WorldSet::Reload), between fixed ticks. Development only: add it
/// after `WorldPlugin`.
pub struct LiveReloadPlugin {
    /// How often the project directory is scanned.
    pub interval: Duration,
}

impl Default for LiveReloadPlugin {
    fn default() -> Self {
        Self {
            interval: Duration::from_millis(250),
        }
    }
}

impl Plugin for LiveReloadPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(LiveReload {
            interval: self.interval,
            next_scan: None,
            stamps: BTreeMap::new(),
        })
        .add_message::<LiveReloaded>()
        .add_systems(First, poll_sources.in_set(crate::WorldSet::Reload));
    }
}

/// Polling state. Its presence also makes new instances record [`Authored`].
#[derive(Resource)]
pub struct LiveReload {
    interval: Duration,
    next_scan: Option<Instant>,
    stamps: BTreeMap<PathBuf, (SystemTime, u64)>,
}

/// What an instance was built from: its authored components before runtime changed them.
#[derive(Component)]
pub struct Authored {
    components: Vec<(TypeId, Box<dyn PartialReflect>)>,
    /// The definition's own transform, applied under the placement.
    local: Transform,
}

impl Authored {
    pub(crate) fn new(resolved: &Resolved) -> Self {
        Self {
            components: resolved
                .components
                .iter()
                .filter(|c| c.type_id != TypeId::of::<Transform>())
                .map(|c| (c.type_id, c.value.to_dynamic()))
                .collect(),
            local: resolved
                .component::<Transform>()
                .copied()
                .unwrap_or_default(),
        }
    }

    fn get(&self, type_id: TypeId) -> Option<&dyn PartialReflect> {
        self.components
            .iter()
            .find_map(|(id, value)| (*id == type_id).then_some(&**value))
    }
}

/// What one [`reload_sources`] call changed, also sent as a message by [`LiveReloadPlugin`].
#[derive(Message, Clone, Debug, Default, PartialEq, Eq)]
pub struct LiveReloaded {
    /// Project-relative source files that were applied.
    pub files: Vec<String>,
    /// Definitions whose resolved data changed (descendants included).
    pub definitions: Vec<String>,
    /// Instances that had at least one field rewritten.
    pub refreshed: usize,
    /// Instances placed again because their authored placement changed.
    pub moved: usize,
    /// The instances counted in `moved`. Their new `Transform` is authoritative; a host that
    /// drives transforms from elsewhere (physics) carries it over.
    pub placed: Vec<Entity>,
    /// Instances removed because their spawn or spawner is gone.
    pub despawned: usize,
    /// Changes that need a restart to show up, such as a spawn added to a spawner that already
    /// ran.
    pub restart_needed: Vec<String>,
}

impl LiveReloaded {
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
            && self.refreshed == 0
            && self.moved == 0
            && self.despawned == 0
            && self.restart_needed.is_empty()
    }
}

fn scan(root: &Path, into: &mut BTreeMap<PathBuf, (SystemTime, u64)>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan(&path, into);
        } else if path.extension().is_some_and(|e| e == "jsonc")
            && let Ok(meta) = entry.metadata()
        {
            let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            into.insert(path, (modified, meta.len()));
        }
    }
}

fn poll_sources(world: &mut World) {
    let root = world.resource::<DefinitionStore>().root().to_owned();
    let now = Instant::now();
    let mut state = world.resource_mut::<LiveReload>();
    if state.next_scan.is_some_and(|next| now < next) {
        return;
    }
    let first = state.next_scan.is_none();
    state.next_scan = Some(now + state.interval);
    let mut stamps = BTreeMap::new();
    scan(&root, &mut stamps);
    let previous = std::mem::replace(&mut state.stamps, stamps.clone());
    if first {
        return;
    }
    let changed: Vec<PathBuf> = stamps
        .iter()
        .filter(|(path, stamp)| previous.get(*path) != Some(stamp))
        .map(|(path, _)| path.clone())
        .chain(
            previous
                .keys()
                .filter(|path| !stamps.contains_key(*path))
                .cloned(),
        )
        .collect();
    if changed.is_empty() {
        return;
    }
    let report = reload_sources(world, &changed);
    info!(
        "live reload: {:?} changed {} definitions, refreshed {}, moved {}, despawned {}",
        report.files,
        report.definitions.len(),
        report.refreshed,
        report.moved,
        report.despawned
    );
    for item in &report.restart_needed {
        warn!("live reload needs a restart: {item}");
    }
    world.write_message(report);
}

/// Applies changed source files (absolute, or relative to the project root; deleted files
/// included) to the running world. This is what [`LiveReloadPlugin`] calls after a save, and what
/// a tool that knows which files it wrote can call directly. Problems go to [`WorldErrors`]; a
/// definition that no longer resolves keeps its last good version.
pub fn reload_sources(world: &mut World, files: &[PathBuf]) -> LiveReloaded {
    let root = world.resource::<DefinitionStore>().root().to_owned();
    let mut report = LiveReloaded::default();
    let mut changed = BTreeSet::new();
    let mut scenes = false;
    for file in files {
        let full = if file.is_absolute() {
            file.clone()
        } else {
            root.join(file)
        };
        let Ok(rel) = full.strip_prefix(&root) else {
            continue;
        };
        let rel = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if rel.starts_with(&format!("{SCENES_DIR}/")) {
            scenes = true;
        } else {
            let reloaded = reload_definition_file(world, &full);
            world.resource_mut::<WorldErrors>().record(reloaded.errors);
            changed.extend(reloaded.changed);
            changed.extend(reloaded.removed);
        }
        report.files.push(rel);
    }
    report.definitions = changed.iter().cloned().collect();
    if !scenes && changed.is_empty() {
        return report;
    }

    // The catalog validates spawns against definitions, so it is rebuilt either way.
    let old = world.remove_resource::<SceneCatalog>().unwrap_or_default();
    let types = world.resource::<AppTypeRegistry>().clone();
    let catalog = SceneCatalog::load(&root, world.resource::<DefinitionStore>(), &types.read());
    world
        .resource_mut::<WorldErrors>()
        .record(catalog.errors().iter().cloned());
    world.insert_resource(catalog);

    if scenes {
        sync_zones(world);
        sync_spawners(world, &old, &mut report);
    }
    refresh_instances(world, &old, &changed, scenes, &mut report);
    world.flush();
    report
}

fn sync_zones(world: &mut World) {
    let catalog = world.resource::<SceneCatalog>();
    let zones: Vec<_> = catalog
        .zones()
        .map(|zone| (zone.path.clone(), zone.transform()))
        .collect();
    let mut query = world.query_filtered::<(Entity, &EntityPath), (With<Zone>, Allow<Disabled>)>();
    let live: BTreeMap<EntityPath, Entity> = query
        .iter(world)
        .map(|(entity, path)| (path.clone(), entity))
        .collect();
    for (path, transform) in zones {
        match live.get(&path) {
            Some(&entity) => {
                world.entity_mut(entity).insert(transform);
            }
            None => {
                world.spawn((
                    WorldEntity,
                    Zone,
                    Name::new(path.to_string()),
                    path,
                    transform,
                ));
            }
        }
    }
}

/// Moves spawners, despawns what the scene no longer has and adds new spawners, which run on the
/// next tick like any loaded spawner.
fn sync_spawners(world: &mut World, old: &SceneCatalog, report: &mut LiveReloaded) {
    let size = *world.resource::<CellSize>();
    let mut query = world.query_filtered::<(Entity, &EntityPath, &Spawner), Allow<Disabled>>();
    let live: BTreeMap<EntityPath, (Entity, bool)> = query
        .iter(world)
        .map(|(entity, path, spawner)| (path.clone(), (entity, spawner.has_run())))
        .collect();
    let mut instances = world.query_filtered::<(Entity, &Spawned), Allow<Disabled>>();
    let spawned: Vec<(Entity, Entity, String)> = instances
        .iter(world)
        .map(|(entity, s)| (entity, s.spawner, s.spawn.clone()))
        .collect();

    let catalog = world.resource::<SceneCatalog>();
    let mut despawn = Vec::new();
    let mut place = Vec::new();
    let mut add = Vec::new();
    for (path, &(entity, ran)) in &live {
        let Some(def) = catalog.spawner(path) else {
            despawn.push(entity);
            despawn.extend(
                spawned
                    .iter()
                    .filter(|(_, spawner, _)| *spawner == entity)
                    .map(|(instance, _, _)| *instance),
            );
            continue;
        };
        let zone = catalog.zone_transform(&def.zone);
        place.push((entity, def.world_transform(&zone)));
        if !ran {
            continue;
        }
        let known: BTreeSet<&str> = old
            .spawner(path)
            .map(|d| d.spawns.iter().map(|s| s.name.as_str()).collect())
            .unwrap_or_default();
        for spawn in def
            .spawns
            .iter()
            .filter(|s| !known.contains(s.name.as_str()))
        {
            report.restart_needed.push(format!(
                "new spawn {} in a spawner that already ran",
                spawn.path
            ));
        }
        for (instance, _, name) in spawned.iter().filter(|(_, s, _)| *s == entity) {
            if !def.spawns.iter().any(|s| &s.name == name) {
                despawn.push(*instance);
            }
        }
    }
    for def in catalog.spawners().filter(|d| !live.contains_key(&d.path)) {
        let zone = catalog.zone_transform(&def.zone);
        add.push((def.path.clone(), def.world_transform(&zone)));
    }

    for (entity, transform) in place {
        world
            .entity_mut(entity)
            .insert((transform, size.cell_of(transform.translation)));
    }
    for entity in despawn {
        if world.get::<Spawned>(entity).is_some() {
            report.despawned += 1;
        }
        world.despawn(entity);
    }
    for (path, transform) in add {
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

struct Target {
    entity: Entity,
    definition: String,
    overrides: Option<Node>,
    /// The new world placement, when the authored one changed.
    placement: Option<Transform>,
}

fn refresh_instances(
    world: &mut World,
    old: &SceneCatalog,
    changed: &BTreeSet<String>,
    scenes: bool,
    report: &mut LiveReloaded,
) {
    let mut targets = Vec::new();
    let mut spawned = world.query_filtered::<(Entity, &Spawned), Allow<Disabled>>();
    let spawned: Vec<(Entity, Entity, String)> = spawned
        .iter(world)
        .map(|(entity, s)| (entity, s.spawner, s.spawn.clone()))
        .collect();
    let catalog = world.resource::<SceneCatalog>();
    for (entity, spawner, name) in spawned {
        let Some(path) = world.get::<EntityPath>(spawner) else {
            continue;
        };
        let Some(def) = catalog.spawner(path) else {
            continue;
        };
        let Some(spawn) = def.spawns.iter().find(|s| s.name == name) else {
            continue;
        };
        let placement = old.spawner(path).and_then(|old_def| {
            let old_spawn = old_def.spawns.iter().find(|s| s.name == name)?;
            let before = old_def.spawn_transform(&old.zone_transform(&old_def.zone), old_spawn);
            let after = def.spawn_transform(&catalog.zone_transform(&def.zone), spawn);
            (before != after).then_some(after)
        });
        if scenes || changed.contains(&spawn.definition) {
            targets.push(Target {
                entity,
                definition: spawn.definition.clone(),
                overrides: spawn.overrides.clone(),
                placement,
            });
        }
    }
    let mut runtime = world.query_filtered::<
        (Entity, &struction_core::Definition),
        (With<RuntimeCreated>, Allow<Disabled>),
    >();
    for (entity, definition) in runtime.iter(world) {
        if changed.contains(definition.path.as_str()) {
            targets.push(Target {
                entity,
                definition: definition.path.as_str().to_owned(),
                overrides: None,
                placement: None,
            });
        }
    }

    let types = world.resource::<AppTypeRegistry>().clone();
    let types = types.read();
    for target in targets {
        let resolved = world.resource::<DefinitionStore>().instantiate(
            &target.definition,
            target.overrides.as_ref(),
            &types,
        );
        match resolved {
            Ok(resolved) => refresh(world, &target, &resolved, &types, report),
            Err(errors) => world.resource_mut::<WorldErrors>().record(errors),
        }
    }
}

/// Writes the fields whose authored value changed since the instance was built, and places it
/// again if its authored placement moved.
fn refresh(
    world: &mut World,
    target: &Target,
    resolved: &Resolved,
    types: &TypeRegistry,
    report: &mut LiveReloaded,
) {
    let actions = world.resource::<ActionRegistry>();
    let relations = resolved
        .grants_to_wards(types, actions)
        .and_then(|grants| Ok((grants, resolved.reactions(actions)?)));
    let (grants, reactions) = match relations {
        Ok(relations) => relations,
        Err(errors) => {
            world.resource_mut::<WorldErrors>().record(errors);
            return;
        }
    };
    let size = *world.resource::<CellSize>();
    let mut entity = world.entity_mut(target.entity);
    let old = entity.take::<Authored>();
    let new = Authored::new(resolved);
    let mut touched = false;
    for (type_id, value) in &new.components {
        let before = old.as_ref().and_then(|old| old.get(*type_id));
        if before.is_some_and(|before| before.reflect_partial_eq(&**value) == Some(true)) {
            continue;
        }
        let reflect = types
            .get_type_data::<ReflectComponent>(*type_id)
            .expect("checked when the component was built");
        touched = true;
        match (before, reflect.reflect_mut(&mut entity)) {
            (Some(before), Some(mut live)) => {
                patch(live.as_partial_reflect_mut(), before, &**value)
            }
            _ => reflect.insert(&mut entity, &**value, types),
        }
    }
    if let Some(old) = &old {
        for (type_id, _) in &old.components {
            if new.get(*type_id).is_none()
                && let Some(reflect) = types.get_type_data::<ReflectComponent>(*type_id)
            {
                reflect.remove(&mut entity);
                touched = true;
            }
        }
    }

    let current = entity.get::<Transform>().copied().unwrap_or_default();
    let old_local = old.as_ref().map_or(new.local, |old| old.local);
    let transform = match target.placement {
        Some(placement) => Some(placement.mul_transform(new.local)),
        None if old_local != new.local => {
            let placement =
                Transform::from_matrix(current.to_matrix() * old_local.to_matrix().inverse());
            Some(placement.mul_transform(new.local))
        }
        None => None,
    };
    if let Some(transform) = transform {
        entity.insert((transform, size.cell_of(transform.translation)));
        if target.placement.is_some() {
            report.moved += 1;
            report.placed.push(target.entity);
        } else {
            touched = true;
        }
    }
    entity.insert((resolved.definition(), new));
    if let Some(grants) = grants {
        entity.insert(grants);
    }
    if let Some(reactions) = reactions {
        entity.insert(reactions);
    }
    if touched {
        report.refreshed += 1;
    }
}

/// Applies `new` over `live` where it differs from `before`, field by field through structs, so
/// fields the source did not change keep their runtime values.
fn patch(live: &mut dyn PartialReflect, before: &dyn PartialReflect, new: &dyn PartialReflect) {
    if let (ReflectMut::Struct(live), ReflectRef::Struct(before), ReflectRef::Struct(new)) =
        (live.reflect_mut(), before.reflect_ref(), new.reflect_ref())
    {
        for index in 0..new.field_len() {
            let (Some(name), Some(value)) = (new.name_at(index), new.field_at(index)) else {
                continue;
            };
            let Some(field) = live.field_mut(name) else {
                continue;
            };
            match before.field(name) {
                Some(previous) if previous.reflect_partial_eq(value) == Some(true) => {}
                Some(previous) => patch(field, previous, value),
                None => {
                    let _ = field.try_apply(value);
                }
            }
        }
        return;
    }
    if let Err(error) = live.try_apply(new) {
        warn!("live reload could not apply a value: {error}");
    }
}
