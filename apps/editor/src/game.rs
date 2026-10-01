//! Every engine package (`struction_scene::authoring_app`), plus the small authoring example’s
//! Health behavior.
use std::path::Path;

use bevy::prelude::*;
use struction_core::CoreSet;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health {
    current: f32,
    max: f32,
}

pub fn factory(root: &Path) -> App {
    let mut app = struction_scene::authoring_app(root);
    app.register_type::<Health>()
        .add_systems(FixedUpdate, regenerate.in_set(CoreSet::Invoke));
    app
}

fn regenerate(mut health: Query<&mut Health>, time: Res<Time<Fixed>>) {
    for mut health in &mut health {
        health.current = (health.current + time.delta_secs()).min(health.max);
    }
}
