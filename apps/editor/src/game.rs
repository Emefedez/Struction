//! The game whose data the editor authors. Preview and play build it headless through
//! `AuthoringProject`; this is the registration of the authoring example
//! (`examples/authoring`) until a game crate provides its own. The playground includes this file
//! for `--project`, so both apps run the same game.
use std::path::Path;

use bevy::prelude::*;
use struction_core::{CorePlugin, CoreSet};
use struction_data::DataPlugin;
use struction_world::WorldPlugin;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health {
    current: f32,
    max: f32,
}

pub fn factory(root: &Path) -> App {
    let mut app = App::new();
    add_game(&mut app, root);
    app
}

/// The game's plugins, types and systems on an existing app.
pub fn add_game(app: &mut App, root: &Path) {
    app.add_plugins((
        CorePlugin { seed: 7 },
        DataPlugin::new(root).primordial("Actor"),
        WorldPlugin::default(),
    ))
    .register_type::<Health>()
    .add_systems(FixedUpdate, regenerate.in_set(CoreSet::Invoke));
}

fn regenerate(mut health: Query<&mut Health>, time: Res<Time<Fixed>>) {
    for mut health in &mut health {
        health.current = (health.current + time.delta_secs()).min(health.max);
    }
}
