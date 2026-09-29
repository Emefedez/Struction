use avian3d::prelude::*;
use bevy::prelude::*;

use crate::Volume;

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<CameraZone>()
        .register_type::<CameraTarget>()
        .register_type::<InCameraZones>()
        .add_systems(
            FixedPostUpdate,
            detect_camera_zones.in_set(PhysicsSystems::Last),
        );
}

/// How the camera should frame the tracked entity while inside a zone.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum CameraMode {
    /// Trail the target at `distance`, looking down by `pitch` radians.
    Follow { distance: f32, pitch: f32 },
    /// Stay at a world position and look at the target.
    Fixed { position: Vec3 },
}

/// Camera constraint data. Solving it is the camera package's job.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct CameraConstraint {
    pub mode: CameraMode,
    /// Influence when blended with other constraints, from 0 to 1.
    pub weight: f32,
    /// Higher priority wins when zones overlap.
    pub priority: i32,
}

/// A [`Volume`] that carries a [`CameraConstraint`]. Spawn it next to a `Volume`, or use
/// [`CameraZone::bundle`].
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct CameraZone {
    pub constraint: CameraConstraint,
}

impl CameraZone {
    pub fn bundle(volume: Volume, constraint: CameraConstraint) -> impl Bundle {
        (volume, CameraZone { constraint })
    }
}

/// Marks the entity (the player) whose position selects the camera zone.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
#[require(InCameraZones)]
pub struct CameraTarget;

/// The camera zones a [`CameraTarget`] is inside, best first: highest priority, ties broken by
/// entity order.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct InCameraZones(pub Vec<Entity>);

impl InCameraZones {
    pub fn active(&self) -> Option<Entity> {
        self.0.first().copied()
    }
}

fn detect_camera_zones(
    zones: Query<(Entity, &CameraZone, &CollidingEntities)>,
    colliders: Query<&ColliderOf>,
    mut targets: Query<&mut InCameraZones, With<CameraTarget>>,
) {
    let mut found: Vec<(Entity, i32, Entity)> = Vec::new();
    for (zone_entity, zone, touching) in &zones {
        for &collider in touching.iter() {
            let body = colliders.get(collider).map_or(collider, |of| of.body);
            if targets.contains(body) {
                found.push((body, zone.constraint.priority, zone_entity));
            }
        }
    }
    // Per target: priority descending, then entity ascending.
    found
        .sort_unstable_by_key(|&(body, priority, zone)| (body, core::cmp::Reverse(priority), zone));
    found.dedup();

    for mut in_zones in &mut targets {
        if !in_zones.0.is_empty() {
            in_zones.0.clear();
        }
    }
    for (body, _, zone) in found {
        if let Ok(mut in_zones) = targets.get_mut(body) {
            in_zones.0.push(zone);
        }
    }
}
