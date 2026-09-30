//! Character package: input actions, and a character controller for variable up.
//!
//! Input flows one way. Devices are mapped to [`InputActions`] (`Move`, `Look`, `Jump`, `Roll`, `Grab`);
//! for entities marked [`PlayerControlled`] the actions are copied into a [`CharacterIntent`]
//! command; the fixed-step simulation reads only the intent. AI or remote players write intents
//! directly and never touch input.
//!
//! The controller drives a dynamic capsule with velocity changes, aligned to
//! [`LocalUp`](struction_gravity::LocalUp): it works on flat floors, around planets, and in water.
//! [`CharacterAnimationPlugin`] drives a procedural `struction_anim` rig from the controller;
//! meshes are out of scope.

use bevy::{app::PluginGroupBuilder, prelude::*};

mod animation;
mod controller;
mod input;
mod roll;

pub use roll::{RollAbility, RollActionsPlugin, RollRecovery, Rolling};

pub use animation::{CharacterAnimationPlugin, RigOf, spawn_rig};
pub use controller::{
    CharacterController, CharacterControllerPlugin, CharacterIntent, CharacterLook, CharacterState,
    CharacterSystems,
};
pub use input::{
    Binding, ButtonAction, InputActions, InputActionsPlugin, InputMap, InputSystems,
    PlayerControlled,
};

pub mod prelude {
    pub use crate::{
        Binding, ButtonAction, CharacterController, CharacterIntent, CharacterLook,
        CharacterPlugins, CharacterState, CharacterSystems, InputActions, InputMap,
        PlayerControlled, RollAbility, RollRecovery, Rolling,
    };
}

/// Input mapping and the controller. Add [`struction_physics::PhysicsPlugin`] separately.
pub struct CharacterPlugins;

impl PluginGroup for CharacterPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(InputActionsPlugin)
            .add(CharacterControllerPlugin)
    }
}
