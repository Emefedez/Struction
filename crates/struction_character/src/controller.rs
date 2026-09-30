use avian3d::{physics_transform::PhysicsTransformSystems, prelude::*};
use bevy::prelude::*;
use struction_gravity::{LocalGravity, LocalUp};
use struction_physics::{EnvironmentSystems, Submersion, Surface};

/// Speed (m/s) along up above which a character counts as airborne even if the ground probe
/// still reaches: it is leaving the ground, not standing on it.
const LEAVING_GROUND_SPEED: f32 = 1.0;
const PITCH_LIMIT: f32 = 1.5;
/// Gravity assumed for jump speed when there is none to derive it from.
const FALLBACK_GRAVITY: f32 = 9.81;

/// A capsule character driven by velocity changes, oriented to local up. Tuning lives here; what
/// the character is asked to do lives in [`CharacterIntent`]. Override the required `Collider`
/// to change its shape.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
#[require(
    RigidBody::Dynamic,
    Collider = Collider::capsule(0.3, 1.0),
    // The controller supplies its own traction from the ground's `Surface`; contact friction
    // would fight it. `Min` with a `Surface`'s `Multiply` still resolves to zero.
    Friction = Friction::ZERO.with_combine_rule(CoefficientCombine::Min),
    Restitution = Restitution::ZERO.with_combine_rule(CoefficientCombine::Min),
    // Slightly denser than water: it sinks slowly and swims up on demand.
    ColliderDensity(1100.0),
    LockedAxes = LockedAxes::ROTATION_LOCKED,
    SleepingDisabled,
    CharacterIntent,
    CharacterState,
    CharacterLook,
)]
pub struct CharacterController {
    /// Top speed on the ground and in the air (m/s).
    pub move_speed: f32,
    /// Ground acceleration per unit of the ground's `Surface` friction (m/s^2). On a friction 1.0
    /// floor the character reaches speed in a fraction of a second; on ice it barely moves.
    pub traction: f32,
    /// Acceleration while airborne (m/s^2).
    pub air_acceleration: f32,
    /// Apex height of a jump above the take-off point (m).
    pub jump_height: f32,
    /// Fraction of the body underwater (see `Submersion`) from which the character swims.
    pub swim_threshold: f32,
    /// Top speed while swimming (m/s), horizontal and upward.
    pub swim_speed: f32,
    pub swim_acceleration: f32,
    /// How far below the feet ground still counts as ground (m).
    pub ground_probe: f32,
    /// Steepest walkable slope (radians from level).
    pub max_slope: f32,
    /// How quickly the body turns to its target orientation (1/s).
    pub align_rate: f32,
}

impl Default for CharacterController {
    fn default() -> Self {
        Self {
            move_speed: 5.0,
            traction: 40.0,
            air_acceleration: 10.0,
            jump_height: 1.2,
            swim_threshold: 0.5,
            swim_speed: 2.5,
            swim_acceleration: 12.0,
            ground_probe: 0.15,
            max_slope: 50.0_f32.to_radians(),
            align_rate: 12.0,
        }
    }
}

/// What a character is asked to do this tick: the command the simulation consumes. Written by
/// input mapping for [`PlayerControlled`](crate::PlayerControlled) entities, by anything else
/// (AI, network) for the rest.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct CharacterIntent {
    /// x to the right, y forward, relative to the character's heading; length at most 1.
    pub movement: Vec2,
    /// Optional world-space movement frame, supplied as command data by input, AI or networking.
    /// The simulation projects it onto local ground without reading a camera.
    pub movement_forward: Option<Vec3>,
    /// Yaw and pitch change in radians since the last tick; cleared by the simulation.
    pub look: Vec2,
    /// A jump was asked for since the last tick; cleared by the simulation.
    pub jump_requested: bool,
    /// Jump is held: swims upward in water.
    pub jump_held: bool,
}

/// Where the character faces. `forward` is kept tangent to the local up, so movement stays
/// relative to it while walking around a planet.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct CharacterLook {
    pub forward: Vec3,
    /// Up used by this heading, retained to transport it when gravity changes.
    pub up: Vec3,
    /// Camera pitch in radians, positive looks up. The body does not pitch.
    pub pitch: f32,
}

impl Default for CharacterLook {
    fn default() -> Self {
        Self {
            forward: Vec3::NEG_Z,
            up: Vec3::Y,
            pitch: 0.0,
        }
    }
}

/// Result of the last controller tick, for animation and gameplay to read.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct CharacterState {
    pub grounded: bool,
    pub swimming: bool,
    pub ground: Option<Entity>,
    pub ground_normal: Vec3,
    /// Friction of the ground's `Surface`, 1.0 when it has none.
    pub ground_friction: f32,
}

impl Default for CharacterState {
    fn default() -> Self {
        Self {
            grounded: false,
            swimming: false,
            ground: None,
            ground_normal: Vec3::Y,
            ground_friction: 1.0,
        }
    }
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CharacterSystems {
    /// Turns intents into velocity, jumps, and orientation for the next physics step.
    Control,
}

/// Runs the controller in `FixedPostUpdate`, right before the physics step, after gravity and
/// fluid forces are known.
pub struct CharacterControllerPlugin;

impl Plugin for CharacterControllerPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<CharacterController>()
            .register_type::<CharacterIntent>()
            .register_type::<CharacterLook>()
            .register_type::<CharacterState>()
            .configure_sets(
                FixedPostUpdate,
                CharacterSystems::Control
                    .in_set(PhysicsSystems::Prepare)
                    .after(EnvironmentSystems::Effects)
                    .after(PhysicsTransformSystems::TransformToPosition),
            )
            .add_systems(
                FixedPostUpdate,
                (probe_ground, control_characters)
                    .chain()
                    .in_set(CharacterSystems::Control),
            );
    }
}

/// Moves `current` toward `target` by at most `max_delta`.
fn step_toward(current: Vec3, target: Vec3, max_delta: f32) -> Vec3 {
    let delta = target - current;
    let length = delta.length();
    if length <= max_delta {
        target
    } else {
        current + delta * (max_delta / length)
    }
}

/// Rotation with local Y along `up` and local -Z along `forward` (tangent to `up`).
fn orientation(forward: Vec3, up: Vec3) -> Quat {
    let z = -forward;
    let x = up.cross(z);
    Quat::from_mat3(&Mat3::from_cols(x, up, z))
}

/// Looks for ground along local down with a sphere a bit narrower than the body, cast from the
/// feet. Sensors (volumes) and the character's own colliders do not count.
#[allow(clippy::type_complexity)]
fn probe_ground(
    spatial: SpatialQuery,
    surfaces: Query<&Surface>,
    sensors: Query<(), With<Sensor>>,
    colliders: Query<&ColliderOf>,
    mut characters: Query<(
        Entity,
        &CharacterController,
        &mut CharacterState,
        &LocalUp,
        &Position,
        &Collider,
    )>,
) {
    for (entity, controller, mut state, up, position, collider) in &mut characters {
        let up = *up.0;
        let extents = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
        let feet = -extents.min.y;
        let probe_radius = 0.9 * extents.max.x.min(extents.max.z);
        let origin = position.0 - up * (feet - probe_radius);
        let hit = Dir3::new(-up).ok().and_then(|down| {
            spatial.cast_shape_predicate(
                &Collider::sphere(probe_radius),
                origin,
                Quat::IDENTITY,
                down,
                &ShapeCastConfig::from_max_distance(controller.ground_probe),
                &SpatialQueryFilter::default(),
                &|other| {
                    let own = colliders.get(other).is_ok_and(|of| of.body == entity);
                    other != entity && !own && !sensors.contains(other)
                },
            )
        });
        let ground =
            hit.filter(|hit| hit.normal1.normalize_or_zero().dot(up) >= controller.max_slope.cos());

        state.grounded = ground.is_some();
        state.ground = ground.map(|hit| hit.entity);
        state.ground_normal = ground.map_or(up, |hit| hit.normal1);
        state.ground_friction = ground.map_or(1.0, |hit| {
            let body = colliders.get(hit.entity).map_or(hit.entity, |of| of.body);
            surfaces
                .get(hit.entity)
                .or_else(|_| surfaces.get(body))
                .map_or(1.0, |surface| surface.friction)
        });
    }
}

#[allow(clippy::type_complexity)]
fn control_characters(
    time: Res<Time>,
    mut characters: Query<(
        &CharacterController,
        &mut CharacterIntent,
        &mut CharacterState,
        &mut CharacterLook,
        &LocalUp,
        &LocalGravity,
        &Submersion,
        &mut Rotation,
        &mut LinearVelocity,
    )>,
) {
    let dt = time.delta_secs();
    for (
        controller,
        mut intent,
        mut state,
        mut look,
        up,
        gravity,
        submersion,
        mut rotation,
        mut velocity,
    ) in &mut characters
    {
        let up = *up.0;

        // Transport the heading with gravity; projection alone can reverse it at a field boundary.
        let transported = Quat::from_rotation_arc(look.up, up) * look.forward;
        let mut forward = transported - up * transported.dot(up);
        if forward.length_squared() < 1e-6 {
            forward = up.any_orthonormal_vector();
        }
        forward = forward.normalize();
        let look_delta = core::mem::take(&mut intent.look);
        forward = Quat::from_axis_angle(up, -look_delta.x) * forward;
        look.forward = forward;
        look.up = up;
        look.pitch = (look.pitch - look_delta.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);
        let movement_forward = intent
            .movement_forward
            .and_then(|direction| (direction - up * direction.dot(up)).try_normalize())
            .unwrap_or(forward);
        let right = movement_forward.cross(up);

        let mut vertical = velocity.0.dot(up);
        let mut tangent = velocity.0 - up * vertical;

        // A probe that still reaches the ground while the body shoots up means leaving it.
        state.grounded &= vertical <= LEAVING_GROUND_SPEED;
        let swimming = submersion.0 >= controller.swim_threshold;
        state.swimming = swimming;

        let wish = (movement_forward * intent.movement.y + right * intent.movement.x)
            .clamp_length_max(1.0);
        if swimming {
            let target = wish * controller.swim_speed;
            tangent = step_toward(tangent, target, controller.swim_acceleration * dt);
            if intent.jump_held {
                vertical += (controller.swim_speed - vertical)
                    .clamp(0.0, controller.swim_acceleration * dt);
            }
        } else if state.grounded {
            let target = wish * controller.move_speed;
            tangent = step_toward(
                tangent,
                target,
                controller.traction * state.ground_friction * dt,
            );
            // Stay on the ground when the surface curves away or the probe was mid-hop.
            vertical = vertical.min(0.0);
        } else if wish != Vec3::ZERO {
            let target = wish * controller.move_speed;
            tangent = step_toward(tangent, target, controller.air_acceleration * dt);
        }

        if core::mem::take(&mut intent.jump_requested) && state.grounded && !swimming {
            let gravity = gravity.0.length();
            let gravity = if gravity > struction_gravity::MIN_GRAVITY {
                gravity
            } else {
                FALLBACK_GRAVITY
            };
            vertical = (2.0 * gravity * controller.jump_height).sqrt();
            state.grounded = false;
        }

        let linear = tangent + up * vertical;
        if linear != velocity.0 {
            velocity.0 = linear;
        }

        // Turn toward the target orientation; up changes discretely as fields change.
        let target = orientation(forward, up);
        let blend = 1.0 - (-controller.align_rate * dt).exp();
        rotation.0 = rotation.0.slerp(target, blend).normalize();
    }
}
