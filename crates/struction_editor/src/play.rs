//! Device-independent play commands. Edges accumulate until the simulation consumes them;
//! held movement persists across fixed steps. Authoring and playback remain separate worlds.
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use struction_camera::CameraSystems;
use struction_character::{InputActions, InputActionsPlugin, InputSystems};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PlayInput {
    pub movement: [f32; 2],
    pub look: [f32; 2],
    pub jump_held: bool,
    pub jump_pressed: bool,
    pub roll_pressed: bool,
    pub attack_pressed: bool,
    pub toggle_view_pressed: bool,
    pub zoom: f32,
}

#[derive(Resource, Default)]
pub(crate) struct PendingInput(pub PlayInput);

impl PendingInput {
    pub fn push(&mut self, input: PlayInput) {
        self.0.movement = Vec2::from_array(input.movement)
            .clamp_length_max(1.0)
            .to_array();
        self.0.look = (Vec2::from_array(self.0.look) + Vec2::from_array(input.look)).to_array();
        self.0.jump_held = input.jump_held;
        self.0.jump_pressed |= input.jump_pressed;
        self.0.roll_pressed |= input.roll_pressed;
        self.0.attack_pressed |= input.attack_pressed;
        self.0.toggle_view_pressed |= input.toggle_view_pressed;
        self.0.zoom += input.zoom;
    }
}

pub struct PlayInputPlugin;
impl Plugin for PlayInputPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<InputActionsPlugin>() {
            app.add_plugins(InputActionsPlugin);
        }
        app.init_resource::<PendingInput>().add_systems(
            PreUpdate,
            inject
                .after(InputSystems::Map)
                .before(InputSystems::Command)
                .before(CameraSystems::Input),
        );
    }
}

fn inject(mut pending: ResMut<PendingInput>, mut actions: ResMut<InputActions>) {
    let input = &mut pending.0;
    actions.movement = Vec2::from_array(input.movement);
    actions.look = Vec2::from_array(std::mem::take(&mut input.look));
    actions.jump.held = input.jump_held;
    actions.jump.pressed = std::mem::take(&mut input.jump_pressed);
    actions.roll.pressed = std::mem::take(&mut input.roll_pressed);
    actions.attack.pressed = std::mem::take(&mut input.attack_pressed);
    actions.toggle_view.pressed = std::mem::take(&mut input.toggle_view_pressed);
    actions.zoom = std::mem::take(&mut input.zoom);
}
