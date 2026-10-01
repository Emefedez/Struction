//! Relations (`descendsFrom`, `masterIs`), the action and extensor registries, queued reactions,
//! and capability grants.
//!
//! [`CorePlugin`] runs the action machinery in `FixedUpdate` through the phases of [`CoreSet`].

mod actions;
mod dispatch;
mod extensors;
mod grants;
mod identity;
mod relations;

use bevy::prelude::*;

pub use actions::{
    ActionAppExt, ActionArgs, ActionCall, ActionDescriptor, ActionError, ActionId, ActionMeta,
    ActionName, ActionRegistry, ArgError, ArgValue, ParamSpec, ParamType, RequiredComponent,
};
pub use dispatch::{
    ActionCommands, ActionErrors, ActionInvocation, ActionQueue, Reaction, ReactionDepthLimit,
    ReactionHook, ReactionSource, Reactions, invoke_action, notify_wards, order_wards,
};
pub use extensors::{
    ExtensorAppExt, ExtensorMeta, ExtensorRegistry, OwnedComponent, Participation,
};
pub use grants::{ActionSet, GrantRecord, GrantRule, GrantsToWards, RefusesGrants};
pub use identity::{Definition, DefinitionPath, IdentityError, StableId, StableIdGenerator};
pub use relations::{MasterIs, Order, Orders, Relations, Wards};

/// Phases of the action machinery in `FixedUpdate`, in this order.
#[derive(SystemSet, Clone, PartialEq, Eq, Hash, Debug)]
pub enum CoreSet {
    /// Systems that turn state into invocations (`Health <= 0` -> `die`, a finished timer ->
    /// its completion action) or that notify and order wards. They queue; nothing runs yet.
    Invoke,
    /// The queue is drained: actions and their reactions execute. Nothing else in the core
    /// runs concurrently with this phase.
    Dispatch,
    /// Systems that observe the results of this tick's actions.
    Post,
}

#[derive(Default)]
pub struct CorePlugin {
    /// Seed of the [`StableIdGenerator`].
    pub seed: u64,
}

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActionRegistry>()
            .init_resource::<ExtensorRegistry>()
            .init_resource::<ActionQueue>()
            .init_resource::<ActionErrors>()
            .init_resource::<ReactionDepthLimit>()
            .insert_resource(StableIdGenerator::new(self.seed))
            .configure_sets(
                FixedUpdate,
                (CoreSet::Invoke, CoreSet::Dispatch, CoreSet::Post).chain(),
            )
            .add_systems(
                FixedUpdate,
                dispatch::dispatch_actions.in_set(CoreSet::Dispatch),
            )
            .add_observer(grants::on_master_inserted)
            .add_observer(grants::on_master_discarded);
    }
}
