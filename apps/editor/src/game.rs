//! The game whose data the editor authors. Preview and play build it headless through
//! `AuthoringProject`; this is the registration of the authoring example
//! (`examples/authoring`) until a game crate provides its own.
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
    app.add_plugins((
        CorePlugin { seed: 7 },
        DataPlugin::new(root).primordial("Actor"),
        WorldPlugin::default(),
    ))
    .register_type::<Health>()
    .add_systems(FixedUpdate, regenerate.in_set(CoreSet::Invoke));
    app
}

fn regenerate(mut health: Query<&mut Health>, time: Res<Time<Fixed>>) {
    for mut health in &mut health {
        health.current = (health.current + time.delta_secs()).min(health.max);
    }
}
