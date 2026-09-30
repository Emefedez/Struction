//! Behavior trees, sensing filtered by lineage, ward orders as decision input, and boids.
//!
//! [`AiPlugin`] runs in `FixedUpdate` inside [`CoreSet::Invoke`], through the phases of
//! [`AiSet`]: entities sense, their trees think and queue actions, groups steer; the core then
//! dispatches the queued actions.
//!
//! Built-in conditions: `orders/any`, `orders/current { action }` and
//! `sensing/sees { definition }` (lineage match, anything when empty). The crate uses no randomness, and entities are visited in a
//! fixed order, so a simulation replays exactly.

mod behavior;
mod condition;
mod definition;
mod error;
mod flocking;
mod sensing;

use bevy::prelude::*;
use struction_core::{
    ActionName, CorePlugin, CoreSet, Definition, DefinitionPath, Orders, ParamType,
};

pub use behavior::{BehaviorTree, BehaviorTreeAppExt, BehaviorTrees, Brain, BrainState};
pub use condition::{
    ConditionAppExt, ConditionCall, ConditionDescriptor, ConditionError, ConditionId,
    ConditionMeta, ConditionName, ConditionRegistry,
};
pub use definition::{ArgDef, BehaviorTreeDef, LeafDef, NodeDef, ParallelPolicy, TaskDef};
pub use error::{BrainError, BrainErrors};
pub use flocking::{Boid, DesiredVelocity, Flock};
pub use sensing::{LineOfSight, Sensed, Sensing, SensingAppExt, Sightline};

/// Result of ticking a behavior tree node.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Status {
    Success,
    Failure,
    /// Not finished: the node resumes here next tick.
    Running,
}

/// Phases of the AI in `FixedUpdate`, in this order, all inside [`CoreSet::Invoke`] so what
/// they queue is dispatched in the same tick.
#[derive(SystemSet, Clone, PartialEq, Eq, Hash, Debug)]
pub enum AiSet {
    /// [`Sensed`] is rebuilt.
    Sense,
    /// Behavior trees tick and queue actions.
    Think,
    /// Continuous behavior of AI state: steering, state components such as "walk to X".
    Act,
}

/// Requires [`CorePlugin`], added before it.
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<CorePlugin>(),
            "AiPlugin needs CorePlugin, added first"
        );
        app.init_resource::<ConditionRegistry>()
            .init_resource::<BehaviorTrees>()
            .init_resource::<BrainErrors>()
            .init_resource::<Time<Fixed>>()
            .register_type::<BehaviorTreeDef>()
            .register_type::<NodeDef>()
            .register_type::<LeafDef>()
            .register_type::<TaskDef>()
            .register_type::<ArgDef>()
            .register_type::<ParallelPolicy>()
            .register_type::<Sensing>()
            .register_type::<Flock>()
            .register_type::<Boid>()
            .register_type::<DesiredVelocity>()
            .configure_sets(
                FixedUpdate,
                (AiSet::Sense, AiSet::Think, AiSet::Act)
                    .chain()
                    .in_set(CoreSet::Invoke),
            )
            .add_systems(
                FixedUpdate,
                (
                    (sensing::sense, sensing::apply_line_of_sight)
                        .chain()
                        .in_set(AiSet::Sense),
                    behavior::think.in_set(AiSet::Think),
                    flocking::flock.in_set(AiSet::Act),
                ),
            );
        register_builtin_conditions(app);
    }
}

fn register_builtin_conditions(app: &mut App) {
    app.register_condition(
        ConditionMeta::new("orders/any").doc("The master has given an order not yet carried out"),
        |In(call): In<ConditionCall>, orders: Query<&Orders>| {
            orders
                .get(call.entity)
                .is_ok_and(|orders| !orders.is_empty())
        },
    )
    .register_condition(
        ConditionMeta::new("orders/current")
            .doc("The next order to carry out is `action`")
            .param("action", ParamType::Str),
        |In(call): In<ConditionCall>, orders: Query<&Orders>| {
            let action = ActionName::new(call.args.str("action").unwrap_or_default());
            orders
                .get(call.entity)
                .ok()
                .and_then(Orders::current)
                .is_some_and(|order| order.action == action)
        },
    )
    .register_condition(
        ConditionMeta::new("sensing/sees")
            .doc("Something visible is, or descends from, `definition`; anything if it is empty")
            .param_or("definition", ParamType::Str, ""),
        |In(call): In<ConditionCall>, sensed: Query<&Sensed>, definitions: Query<&Definition>| {
            let Ok(sensed) = sensed.get(call.entity) else {
                return false;
            };
            let wanted = call.args.str("definition").unwrap_or_default();
            if wanted.is_empty() {
                return !sensed.visible.is_empty();
            }
            let wanted = DefinitionPath::new(wanted);
            sensed.visible.iter().any(|&seen| {
                definitions
                    .get(seen)
                    .is_ok_and(|definition| definition.descends_from(&wanted))
            })
        },
    );
}
