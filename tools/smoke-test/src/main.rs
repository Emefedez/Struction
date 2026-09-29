//! Development environment check, not the engine implementation.

use bevy::{app::AppExit, prelude::*};

#[derive(Component)]
struct SpinningCube;

#[derive(Resource)]
struct SmokeTest(bool);

fn main() {
    App::new()
        .insert_resource(SmokeTest(std::env::args().any(|arg| arg == "--smoke-test")))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Struction — environment check".into(),
                resolution: (960, 600).into(),
                ..default()
            }),
            ..default()
        }))
        .add_systems(Startup, setup)
        .add_systems(Update, (spin, exit))
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.3, 0.55, 0.9))),
        Transform::from_xyz(0.0, 0.75, 0.0),
        SpinningCube,
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(6.0, 6.0))),
        MeshMaterial3d(materials.add(Color::srgb(0.2, 0.23, 0.25))),
    ));
    commands.spawn((PointLight::default(), Transform::from_xyz(4.0, 6.0, 4.0)));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(3.0, 2.5, 5.0).looking_at(Vec3::new(0.0, 0.5, 0.0), Vec3::Y),
    ));
}

fn spin(time: Res<Time>, mut cubes: Query<&mut Transform, With<SpinningCube>>) {
    for mut transform in &mut cubes {
        transform.rotate_y(time.delta_secs() * 0.8);
    }
}

fn exit(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    smoke: Res<SmokeTest>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Escape) || (smoke.0 && time.elapsed_secs() >= 8.0) {
        exit.write(AppExit::Success);
    }
}
