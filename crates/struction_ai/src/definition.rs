//! The plain-data form of a behavior tree, as authored in a `brain` asset.
//!
//! A brain is shared by every entity whose definition names it; this is the description of the
//! tree, and per-entity progress lives elsewhere ([`BrainState`](crate::BrainState)). The format
//! is the default serde (and Reflect) enum representation: a node is a variant name, or an
//! object with one key, the variant name. No renaming attributes, so both loaders read the same
//! files. Files are JSONC, so comments are welcome.
//!
//! ```jsonc
//! // ai/simple_ogre.jsonc
//! {
//!   "root": {
//!     // Selector: the first child that does not fail wins, and it is re-evaluated from the top
//!     // every tick, so earlier children interrupt later ones.
//!     "Selector": { "children": [
//!       // Survival overrides any order.
//!       { "Sequence": { "children": [
//!         { "Condition": { "name": "health/below", "args": { "fraction": { "Float": 0.25 } } } },
//!         { "Action": { "name": "minions/ogre/flee" } }
//!       ] } },
//!       // Follow the master's current order, if there is one.
//!       { "Sequence": { "children": [
//!         { "Condition": { "name": "orders/any" } },
//!         "ExecuteOrder"
//!       ] } },
//!       // Walk to the player while it is visible; runs across ticks until `until` holds.
//!       { "Sequence": { "children": [
//!         { "Condition": { "name": "sensing/sees", "args": { "definition": { "Str": "player" } } } },
//!         { "Task": {
//!           "start": { "name": "minions/ogre/chase" },
//!           "until": { "name": "minions/ogre/arrived" },
//!           "cancel": { "name": "minions/ogre/stop" }
//!         } }
//!       ] } },
//!       { "Action": { "name": "minions/ogre/idle" } }
//!     ] }
//!   }
//! }
//! ```
//!
//! Nodes:
//!
//! - `Sequence { children }`: runs children in order and resumes at the one that was running.
//!   Fails at the first failure; succeeds after the last success.
//! - `Selector { children }`: tries children in order, always from the first. Succeeds or keeps
//!   running with the first child that does not fail. When a higher-priority child takes over
//!   from a running one, the running subtree is interrupted (see [`Task`](NodeDef::Task)).
//! - `Parallel { policy, children }`: runs every child each tick. `RequireAll` fails as soon as
//!   one child fails and succeeds when all have succeeded; `RequireOne` succeeds as soon as one
//!   succeeds and fails when all have failed. Children that already finished with the
//!   non-deciding result stay finished unless they are pure conditions, which are re-checked
//!   every tick (`Parallel` of a `Condition` and a `Task` is a guarded task). At most 64
//!   children.
//! - `Inverter { children }`: exactly one child; swaps success and failure.
//! - `Condition { name, args }`: succeeds when the registered condition holds.
//! - `Action { name, args }`: queues the registered action on the entity and succeeds.
//!   Actions are instantaneous, so it never runs.
//! - `Task { start, until, cancel }`: queues the `start` action once and keeps running until
//!   the `until` condition holds, then succeeds. The action typically inserts a state component
//!   ("walk to X") that the condition observes. If the task is interrupted, `cancel` (optional)
//!   is queued so the state can be removed.
//! - `ExecuteOrder`: pops the entity's current order and queues its action; fails if there is no
//!   order or the entity may not perform it.
//!
//! Arguments are `{ "Bool": true }`, `{ "Int": 3 }`, `{ "Float": 0.25 }` or `{ "Str": "x" }`.
//! `args` may be omitted.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use struction_core::{ActionArgs, ArgValue};

/// A behavior tree asset.
#[derive(Reflect, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct BehaviorTreeDef {
    pub root: NodeDef,
}

#[derive(Reflect, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum NodeDef {
    Sequence {
        children: Vec<NodeDef>,
    },
    Selector {
        children: Vec<NodeDef>,
    },
    Parallel {
        policy: ParallelPolicy,
        children: Vec<NodeDef>,
    },
    /// Takes a list because reflection cannot hold a node inside a node; it must have exactly
    /// one child.
    Inverter {
        children: Vec<NodeDef>,
    },
    Condition(LeafDef),
    Action(LeafDef),
    Task(TaskDef),
    ExecuteOrder,
}

#[derive(Reflect, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParallelPolicy {
    RequireAll,
    RequireOne,
}

/// A reference to a registered action or condition, with arguments.
#[derive(Reflect, Serialize, Deserialize, Clone, PartialEq, Default, Debug)]
#[reflect(Default)]
pub struct LeafDef {
    pub name: String,
    #[serde(default)]
    pub args: BTreeMap<String, ArgDef>,
}

impl LeafDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            args: BTreeMap::new(),
        }
    }

    pub fn arg(mut self, name: impl Into<String>, value: ArgDef) -> Self {
        self.args.insert(name.into(), value);
        self
    }

    pub(crate) fn action_args(&self) -> ActionArgs {
        self.args
            .iter()
            .fold(ActionArgs::new(), |args, (name, value)| {
                args.with(name.clone(), ArgValue::from(value))
            })
    }
}

#[derive(Reflect, Serialize, Deserialize, Clone, PartialEq, Default, Debug)]
#[reflect(Default)]
pub struct TaskDef {
    pub start: LeafDef,
    pub until: LeafDef,
    #[serde(default)]
    pub cancel: Option<LeafDef>,
}

/// An argument value as written in data. Entities cannot be named in data.
#[derive(Reflect, Serialize, Deserialize, Clone, PartialEq, Debug)]
pub enum ArgDef {
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
}

impl From<&ArgDef> for ArgValue {
    fn from(value: &ArgDef) -> Self {
        match value {
            ArgDef::Bool(v) => ArgValue::Bool(*v),
            ArgDef::Int(v) => ArgValue::Int(*v),
            ArgDef::Float(v) => ArgValue::Float(*v),
            ArgDef::Str(v) => ArgValue::Str(v.clone()),
        }
    }
}
