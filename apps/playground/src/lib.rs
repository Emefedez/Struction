//! Game registration shared by the native playground and editor.
pub mod scene;

use bevy::prelude::*;
use scene::ScenePlugin;
use std::path::Path;

/// The same game registrations for editor preview and isolated headless play.
pub fn factory(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        struction_physics::PhysicsPlugin::default(),
        struction_character::CharacterControllerPlugin,
        ScenePlugin {
            root: root.to_owned(),
        },
    ))
    .register_type::<struction_character::PlayerControlled>();
    app
}
