//! Sensing: what an entity perceives, filtered by lineage.
//!
//! A definition declares `"sensing": { "sees": ["player"], "flocksWith": ["minions/ogre"] }`.
//! An entry matches instances of that definition and of everything descending from it. Each
//! fixed tick, in [`AiSet::Sense`](crate::AiSet::Sense), [`Sensed`] is rebuilt:
//!
//! - `visible`: entities matching `sees` within `range`, inside the field of view and, when a
//!   [`LineOfSight`] check is installed, not occluded. Nearest first.
//! - `flockmates`: entities matching `flocksWith` within `range`, in any direction and without
//!   occlusion: neighbors are felt rather than seen.
//!
//! Positions and facing come from `Transform` (forward is -Z), so sensing entities and what they
//! sense are expected to be top-level, as simulated bodies are. Line of sight is pluggable so this
//! crate does not depend on physics: a physics package installs a batched raycast.

use bevy::ecs::system::SystemId;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use struction_core::{Definition, DefinitionPath};

/// The `sensing` section of a definition.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(default, rename_all = "camelCase")]
#[reflect(Component, Default)]
#[require(Sensed)]
pub struct Sensing {
    /// Definition paths; descendants match too.
    pub sees: Vec<String>,
    pub flocks_with: Vec<String>,
    /// Meters.
    pub range: f32,
    /// Full angle in degrees around forward (-Z); 360 or more sees all around.
    pub field_of_view: f32,
}

impl Default for Sensing {
    fn default() -> Self {
        Self {
            sees: Vec::new(),
            flocks_with: Vec::new(),
            range: 20.0,
            field_of_view: 120.0,
        }
    }
}

/// What an entity perceived this tick. Derived every tick, never saved.
#[derive(Component, Default, Clone, PartialEq, Debug)]
pub struct Sensed {
    /// Nearest first.
    pub visible: Vec<Entity>,
    /// Nearest first.
    pub flockmates: Vec<Entity>,
}

/// One observer-target pair for the occlusion check.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Sightline {
    pub observer: Entity,
    pub target: Entity,
    pub from: Vec3,
    pub to: Vec3,
}

/// The installed occlusion check: a system receiving every sightline that passed range and field
/// of view this tick and returning, in the same order, whether each is clear.
#[derive(Resource, Clone, Copy, Debug)]
pub struct LineOfSight(pub SystemId<In<Vec<Sightline>>, Vec<bool>>);

/// Installation of the occlusion check on [`App`].
pub trait SensingAppExt {
    /// Replaces the line-of-sight check. Without one, nothing occludes.
    fn set_line_of_sight<M>(
        &mut self,
        system: impl IntoSystem<In<Vec<Sightline>>, Vec<bool>, M> + 'static,
    ) -> &mut Self;
}

impl SensingAppExt for App {
    fn set_line_of_sight<M>(
        &mut self,
        system: impl IntoSystem<In<Vec<Sightline>>, Vec<bool>, M> + 'static,
    ) -> &mut Self {
        let world = self.world_mut();
        let id = world.register_system(system);
        if let Some(LineOfSight(previous)) = world.remove_resource::<LineOfSight>() {
            // The old check may already be gone; nothing else refers to it.
            let _ = world.unregister_system(previous);
        }
        world.insert_resource(LineOfSight(id));
        self
    }
}

fn paths(names: &[String]) -> Vec<DefinitionPath> {
    names.iter().map(DefinitionPath::new).collect()
}

fn matches(definition: &Definition, paths: &[DefinitionPath]) -> bool {
    paths.iter().any(|path| definition.descends_from(path))
}

/// Range, field of view and lineage.
pub(crate) fn sense(
    mut observers: Query<(Entity, &Sensing, &Transform, &mut Sensed)>,
    candidates: Query<(Entity, &Definition, &Transform)>,
) {
    for (observer, sensing, transform, mut sensed) in &mut observers {
        let sees = paths(&sensing.sees);
        let flocks_with = paths(&sensing.flocks_with);
        let origin = transform.translation;
        let forward = transform.forward();
        let min_cos = (sensing.field_of_view.to_radians() / 2.0).cos();
        let range_sq = sensing.range * sensing.range;

        let mut visible = Vec::new();
        let mut flockmates = Vec::new();
        for (candidate, definition, other) in &candidates {
            if candidate == observer {
                continue;
            }
            let offset = other.translation - origin;
            let distance_sq = offset.length_squared();
            if distance_sq > range_sq {
                continue;
            }
            if matches(definition, &flocks_with) {
                flockmates.push((distance_sq, candidate));
            }
            let in_view = sensing.field_of_view >= 360.0
                || distance_sq == 0.0
                || forward.dot(offset / distance_sq.sqrt()) >= min_cos;
            if in_view && matches(definition, &sees) {
                visible.push((distance_sq, candidate));
            }
        }
        sensed.visible = nearest_first(visible);
        sensed.flockmates = nearest_first(flockmates);
    }
}

fn nearest_first(mut found: Vec<(f32, Entity)>) -> Vec<Entity> {
    found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    found.into_iter().map(|(_, entity)| entity).collect()
}

/// Drops occluded entities from `visible`, with one call to the installed check.
pub(crate) fn apply_line_of_sight(
    world: &mut World,
    observers: &mut QueryState<(Entity, &Sensed, &Transform)>,
) {
    let Some(LineOfSight(check)) = world.get_resource::<LineOfSight>().copied() else {
        return;
    };
    let mut lines = Vec::new();
    for (observer, sensed, transform) in observers.iter(world) {
        for &target in &sensed.visible {
            if let Some(to) = world.get::<Transform>(target) {
                lines.push(Sightline {
                    observer,
                    target,
                    from: transform.translation,
                    to: to.translation,
                });
            }
        }
    }
    if lines.is_empty() {
        return;
    }
    let clear = match world.run_system_with(check, lines.clone()) {
        Ok(clear) if clear.len() == lines.len() => clear,
        Ok(clear) => {
            error!(
                "line-of-sight check answered {} of {} sightlines; ignoring it",
                clear.len(),
                lines.len()
            );
            return;
        }
        Err(error) => {
            error!("line-of-sight check failed: {error}");
            return;
        }
    };
    for (line, clear) in lines.into_iter().zip(clear) {
        if !clear && let Some(mut sensed) = world.get_mut::<Sensed>(line.observer) {
            sensed.visible.retain(|&entity| entity != line.target);
        }
    }
}
