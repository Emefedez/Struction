use avian3d::prelude::*;
use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use struction_gravity::LocalGravity;

use crate::EnvironmentSystems;

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<Volume>()
        .register_type::<Buoyancy>()
        .register_type::<VolumeDrag>()
        .register_type::<DamageField>()
        .register_type::<Submersion>()
        .add_systems(
            FixedPostUpdate,
            (reset_submersion, apply_volume_effects)
                .chain()
                .in_set(EnvironmentSystems::Effects)
                .after(crate::surface::apply_surface_drag),
        );
}

/// Shape of a [`Volume`] in its local space.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum VolumeShape {
    Box { half_extents: Vec3 },
    Sphere { radius: f32 },
}

impl VolumeShape {
    fn collider(self) -> Collider {
        match self {
            Self::Box { half_extents } => Collider::cuboid(
                half_extents.x * 2.0,
                half_extents.y * 2.0,
                half_extents.z * 2.0,
            ),
            Self::Sphere { radius } => Collider::sphere(radius),
        }
    }

    /// The liquid surface nearest to `point`: a point on it and its outward normal. A box is
    /// filled up to its top face; a sphere is a ball of liquid, so its surface is radial.
    fn surface_at(self, center: Vec3, rotation: Quat, point: Vec3) -> (Vec3, Vec3) {
        match self {
            Self::Box { half_extents } => {
                let up = rotation * Vec3::Y;
                (center + up * half_extents.y, up)
            }
            Self::Sphere { radius } => {
                let normal = (point - center).try_normalize().unwrap_or(Vec3::Y);
                (center + normal * radius, normal)
            }
        }
    }
}

/// A region of space, detected with an Avian sensor collider created from `shape`. Effects are
/// separate components on the same entity ([`Buoyancy`], [`VolumeDrag`], [`DamageField`],
/// [`CameraZone`](crate::CameraZone)), so water, lava, and camera zones are compositions.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
#[component(on_insert = insert_volume_collider)]
#[require(RigidBody::Static, Sensor, CollidingEntities)]
pub struct Volume {
    pub shape: VolumeShape,
}

fn insert_volume_collider(mut world: DeferredWorld, context: HookContext) {
    let shape = world.get::<Volume>(context.entity).unwrap().shape;
    world
        .commands()
        .entity(context.entity)
        .insert(shape.collider());
}

/// The volume is filled with a fluid that pushes bodies up against their local gravity with the
/// weight of the displaced fluid.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct Buoyancy {
    /// Fluid density in kg/m^3 (water is 1000). Compare with the body's `ColliderDensity`.
    pub fluid_density: f32,
}

/// Drag inside the volume, as decay rates (1/s) of linear and angular velocity at full
/// submersion.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct VolumeDrag {
    pub linear: f32,
    pub angular: f32,
}

/// Bodies in the volume take damage. This only emits [`VolumeDamage`]; health is another
/// package's business.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct DamageField {
    pub per_second: f32,
}

/// Damage a body took from a [`DamageField`] during one fixed tick. Triggered on the body.
#[derive(EntityEvent, Clone, Copy, Debug)]
pub struct VolumeDamage {
    #[event_target]
    pub body: Entity,
    pub volume: Entity,
    pub amount: f32,
}

/// How much of a body is inside a fluid volume, from 0 to 1 (the maximum over volumes). Written
/// every tick for every rigid body. A fluid volume carries [`Buoyancy`]; camera and other
/// non-fluid volumes do not make a character swim.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct Submersion(pub f32);

pub fn water(shape: VolumeShape) -> impl Bundle {
    (
        Volume { shape },
        Buoyancy {
            fluid_density: 1000.0,
        },
        VolumeDrag {
            linear: 1.5,
            angular: 1.0,
        },
    )
}

/// Water that is denser, thicker, and hurts.
pub fn lava(shape: VolumeShape) -> impl Bundle {
    (
        Volume { shape },
        Buoyancy {
            fluid_density: 3100.0,
        },
        VolumeDrag {
            linear: 6.0,
            angular: 3.0,
        },
        DamageField { per_second: 25.0 },
    )
}

fn reset_submersion(mut bodies: Query<&mut Submersion>) {
    for mut submersion in &mut bodies {
        submersion.set_if_neq(Submersion(0.0));
    }
}

/// Fraction of a collider's extent along `normal` that lies below the liquid surface plane.
fn submerged_fraction(surface_point: Vec3, normal: Vec3, aabb: &ColliderAabb) -> f32 {
    let center = (aabb.min + aabb.max) * 0.5;
    let half_extent = (aabb.max - aabb.min) * 0.5;
    // Projected half size of the (axis-aligned) box onto the normal.
    let radius = half_extent.dot(normal.abs());
    let depth = (surface_point - center).dot(normal) + radius;
    if radius <= f32::EPSILON {
        return if depth > 0.0 { 1.0 } else { 0.0 };
    }
    (depth / (2.0 * radius)).clamp(0.0, 1.0)
}

#[allow(clippy::type_complexity)]
fn apply_volume_effects(
    volumes: Query<(
        Entity,
        &Volume,
        &Position,
        &Rotation,
        &CollidingEntities,
        Option<&Buoyancy>,
        Option<&VolumeDrag>,
        Option<&DamageField>,
    )>,
    colliders: Query<(&Collider, &ColliderAabb, &ColliderOf), Without<Sensor>>,
    mut bodies: Query<(Forces, &RigidBody, &LocalGravity, &mut Submersion)>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (volume_entity, volume, position, rotation, touching, buoyancy, drag, damage) in &volumes {
        // Sorted so that a body's colliders are adjacent and results do not depend on hash order.
        let mut occupants: Vec<(Entity, Entity)> = touching
            .iter()
            .filter_map(|&entity| Some((colliders.get(entity).ok()?.2.body, entity)))
            .collect();
        occupants.sort_unstable();

        for group in occupants.chunk_by(|a, b| a.0 == b.0) {
            let body_entity = group[0].0;
            let Ok((mut forces, body, gravity, mut submersion)) = bodies.get_mut(body_entity)
            else {
                continue;
            };
            if !body.is_dynamic() {
                continue;
            }

            let mut fraction = 0.0_f32;
            for &(_, collider_entity) in group {
                let (collider, aabb, _) = colliders.get(collider_entity).unwrap();
                if !aabb.min.is_finite() {
                    continue;
                }
                let center = (aabb.min + aabb.max) * 0.5;
                let (surface_point, normal) =
                    volume.shape.surface_at(position.0, rotation.0, center);
                let collider_fraction = submerged_fraction(surface_point, normal, aabb);
                fraction = fraction.max(collider_fraction);

                if let Some(buoyancy) = buoyancy {
                    let displaced_volume = collider.mass(1.0);
                    let force =
                        -gravity.0 * buoyancy.fluid_density * displaced_volume * collider_fraction;
                    forces.apply_force_at_point(force, center);
                }
            }
            if fraction <= 0.0 {
                continue;
            }
            if buoyancy.is_some() {
                submersion.0 = submersion.0.max(fraction);
            }

            if let Some(drag) = drag {
                // Never decelerate past standstill within one tick.
                let max_rate = 1.0 / dt;
                let linear = forces.linear_velocity() * -(drag.linear.min(max_rate) * fraction);
                let angular = forces.angular_velocity() * -(drag.angular.min(max_rate) * fraction);
                forces.apply_linear_acceleration(linear);
                forces.apply_angular_acceleration(angular);
            }
            if let Some(damage) = damage {
                commands.trigger(VolumeDamage {
                    body: body_entity,
                    volume: volume_entity,
                    amount: damage.per_second * dt,
                });
            }
        }
    }
}
