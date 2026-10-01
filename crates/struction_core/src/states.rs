//! Components a definition switches with its states: its `states` section, built by
//! `struction_data` and applied by the package that knows when each state holds.

use std::any::TypeId;
use std::sync::Arc;

use bevy::prelude::*;

/// What one state switches while it holds.
#[derive(Debug)]
pub struct StateRule {
    /// A state an extensor contributes, such as `Rolling`.
    pub state: String,
    /// Inserted while the state holds, replacing any value already there.
    pub enable: Vec<(TypeId, Box<dyn Reflect>)>,
    /// Removed while the state holds.
    pub disable: Vec<TypeId>,
}

/// An entity's state rules, in authored order. Leaving a state restores what it changed.
#[derive(Component, Clone, Debug)]
pub struct StateRules(pub Arc<[StateRule]>);
