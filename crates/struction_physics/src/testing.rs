//! Headless harness: an app with everything physics needs and a manual clock, so tests advance
//! the fixed timestep deterministically without a window or GPU.

use bevy::{
    app::{App, Plugins},
    prelude::*,
    time::TimeUpdateStrategy,
};
use core::time::Duration;

use crate::PhysicsPlugin;

/// `MinimalPlugins`, `TransformPlugin`, and [`PhysicsPlugin`]. Each `app.update()` advances the
/// clock by exactly one fixed tick, so one update is one simulation step.
pub fn headless_app() -> App {
    headless_app_with(())
}

/// Like [`headless_app`], with more plugins added before the app is finished.
pub fn headless_app_with<M>(plugins: impl Plugins<M>) -> App {
    let physics = PhysicsPlugin::default();
    let tick = Duration::from_secs_f64(1.0 / physics.tick_hz);
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, physics, plugins))
        .insert_resource(TimeUpdateStrategy::ManualDuration(tick));
    app.finish();
    app.cleanup();
    app
}

/// Runs `ticks` updates.
pub fn step(app: &mut App, ticks: usize) {
    for _ in 0..ticks {
        app.update();
    }
}
