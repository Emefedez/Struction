use avian3d::prelude::*;
use bevy::prelude::*;
use struction_anim::{
    AnimError, AnimPlugin, AnimSystems,
    locomotion::{Ground, GroundHit, LocomotionParams},
    plugin::{AnimMotion, CustomGround, Locomotor, spawn_character},
    rig::Rig,
    roll::RollPose,
};
use struction_gravity::{LocalGravity, LocalUp};

use crate::{CharacterState, CharacterSystems, Rolling};

/// On an animation rig root: the character body it follows. The rig stays unparented so
/// animation reads this frame's root instead of last frame's `GlobalTransform`.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component)]
pub struct RigOf(pub Entity);

/// Feeds rigs from the controller in `PostUpdate`: after physics interpolation has written the
/// body's `Transform`, before [`AnimSystems::Motion`]. Feet are placed with physics ray casts.
pub struct CharacterAnimationPlugin;

impl Plugin for CharacterAnimationPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<AnimPlugin>() {
            app.add_plugins(AnimPlugin);
        }
        app.register_type::<RigOf>()
            .configure_sets(
                PostUpdate,
                CharacterSystems::Animate.before(AnimSystems::Motion),
            )
            .add_systems(
                PostUpdate,
                (
                    follow_bodies.in_set(CharacterSystems::Animate),
                    step_locomotion.in_set(AnimSystems::Locomotion),
                ),
            );
    }
}

/// Spawns an animation rig that follows `body`, a [`CharacterController`](crate::CharacterController).
pub fn spawn_rig(
    commands: &mut Commands,
    body: Entity,
    rig: Rig,
    params: LocomotionParams,
) -> Result<Entity, AnimError> {
    let root = spawn_character(commands, rig, Transform::IDENTITY, params)?;
    commands
        .entity(root)
        .insert((RigOf(body), CustomGround, RollPose::default()));
    Ok(root)
}

type Body = (
    &'static Transform,
    &'static LocalUp,
    &'static LocalGravity,
    &'static LinearVelocity,
    &'static CharacterState,
    &'static Collider,
    Option<&'static Rolling>,
);

fn follow_bodies(
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    bodies: Query<Body, Without<RigOf>>,
    mut rigs: Query<(
        &RigOf,
        &mut Transform,
        &mut AnimMotion,
        &mut struction_anim::plugin::LocalUp,
        &mut RollPose,
    )>,
) {
    for (of, mut root, mut motion, mut anim_up, mut pose) in &mut rigs {
        let Ok((body, up, gravity, velocity, state, collider, rolling)) = bodies.get(of.0) else {
            continue;
        };
        // The rig origin is on the ground below the pelvis; the body origin is the capsule center.
        let feet = -collider.aabb(Vec3::ZERO, Quat::IDENTITY).min.y;
        root.translation = body.translation - body.rotation * Vec3::Y * feet;
        root.rotation = body.rotation;
        anim_up.0 = *up.0;
        if let Some(rolling) = rolling {
            let elapsed = (rolling.elapsed
                - fixed.timestep().as_secs_f32() * (1.0 - fixed.overstep_fraction()))
            .max(0.0);
            *pose = RollPose {
                phase: (elapsed / rolling.ability.duration).clamp(0.0, 1.0),
                weight: 1.0,
                direction: rolling.direction,
            };
        } else {
            pose.weight = (pose.weight - time.delta_secs() / 0.12).max(0.0);
        }
        *motion = AnimMotion {
            velocity: velocity.0,
            // Swimming legs should not plant on the pool floor.
            grounded: state.grounded && !state.swimming,
            gravity: (gravity.0.length() > struction_gravity::MIN_GRAVITY).then_some(gravity.0),
        };
    }
}

/// Ground under a character's feet, as seen by a ray cast predicate.
struct PhysicsGround<'a, 'w, 's> {
    spatial: &'a SpatialQuery<'w, 's>,
    solid: &'a dyn Fn(Entity) -> bool,
}

impl Ground for PhysicsGround<'_, '_, '_> {
    fn cast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<GroundHit> {
        let direction = Dir3::new(direction).ok()?;
        // Solid: a probe starting inside a wall or step lands on its origin, not the far side.
        let hit = self.spatial.cast_ray_predicate(
            origin,
            direction,
            max_distance,
            true,
            &SpatialQueryFilter::default(),
            self.solid,
        )?;
        let normal = if hit.normal == Vec3::ZERO {
            -*direction
        } else {
            hit.normal
        };
        Some(GroundHit {
            point: origin + *direction * hit.distance,
            normal,
        })
    }
}

fn step_locomotion(
    time: Res<Time>,
    spatial: SpatialQuery,
    colliders: Query<&ColliderOf>,
    sensors: Query<(), With<Sensor>>,
    mut rigs: Query<
        (
            &RigOf,
            &Transform,
            &AnimMotion,
            &struction_anim::plugin::LocalUp,
            &mut Locomotor,
            &RollPose,
        ),
        With<CustomGround>,
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (of, root, motion, up, mut locomotor, roll) in &mut rigs {
        // Feet stand on solid colliders other than the character's own body.
        let body = of.0;
        let solid = |other: Entity| {
            let own = other == body || colliders.get(other).is_ok_and(|c| c.body == body);
            !own && !sensors.contains(other)
        };
        let ground = PhysicsGround {
            spatial: &spatial,
            solid: &solid,
        };
        let mut input = motion.locomotion_input(*root, up.0);
        if roll.weight > 0.0 {
            locomotor.state.reset();
            input.grounded = false;
        }
        locomotor.state.update(&input, &ground, dt);
    }
}
