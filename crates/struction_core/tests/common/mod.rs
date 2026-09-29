#![allow(dead_code)]

use bevy::prelude::*;
use struction_core::*;

#[derive(Resource, Default)]
pub struct Log(pub Vec<String>);

impl Log {
    pub fn entries(&self) -> Vec<&str> {
        self.0.iter().map(String::as_str).collect()
    }
}

pub fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins(CorePlugin::default())
        .init_resource::<Log>();
    app
}

/// Runs one fixed tick directly, independent of wall-clock time.
pub fn step(app: &mut App) {
    app.world_mut().run_schedule(FixedUpdate);
}

pub fn errors(app: &mut App) -> Vec<ActionError> {
    app.world_mut().resource_mut::<ActionErrors>().drain()
}

pub fn log(app: &App) -> Vec<String> {
    app.world().resource::<Log>().0.clone()
}

pub fn named(app: &mut App, name: &'static str) -> Entity {
    app.world_mut().spawn(Name::new(name)).id()
}

/// Registers an action that logs `action@entity-name`.
pub fn log_action(app: &mut App, action: &'static str) {
    app.register_action(
        ActionMeta::new(action),
        move |In(call): In<ActionCall>, mut log: ResMut<Log>, names: Query<&Name>| {
            let who = names.get(call.target).map_or("?", |n| n.as_str());
            log.0.push(format!("{action}@{who}"));
        },
    );
}

pub fn actor(path: &str, lineage: &[&str]) -> Definition {
    Definition::new(path, lineage.iter().map(DefinitionPath::new).collect())
}
