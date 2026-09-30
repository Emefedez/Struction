//! `--project DIR`: runs an authored project inside the playground and applies its saved sources
//! live, so edits made in the editor (or by an AI tool, or by hand) show up without a restart.

use std::path::PathBuf;

use bevy::prelude::*;
use struction_world::{LiveReloadPlugin, Spawned};

// The editor's game registration, shared until a game crate supplies it to both apps.
#[path = "../../editor/src/game.rs"]
#[expect(
    dead_code,
    reason = "the playground adds the game to its own app, not a factory one"
)]
mod game;

/// Does nothing without a project.
pub struct AuthoredPlugin {
    pub root: Option<PathBuf>,
}

impl Plugin for AuthoredPlugin {
    fn build(&self, app: &mut App) {
        let Some(root) = &self.root else {
            return;
        };
        game::add_game(app, root);
        app.add_plugins(LiveReloadPlugin::default()).add_systems(
            Update,
            dress_instances.in_set(super::PlaygroundSystems::Dress),
        );
    }
}

/// Authored instances have no meshes yet; a block marks where each one stands.
fn dress_instances(
    mut commands: Commands,
    bare: Query<Entity, (With<Spawned>, Without<Mesh3d>)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut look: Local<Option<(Handle<Mesh>, Handle<StandardMaterial>)>>,
) {
    if bare.is_empty() {
        return;
    }
    let (mesh, material) = look
        .get_or_insert_with(|| {
            (
                meshes.add(Cuboid::new(1.0, 2.0, 1.0)),
                materials.add(super::matte(Color::srgb(0.86, 0.45, 0.16))),
            )
        })
        .clone();
    for entity in &bare {
        commands
            .entity(entity)
            .insert((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone())));
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use struction_world::EntityPath;

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            if entry.path().is_dir() {
                copy(&entry.path(), &to.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        }
    }

    fn ogre(app: &mut App) -> (Transform, bool) {
        let mut query = app
            .world_mut()
            .query::<(&EntityPath, &Transform, Has<Mesh3d>)>();
        let (_, transform, dressed) = query
            .iter(app.world())
            .find(|(path, ..)| path.as_str() == "Court/guards/ogre")
            .expect("the ogre is spawned");
        (*transform, dressed)
    }

    #[test]
    fn saved_edits_reach_the_running_playground() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("authoring");
        copy(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/authoring"),
            &root,
        );
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Time<Fixed>>()
            .add_plugins(AuthoredPlugin {
                root: Some(root.clone()),
            });
        app.finish();
        app.cleanup();
        let world = app.world_mut();
        world.run_schedule(Startup);
        world.run_schedule(FixedUpdate);
        world.run_schedule(Update);
        world.run_schedule(First);
        let (before, dressed) = ogre(&mut app);
        assert!(dressed);

        let scene = root.join("scenes/courtyard.jsonc");
        let text = std::fs::read_to_string(&scene).unwrap();
        std::fs::write(
            &scene,
            text.replace(r#""offset": [1, 0, 0]"#, r#""offset": [1, 0, 3]"#),
        )
        .unwrap();
        // Past the default scan interval.
        std::thread::sleep(std::time::Duration::from_millis(300));
        app.world_mut().run_schedule(First);

        let (after, dressed) = ogre(&mut app);
        assert!(dressed);
        // The spawner is turned 90° about y, so offset z moves the ogre along world x.
        assert!(
            (after.translation - before.translation).abs_diff_eq(Vec3::X * 3.0, 1e-4),
            "{before:?} -> {after:?}"
        );
    }
}
