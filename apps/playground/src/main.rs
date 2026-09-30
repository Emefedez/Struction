//! Native milestone 1 physics playground.

use bevy::{
    app::AppExit,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use struction_character::{
    CharacterController, CharacterLook, CharacterState, InputActions, InputMap, InputSystems,
    PlayerControlled,
};
use struction_gravity::{GravityField, LocalUp};
use struction_physics::{
    CameraConstraint, CameraMode, CameraTarget, CameraZone, InCameraZones, PhysicsPlugin,
    Submersion, Surface, Volume, VolumeShape, avian3d::prelude::*, water,
};

#[derive(Resource)]
struct PlaygroundOptions {
    smoke: bool,
    jump_sent: bool,
    cursor_grabbed: bool,
    escape_released_cursor: bool,
}

#[derive(Component)]
struct Player;

#[derive(Component)]
struct Hud;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PlaygroundSystems {
    Cursor,
    Script,
    Camera,
    Hud,
    Exit,
}

fn main() {
    let smoke = std::env::args().any(|arg| arg == "--smoke-test");
    App::new()
        .insert_resource(PlaygroundOptions {
            smoke,
            jump_sent: false,
            cursor_grabbed: false,
            escape_released_cursor: false,
        })
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Struction | Physics playground".into(),
                resolution: (1280, 800).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((
            PhysicsPlugin::default(),
            struction_character::CharacterPlugins,
        ))
        .configure_sets(
            PreUpdate,
            PlaygroundSystems::Cursor.before(InputSystems::Map),
        )
        .configure_sets(
            PreUpdate,
            PlaygroundSystems::Script
                .after(InputSystems::Map)
                .before(InputSystems::Command),
        )
        .configure_sets(
            Update,
            (
                PlaygroundSystems::Camera,
                PlaygroundSystems::Hud,
                PlaygroundSystems::Exit,
            )
                .chain(),
        )
        .add_systems(Startup, setup)
        .add_systems(PreUpdate, cursor_controls.in_set(PlaygroundSystems::Cursor))
        .add_systems(PreUpdate, scripted_input.in_set(PlaygroundSystems::Script))
        .add_systems(Update, follow_camera.in_set(PlaygroundSystems::Camera))
        .add_systems(Update, update_hud.in_set(PlaygroundSystems::Hud))
        .add_systems(Update, exit_control.in_set(PlaygroundSystems::Exit))
        .run();
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(GlobalAmbientLight {
        color: Color::srgb(0.68, 0.78, 1.0),
        brightness: 180.0,
        ..default()
    });
    commands.spawn((
        DirectionalLight {
            illuminance: 13_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(8.0, 16.0, 9.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((Camera3d::default(), Transform::from_xyz(0.0, 4.0, 11.0)));
    spawn_scene_gravity(&mut commands);
    spawn_floor(&mut commands, &mut meshes, &mut materials);
    spawn_pool(&mut commands, &mut meshes, &mut materials);
    spawn_planet(&mut commands, &mut meshes, &mut materials);
    spawn_camera_zone(&mut commands, &mut meshes, &mut materials);
    spawn_cubes(&mut commands, &mut meshes, &mut materials);
    spawn_player(&mut commands, &mut meshes, &mut materials);
    spawn_hud(&mut commands);
}

fn material(materials: &mut Assets<StandardMaterial>, color: Color) -> Handle<StandardMaterial> {
    materials.add(StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.88,
        ..default()
    })
}

fn spawn_scene_gravity(commands: &mut Commands) {
    commands.spawn((
        Name::new("Scene gravity"),
        GravityField::scene(Vec3::NEG_Y * 9.81),
    ));
}

fn spawn_floor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let tile = meshes.add(Cuboid::new(14.0, 0.5, 12.0));
    for (name, z, surface, color) in [
        (
            "Stone floor",
            5.0,
            Surface::default(),
            Color::srgb(0.31, 0.36, 0.38),
        ),
        (
            "Slippery floor",
            -7.0,
            Surface::slippery(),
            Color::srgb(0.20, 0.57, 0.72),
        ),
    ] {
        commands.spawn((
            Name::new(name),
            RigidBody::Static,
            Collider::cuboid(14.0, 0.5, 12.0),
            surface,
            Mesh3d(tile.clone()),
            MeshMaterial3d(material(materials, color)),
            Transform::from_xyz(0.0, -0.25, z),
        ));
    }
    commands.spawn((
        Name::new("Planet approach"),
        RigidBody::Static,
        Collider::cuboid(4.0, 0.5, 7.0),
        Surface::default(),
        Mesh3d(meshes.add(Cuboid::new(4.0, 0.5, 7.0))),
        MeshMaterial3d(material(materials, Color::srgb(0.37, 0.34, 0.34))),
        Transform::from_xyz(0.0, -0.25, -16.5),
    ));
    // A shallow rim makes the pool's edge readable and keeps its base solid.
    commands.spawn((
        Name::new("Pool floor"),
        RigidBody::Static,
        Collider::cuboid(7.0, 0.5, 8.0),
        Mesh3d(meshes.add(Cuboid::new(7.0, 0.5, 8.0))),
        MeshMaterial3d(material(materials, Color::srgb(0.16, 0.30, 0.34))),
        Transform::from_xyz(10.5, -2.75, -5.0),
    ));
}

fn spawn_pool(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let shape = VolumeShape::Box {
        half_extents: Vec3::new(3.5, 1.5, 4.0),
    };
    commands.spawn((
        Name::new("Water pool"),
        water(shape),
        Transform::from_xyz(10.5, -1.0, -5.0),
        Mesh3d(meshes.add(Cuboid::new(7.0, 3.0, 8.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.04, 0.46, 0.72, 0.48),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: 0.24,
            cull_mode: None,
            ..default()
        })),
    ));
}

fn spawn_planet(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let center = Vec3::new(0.0, 4.0, -20.0);
    commands.spawn((
        Name::new("Gravity planet"),
        RigidBody::Static,
        Collider::sphere(4.0),
        GravityField::planet(24.0, 8.0),
        Surface::default(),
        Mesh3d(meshes.add(Sphere::new(4.0).mesh().ico(5).expect("valid sphere"))),
        MeshMaterial3d(material(materials, Color::srgb(0.52, 0.35, 0.24))),
        Transform::from_translation(center),
    ));
}

fn spawn_camera_zone(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    commands.spawn((
        Name::new("Overhead camera zone"),
        CameraZone::bundle(
            Volume {
                shape: VolumeShape::Box {
                    half_extents: Vec3::new(6.0, 3.0, 2.2),
                },
            },
            CameraConstraint {
                mode: CameraMode::Fixed {
                    position: Vec3::new(0.0, 14.0, -8.0),
                },
                weight: 1.0,
                priority: 10,
            },
        ),
        Transform::from_xyz(0.0, 1.0, -8.0),
    ));
    for x in [-5.8, 5.8] {
        commands.spawn((
            Name::new("Camera zone marker"),
            Mesh3d(meshes.add(Cuboid::new(0.12, 2.0, 0.12))),
            MeshMaterial3d(material(materials, Color::srgb(0.95, 0.77, 0.28))),
            Transform::from_xyz(x, 1.0, -8.0),
        ));
    }
}

fn spawn_cubes(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let cube = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
    for (name, position, density, color) in [
        (
            "Pushable cube",
            Vec3::new(0.0, 0.8, 3.0),
            80.0,
            Color::srgb(0.96, 0.52, 0.20),
        ),
        (
            "Floating cube",
            Vec3::new(10.5, 1.0, -5.0),
            500.0,
            Color::srgb(0.92, 0.72, 0.38),
        ),
    ] {
        commands.spawn((
            Name::new(name),
            RigidBody::Dynamic,
            Collider::cuboid(1.0, 1.0, 1.0),
            ColliderDensity(density),
            Mesh3d(cube.clone()),
            MeshMaterial3d(material(materials, color)),
            Transform::from_translation(position),
        ));
    }
}

fn spawn_player(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    commands.spawn((
        Name::new("Player"),
        Player,
        CharacterController::default(),
        PlayerControlled,
        CameraTarget,
        Mesh3d(meshes.add(Capsule3d::new(0.3, 1.0))),
        MeshMaterial3d(material(materials, Color::srgb(0.96, 0.86, 0.44))),
        Transform::from_xyz(0.0, 0.9, 8.0),
    ));
}

fn spawn_hud(commands: &mut Commands) {
    commands.spawn((
        Hud,
        Text::new(""),
        TextFont {
            font_size: FontSize::Px(19.0),
            ..default()
        },
        TextColor(Color::srgb(0.97, 0.98, 1.0)),
        Node {
            position_type: PositionType::Absolute,
            top: px(18),
            left: px(18),
            ..default()
        },
    ));
}

fn cursor_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>,
    mut map: ResMut<InputMap>,
    mut options: ResMut<PlaygroundOptions>,
) {
    options.escape_released_cursor = false;
    if keys.just_pressed(KeyCode::KeyM) {
        options.cursor_grabbed = !options.cursor_grabbed;
    }
    if keys.just_pressed(KeyCode::Escape) {
        options.escape_released_cursor = options.cursor_grabbed;
        options.cursor_grabbed = false;
    }
    if let Ok(mut cursor) = windows.single_mut() {
        cursor.grab_mode = if options.cursor_grabbed {
            CursorGrabMode::Locked
        } else {
            CursorGrabMode::None
        };
        cursor.visible = !options.cursor_grabbed;
    }
    map.look_sensitivity = if options.cursor_grabbed { 0.003 } else { 0.0 };
}

fn scripted_input(
    time: Res<Time>,
    mut options: ResMut<PlaygroundOptions>,
    mut actions: ResMut<InputActions>,
) {
    if !options.smoke {
        return;
    }
    let seconds = time.elapsed_secs();
    actions.movement = if seconds < 7.5 { Vec2::Y } else { Vec2::ZERO };
    if seconds >= 1.3 && !options.jump_sent {
        actions.jump.pressed = true;
        actions.jump.held = true;
        options.jump_sent = true;
    }
}

fn follow_camera(
    player: Query<(&Transform, &LocalUp, &CharacterLook, &InCameraZones), With<Player>>,
    zones: Query<&CameraZone>,
    mut camera: Query<&mut Transform, (With<Camera3d>, Without<Player>)>,
    time: Res<Time>,
) {
    let (Ok((player, up, look, in_zones)), Ok(mut camera)) = (player.single(), camera.single_mut())
    else {
        return;
    };
    let up = *up.0;
    let focus = player.translation + up * 0.5;
    let default = CameraMode::Follow {
        distance: 5.5,
        pitch: 0.35,
    };
    let mode = in_zones
        .active()
        .and_then(|zone| zones.get(zone).ok())
        .map_or(default, |zone| zone.constraint.mode);
    let target = match mode {
        CameraMode::Follow { distance, pitch } => {
            let angle = look.pitch + pitch;
            focus - look.forward * (distance * angle.cos()) + up * (distance * angle.sin() + 0.7)
        }
        CameraMode::Fixed { position } => position,
    };
    let smoothing = 1.0 - (-8.0 * time.delta_secs()).exp();
    camera.translation = camera.translation.lerp(target, smoothing);
    camera.look_at(focus, up);
}

fn update_hud(
    player: Query<(&CharacterState, &Submersion, &InCameraZones), With<Player>>,
    mut hud: Query<&mut Text, With<Hud>>,
    time: Res<Time>,
    options: Res<PlaygroundOptions>,
) {
    let (Ok((state, submersion, zones)), Ok(mut text)) = (player.single(), hud.single_mut()) else {
        return;
    };
    let zone = if zones.active().is_some() {
        "overhead"
    } else {
        "follow"
    };
    let fps = if time.delta_secs() > 0.0 {
        1.0 / time.delta_secs()
    } else {
        0.0
    };
    **text = format!(
        "STRUCTION / PHYSICS PLAYGROUND\nWASD move  ·  Space jump / swim  ·  M mouse look [{}]  ·  Esc release / quit\nGrounded: {}   Swimming: {} ({:.0}%)   Camera: {}   FPS: {:.0}\nBlue tile: slippery   ·   Orange cube: push   ·   Right: water   ·   Ahead: gravity planet",
        if options.cursor_grabbed { "on" } else { "off" },
        state.grounded,
        state.swimming,
        submersion.0 * 100.0,
        zone,
        fps
    );
}

fn exit_control(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    options: Res<PlaygroundOptions>,
    player: Query<&Position, With<Player>>,
    mut exit: MessageWriter<AppExit>,
) {
    if options.smoke && time.elapsed_secs() >= 10.0 {
        if let Ok(position) = player.single() {
            info!("Smoke test character position: {:?}", position.0);
        }
        exit.write(AppExit::Success);
    } else if keys.just_pressed(KeyCode::Escape) && !options.escape_released_cursor {
        exit.write(AppExit::Success);
    }
}
