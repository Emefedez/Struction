use bevy::{
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
};

use crate::controller::CharacterIntent;

/// A physical input that can trigger a button action.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
}

/// Edge and level state of a button action for the current frame.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ButtonAction {
    pub held: bool,
    pub pressed: bool,
    pub released: bool,
}

/// What the player wants, independent of the device that produced it. Gameplay reads this and
/// nothing lower-level.
#[derive(Resource, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Resource)]
pub struct InputActions {
    /// `Move`: x to the right, y forward, length at most 1.
    pub movement: Vec2,
    /// Optional world-space forward for movement, captured by a camera input adapter.
    /// `None` keeps movement relative to the character's heading.
    pub movement_forward: Option<Vec3>,
    /// Set by a camera input adapter whose view orbits freely: the character turns toward its
    /// movement instead of following `look`.
    pub face_movement: bool,
    /// `Look`: yaw and pitch change this frame in radians (positive x turns right, positive y
    /// looks down).
    pub look: Vec2,
    pub jump: ButtonAction,
    /// `Grab`: bound now, consumed by the grabbing package later.
    pub grab: ButtonAction,
    /// `ToggleView`: switch between third and first person.
    pub toggle_view: ButtonAction,
    /// `Zoom`: positive moves the camera closer, in scroll lines this frame.
    pub zoom: f32,
}

/// Keyboard and mouse bindings. Other devices add their own mapping systems in
/// [`InputSystems::Map`] that merge into [`InputActions`].
#[derive(Resource, Reflect, Clone, Debug, PartialEq)]
#[reflect(Resource)]
pub struct InputMap {
    pub forward: Vec<Binding>,
    pub back: Vec<Binding>,
    pub left: Vec<Binding>,
    pub right: Vec<Binding>,
    pub jump: Vec<Binding>,
    pub grab: Vec<Binding>,
    pub toggle_view: Vec<Binding>,
    /// Radians of look per pixel of mouse motion.
    pub look_sensitivity: f32,
    /// `Zoom` per pixel of touchpad scrolling; wheels report whole lines.
    pub zoom_pixel_scale: f32,
}

impl Default for InputMap {
    fn default() -> Self {
        Self {
            forward: vec![Binding::Key(KeyCode::KeyW), Binding::Key(KeyCode::ArrowUp)],
            back: vec![
                Binding::Key(KeyCode::KeyS),
                Binding::Key(KeyCode::ArrowDown),
            ],
            left: vec![
                Binding::Key(KeyCode::KeyA),
                Binding::Key(KeyCode::ArrowLeft),
            ],
            right: vec![
                Binding::Key(KeyCode::KeyD),
                Binding::Key(KeyCode::ArrowRight),
            ],
            jump: vec![Binding::Key(KeyCode::Space)],
            grab: vec![
                Binding::Key(KeyCode::KeyE),
                Binding::Mouse(MouseButton::Left),
            ],
            toggle_view: vec![Binding::Key(KeyCode::KeyV)],
            look_sensitivity: 0.003,
            zoom_pixel_scale: 0.02,
        }
    }
}

/// Marks the character that receives [`InputActions`] as its [`CharacterIntent`].
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
#[require(CharacterIntent)]
pub struct PlayerControlled;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InputSystems {
    /// Devices are mapped to [`InputActions`].
    Map,
    /// [`InputActions`] become [`CharacterIntent`] commands.
    Command,
}

/// Runs in `PreUpdate`, so intents are ready before the fixed-step loop of the same frame.
pub struct InputActionsPlugin;

impl Plugin for InputActionsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InputActions>()
            .init_resource::<InputMap>()
            .register_type::<InputActions>()
            .register_type::<InputMap>()
            .register_type::<PlayerControlled>()
            .configure_sets(
                PreUpdate,
                (InputSystems::Map, InputSystems::Command).chain(),
            )
            .add_systems(
                PreUpdate,
                (reset_actions, map_keyboard_and_mouse)
                    .chain()
                    .in_set(InputSystems::Map),
            )
            .add_systems(PreUpdate, send_player_intent.in_set(InputSystems::Command));
    }
}

fn reset_actions(mut actions: ResMut<InputActions>) {
    *actions = InputActions::default();
}

fn map_keyboard_and_mouse(
    map: Res<InputMap>,
    keys: Option<Res<ButtonInput<KeyCode>>>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    motion: Option<Res<AccumulatedMouseMotion>>,
    scroll: Option<Res<AccumulatedMouseScroll>>,
    mut actions: ResMut<InputActions>,
) {
    let held = |bindings: &[Binding]| {
        bindings.iter().any(|binding| match binding {
            Binding::Key(key) => keys.as_ref().is_some_and(|keys| keys.pressed(*key)),
            Binding::Mouse(button) => mouse.as_ref().is_some_and(|mouse| mouse.pressed(*button)),
        })
    };
    let pressed = |bindings: &[Binding]| {
        bindings.iter().any(|binding| match binding {
            Binding::Key(key) => keys.as_ref().is_some_and(|keys| keys.just_pressed(*key)),
            Binding::Mouse(button) => mouse
                .as_ref()
                .is_some_and(|mouse| mouse.just_pressed(*button)),
        })
    };
    let released = |bindings: &[Binding]| {
        bindings.iter().any(|binding| match binding {
            Binding::Key(key) => keys.as_ref().is_some_and(|keys| keys.just_released(*key)),
            Binding::Mouse(button) => mouse
                .as_ref()
                .is_some_and(|mouse| mouse.just_released(*button)),
        })
    };
    let axis = |positive: &[Binding], negative: &[Binding]| {
        f32::from(held(positive)) - f32::from(held(negative))
    };
    let button = |bindings: &[Binding]| ButtonAction {
        held: held(bindings),
        pressed: pressed(bindings),
        released: released(bindings),
    };

    let movement = Vec2::new(axis(&map.right, &map.left), axis(&map.forward, &map.back));
    actions.movement = (actions.movement + movement).clamp_length_max(1.0);
    if let Some(motion) = motion {
        actions.look += motion.delta * map.look_sensitivity;
    }
    if let Some(scroll) = scroll {
        actions.zoom += match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y * map.zoom_pixel_scale,
        };
    }
    let jump = button(&map.jump);
    let grab = button(&map.grab);
    let toggle_view = button(&map.toggle_view);
    actions.jump = merge(actions.jump, jump);
    actions.grab = merge(actions.grab, grab);
    actions.toggle_view = merge(actions.toggle_view, toggle_view);
}

/// Combines the same action from two devices.
fn merge(a: ButtonAction, b: ButtonAction) -> ButtonAction {
    ButtonAction {
        held: a.held || b.held,
        pressed: a.pressed || b.pressed,
        released: a.released || b.released,
    }
}

/// The only place input reaches the simulation: as a command on the controlled character.
fn send_player_intent(
    actions: Res<InputActions>,
    mut players: Query<&mut CharacterIntent, With<PlayerControlled>>,
) {
    for mut intent in &mut players {
        intent.movement = actions.movement;
        intent.movement_forward = actions.movement_forward;
        intent.face_movement = actions.face_movement;
        intent.jump_held = actions.jump.held;
        // Edges and look deltas are latched: several frames can pass between fixed ticks, and
        // the simulation clears them when it consumes them.
        intent.jump_requested |= actions.jump.pressed;
        intent.look += actions.look;
    }
}
