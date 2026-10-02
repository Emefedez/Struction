use avian3d::prelude::*;
use bevy::prelude::*;
use struction_anim::{
    AnimError, AnimPlugin, AnimSystems,
    base_pose::{BasePoseSet, RigPoseSet},
    locomotion::{Ground, GroundHit, LocomotionParams},
    moves::{LoopPose, MovePose, PlaySequence},
    plugin::{AnimMotion, CustomGround, Locomotor, spawn_character},
    rig::Rig,
};
use struction_gravity::{LocalGravity, LocalUp};

use crate::{Attacking, CharacterState, CharacterSystems, Rolling};

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
    commands.entity(root).insert((
        RigOf(body),
        CustomGround,
        MovePose::default(),
        LoopPose::default(),
    ));
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
    Option<&'static Attacking>,
    Option<&'static PlaySequence>,
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
        &mut MovePose,
        &mut LoopPose,
    )>,
) {
    // Simulation timers run ahead of the interpolated body by the unspent part of a tick.
    let lag = fixed.timestep().as_secs_f32() * (1.0 - fixed.overstep_fraction());
    for (of, mut root, mut motion, mut anim_up, mut pose, mut looping) in &mut rigs {
        let Ok((body, up, gravity, velocity, state, collider, rolling, attacking, play)) =
            bodies.get(of.0)
        else {
            continue;
        };
        // The rig origin is on the ground below the pelvis; the body origin is the capsule center.
        let feet = -collider.aabb(Vec3::ZERO, Quat::IDENTITY).min.y;
        root.translation = body.translation - body.rotation * Vec3::Y * feet;
        root.rotation = body.rotation;
        anim_up.0 = *up.0;
        let running = rolling
            .map(|r| {
                (
                    &r.tuning.sequence,
                    r.elapsed,
                    r.tuning.duration,
                    r.direction,
                )
            })
            .or(attacking.map(|a| {
                (
                    &a.tuning.sequence,
                    a.elapsed,
                    a.tuning.duration,
                    a.direction,
                )
            }));
        if let Some((sequence, elapsed, duration, direction)) = running {
            *pose = MovePose {
                sequence: sequence.clone(),
                phase: ((elapsed - lag).max(0.0) / duration).clamp(0.0, 1.0),
                weight: 1.0,
                direction,
            };
        } else {
            pose.weight = (pose.weight - time.delta_secs() / 0.12).max(0.0);
        }
        looping.follow(play.map(|play| play.sequence.as_str()), time.delta_secs());
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

#[allow(clippy::type_complexity)]
fn step_locomotion(
    time: Res<Time>,
    poses: Option<Res<BasePoseSet>>,
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
            &MovePose,
            Option<&RigPoseSet>,
        ),
        With<CustomGround>,
    >,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    for (of, root, motion, up, mut locomotor, moving, library) in &mut rigs {
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
        // Feet do not stay planted at the takeoff point of a whole-body move like a roll, nor
        // while it fades out.
        let library = library.map(|set| &set.0).or(poses.as_deref());
        let whole_body = library
            .and_then(|library| moving.sequence(library))
            .is_some_and(|sequence| sequence.takeover >= 1.0);
        if whole_body && moving.weight > 0.0 {
            locomotor.state.reset();
            input.grounded = false;
        }
        locomotor.state.update(&input, &ground, dt);
    }
}
