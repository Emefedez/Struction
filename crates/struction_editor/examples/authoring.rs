//! Minimal game host for the shared authoring protocol. See docs/authoring.md.
use std::{io, path::Path};

use bevy::prelude::*;
use struction_core::{CorePlugin, CoreSet};
use struction_data::DataPlugin;
use struction_editor::{AuthoringProject, protocol};
use struction_world::WorldPlugin;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health {
    current: f32,
    max: f32,
}

fn game(root: &Path) -> App {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args()
        .nth(1)
        .ok_or("usage: authoring <project-directory>")?;
    let mut project = AuthoringProject::open(root, game)?;
    protocol::serve(&mut project, io::stdin().lock(), io::stdout().lock())?;
    Ok(())
}
