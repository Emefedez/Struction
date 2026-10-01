//! Character package: input actions, a character controller for variable up, and the optional
//! moves it can be extended with.
//!
//! Input flows one way. Devices are mapped to [`InputActions`] (`Move`, `Look`, `Jump`, `Roll`,
//! `Attack`, `Grab`);
//! for entities marked [`PlayerControlled`] the actions are copied into a [`CharacterIntent`]
//! command; the fixed-step simulation reads only the intent. AI or remote players write intents
//! directly and never touch input.
//!
//! The controller drives a dynamic capsule with velocity changes, aligned to
//! [`LocalUp`](struction_gravity::LocalUp): it works on flat floors, around planets, and in water.
//! Timed moves are extensors a definition opts into: `dodge` ([`Roll`]) and `combat`
//! ([`Attack`]). They hold the character through [`CharacterMove`], one at a time.
//! [`CharacterAnimationPlugin`] drives a procedural `struction_anim` rig from the controller;
//! meshes are out of scope.

use bevy::{app::PluginGroupBuilder, prelude::*};

mod animation;
mod combat;
mod controller;
mod dodge;
mod input;

pub use combat::{Attack, Attacking, CombatPlugin};
pub use dodge::{DodgePlugin, Roll, Rolling};

pub use animation::{CharacterAnimationPlugin, RigOf, spawn_rig};
pub use controller::{
    CancelInto, CharacterAction, CharacterCondition, CharacterController,
    CharacterControllerPlugin, CharacterIntent, CharacterLook, CharacterMove, CharacterState,
    CharacterSystems, cancel_opened, transport,
};
pub use input::{
    Binding, ButtonAction, InputActions, InputActionsPlugin, InputMap, InputSystems,
    PlayerControlled,
};

pub mod prelude {
    pub use crate::{
        Attack, Attacking, Binding, ButtonAction, CancelInto, CharacterAction, CharacterCondition,
        CharacterController, CharacterIntent, CharacterLook, CharacterMove, CharacterPlugins,
        CharacterState, CharacterSystems, CombatPlugin, DodgePlugin, InputActions, InputMap,
        PlayerControlled, Roll, Rolling,
    };
}

/// Input mapping and the controller. Add [`struction_physics::PhysicsPlugin`] separately, and
/// the move packages a game's definitions use ([`DodgePlugin`], [`CombatPlugin`]).
pub struct CharacterPlugins;

impl PluginGroup for CharacterPlugins {
    fn build(self) -> PluginGroupBuilder {
        PluginGroupBuilder::start::<Self>()
            .add(InputActionsPlugin)
            .add(CharacterControllerPlugin)
    }
}
