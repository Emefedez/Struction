#![allow(dead_code)]

use bevy::prelude::*;
use struction_ai::*;
use struction_core::*;

#[derive(Resource, Default)]
pub struct Log(pub Vec<String>);

/// Named facts the `flag` condition reads.
#[derive(Component, Default)]
pub struct Flags(pub Vec<&'static str>);

/// A duration-like state: "walk to `target`", moved by [`walk`] until it arrives.
#[derive(Component, Clone, Copy, Debug)]
pub struct MoveTo {
    pub target: Vec3,
    /// Meters per tick, so tests count ticks rather than seconds.
    pub step: f32,
}

pub fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins((CorePlugin::default(), AiPlugin))
        .init_resource::<Log>()
        .add_systems(FixedUpdate, walk.in_set(AiSet::Act));
    app.register_condition(
        ConditionMeta::new("flag").param("name", ParamType::Str),
        |In(call): In<ConditionCall>, flags: Query<&Flags>| {
            let name = call.args.str("name").unwrap();
            flags.get(call.entity).is_ok_and(|f| f.0.contains(&name))
        },
    );
    register_move_to(&mut app);
    app
}

fn register_move_to(app: &mut App) {
    app.register_action(
        ActionMeta::new("move_to")
            .param("x", ParamType::Float)
            .param("z", ParamType::Float),
        |In(call): In<ActionCall>, mut commands: Commands, mut log: ResMut<Log>| {
            let target = Vec3::new(
                call.args.float("x").unwrap() as f32,
                0.0,
                call.args.float("z").unwrap() as f32,
            );
            log.0.push(format!("move_to@{}", call.target));
            commands
                .entity(call.target)
                .insert(MoveTo { target, step: 1.0 });
        },
    )
    .register_action(
        ActionMeta::new("move_to/stop"),
        |In(call): In<ActionCall>, mut commands: Commands, mut log: ResMut<Log>| {
            log.0.push(format!("stop@{}", call.target));
            commands.entity(call.target).remove::<MoveTo>();
        },
    )
    .register_condition(
        ConditionMeta::new("move_to/arrived"),
        |In(call): In<ConditionCall>, walkers: Query<Has<MoveTo>>| {
            walkers.get(call.entity).is_ok_and(|walking| !walking)
        },
    );
}

/// Moves walkers and ends the state on arrival.
fn walk(mut commands: Commands, mut walkers: Query<(Entity, &MoveTo, &mut Transform)>) {
    for (entity, move_to, mut transform) in &mut walkers {
        let offset = move_to.target - transform.translation;
        if offset.length() <= move_to.step {
            transform.translation = move_to.target;
            commands.entity(entity).remove::<MoveTo>();
        } else {
            transform.translation += offset.normalize() * move_to.step;
        }
    }
}

/// Runs one fixed tick directly, independent of wall-clock time.
pub fn step(app: &mut App) {
    app.world_mut().run_schedule(FixedUpdate);
}

pub fn steps(app: &mut App, n: usize) {
    for _ in 0..n {
        step(app);
    }
}

/// Registers an action that logs `action@entity`.
pub fn log_action(app: &mut App, action: &'static str) {
    app.register_action(
        ActionMeta::new(action),
        move |In(call): In<ActionCall>, mut log: ResMut<Log>| {
            log.0.push(format!("{action}@{}", call.target));
        },
    );
}

pub fn take_log(app: &mut App) -> Vec<String> {
    std::mem::take(&mut app.world_mut().resource_mut::<Log>().0)
}

pub fn status(app: &App, entity: Entity) -> Option<Status> {
    app.world().get::<BrainState>(entity).unwrap().status()
}

pub fn set_flag(app: &mut App, entity: Entity, flag: &'static str) {
    app.world_mut()
        .entity_mut(entity)
        .entry::<Flags>()
        .or_default()
        .into_mut()
        .0
        .push(flag);
}

pub fn clear_flag(app: &mut App, entity: Entity, flag: &'static str) {
    if let Some(mut flags) = app.world_mut().get_mut::<Flags>(entity) {
        flags.0.retain(|f| *f != flag);
    }
}

pub fn actor(path: &str, lineage: &[&str]) -> Definition {
    Definition::new(path, lineage.iter().map(DefinitionPath::new).collect())
}

// Tree builders.

pub fn seq(children: Vec<NodeDef>) -> NodeDef {
    NodeDef::Sequence { children }
}

pub fn sel(children: Vec<NodeDef>) -> NodeDef {
    NodeDef::Selector { children }
}

pub fn act(name: &str) -> NodeDef {
    NodeDef::Action(LeafDef::new(name))
}

pub fn flag(name: &str) -> NodeDef {
    NodeDef::Condition(LeafDef::new("flag").arg("name", ArgDef::Str(name.into())))
}

pub fn walk_to(x: f64, z: f64) -> NodeDef {
    NodeDef::Task(TaskDef {
        start: LeafDef::new("move_to")
            .arg("x", ArgDef::Float(x))
            .arg("z", ArgDef::Float(z)),
        until: LeafDef::new("move_to/arrived"),
        cancel: Some(LeafDef::new("move_to/stop")),
    })
}

pub fn tree(root: NodeDef) -> BehaviorTreeDef {
    BehaviorTreeDef { root }
}
