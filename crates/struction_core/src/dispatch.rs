//! Queued action invocations and reactions.
//!
//! Invoking an action only appends an [`ActionInvocation`] to the [`ActionQueue`]. The queue is
//! drained by one exclusive system in [`CoreSet::Dispatch`](crate::CoreSet::Dispatch), never
//! while other systems iterate. Reactions run inside that drain, depth first: an invocation runs
//! its `before` reactions, then the action, then its `after` reactions, and each reaction is
//! itself an invocation with its own reactions.
//!
//! Reactions bind to invocations, never to state changes. Which reactions fire is decided when
//! the invocation starts, so an action that despawns or re-parents its target does not change who
//! reacts to it.

use std::collections::VecDeque;

use bevy::prelude::*;

use crate::actions::{ActionArgs, ActionCall, ActionError, ActionName, ActionRegistry};
use crate::relations::{MasterIs, Order, Orders, Wards};

/// A request to run `action` on `target`.
#[derive(Clone, Debug)]
pub struct ActionInvocation {
    pub action: ActionName,
    pub target: Entity,
    pub args: ActionArgs,
    depth: u32,
}

impl ActionInvocation {
    pub fn new(action: impl Into<ActionName>, target: Entity, args: ActionArgs) -> Self {
        Self {
            action: action.into(),
            target,
            args,
            depth: 0,
        }
    }
}

/// Invocations waiting for the next [`CoreSet::Dispatch`](crate::CoreSet::Dispatch).
#[derive(Resource, Default)]
pub struct ActionQueue {
    queue: VecDeque<ActionInvocation>,
    /// Depth of the invocation currently executing, so what it queues counts as nested.
    running_depth: Option<u32>,
}

impl ActionQueue {
    pub fn invoke(&mut self, mut invocation: ActionInvocation) {
        invocation.depth = self.running_depth.map_or(0, |depth| depth + 1);
        self.queue.push_back(invocation);
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

/// How deeply invocations may nest (reactions of reactions, actions invoking actions) before the
/// chain is cut with [`ActionError::DepthExceeded`].
#[derive(Resource, Clone, Copy, Debug)]
pub struct ReactionDepthLimit(pub u32);

impl Default for ReactionDepthLimit {
    fn default() -> Self {
        Self(16)
    }
}

/// Failures of queued invocations, oldest first. They are also logged.
#[derive(Resource, Default, Debug)]
pub struct ActionErrors(Vec<ActionError>);

impl ActionErrors {
    const CAPACITY: usize = 128;

    fn record(&mut self, error: ActionError) {
        error!("{error}");
        if self.0.len() == Self::CAPACITY {
            self.0.remove(0);
        }
        self.0.push(error);
    }

    pub fn iter(&self) -> impl Iterator<Item = &ActionError> {
        self.0.iter()
    }

    pub fn drain(&mut self) -> Vec<ActionError> {
        std::mem::take(&mut self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Whose actions a reaction listens to, relative to the entity that owns the reaction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReactionSource {
    /// The owner's own actions.
    This,
    /// Actions invoked on the owner's master.
    Master,
    /// Actions invoked on any of the owner's wards.
    Wards,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ReactionHook {
    /// Runs before the observed action.
    Before,
    /// Runs after the observed action completed.
    After,
}

/// "When `source` gets `on` invoked, `call` on me, at `hook`."
#[derive(Clone, PartialEq, Debug)]
pub struct Reaction {
    pub source: ReactionSource,
    pub hook: ReactionHook,
    pub on: ActionName,
    pub call: ActionName,
    pub args: ActionArgs,
}

impl Reaction {
    fn new(
        hook: ReactionHook,
        source: ReactionSource,
        on: impl Into<ActionName>,
        call: impl Into<ActionName>,
    ) -> Self {
        Self {
            source,
            hook,
            on: on.into(),
            call: call.into(),
            args: ActionArgs::new(),
        }
    }

    pub fn before(
        source: ReactionSource,
        on: impl Into<ActionName>,
        call: impl Into<ActionName>,
    ) -> Self {
        Self::new(ReactionHook::Before, source, on, call)
    }

    pub fn after(
        source: ReactionSource,
        on: impl Into<ActionName>,
        call: impl Into<ActionName>,
    ) -> Self {
        Self::new(ReactionHook::After, source, on, call)
    }

    pub fn with_args(mut self, args: ActionArgs) -> Self {
        self.args = args;
        self
    }

    /// Checks that both actions exist and that `args` match the called action's signature.
    pub fn validate(&self, registry: &ActionRegistry) -> Result<(), ActionError> {
        registry.resolve(&self.on)?;
        registry.validate_call(&self.call, &self.args)
    }
}

/// Reactions declared by an entity.
#[derive(Component, Default, Clone, Debug)]
pub struct Reactions(pub Vec<Reaction>);

/// Queues `action` on `target`.
pub fn invoke_action(
    world: &mut World,
    action: impl Into<ActionName>,
    target: Entity,
    args: ActionArgs,
) {
    world
        .resource_mut::<ActionQueue>()
        .invoke(ActionInvocation::new(action, target, args));
}

fn wards_of(world: &World, master: Entity) -> Vec<Entity> {
    world
        .get::<Wards>(master)
        .map(|wards| wards.to_vec())
        .unwrap_or_default()
}

/// Notify: queues `action` on every ward of `master`.
pub fn notify_wards(
    world: &mut World,
    master: Entity,
    action: impl Into<ActionName>,
    args: ActionArgs,
) {
    let action = action.into();
    let wards = wards_of(world, master);
    let mut queue = world.resource_mut::<ActionQueue>();
    for ward in wards {
        queue.invoke(ActionInvocation::new(action.clone(), ward, args.clone()));
    }
}

/// Order: hands every ward of `master` an [`Order`] to consider. Nothing is executed.
pub fn order_wards(
    world: &mut World,
    master: Entity,
    action: impl Into<ActionName>,
    args: ActionArgs,
) {
    let action = action.into();
    for ward in wards_of(world, master) {
        let order = Order {
            action: action.clone(),
            args: args.clone(),
            issuer: master,
        };
        world
            .entity_mut(ward)
            .entry::<Orders>()
            .or_default()
            .into_mut()
            .push(order);
    }
}

/// Deferred versions of [`invoke_action`], [`notify_wards`] and [`order_wards`].
pub trait ActionCommands {
    fn invoke_action(&mut self, action: impl Into<ActionName>, target: Entity, args: ActionArgs);
    fn notify_wards(&mut self, master: Entity, action: impl Into<ActionName>, args: ActionArgs);
    fn order_wards(&mut self, master: Entity, action: impl Into<ActionName>, args: ActionArgs);
}

impl ActionCommands for Commands<'_, '_> {
    fn invoke_action(&mut self, action: impl Into<ActionName>, target: Entity, args: ActionArgs) {
        let action = action.into();
        self.queue(move |world: &mut World| invoke_action(world, action, target, args));
    }

    fn notify_wards(&mut self, master: Entity, action: impl Into<ActionName>, args: ActionArgs) {
        let action = action.into();
        self.queue(move |world: &mut World| notify_wards(world, master, action, args));
    }

    fn order_wards(&mut self, master: Entity, action: impl Into<ActionName>, args: ActionArgs) {
        let action = action.into();
        self.queue(move |world: &mut World| order_wards(world, master, action, args));
    }
}

/// Drains the queue, running each invocation with its reactions.
pub(crate) fn dispatch_actions(world: &mut World) {
    while let Some(invocation) = next_invocation(world) {
        execute(world, invocation);
    }
    world.resource_mut::<ActionQueue>().running_depth = None;
}

fn next_invocation(world: &mut World) -> Option<ActionInvocation> {
    world.resource_mut::<ActionQueue>().queue.pop_front()
}

fn execute(world: &mut World, invocation: ActionInvocation) {
    if let Err(error) = run(world, invocation) {
        world.resource_mut::<ActionErrors>().record(error);
    }
}

fn run(world: &mut World, invocation: ActionInvocation) -> Result<(), ActionError> {
    let ActionInvocation {
        action,
        target,
        args,
        depth,
    } = invocation;

    let limit = world.resource::<ReactionDepthLimit>().0;
    if depth > limit {
        return Err(ActionError::DepthExceeded {
            action,
            target,
            limit,
        });
    }

    let registry = world.resource::<ActionRegistry>();
    let id = registry.resolve(&action)?;
    let meta = registry.meta(id);
    let system = registry.system(id);
    let args = meta.resolve_args(&args)?;
    let target_ref = world
        .get_entity(target)
        .map_err(|_| ActionError::TargetMissing {
            action: action.clone(),
            target,
        })?;
    if let Some(missing) = meta
        .requires
        .iter()
        .find(|required| !target_ref.contains_type_id(required.type_id))
    {
        return Err(ActionError::MissingComponent {
            action,
            target,
            component: missing.name,
        });
    }

    let reactions = matching_reactions(world, target, &action);
    fire(world, &reactions, ReactionHook::Before, depth);

    if world.get_entity(target).is_err() {
        return Err(ActionError::TargetMissing { action, target });
    }
    let previous = world
        .resource_mut::<ActionQueue>()
        .running_depth
        .replace(depth);
    let outcome = world.run_system_with(
        system,
        ActionCall {
            action: action.clone(),
            target,
            args,
        },
    );
    world.resource_mut::<ActionQueue>().running_depth = previous;
    outcome.map_err(|error| ActionError::SystemFailed {
        action: action.clone(),
        message: error.to_string(),
    })?;

    fire(world, &reactions, ReactionHook::After, depth);
    Ok(())
}

struct Triggered {
    reactor: Entity,
    reaction: Reaction,
}

/// Reactions bound to `action` being invoked on `target`: the target's own, those of its wards
/// listening to their master, and its master's listening to its wards.
fn matching_reactions(world: &World, target: Entity, action: &ActionName) -> Vec<Triggered> {
    let mut found = Vec::new();
    let mut scan = |reactor: Entity, source: ReactionSource| {
        let Some(reactions) = world.get::<Reactions>(reactor) else {
            return;
        };
        for reaction in &reactions.0 {
            if reaction.source == source && reaction.on == *action {
                found.push(Triggered {
                    reactor,
                    reaction: reaction.clone(),
                });
            }
        }
    };
    scan(target, ReactionSource::This);
    if let Some(wards) = world.get::<Wards>(target) {
        for ward in wards.iter() {
            scan(ward, ReactionSource::Master);
        }
    }
    if let Some(master) = world.get::<MasterIs>(target) {
        scan(master.0, ReactionSource::Wards);
    }
    found
}

fn fire(world: &mut World, triggered: &[Triggered], hook: ReactionHook, depth: u32) {
    for Triggered { reactor, reaction } in triggered {
        // A reactor despawned by an earlier reaction has nothing left to react with.
        if reaction.hook != hook || world.get_entity(*reactor).is_err() {
            continue;
        }
        execute(
            world,
            ActionInvocation {
                action: reaction.call.clone(),
                target: *reactor,
                args: reaction.args.clone(),
                depth: depth + 1,
            },
        );
    }
}
