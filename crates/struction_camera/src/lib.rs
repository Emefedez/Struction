//! Player camera package: a third-person orbit and a first-person view, and nothing else. There
//! is no free camera; authored [`CameraZone`]s can only reframe the third-person view.
//!
//! Input goes through the camera before it reaches the character. In third person, `Look`
//! orbits the camera around the target and the character turns toward where it moves; in first
//! person, `Look` turns the character itself and the camera sits at its eyes. Both views follow
//! the target's [`LocalUp`], so they work around planets. The camera is presentation: it reads
//! simulation state and writes only [`InputActions`] commands, never the simulation.

use avian3d::prelude::*;
use bevy::prelude::*;
use struction_character::{CharacterIntent, CharacterLook, InputActions, InputSystems};
use struction_gravity::LocalUp;
use struction_physics::{CameraMode, CameraZone, InCameraZones};

/// The only two ways the player sees the world.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewMode {
    #[default]
    ThirdPerson,
    FirstPerson,
}

/// Camera tuning and the entity it follows. The target needs a [`CharacterLook`] and a
/// [`LocalUp`]; its `Transform` is read after physics interpolation.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
#[require(Transform, PlayerCameraState)]
pub struct PlayerCamera {
    pub target: Entity,
    pub view: ViewMode,
    /// Preferred third-person distance from the focus (m), changed by `Zoom`. Zooming in past
    /// `min_distance` switches to first person; zooming out of first person comes back here.
    pub distance: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    /// Meters of distance per scroll line.
    pub zoom_step: f32,
    /// Resting third-person pitch (radians above the focus), used when entering third person.
    pub pitch: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    /// Height of the orbit focus above the target's origin, along its up (m).
    pub focus_height: f32,
    /// Height of the first-person eye above the target's origin (m).
    pub eye_height: f32,
    /// Clearance kept between a third-person camera and geometry behind it (m).
    pub collision_radius: f32,
    /// Rate at which a camera pulled in by geometry eases back out (1/s). Pulling in is
    /// immediate so the view never enters a wall.
    pub recover_rate: f32,
    /// Rate at which the view follows discrete changes of up between gravity fields (1/s).
    pub up_rate: f32,
    /// Rate of the blend into and out of a fixed camera zone (1/s).
    pub zone_rate: f32,
}

impl PlayerCamera {
    pub fn new(target: Entity) -> Self {
        Self {
            target,
            view: ViewMode::ThirdPerson,
            distance: 5.5,
            min_distance: 1.5,
            max_distance: 12.0,
            zoom_step: 0.6,
            pitch: 0.35,
            min_pitch: -0.6,
            max_pitch: 1.3,
            focus_height: 0.7,
            eye_height: 0.62,
            collision_radius: 0.25,
            recover_rate: 4.0,
            up_rate: 6.0,
            zone_rate: 3.0,
        }
    }
}

/// What the camera solved last frame. Third-person heading and pitch belong to the camera, not
/// the character, so orbiting does not turn the body.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct PlayerCameraState {
    /// Ground heading of the third-person view, tangent to `up`.
    pub forward: Vec3,
    /// Up of the view, easing toward the target's [`LocalUp`].
    pub up: Vec3,
    /// Third-person pitch, positive above the focus looking down.
    pub pitch: f32,
    /// Third-person distance after collision.
    pub distance: f32,
    /// Blend toward the active fixed zone's framing, from 0 to 1.
    pub zone_weight: f32,
    /// Position of the last fixed zone, kept while blending out of it.
    pub zone_position: Option<Vec3>,
    initialized: bool,
}

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CameraSystems {
    /// `PreUpdate`, between [`InputSystems::Map`] and [`InputSystems::Command`]: view switches,
    /// zoom, and routing `Look` to the orbit or the character.
    Input,
    /// `Update`: places the camera from the interpolated target.
    Follow,
}

pub struct PlayerCameraPlugin;

impl Plugin for PlayerCameraPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<PlayerCamera>()
            .register_type::<PlayerCameraState>()
            .configure_sets(
                PreUpdate,
                CameraSystems::Input
                    .after(InputSystems::Map)
                    .before(InputSystems::Command),
            )
            .add_systems(PreUpdate, route_input.in_set(CameraSystems::Input))
            .add_systems(Update, follow_target.in_set(CameraSystems::Follow));
    }
}

fn route_input(
    mut actions: ResMut<InputActions>,
    mut cameras: Query<(&mut PlayerCamera, &mut PlayerCameraState, &Transform)>,
    targets: Query<(&CharacterLook, &LocalUp)>,
) {
    let Some((mut camera, mut state, transform)) = cameras.iter_mut().next() else {
        return;
    };
    let Ok((look, up)) = targets.get(camera.target) else {
        return;
    };
    let up = *up.0;
    if !state.initialized {
        initialize(&camera, &mut state, look, up);
    }

    let zoom = core::mem::take(&mut actions.zoom);
    let mut view = camera.view;
    if actions.toggle_view.pressed {
        view = match view {
            ViewMode::ThirdPerson => ViewMode::FirstPerson,
            ViewMode::FirstPerson => ViewMode::ThirdPerson,
        };
    }
    match view {
        ViewMode::ThirdPerson if zoom != 0.0 => {
            let distance = camera.distance - zoom * camera.zoom_step;
            if distance < camera.min_distance
                && zoom > 0.0
                && camera.distance <= camera.min_distance
            {
                view = ViewMode::FirstPerson;
            }
            camera.distance = distance.clamp(camera.min_distance, camera.max_distance);
        }
        ViewMode::FirstPerson if zoom < 0.0 && camera.view == ViewMode::FirstPerson => {
            view = ViewMode::ThirdPerson;
            camera.distance = camera.min_distance;
        }
        _ => {}
    }

    if view != camera.view {
        match view {
            // Face where the orbit was looking, so the switch keeps the view's direction.
            ViewMode::FirstPerson => {
                let heading = project(state.forward, up).unwrap_or(look.forward);
                actions.look.x -= signed_angle(look.forward, heading, up);
                actions.look.y += look.pitch - (camera.pitch - state.pitch);
            }
            ViewMode::ThirdPerson => {
                state.forward = look.forward;
                state.up = look.up;
                state.pitch = (camera.pitch - look.pitch).clamp(camera.min_pitch, camera.max_pitch);
                state.distance = camera.distance;
            }
        }
        camera.view = view;
    }

    match camera.view {
        ViewMode::ThirdPerson => {
            let look_delta = core::mem::take(&mut actions.look);
            state.forward = Quat::from_axis_angle(state.up, -look_delta.x) * state.forward;
            state.pitch = (state.pitch + look_delta.y).clamp(camera.min_pitch, camera.max_pitch);
            actions.face_movement = true;
            actions.movement_forward =
                ground_forward(transform, up).or_else(|| project(state.forward, up));
        }
        ViewMode::FirstPerson => {
            actions.face_movement = false;
            actions.movement_forward = None;
        }
    }
}

#[allow(clippy::type_complexity)]
fn follow_target(
    time: Res<Time>,
    spatial: SpatialQuery,
    sensors: Query<(), With<Sensor>>,
    zones: Query<&CameraZone>,
    targets: Query<
        (
            &Transform,
            &LocalUp,
            &CharacterLook,
            Option<&CharacterIntent>,
            Option<&InCameraZones>,
        ),
        Without<PlayerCamera>,
    >,
    mut cameras: Query<(&PlayerCamera, &mut PlayerCameraState, &mut Transform)>,
) {
    let dt = time.delta_secs();
    let rate = |rate: f32| 1.0 - (-rate * dt).exp();
    for (camera, mut state, mut transform) in &mut cameras {
        let Ok((target, up, look, intent, in_zones)) = targets.get(camera.target) else {
            continue;
        };
        let up = *up.0;
        if !state.initialized {
            initialize(camera, &mut state, look, up);
        }
        ease_up(&mut state, up, rate(camera.up_rate));

        if camera.view == ViewMode::FirstPerson {
            // Look deltas not yet consumed by a fixed tick are applied here, so the view turns
            // every frame instead of every tick.
            let pending = intent.map_or(Vec2::ZERO, |intent| intent.look);
            let forward = Quat::from_axis_angle(look.up, -pending.x) * look.forward;
            let pitch = look.pitch - pending.y;
            // Tilted onto the eased up, so crossing between gravity fields does not snap.
            let tilt = Quat::from_rotation_arc(look.up, state.up);
            let direction = tilt * (forward * pitch.cos() + look.up * pitch.sin());
            let eye = target.translation + state.up * camera.eye_height;
            *transform = Transform::from_translation(eye).looking_to(direction, state.up);
            state.zone_weight = 0.0;
            continue;
        }

        let mut preferred = camera.distance;
        let mut fixed = None;
        let active = in_zones
            .and_then(InCameraZones::active)
            .and_then(|zone| zones.get(zone).ok());
        match active.map(|zone| zone.constraint.mode) {
            Some(CameraMode::Fixed { position }) => fixed = Some(position),
            Some(CameraMode::Follow { distance, .. }) => preferred = distance,
            None => {}
        }

        let focus = target.translation + state.up * camera.focus_height;
        let direction = -state.forward * state.pitch.cos() + state.up * state.pitch.sin();
        let clear = clear_distance(&spatial, &sensors, camera, focus, direction, preferred);
        state.distance = if clear < state.distance {
            clear
        } else {
            state.distance + (clear - state.distance) * rate(camera.recover_rate)
        };
        let follow = Transform::from_translation(focus + direction * state.distance)
            .looking_to(-direction, state.up);

        if fixed.is_some() {
            state.zone_position = fixed;
        }
        let goal = if fixed.is_some() { 1.0 } else { 0.0 };
        state.zone_weight += (goal - state.zone_weight) * rate(camera.zone_rate);
        if fixed.is_none() && state.zone_weight < 1e-3 {
            state.zone_weight = 0.0;
            state.zone_position = None;
        }
        *transform = match state.zone_position {
            Some(position) if state.zone_weight > 0.0 => {
                // Overhead views use the orbit heading as screen-up, avoiding the look-at pole.
                let overhead =
                    Transform::from_translation(position).looking_at(focus, state.forward);
                let weight = smoothstep(state.zone_weight);
                Transform {
                    translation: follow.translation.lerp(overhead.translation, weight),
                    rotation: follow.rotation.slerp(overhead.rotation, weight).normalize(),
                    ..follow
                }
            }
            _ => follow,
        };
    }
}

fn initialize(
    camera: &PlayerCamera,
    state: &mut PlayerCameraState,
    look: &CharacterLook,
    up: Vec3,
) {
    *state = PlayerCameraState {
        forward: project(look.forward, up).unwrap_or_else(|| up.any_orthonormal_vector()),
        up,
        pitch: camera.pitch,
        distance: camera.distance,
        zone_weight: 0.0,
        zone_position: None,
        initialized: true,
    };
}

/// Turns the view's up part of the way toward `up`, carrying the heading with it; projection
/// alone could reverse the heading when gravity flips.
fn ease_up(state: &mut PlayerCameraState, up: Vec3, blend: f32) {
    let turn = Quat::IDENTITY.slerp(Quat::from_rotation_arc(state.up, up), blend);
    let eased = (turn * state.up).normalize();
    state.forward =
        project(turn * state.forward, eased).unwrap_or_else(|| eased.any_orthonormal_vector());
    state.up = eased;
}

/// How far from `focus` the camera can sit along `direction` before touching solid geometry.
/// Sensors (water, camera zones) and the target itself do not block it.
fn clear_distance(
    spatial: &SpatialQuery,
    sensors: &Query<(), With<Sensor>>,
    camera: &PlayerCamera,
    focus: Vec3,
    direction: Vec3,
    distance: f32,
) -> f32 {
    let Ok(direction) = Dir3::new(direction) else {
        return distance;
    };
    let radius = camera.collision_radius.max(0.01);
    spatial
        .cast_shape_predicate(
            &Collider::sphere(radius),
            focus,
            Quat::IDENTITY,
            direction,
            &ShapeCastConfig::from_max_distance(distance),
            &SpatialQueryFilter::default().with_excluded_entities([camera.target]),
            &|entity| !sensors.contains(entity),
        )
        .map_or(distance, |hit| hit.distance.min(distance))
}

/// Forward along the ground for a camera: screen-right stays defined when it looks straight
/// down, so overhead views keep a movement frame.
pub fn ground_forward(camera: &Transform, up: Vec3) -> Option<Vec3> {
    let right = *camera.right();
    let right = (right - up * right.dot(up)).try_normalize()?;
    Some(up.cross(right))
}

fn project(direction: Vec3, up: Vec3) -> Option<Vec3> {
    (direction - up * direction.dot(up)).try_normalize()
}

/// Angle about `up` from `from` to `to`, positive counterclockwise seen from above.
fn signed_angle(from: Vec3, to: Vec3, up: Vec3) -> f32 {
    from.cross(to).dot(up).atan2(from.dot(to))
}

fn smoothstep(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}
