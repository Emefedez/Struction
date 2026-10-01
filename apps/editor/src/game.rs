//! Every engine package (`struction_scene::authoring_app`), plus the small authoring example's
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
    app.add_plugins((
        struction_editor::PlayInputPlugin,
        struction_camera::PlayerCameraPlugin,
        struction_scene::SceneRigPlugin,
        struction_character::CharacterAnimationPlugin,
    ))
    .add_systems(
        Update,
        attach_camera.before(struction_camera::CameraSystems::Follow),
    )
    .register_type::<Health>()
    .add_systems(FixedUpdate, regenerate.in_set(CoreSet::Invoke));
    app
}

fn regenerate(mut health: Query<&mut Health>, time: Res<Time<Fixed>>) {
    for mut health in &mut health {
        health.current = (health.current + time.delta_secs()).min(health.max);
    }
}

fn attach_camera(
    mut commands: Commands,
    players: Query<Entity, Added<struction_character::PlayerControlled>>,
) {
    if let Some(player) = players.iter().next() {
        commands
            .entity(player)
            .insert(struction_physics::CameraTarget);
        commands.spawn(struction_camera::PlayerCamera::new(player));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use struction_character::PlayerControlled;
    use struction_editor::{AuthoringProject, PlayInput};

    #[test]
    fn play_accepts_player_commands_and_preserves_the_authored_world() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scenes")).unwrap();
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../playground/project/scenes/milestone1.jsonc"),
            dir.path().join("scenes/milestone1.jsonc"),
        )
        .unwrap();
        let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
        let player = |world: &World| {
            world
                .iter_entities()
                .find(|e| e.contains::<PlayerControlled>())
                .unwrap()
                .get::<Transform>()
                .unwrap()
                .translation
        };
        let authored = player(project.preview());
        assert!(project.play_input(PlayInput::default()).is_err());
        project.start_play().unwrap();
        project.step_play(30).unwrap();
        let start = player(project.play_world().unwrap());
        project
            .play_input(PlayInput {
                movement: [0.0, 1.0],
                ..default()
            })
            .unwrap();
        project.step_play(90).unwrap();
        let end = player(project.play_world().unwrap());
        assert!(
            Vec2::new(end.x - start.x, end.z - start.z).length() > 2.0,
            "{start:?} -> {end:?}"
        );
        assert_eq!(player(project.preview()), authored);
        let world = project.play_world().unwrap();
        assert!(
            world
                .iter_entities()
                .any(|e| e.contains::<struction_anim::plugin::SolvedPose>())
        );
        project.release_play_input();
        project.step_play(30).unwrap();
        assert!(
            project
                .play_world()
                .unwrap()
                .resource::<struction_character::InputActions>()
                .movement
                .length()
                < 1e-5
        );
        project
            .play_input(PlayInput {
                toggle_view_pressed: true,
                ..default()
            })
            .unwrap();
        project.step_play(2).unwrap();
        assert_eq!(
            project
                .play_world()
                .unwrap()
                .iter_entities()
                .find_map(|e| e.get::<struction_camera::PlayerCamera>())
                .unwrap()
                .view,
            struction_camera::ViewMode::FirstPerson
        );
        project.stop_play();
        assert_eq!(player(project.preview()), authored);
        assert!(!project.session().history().can_undo());
    }
}
