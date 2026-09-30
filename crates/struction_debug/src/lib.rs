//! Opt-in, headless simulation traces. Add [`DebugTracePlugin`], register components with
//! [`TraceAppExt::trace_component`], and mark entities with [`TraceEntity`] (entities carrying
//! `StableId` are included automatically). Capture runs in `FixedLast`, after simulation.
//!
//! Tools can drain [`TraceLog`] or install a [`TraceWriter`] for JSON Lines output. The trace
//! records snapshots and before/after component values, including addition/removal. It observes
//! only registered components; register simulation `Position`, not interpolated `Transform`.
//! Events describe end-of-tick state: changes that cancel within one tick are not recorded.

use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::Write;

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::reflect::ReflectComponent;
use bevy::prelude::*;
use bevy::reflect::{GetTypeRegistration, serde::TypedReflectSerializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use struction_core::{Definition, StableId};

pub const TRACE_VERSION: u32 = 1;

#[derive(Component, Default)]
pub struct TraceEntity;

#[derive(Resource)]
pub struct TraceSettings {
    pub enabled: bool,
    /// Maximum number of recent events retained in memory. File output keeps every event.
    pub capacity: usize,
}

impl Default for TraceSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            capacity: 1024,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TraceIdentity {
    /// Bevy's generation-bearing entity bits: unique within this run.
    pub entity: u64,
    pub stable_id: Option<String>,
    pub name: Option<String>,
    pub definition: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ValueChange {
    pub before: Option<Value>,
    pub after: Option<Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceChange {
    Spawned {
        active: bool,
        components: BTreeMap<String, Value>,
    },
    Changed {
        components: BTreeMap<String, ValueChange>,
        #[serde(skip_serializing_if = "Option::is_none")]
        active: Option<bool>,
    },
    Removed,
    Untracked,
    Error {
        component: String,
        message: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct TraceEvent {
    pub version: u32,
    /// Fixed ticks since tracing was installed, starting at 1.
    pub tick: u64,
    pub identity: TraceIdentity,
    #[serde(flatten)]
    pub change: TraceChange,
}

#[derive(Resource, Default)]
pub struct TraceLog(VecDeque<TraceEvent>);

impl TraceLog {
    pub fn iter(&self) -> impl Iterator<Item = &TraceEvent> {
        self.0.iter()
    }
    pub fn drain(&mut self) -> impl Iterator<Item = TraceEvent> + '_ {
        self.0.drain(..)
    }
}

/// Optional streaming sink. A failed write disables the sink and stores a diagnostic; capture
/// and the bounded in-memory log continue. Files should be opened explicitly by the application.
#[derive(Resource)]
pub struct TraceWriter {
    writer: Box<dyn Write + Send + Sync>,
    error: Option<String>,
}

impl TraceWriter {
    pub fn new(writer: impl Write + Send + Sync + 'static) -> Self {
        Self {
            writer: Box::new(writer),
            error: None,
        }
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
}

#[derive(Resource, Default)]
struct TrackedComponents(Vec<TypeId>);

pub trait TraceAppExt {
    fn trace_component<C: Component + Reflect + TypePath + GetTypeRegistration>(
        &mut self,
    ) -> &mut Self;
}

impl TraceAppExt for App {
    fn trace_component<C: Component + Reflect + TypePath + GetTypeRegistration>(
        &mut self,
    ) -> &mut Self {
        self.register_type::<C>()
            .register_type_data::<C, ReflectComponent>()
            .init_resource::<TrackedComponents>();
        let mut components = self.world_mut().resource_mut::<TrackedComponents>();
        if !components.0.contains(&TypeId::of::<C>()) {
            components.0.push(TypeId::of::<C>());
        }
        self
    }
}

#[derive(Clone)]
struct Snapshot {
    identity: TraceIdentity,
    active: bool,
    components: BTreeMap<String, Value>,
}

#[derive(Resource, Default)]
struct TraceState {
    tick: u64,
    snapshots: BTreeMap<u64, Snapshot>,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TraceSystems;

pub struct DebugTracePlugin;

impl Plugin for DebugTracePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TraceSettings>()
            .init_resource::<TraceLog>()
            .init_resource::<TrackedComponents>()
            .init_resource::<TraceState>()
            .configure_sets(FixedLast, TraceSystems)
            .add_systems(FixedLast, capture.in_set(TraceSystems));
    }
}

fn capture(world: &mut World) {
    let tick = {
        let mut state = world.resource_mut::<TraceState>();
        state.tick += 1;
        state.tick
    };
    if !world.resource::<TraceSettings>().enabled {
        world.resource_mut::<TraceState>().snapshots.clear();
        return;
    }
    let types = world.resource::<AppTypeRegistry>().clone();
    let types = types.read();
    let tracked = world.resource::<TrackedComponents>().0.clone();
    let mut query = world
        .query_filtered::<Entity, (Or<(With<TraceEntity>, With<StableId>)>, Allow<Disabled>)>();
    let mut entities: Vec<_> = query.iter(world).collect();
    entities.sort_by_key(|entity| entity.to_bits());
    let mut snapshots = BTreeMap::new();
    let mut events = Vec::new();
    for entity in entities {
        let entity_ref = world.entity(entity);
        let identity = TraceIdentity {
            entity: entity.to_bits(),
            stable_id: entity_ref.get::<StableId>().map(ToString::to_string),
            name: entity_ref
                .get::<Name>()
                .map(|name| name.as_str().to_owned()),
            definition: entity_ref
                .get::<Definition>()
                .map(|definition| definition.path.to_string()),
        };
        let mut components = BTreeMap::new();
        for type_id in &tracked {
            let registration = types
                .get(*type_id)
                .expect("trace_component registered the type");
            let reflect = registration
                .data::<ReflectComponent>()
                .expect("registered as a component");
            let Some(value) = reflect.reflect(entity_ref) else {
                continue;
            };
            let component = registration.type_info().type_path().to_owned();
            match serde_json::to_value(TypedReflectSerializer::new(
                value.as_partial_reflect(),
                &types,
            )) {
                Ok(value) => {
                    components.insert(component, value);
                }
                Err(error) => events.push(TraceEvent {
                    version: TRACE_VERSION,
                    tick,
                    identity: identity.clone(),
                    change: TraceChange::Error {
                        component,
                        message: error.to_string(),
                    },
                }),
            }
        }
        snapshots.insert(
            entity.to_bits(),
            Snapshot {
                identity,
                active: !entity_ref.contains::<Disabled>(),
                components,
            },
        );
    }
    drop(types);
    let previous = std::mem::take(&mut world.resource_mut::<TraceState>().snapshots);
    for (entity, snapshot) in &snapshots {
        let change = if let Some(before) = previous.get(entity) {
            let keys: BTreeSet<_> = before
                .components
                .keys()
                .chain(snapshot.components.keys())
                .collect();
            let components: BTreeMap<_, _> = keys
                .into_iter()
                .filter_map(|key| {
                    let old = before.components.get(key);
                    let new = snapshot.components.get(key);
                    (old != new).then(|| {
                        (
                            key.clone(),
                            ValueChange {
                                before: old.cloned(),
                                after: new.cloned(),
                            },
                        )
                    })
                })
                .collect();
            let active = (before.active != snapshot.active).then_some(snapshot.active);
            if components.is_empty() && active.is_none() && before.identity == snapshot.identity {
                continue;
            }
            TraceChange::Changed { components, active }
        } else {
            TraceChange::Spawned {
                active: snapshot.active,
                components: snapshot.components.clone(),
            }
        };
        events.push(TraceEvent {
            version: TRACE_VERSION,
            tick,
            identity: snapshot.identity.clone(),
            change,
        });
    }
    for (entity, snapshot) in previous {
        if !snapshots.contains_key(&entity) {
            let change = if world.get_entity(Entity::from_bits(entity)).is_ok() {
                TraceChange::Untracked
            } else {
                TraceChange::Removed
            };
            events.push(TraceEvent {
                version: TRACE_VERSION,
                tick,
                identity: snapshot.identity,
                change,
            });
        }
    }
    world.resource_mut::<TraceState>().snapshots = snapshots;
    if let Some(mut sink) = world.get_resource_mut::<TraceWriter>()
        && sink.error.is_none()
    {
        let result = (|| -> Result<(), Box<dyn std::error::Error>> {
            for event in &events {
                serde_json::to_writer(&mut sink.writer, event)?;
                sink.writer.write_all(b"\n")?;
            }
            sink.writer.flush()?;
            Ok(())
        })();
        if let Err(error) = result {
            let message = error.to_string();
            error!("debug trace output disabled: {message}");
            sink.error = Some(message);
        }
    }
    let capacity = world.resource::<TraceSettings>().capacity;
    let mut log = world.resource_mut::<TraceLog>();
    log.0.extend(events);
    while log.0.len() > capacity {
        log.0.pop_front();
    }
}
