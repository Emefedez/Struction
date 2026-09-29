//! Behavior tree runtime.
//!
//! A [`BehaviorTreeDef`] is compiled once into a [`BehaviorTree`]: leaves are resolved against
//! the action and condition registries (so a missing or mistyped reference is an error naming it,
//! at registration and not in the middle of a game) and nodes are laid out in preorder, which
//! makes "the subtree of node n" a contiguous range. Compiled trees are shared by every entity
//! that names them in its [`Brain`]; what an entity remembers between ticks is its
//! [`BrainState`], one small slot per node.
//!
//! The `think` system ticks each brain once per fixed step, from the root. A node that returns
//! [`Status::Running`] leaves its position in its slot so the next tick resumes there. Leaves
//! only queue actions on the [`ActionQueue`]; they run later, in the core's dispatch phase.
//!
//! Interruption: a `Selector` re-evaluates from its first child every tick. When a different
//! child takes over from one that was running, the old subtree is reset, and every `Task` in it
//! that had started queues its `cancel` action so the state component it inserted can be
//! removed. A finished node always leaves its subtree idle.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use struction_core::{
    ActionAppExt as _, ActionInvocation, ActionName, ActionQueue, ActionRegistry, ActionSet,
    Orders,
};

use crate::condition::{ConditionCall, ConditionId, ConditionName, ConditionRegistry};
use crate::definition::{BehaviorTreeDef, LeafDef, NodeDef, ParallelPolicy};
use crate::error::{BrainError, BrainErrors};
use crate::{ActionArgs, Status};

const MAX_PARALLEL_CHILDREN: usize = 64;

/// The behavior tree asset an entity's definition names (`"brain": "ai/simple_ogre"`).
#[derive(Component, Clone, PartialEq, Eq, Debug)]
#[require(BrainState)]
pub struct Brain(pub String);

impl Brain {
    pub fn new(tree: impl Into<String>) -> Self {
        Self(tree.into())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
enum Slot {
    #[default]
    Idle,
    /// A `Sequence` resuming at, or a `Selector` last running, this child index.
    Child(u32),
    /// A `Task` that started and has not finished.
    Task,
    /// A `Parallel` remembering which impure children already reached their non-deciding result.
    Latched(u64),
}

/// What one entity remembers about its tree between ticks. Runtime state, rebuilt after loading.
///
/// It is reset when the tree it belongs to is replaced (hot reload), without cancelling tasks.
#[derive(Component, Default, Debug)]
pub struct BrainState {
    generation: u32,
    slots: Vec<Slot>,
    status: Option<Status>,
    missing_reported: bool,
}

impl BrainState {
    /// Result of the last tick, or `None` before the first one.
    pub fn status(&self) -> Option<Status> {
        self.status
    }

    pub fn is_running(&self) -> bool {
        self.status == Some(Status::Running)
    }
}

struct Node {
    kind: Kind,
    /// End of this node's subtree: it spans `id..end` in preorder.
    end: u32,
    /// Side-effect free (conditions and combinations of them): safe to re-evaluate every tick.
    pure: bool,
}

struct ActionRef {
    name: ActionName,
    args: ActionArgs,
}

struct ConditionRef {
    id: ConditionId,
    name: ConditionName,
    args: ActionArgs,
}

enum Kind {
    Sequence(Vec<u32>),
    Selector(Vec<u32>),
    Parallel {
        policy: ParallelPolicy,
        children: Vec<u32>,
    },
    Inverter(u32),
    Condition(ConditionRef),
    Action(ActionRef),
    Task {
        start: ActionRef,
        until: ConditionRef,
        cancel: Option<ActionRef>,
    },
    ExecuteOrder,
}

/// A validated tree, shared between entities.
pub struct BehaviorTree {
    name: String,
    generation: u32,
    nodes: Vec<Node>,
}

impl BehaviorTree {
    /// Resolves every reference in `def`. All problems are reported, not just the first.
    pub fn compile(
        name: impl Into<String>,
        def: &BehaviorTreeDef,
        actions: &ActionRegistry,
        conditions: &ConditionRegistry,
    ) -> Result<Self, Vec<BrainError>> {
        let mut compiler = Compiler {
            tree: name.into(),
            actions,
            conditions,
            nodes: Vec::new(),
            errors: Vec::new(),
        };
        let root = format!("root:{}", kind_name(&def.root));
        compiler.node(&def.root, &root);
        if !compiler.errors.is_empty() {
            return Err(compiler.errors);
        }
        Ok(Self {
            name: compiler.tree,
            generation: 0,
            nodes: compiler.nodes,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Number of nodes.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

fn kind_name(def: &NodeDef) -> &'static str {
    match def {
        NodeDef::Sequence { .. } => "Sequence",
        NodeDef::Selector { .. } => "Selector",
        NodeDef::Parallel { .. } => "Parallel",
        NodeDef::Inverter { .. } => "Inverter",
        NodeDef::Condition(_) => "Condition",
        NodeDef::Action(_) => "Action",
        NodeDef::Task(_) => "Task",
        NodeDef::ExecuteOrder => "ExecuteOrder",
    }
}

struct Compiler<'a> {
    tree: String,
    actions: &'a ActionRegistry,
    conditions: &'a ConditionRegistry,
    nodes: Vec<Node>,
    errors: Vec<BrainError>,
}

impl Compiler<'_> {
    fn shape(&mut self, path: &str, problem: impl Into<String>) {
        self.errors.push(BrainError::Shape {
            tree: self.tree.clone(),
            node: path.into(),
            problem: problem.into(),
        });
    }

    fn action(&mut self, leaf: &LeafDef, path: &str) -> ActionRef {
        let name = ActionName::new(&leaf.name);
        let mut args = leaf.action_args();
        let checked = self
            .actions
            .resolve(&name)
            .and_then(|id| self.actions.meta(id).resolve_args(&args));
        match checked {
            Ok(resolved) => args = resolved,
            Err(error) => self.errors.push(BrainError::Action {
                tree: self.tree.clone(),
                node: path.into(),
                error,
            }),
        }
        ActionRef { name, args }
    }

    fn condition(&mut self, leaf: &LeafDef, path: &str) -> ConditionRef {
        let name = ConditionName::new(&leaf.name);
        let mut args = leaf.action_args();
        let checked = self.conditions.resolve(&name).and_then(|id| {
            self.conditions
                .meta(id)
                .resolve_args(&args)
                .map(|resolved| (id, resolved))
        });
        let id = match checked {
            Ok((id, resolved)) => {
                args = resolved;
                id
            }
            Err(error) => {
                self.errors.push(BrainError::Condition {
                    tree: self.tree.clone(),
                    node: path.into(),
                    error,
                });
                // Never run: the tree is discarded when errors were found.
                ConditionId::placeholder()
            }
        };
        ConditionRef { id, name, args }
    }

    fn children(&mut self, defs: &[NodeDef], path: &str) -> Vec<u32> {
        defs.iter()
            .enumerate()
            .map(|(i, child)| self.node(child, &format!("{path}/{i}:{}", kind_name(child))))
            .collect()
    }

    fn node(&mut self, def: &NodeDef, path: &str) -> u32 {
        let id = self.nodes.len() as u32;
        self.nodes.push(Node {
            kind: Kind::ExecuteOrder,
            end: 0,
            pure: false,
        });
        let (kind, pure) = match def {
            NodeDef::Sequence { children } | NodeDef::Selector { children } => {
                if children.is_empty() {
                    self.shape(path, "needs at least one child");
                }
                let ids = self.children(children, path);
                let pure = self.all_pure(&ids);
                let kind = if matches!(def, NodeDef::Sequence { .. }) {
                    Kind::Sequence(ids)
                } else {
                    Kind::Selector(ids)
                };
                (kind, pure)
            }
            NodeDef::Parallel { policy, children } => {
                if children.is_empty() {
                    self.shape(path, "needs at least one child");
                }
                if children.len() > MAX_PARALLEL_CHILDREN {
                    self.shape(
                        path,
                        format!("has {} children, at most {MAX_PARALLEL_CHILDREN} are supported", children.len()),
                    );
                }
                let ids = self.children(children, path);
                let pure = self.all_pure(&ids);
                (
                    Kind::Parallel {
                        policy: *policy,
                        children: ids,
                    },
                    pure,
                )
            }
            NodeDef::Inverter { children } => {
                if children.len() != 1 {
                    self.shape(
                        path,
                        format!("needs exactly one child, has {}", children.len()),
                    );
                }
                let ids = self.children(children, path);
                let pure = self.all_pure(&ids);
                (Kind::Inverter(ids.first().copied().unwrap_or(id)), pure)
            }
            NodeDef::Condition(leaf) => (Kind::Condition(self.condition(leaf, path)), true),
            NodeDef::Action(leaf) => (Kind::Action(self.action(leaf, path)), false),
            NodeDef::Task(task) => {
                let start = self.action(&task.start, &format!("{path}.start"));
                let until = self.condition(&task.until, &format!("{path}.until"));
                let cancel = task
                    .cancel
                    .as_ref()
                    .map(|leaf| self.action(leaf, &format!("{path}.cancel")));
                (
                    Kind::Task {
                        start,
                        until,
                        cancel,
                    },
                    false,
                )
            }
            NodeDef::ExecuteOrder => (Kind::ExecuteOrder, false),
        };
        let end = self.nodes.len() as u32;
        self.nodes[id as usize] = Node { kind, end, pure };
        id
    }

    fn all_pure(&self, ids: &[u32]) -> bool {
        ids.iter().all(|&id| self.nodes[id as usize].pure)
    }
}

/// Compiled trees by name, shared by every entity that uses them.
#[derive(Resource, Default)]
pub struct BehaviorTrees {
    trees: HashMap<String, Arc<BehaviorTree>>,
    generations: u32,
}

impl BehaviorTrees {
    /// Compiles `def` against the registered actions and conditions and stores it under `name`,
    /// replacing an earlier tree of that name (hot reload). Register the actions and conditions
    /// it references first.
    pub fn register(
        world: &mut World,
        name: impl Into<String>,
        def: &BehaviorTreeDef,
    ) -> Result<(), Vec<BrainError>> {
        let name = name.into();
        world.init_resource::<ActionRegistry>();
        world.init_resource::<ConditionRegistry>();
        world.init_resource::<Self>();
        let tree = {
            let actions = world.resource::<ActionRegistry>();
            let conditions = world.resource::<ConditionRegistry>();
            BehaviorTree::compile(name.clone(), def, actions, conditions)?
        };
        world.resource_mut::<Self>().insert(tree);
        Ok(())
    }

    fn insert(&mut self, mut tree: BehaviorTree) {
        self.generations += 1;
        tree.generation = self.generations;
        self.trees.insert(tree.name.clone(), Arc::new(tree));
    }

    /// Resolves a `brain` reference from data; the error names the missing tree.
    pub fn resolve(&self, name: &str) -> Result<&Arc<BehaviorTree>, BrainError> {
        self.trees
            .get(name)
            .ok_or_else(|| BrainError::UnknownTree { name: name.into() })
    }

    pub fn len(&self) -> usize {
        self.trees.len()
    }

    pub fn is_empty(&self) -> bool {
        self.trees.is_empty()
    }
}

/// Registration of trees on [`App`].
pub trait BehaviorTreeAppExt {
    /// Registers a tree.
    ///
    /// # Panics
    ///
    /// Panics if a reference does not resolve: like registering an action twice, it is a
    /// build-time mistake. Use [`BehaviorTrees::register`] to handle the errors.
    fn add_behavior_tree(&mut self, name: impl Into<String>, def: &BehaviorTreeDef) -> &mut Self;
}

impl BehaviorTreeAppExt for App {
    fn add_behavior_tree(&mut self, name: impl Into<String>, def: &BehaviorTreeDef) -> &mut Self {
        let name = name.into();
        if let Err(errors) = BehaviorTrees::register(self.world_mut(), name.clone(), def) {
            let errors: Vec<_> = errors.iter().map(ToString::to_string).collect();
            panic!("behavior tree `{name}` is invalid:\n{}", errors.join("\n"));
        }
        self
    }
}

/// Ticks every brain once, in entity order.
pub(crate) fn think(world: &mut World, brains: &mut QueryState<(Entity, &Brain)>) {
    let trees = world.resource::<BehaviorTrees>();
    let mut todo: Vec<_> = brains
        .iter(world)
        .map(|(entity, brain)| {
            let tree = trees.resolve(&brain.0).cloned().map_err(|_| brain.0.clone());
            (entity, tree)
        })
        .collect();
    todo.sort_by_key(|(entity, _)| *entity);

    for (entity, tree) in todo {
        let Some(mut state) = world.get_mut::<BrainState>(entity).map(|mut s| std::mem::take(&mut *s))
        else {
            continue;
        };
        match tree {
            Err(name) => {
                state.generation = 0;
                state.slots.clear();
                state.status = None;
                if !std::mem::replace(&mut state.missing_reported, true) {
                    let error = BrainError::MissingTree { entity, name };
                    world.resource_mut::<BrainErrors>().record(error);
                }
            }
            Ok(tree) => {
                if state.generation != tree.generation {
                    state.generation = tree.generation;
                    state.slots = vec![Slot::Idle; tree.nodes.len()];
                    state.missing_reported = false;
                }
                let mut runner = Runner {
                    world,
                    entity,
                    tree: &tree,
                    slots: &mut state.slots,
                };
                state.status = Some(runner.tick(0));
            }
        }
        if let Some(mut slot) = world.get_mut::<BrainState>(entity) {
            *slot = state;
        }
    }
}

struct Runner<'a> {
    world: &'a mut World,
    entity: Entity,
    tree: &'a BehaviorTree,
    slots: &'a mut [Slot],
}

impl Runner<'_> {
    fn tick(&mut self, id: u32) -> Status {
        let tree = self.tree;
        let index = id as usize;
        match &tree.nodes[index].kind {
            Kind::Sequence(children) => {
                let start = match self.slots[index] {
                    Slot::Child(i) => i as usize,
                    _ => 0,
                };
                for (i, &child) in children.iter().enumerate().skip(start) {
                    match self.tick(child) {
                        Status::Success => {}
                        Status::Failure => {
                            self.slots[index] = Slot::Idle;
                            return Status::Failure;
                        }
                        Status::Running => {
                            self.slots[index] = Slot::Child(i as u32);
                            return Status::Running;
                        }
                    }
                }
                self.slots[index] = Slot::Idle;
                Status::Success
            }
            Kind::Selector(children) => {
                let previous = match self.slots[index] {
                    Slot::Child(i) => Some(i as usize),
                    _ => None,
                };
                for (i, &child) in children.iter().enumerate() {
                    let status = self.tick(child);
                    if status == Status::Failure {
                        continue;
                    }
                    if let Some(previous) = previous.filter(|&p| p != i) {
                        self.reset(children[previous]);
                    }
                    self.slots[index] = if status == Status::Running {
                        Slot::Child(i as u32)
                    } else {
                        Slot::Idle
                    };
                    return status;
                }
                self.slots[index] = Slot::Idle;
                Status::Failure
            }
            Kind::Parallel { policy, children } => self.tick_parallel(index, *policy, children),
            Kind::Inverter(child) => match self.tick(*child) {
                Status::Success => Status::Failure,
                Status::Failure => Status::Success,
                Status::Running => Status::Running,
            },
            Kind::Condition(condition) => self.check(condition).into(),
            Kind::Action(action) => {
                self.invoke(action);
                Status::Success
            }
            Kind::Task {
                start,
                until,
                cancel: _,
            } => {
                if self.slots[index] == Slot::Task {
                    if self.check(until) {
                        self.slots[index] = Slot::Idle;
                        Status::Success
                    } else {
                        Status::Running
                    }
                } else {
                    // The action runs at this tick's dispatch, so `until` is first evaluated
                    // on the next tick, when the state it set up exists.
                    self.invoke(start);
                    self.slots[index] = Slot::Task;
                    Status::Running
                }
            }
            Kind::ExecuteOrder => self.execute_order(),
        }
    }

    fn tick_parallel(&mut self, index: usize, policy: ParallelPolicy, children: &[u32]) -> Status {
        let tree = self.tree;
        let mut latched = match self.slots[index] {
            Slot::Latched(mask) => mask,
            _ => 0,
        };
        // The result that does not decide the node: children that reach it stay finished.
        let (deciding, pending) = match policy {
            ParallelPolicy::RequireAll => (Status::Failure, Status::Success),
            ParallelPolicy::RequireOne => (Status::Success, Status::Failure),
        };
        let mut unfinished = false;
        for (i, &child) in children.iter().enumerate() {
            if latched & (1 << i) != 0 {
                continue;
            }
            let status = self.tick(child);
            if status == deciding {
                self.slots[index] = Slot::Idle;
                for &other in children {
                    self.reset(other);
                }
                return status;
            }
            if status == pending && !tree.nodes[child as usize].pure {
                latched |= 1 << i;
            } else if status == Status::Running || status == pending {
                unfinished = true;
            }
        }
        if !unfinished && latched.count_ones() as usize == children.len() {
            // Every child reached the non-deciding result: all succeeded, or all failed.
            self.slots[index] = Slot::Idle;
            return pending;
        }
        self.slots[index] = if latched == 0 {
            Slot::Idle
        } else {
            Slot::Latched(latched)
        };
        Status::Running
    }

    /// Returns a subtree to idle, cancelling the tasks in it that had started.
    fn reset(&mut self, id: u32) {
        let tree = self.tree;
        for node in id..tree.nodes[id as usize].end {
            if self.slots[node as usize] == Slot::Task
                && let Kind::Task {
                    cancel: Some(cancel),
                    ..
                } = &tree.nodes[node as usize].kind
            {
                self.invoke(cancel);
            }
            self.slots[node as usize] = Slot::Idle;
        }
    }

    fn invoke(&mut self, action: &ActionRef) {
        let invocation =
            ActionInvocation::new(action.name.clone(), self.entity, action.args.clone());
        self.world.resource_mut::<ActionQueue>().invoke(invocation);
    }

    fn check(&mut self, condition: &ConditionRef) -> bool {
        let system = self.world.resource::<ConditionRegistry>().system(condition.id);
        let call = ConditionCall {
            condition: condition.name.clone(),
            entity: self.entity,
            args: condition.args.clone(),
        };
        match self.world.run_system_with(system, call) {
            Ok(holds) => holds,
            Err(error) => {
                let error = BrainError::ConditionFailed {
                    condition: condition.name.to_string(),
                    entity: self.entity,
                    message: error.to_string(),
                };
                self.world.resource_mut::<BrainErrors>().record(error);
                false
            }
        }
    }

    fn execute_order(&mut self) -> Status {
        let entity = self.entity;
        let Some(order) = self
            .world
            .get_mut::<Orders>(entity)
            .and_then(|mut orders| orders.pop())
        else {
            return Status::Failure;
        };
        // An entity with an action set only performs what is in it; one without is unrestricted.
        if let Some(set) = self.world.get::<ActionSet>(entity)
            && !set.contains(&order.action)
        {
            let error = BrainError::OrderRefused {
                entity,
                action: order.action.to_string(),
            };
            self.world.resource_mut::<BrainErrors>().record(error);
            return Status::Failure;
        }
        let invocation = ActionInvocation::new(order.action, entity, order.args);
        self.world.resource_mut::<ActionQueue>().invoke(invocation);
        Status::Success
    }
}

impl From<bool> for Status {
    fn from(holds: bool) -> Self {
        if holds { Status::Success } else { Status::Failure }
    }
}
