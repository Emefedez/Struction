use avian3d::{physics_transform::PhysicsTransformSystems, prelude::*};
use bevy::prelude::*;
use struction_core::{ExtensorAppExt, ExtensorMeta};
use struction_gravity::{LocalGravity, LocalUp};
use struction_physics::{EnvironmentSystems, Submersion, Surface};

use crate::PlayerControlled;

/// Speed (m/s) away from the ground above which a character counts as airborne even if the
/// ground probe still reaches: it is leaving the ground, not standing on it.
const LEAVING_GROUND_SPEED: f32 = 1.0;
const PITCH_LIMIT: f32 = 1.5;
/// Gravity assumed for jump speed when there is none to derive it from.
const FALLBACK_GRAVITY: f32 = 9.81;
// Resting contacts can sit slightly inside a collider; start the probe above that overlap so
// the shape cast returns a surface normal instead of an unreliable penetration normal.
const GROUND_PROBE_LIFT: f32 = 0.02;

/// A capsule character driven by velocity changes, oriented to local up. Tuning lives here; what
/// the character is asked to do lives in [`CharacterIntent`]. Override the required `Collider`
/// to change its shape.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
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
    CharacterMove,
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
    /// How fast the heading turns toward movement when [`CharacterIntent::face_movement`] is
    /// set (radians/s).
    pub turn_speed: f32,
    /// Jumps are refused while any of these holds, besides needing ground and not swimming.
    pub jump_blocked_while: Vec<CharacterCondition>,
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
            turn_speed: 12.0,
            jump_blocked_while: vec![CharacterCondition::Rolling, CharacterCondition::Attacking],
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
    /// A roll was asked for since the last tick; consumed even when it cannot start.
    pub roll_requested: bool,
    /// An attack was asked for since the last tick; consumed even when it cannot start.
    pub attack_requested: bool,
    /// Jump is held: swims upward in water.
    pub jump_held: bool,
    /// Turn the heading toward the movement direction instead of keeping it, so a camera can
    /// orbit freely while the body faces where it goes.
    pub face_movement: bool,
}

/// What a character can be asked to do that may cut a running move short.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharacterAction {
    Jump,
    Roll,
    Attack,
}

/// A move's cancel window: from `after` seconds into it, asking for `action` ends the move
/// without recovery, so the action starts in the same tick (still subject to its own
/// `blocked_while`).
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct CancelInto {
    pub action: CharacterAction,
    pub after: f32,
}

/// Whether an asked-for action opens one of a running move's cancel windows.
pub fn cancel_opened(intent: &CharacterIntent, windows: &[CancelInto], elapsed: f32) -> bool {
    windows
        .iter()
        .any(|window| intent.requests(window.action) && elapsed >= window.after)
}

impl CharacterIntent {
    /// Whether `action` was asked for and not yet consumed.
    pub fn requests(&self, action: CharacterAction) -> bool {
        match action {
            CharacterAction::Jump => self.jump_requested,
            CharacterAction::Roll => self.roll_requested,
            CharacterAction::Attack => self.attack_requested,
        }
    }

    /// The asked-for movement in world space, tangent to `up`, relative to `forward` unless the
    /// intent carries its own movement frame. Length at most 1.
    pub fn wish(&self, forward: Vec3, up: Vec3) -> Vec3 {
        let forward = self
            .movement_forward
            .and_then(|direction| (direction - up * direction.dot(up)).try_normalize())
            .unwrap_or(forward);
        let right = forward.cross(up);
        (forward * self.movement.y + right * self.movement.x).clamp_length_max(1.0)
    }
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

impl CharacterLook {
    /// The heading carried over to `up`; projection alone can reverse it at a field boundary.
    pub fn heading(&self, up: Vec3) -> Vec3 {
        transport(self.forward, self.up, up)
    }
}

/// `direction` rotated from one up to another, kept tangent to the new up.
pub fn transport(direction: Vec3, from_up: Vec3, to_up: Vec3) -> Vec3 {
    let turned = Quat::from_rotation_arc(from_up, to_up) * direction;
    (turned - to_up * turned.dot(to_up)).normalize_or(to_up.any_orthonormal_vector())
}

/// The timed moves holding a character, such as a roll or an attack. The extensor running a move
/// writes this in [`CharacterSystems::Moves`]; the controller then leaves velocity and heading to
/// the move. Which moves may overlap, and how long after one another, is authored data: each
/// move's `blocked_while` conditions, such as `Attacking` or `Recovering`.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct CharacterMove {
    /// The running moves, as their conditions (`Rolling`, `Attacking`).
    pub active: Vec<CharacterCondition>,
    /// Velocity a move drives this tick instead of walking or swimming; cleared every tick.
    pub velocity: Option<Vec3>,
    /// Heading a move holds while the character faces its movement; cleared every tick.
    pub facing: Option<Vec3>,
    /// Seconds left of `Recovering`, which follows the end of a move.
    pub recovery: f32,
}

impl CharacterMove {
    pub fn start(&mut self, condition: CharacterCondition) {
        if !self.active.contains(&condition) {
            self.active.push(condition);
        }
    }

    /// Ends a move and starts recovering from it.
    pub fn end(&mut self, condition: CharacterCondition, recovery: f32) {
        self.active.retain(|&active| active != condition);
        self.recovery = self.recovery.max(recovery);
    }

    /// Whether any of `conditions` holds, such as a move's `blocked_while`. For a move already
    /// `running`, its own condition and `Recovering` (which only refuses starting) are ignored.
    pub fn blocked(
        &self,
        state: &CharacterState,
        conditions: &[CharacterCondition],
        running: Option<CharacterCondition>,
    ) -> bool {
        conditions.iter().any(|&condition| {
            !(running.is_some()
                && (Some(condition) == running || condition == CharacterCondition::Recovering))
                && match condition {
                    CharacterCondition::Grounded => state.grounded && !state.swimming,
                    CharacterCondition::Airborne => !state.grounded && !state.swimming,
                    CharacterCondition::Swimming => state.swimming,
                    CharacterCondition::Rolling | CharacterCondition::Attacking => {
                        self.active.contains(&condition)
                    }
                    CharacterCondition::Recovering => self.recovery > 0.0,
                }
        })
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

/// A state of the character that actions can be refused in: authored as `blocked_while` lists
/// (a move's, which stop it from starting and cancel it when one holds, or the controller's
/// `jump_blocked_while`).
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum CharacterCondition {
    /// On walkable ground, not swimming.
    Grounded,
    /// Jumping or falling: neither on walkable ground nor swimming.
    Airborne,
    Swimming,
    Rolling,
    Attacking,
    /// After a move ended, for its `recovery` seconds. Refuses starting a move, never cuts one
    /// short.
    Recovering,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CharacterSystems {
    /// Measures ground and water for this tick and counts down move recovery.
    Sense,
    /// Running moves whose cancel window an asked-for action opens end, before anything starts.
    Cancel,
    /// Extensors start, run and end timed moves (rolls, attacks) through [`CharacterMove`].
    Moves,
    /// Turns intents into velocity, jumps, and orientation for the next physics step.
    Control,
    /// Copies interpolated bodies and their motion to animation rigs (`PostUpdate`).
    Animate,
}

/// Runs the controller in `FixedPostUpdate`, right before the physics step, after gravity and
/// fluid forces are known. Registers the `character` extensor.
pub struct CharacterControllerPlugin;

impl Plugin for CharacterControllerPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<CharacterController>()
            .register_type::<CharacterIntent>()
            .register_type::<CharacterLook>()
            .register_type::<CharacterState>()
            .register_type::<CharacterMove>()
            .register_type::<PlayerControlled>()
            .register_type::<CharacterCondition>()
            .register_type::<Vec<CharacterCondition>>()
            .register_type::<CharacterAction>()
            .register_type::<CancelInto>()
            .register_type::<Vec<CancelInto>>()
            .register_extensor(
                ExtensorMeta::inferred("character")
                    .doc("A capsule that walks, jumps and swims under local gravity")
                    .owns::<CharacterController>()
                    .owns::<PlayerControlled>()
                    .requires("physics"),
            )
            .configure_sets(
                FixedPostUpdate,
                (
                    CharacterSystems::Sense,
                    CharacterSystems::Cancel,
                    CharacterSystems::Moves,
                    CharacterSystems::Control,
                )
                    .chain()
                    .in_set(PhysicsSystems::Prepare)
                    .after(EnvironmentSystems::Effects)
                    .after(PhysicsTransformSystems::TransformToPosition),
            )
            .add_systems(
                FixedPostUpdate,
                (probe_ground, sense_motion)
                    .chain()
                    .in_set(CharacterSystems::Sense),
            )
            .add_systems(
                FixedPostUpdate,
                (control_characters, hold_on_slopes)
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

/// Rotates `from` about `up` toward `to` by at most `max_angle`; both are tangent to `up`.
fn turn_toward(from: Vec3, to: Vec3, up: Vec3, max_angle: f32) -> Vec3 {
    let angle = from.cross(to).dot(up).atan2(from.dot(to));
    Quat::from_axis_angle(up, angle.clamp(-max_angle, max_angle)) * from
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
        let origin = position.0 - up * (feet - probe_radius - GROUND_PROBE_LIFT);
        let hit = Dir3::new(-up).ok().and_then(|down| {
            spatial.cast_shape_predicate(
                &Collider::sphere(probe_radius),
                origin,
                Quat::IDENTITY,
                down,
                &ShapeCastConfig::from_max_distance(controller.ground_probe + GROUND_PROBE_LIFT),
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

/// Completes the ground probe with motion: a probe that still reaches the ground while the body
/// shoots away from it means leaving it. Measured along the ground normal: walking up a ramp,
/// or across a planet whose up is tilted by other fields, moves along up without leaving the
/// surface.
fn sense_motion(
    time: Res<Time>,
    mut characters: Query<(
        &CharacterController,
        &mut CharacterState,
        &mut CharacterMove,
        &LocalUp,
        &Submersion,
        &LinearVelocity,
    )>,
) {
    for (controller, mut state, mut moving, up, submersion, velocity) in &mut characters {
        let normal = state.ground_normal.normalize_or(*up.0);
        state.grounded &= velocity.0.dot(normal) <= LEAVING_GROUND_SPEED;
        state.swimming = submersion.0 >= controller.swim_threshold;
        moving.recovery = (moving.recovery - time.delta_secs()).max(0.0);
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
        &mut CharacterMove,
        &LocalUp,
        &LocalGravity,
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
        mut moving,
        up,
        gravity,
        mut rotation,
        mut velocity,
    ) in &mut characters
    {
        let up = *up.0;
        let look_delta = core::mem::take(&mut intent.look);
        let forward = Quat::from_axis_angle(up, -look_delta.x) * look.heading(up);
        look.forward = forward;
        look.up = up;
        look.pitch = (look.pitch - look_delta.y).clamp(-PITCH_LIMIT, PITCH_LIMIT);

        let mut vertical = velocity.0.dot(up);
        let mut tangent = velocity.0 - up * vertical;
        let normal = state.ground_normal.normalize_or(up);
        let swimming = state.swimming;
        let wish = intent.wish(forward, up);

        let driven = moving.velocity.take();
        let facing = moving.facing.take();
        if intent.face_movement {
            if let Some(facing) = facing {
                look.forward = facing;
            } else if moving.active.is_empty()
                && let Some(direction) = wish.try_normalize()
            {
                look.forward = turn_toward(forward, direction, up, controller.turn_speed * dt);
            }
        }
        if let Some(linear) = driven {
            vertical = linear.dot(up);
            tangent = linear - up * vertical;
        } else if swimming {
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
        } else if wish != Vec3::ZERO {
            let target = wish * controller.move_speed;
            tangent = step_toward(tangent, target, controller.air_acceleration * dt);
        }

        if core::mem::take(&mut intent.jump_requested)
            && !moving.blocked(&state, &controller.jump_blocked_while, None)
            && state.grounded
            && !swimming
        {
            let gravity = gravity.0.length();
            let gravity = if gravity > struction_gravity::MIN_GRAVITY {
                gravity
            } else {
                FALLBACK_GRAVITY
            };
            vertical = (2.0 * gravity * controller.jump_height).sqrt();
            state.grounded = false;
        }

        let mut linear = tangent + up * vertical;
        if state.grounded && !swimming {
            // Stay on the ground when the surface curves away or the probe was mid-hop.
            let away = linear.dot(normal);
            if away > 0.0 {
                linear -= normal * away;
            }
        }
        if linear != velocity.0 {
            velocity.0 = linear;
        }

        // Turn toward the target orientation; up changes discretely as fields change.
        let target = orientation(look.forward, up);
        let blend = 1.0 - (-controller.align_rate * dt).exp();
        rotation.0 = rotation.0.slerp(target, blend).normalize();
    }
}

/// Cancels the pull of gravity along the ground for grounded characters, so standing still does
/// not creep down walkable slopes. It is an acceleration because gravity is integrated per
/// substep; a velocity change would overshoot uphill. Slippery surfaces keep most of the pull.
fn hold_on_slopes(mut characters: Query<(Forces, &CharacterState, &LocalGravity)>) {
    for (mut forces, state, gravity) in &mut characters {
        if state.grounded && !state.swimming {
            let normal = state.ground_normal.normalize_or_zero();
            let along_ground = gravity.0 - normal * gravity.0.dot(normal);
            forces
                .non_waking()
                .apply_linear_acceleration(-along_ground * state.ground_friction.min(1.0));
        }
    }
}
