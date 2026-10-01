//! Gravity fields: gravity is not a global constant, bodies sum the fields affecting them.
//!
//! A [`GravityField`] entity places a volume and a kind of pull in the world. Every entity with
//! a [`LocalGravity`] gets the sum of all fields whose volume contains it, plus a [`LocalUp`]
//! derived from that sum. Scene gravity is a directional field with an infinite volume; a planet
//! is a radial field with a sphere volume. There is no priority: overlapping fields add up.
//!
//! This crate has no physics dependency. It reads [`GlobalTransform`] and expects to run after
//! transforms are up to date, which is why the plugin takes the schedule to run in.

use bevy::{
    ecs::{intern::Interned, schedule::ScheduleLabel},
    prelude::*,
};

pub mod prelude {
    pub use crate::{
        Falloff, GravityField, GravityHysteresis, GravityInfluences, GravityKind, GravityPlugin,
        GravityPose, GravitySystems, GravityVolume, LocalGravity, LocalUp,
    };
}

/// Gravity below this magnitude (m/s^2) is treated as none when deriving [`LocalUp`].
pub const MIN_GRAVITY: f32 = 1e-4;

/// Region of space a [`GravityField`] applies to, in the field entity's local space.
/// Scale is ignored: sizes are in world units.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum GravityVolume {
    Infinite,
    Sphere { radius: f32 },
    Box { half_extents: Vec3 },
}

/// How the strength of a radial field changes with distance from its center.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum Falloff {
    Constant,
    /// Full strength up to `reference_distance` (a planet's surface), then `1/d^2`.
    InverseSquare {
        reference_distance: f32,
    },
}

/// The pull a [`GravityField`] exerts inside its volume.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum GravityKind {
    /// Constant acceleration, given in the field's local space (rotate the field to aim it).
    Directional { acceleration: Vec3 },
    /// Acceleration toward the field's origin; a negative strength repels.
    Radial { strength: f32, falloff: Falloff },
}

/// A source of gravitational acceleration (m/s^2) inside a volume. The default pulls toward the
/// entity at 9.81 within 3 m, like a small planet.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
#[require(Transform)]
pub struct GravityField {
    pub volume: GravityVolume,
    pub kind: GravityKind,
}

impl Default for GravityField {
    fn default() -> Self {
        Self::planet(9.81, 3.0)
    }
}

impl GravityField {
    /// Scene gravity: the same acceleration everywhere (an infinite directional field).
    pub fn scene(acceleration: Vec3) -> Self {
        Self {
            volume: GravityVolume::Infinite,
            kind: GravityKind::Directional { acceleration },
        }
    }

    /// A planet's pull: constant `surface_gravity` toward the origin inside `influence_radius`.
    pub fn planet(surface_gravity: f32, influence_radius: f32) -> Self {
        Self {
            volume: GravityVolume::Sphere {
                radius: influence_radius,
            },
            kind: GravityKind::Radial {
                strength: surface_gravity,
                falloff: Falloff::Constant,
            },
        }
    }
}

/// Optional boundary hysteresis. A body enters at the field's authored volume and leaves
/// after moving `exit_margin` world units beyond it. Invalid/negative margins behave as zero.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct GravityHysteresis {
    pub exit_margin: f32,
}

/// Fields currently affecting a body, retained between ticks for boundary hysteresis.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct GravityInfluences(pub Vec<Entity>);

/// Optional simulation-owned world pose. Physics bridges write this before gravity sampling
/// so rendered/interpolated transforms cannot change forces or boundary membership.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct GravityPose {
    pub translation: Vec3,
    pub rotation: Quat,
}

/// The summed acceleration (m/s^2) of all fields at the entity's position.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
#[require(Transform, LocalUp, GravityInfluences)]
pub struct LocalGravity(pub Vec3);

/// The direction opposite to [`LocalGravity`]. With no gravity it keeps its last value.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct LocalUp(pub Dir3);

impl Default for LocalUp {
    fn default() -> Self {
        Self(Dir3::Y)
    }
}

/// Whether `local_point` (in the field's space) is inside `volume`.
pub fn volume_contains(volume: &GravityVolume, local_point: Vec3) -> bool {
    match *volume {
        GravityVolume::Infinite => true,
        GravityVolume::Sphere { radius } => local_point.length_squared() <= radius * radius,
        GravityVolume::Box { half_extents } => local_point.abs().cmple(half_extents).all(),
    }
}

/// Acceleration of a single field at `point`, given the field's world `translation` and
/// `rotation`. Zero outside the volume.
pub fn field_acceleration(
    field: &GravityField,
    translation: Vec3,
    rotation: Quat,
    point: Vec3,
) -> Vec3 {
    let offset = point - translation;
    if !volume_contains(&field.volume, rotation.inverse() * offset) {
        return Vec3::ZERO;
    }
    acceleration_inside(field, rotation, offset)
}

fn acceleration_inside(field: &GravityField, rotation: Quat, offset: Vec3) -> Vec3 {
    match field.kind {
        GravityKind::Directional { acceleration } => rotation * acceleration,
        GravityKind::Radial { strength, falloff } => {
            let distance = offset.length();
            let scale = match falloff {
                Falloff::Constant => 1.0,
                Falloff::InverseSquare { reference_distance } => {
                    let ratio = reference_distance / distance.max(reference_distance);
                    ratio * ratio
                }
            };
            // At the center the direction is undefined, so there is no pull.
            -offset.normalize_or_zero() * strength * scale
        }
    }
}

/// Stateful sampling of one field. The returned membership must be retained for the next tick.
/// New bodies in the outer margin remain unaffected until they cross the entry boundary.
pub fn sample_field(
    field: &GravityField,
    hysteresis: Option<&GravityHysteresis>,
    pose: GravityPose,
    point: Vec3,
    previously_inside: bool,
) -> (Vec3, bool) {
    let margin = hysteresis.map_or(0.0, |h| h.exit_margin);
    let margin = if previously_inside && margin.is_finite() {
        margin.max(0.0)
    } else {
        0.0
    };
    let volume = match field.volume {
        GravityVolume::Infinite => GravityVolume::Infinite,
        GravityVolume::Sphere { radius } => GravityVolume::Sphere {
            radius: radius + margin,
        },
        GravityVolume::Box { half_extents } => GravityVolume::Box {
            half_extents: half_extents + Vec3::splat(margin),
        },
    };
    let offset = point - pose.translation;
    let inside = volume_contains(&volume, pose.rotation.inverse() * offset);
    (
        if inside {
            acceleration_inside(field, pose.rotation, offset)
        } else {
            Vec3::ZERO
        },
        inside,
    )
}

/// Sum of all fields at `point`. Fields are `(field, world translation, world rotation)`.
pub fn sum_fields<'a>(
    fields: impl IntoIterator<Item = (&'a GravityField, Vec3, Quat)>,
    point: Vec3,
) -> Vec3 {
    fields
        .into_iter()
        .map(|(field, translation, rotation)| {
            field_acceleration(field, translation, rotation, point)
        })
        .sum()
}

/// Up direction for a gravity vector, or `fallback` when gravity is (almost) zero.
pub fn up_from_gravity(gravity: Vec3, fallback: Dir3) -> Dir3 {
    if gravity.length_squared() < MIN_GRAVITY * MIN_GRAVITY {
        fallback
    } else {
        Dir3::new(-gravity).unwrap_or(fallback)
    }
}

/// System set of the system that writes [`LocalGravity`] and [`LocalUp`].
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GravitySystems {
    Update,
}

/// Adds [`GravitySystems::Update`] to `schedule` (`FixedPostUpdate` by default, where it
/// belongs together with the physics step).
pub struct GravityPlugin {
    schedule: Interned<dyn ScheduleLabel>,
}

impl GravityPlugin {
    pub fn new(schedule: impl ScheduleLabel) -> Self {
        Self {
            schedule: schedule.intern(),
        }
    }
}

impl Default for GravityPlugin {
    fn default() -> Self {
        Self::new(FixedPostUpdate)
    }
}

impl Plugin for GravityPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<GravityField>()
            .register_type::<LocalGravity>()
            .register_type::<LocalUp>()
            .register_type::<GravityHysteresis>()
            .register_type::<GravityInfluences>()
            .register_type::<GravityPose>()
            .add_systems(
                self.schedule,
                update_local_gravity.in_set(GravitySystems::Update),
            );
    }
}

fn pose(transform: &GlobalTransform, simulation: Option<&GravityPose>) -> GravityPose {
    simulation.copied().unwrap_or_else(|| {
        let (_, rotation, translation) = transform.to_scale_rotation_translation();
        GravityPose {
            translation,
            rotation,
        }
    })
}

type FieldData = (
    Entity,
    &'static GravityField,
    &'static GlobalTransform,
    Option<&'static GravityPose>,
    Option<&'static GravityHysteresis>,
);

fn update_local_gravity(
    fields: Query<FieldData>,
    mut bodies: Query<(
        &GlobalTransform,
        Option<&GravityPose>,
        &mut LocalGravity,
        &mut LocalUp,
        &mut GravityInfluences,
    )>,
) {
    // A stable order also keeps floating-point sums independent of archetype migration.
    let mut fields: Vec<_> = fields.iter().collect();
    fields.sort_by_key(|(entity, ..)| *entity);
    for (transform, simulation, mut gravity, mut up, mut influences) in &mut bodies {
        let point = pose(transform, simulation).translation;
        let mut sum = Vec3::ZERO;
        let mut active = Vec::new();
        for &(entity, field, transform, simulation, hysteresis) in &fields {
            let (acceleration, inside) = sample_field(
                field,
                hysteresis,
                pose(transform, simulation),
                point,
                influences.0.contains(&entity),
            );
            sum += acceleration;
            if inside {
                active.push(entity);
            }
        }
        influences.set_if_neq(GravityInfluences(active));
        gravity.set_if_neq(LocalGravity(sum));
        let new_up = up_from_gravity(sum, up.0);
        up.set_if_neq(LocalUp(new_up));
    }
}

#[cfg(test)]
mod tests;
