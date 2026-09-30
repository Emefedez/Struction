//! Native physics playground with a procedurally animated player.

mod camera_occlusion;

use camera_occlusion::{FadeMaterial, FadesWith, fade_material};
#[cfg(test)]
mod planet_tests;

use bevy::{
    app::AppExit,
    light::NotShadowCaster,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use struction_anim::{
    humanoid,
    locomotion::LocomotionParams,
    plugin::{Locomotor, RigJoints},
    rig::Rig,
};
use struction_camera::{CameraSystems, PlayerCamera, PlayerCameraPlugin, ViewMode};
use struction_character::{
    CharacterAnimationPlugin, CharacterController, CharacterLook, CharacterState, InputActions,
    InputMap, InputSystems, PlayerControlled, RigOf, spawn_rig,
};
use struction_debug::{DebugTracePlugin, TraceAppExt, TraceEntity, TraceWriter};
use struction_gravity::{GravityField, GravityHysteresis, GravityInfluences, LocalUp};
use struction_physics::{
    CameraConstraint, CameraMode, CameraOcclusion, CameraTarget, CameraZone, InCameraZones,
    PhysicsPlugin, Submersion, Surface, Volume, VolumeShape, avian3d::prelude::*, water,
};

#[derive(Resource)]
struct PlaygroundOptions {
    smoke: bool,
    jump_sent: bool,
    cursor_grabbed: bool,
    escape_released_cursor: bool,
    footfalls: u32,
}

#[derive(Component)]
struct Player;

#[derive(Component)]
struct Hud;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PlaygroundSystems {
    Cursor,
    Script,
    Dress,
    Camera,
    Hud,
    Exit,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut smoke = false;
    let mut trace = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--smoke-test" => smoke = true,
            "--trace" => {
                trace = Some(
                    args.next()
                        .ok_or("--trace requires a new JSONL file path")?,
                )
            }
            "--help" | "-h" => {
                println!("struction-playground [--smoke-test] [--trace FILE.jsonl]");
                return Ok(());
            }
            _ => return Err(format!("unknown option: {arg}").into()),
        }
    }
    let mut app = App::new();
    if let Some(path) = trace {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        app.add_plugins(DebugTracePlugin)
            .insert_resource(TraceWriter::new(std::io::BufWriter::new(file)))
            .trace_component::<Position>()
            .trace_component::<LinearVelocity>()
            .trace_component::<CharacterState>()
            .trace_component::<CharacterLook>()
            .trace_component::<struction_character::CharacterIntent>()
            .trace_component::<LocalUp>()
            .trace_component::<GravityInfluences>()
            .trace_component::<Submersion>()
            .trace_component::<InCameraZones>();
    }
    app.insert_resource(PlaygroundOptions {
        smoke,
        jump_sent: false,
        cursor_grabbed: false,
        escape_released_cursor: false,
        footfalls: 0,
    })
    .add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "Struction | Playground".into(),
            resolution: (1280, 800).into(),
            ..default()
        }),
        ..default()
    }))
    .add_plugins((
        PhysicsPlugin::default(),
        struction_character::CharacterPlugins,
        CharacterAnimationPlugin,
        PlayerCameraPlugin,
        camera_occlusion::SightFadePlugin,
    ))
    .configure_sets(
        PreUpdate,
        PlaygroundSystems::Cursor.before(InputSystems::Map),
    )
    .configure_sets(
        PreUpdate,
        PlaygroundSystems::Script
            .after(InputSystems::Map)
            .before(CameraSystems::Input),
    )
    .configure_sets(
        Update,
        (
            PlaygroundSystems::Dress,
            PlaygroundSystems::Camera,
            PlaygroundSystems::Hud,
            PlaygroundSystems::Exit,
        )
            .chain(),
    )
    .configure_sets(
        Update,
        CameraSystems::Follow.in_set(PlaygroundSystems::Camera),
    )
    .add_systems(Startup, setup)
    .add_systems(PreUpdate, cursor_controls.in_set(PlaygroundSystems::Cursor))
    .add_systems(PreUpdate, scripted_input.in_set(PlaygroundSystems::Script))
    .add_systems(Update, dress_rigs.in_set(PlaygroundSystems::Dress))
    .add_systems(
        Update,
        (hide_rig_in_first_person, count_footfalls).in_set(PlaygroundSystems::Camera),
    )
    .add_systems(Update, update_hud.in_set(PlaygroundSystems::Hud))
    .add_systems(Update, exit_control.in_set(PlaygroundSystems::Exit))
    .run();
    Ok(())
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut ground_materials: ResMut<Assets<FadeMaterial>>,
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
    spawn_scene_gravity(&mut commands);
    spawn_floor(&mut commands, &mut meshes, &mut ground_materials);
    spawn_pool(&mut commands, &mut meshes, &mut ground_materials);
    spawn_planet(&mut commands, &mut meshes, &mut ground_materials);
    spawn_camera_zone(&mut commands, &mut meshes, &mut materials);
    spawn_cubes(&mut commands, &mut meshes, &mut materials);
    let player = spawn_player(&mut commands);
    commands.spawn((
        Camera3d::default(),
        CameraOcclusion::default(),
        PlayerCamera::new(player),
    ));
    spawn_hud(&mut commands);
}

fn matte(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.88,
        ..default()
    }
}

fn material(materials: &mut Assets<StandardMaterial>, color: Color) -> Handle<StandardMaterial> {
    materials.add(matte(color))
}

/// Ground that can hide the player gets a cut-out along the camera's line of sight.
fn ground(materials: &mut Assets<FadeMaterial>, color: Color) -> Handle<FadeMaterial> {
    fade_material(materials, matte(color))
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
    materials: &mut Assets<FadeMaterial>,
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
            TraceEntity,
            Collider::cuboid(14.0, 0.5, 12.0),
            surface,
            Mesh3d(tile.clone()),
            MeshMaterial3d(ground(materials, color)),
            Transform::from_xyz(0.0, -0.25, z),
        ));
    }
    commands.spawn((
        Name::new("Planet approach"),
        TraceEntity,
        RigidBody::Static,
        Collider::cuboid(4.0, 0.5, 7.0),
        Surface::default(),
        Mesh3d(meshes.add(Cuboid::new(4.0, 0.5, 7.0))),
        MeshMaterial3d(ground(materials, Color::srgb(0.37, 0.34, 0.34))),
        Transform::from_xyz(0.0, -0.25, -16.5),
    ));
    // A shallow rim makes the pool's edge readable and keeps its base solid.
    commands.spawn((
        Name::new("Pool floor"),
        TraceEntity,
        RigidBody::Static,
        Collider::cuboid(7.0, 0.5, 8.0),
        Mesh3d(meshes.add(Cuboid::new(7.0, 0.5, 8.0))),
        MeshMaterial3d(ground(materials, Color::srgb(0.16, 0.30, 0.34))),
        Transform::from_xyz(10.5, -2.75, -5.0),
    ));
}

fn spawn_pool(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FadeMaterial>,
) {
    let shape = VolumeShape::Box {
        half_extents: Vec3::new(3.5, 1.5, 4.0),
    };
    let volume = commands
        .spawn((
            Name::new("Water pool"),
            TraceEntity,
            water(shape),
            Transform::from_xyz(10.5, -1.0, -5.0),
        ))
        .id();
    // A closed transparent box overlaps the pool floor and blends its own unsorted faces.
    commands.spawn((
        Name::new("Water surface"),
        Transform::from_xyz(10.5, 0.5, -5.0),
        Mesh3d(meshes.add(Plane3d::default().mesh().size(7.0, 8.0))),
        NotShadowCaster,
        // A swimmer below the surface stays visible through it.
        FadesWith(volume),
        MeshMaterial3d(fade_material(
            materials,
            StandardMaterial {
                base_color: Color::srgba(0.04, 0.46, 0.72, 0.48),
                alpha_mode: AlphaMode::Blend,
                perceptual_roughness: 0.24,
                cull_mode: None,
                double_sided: true,
                ..default()
            },
        )),
    ));
}

fn spawn_planet(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<FadeMaterial>,
) {
    let center = Vec3::new(0.0, 4.0, -20.0);
    commands.spawn((
        Name::new("Gravity planet"),
        TraceEntity,
        RigidBody::Static,
        Collider::sphere(4.0),
        GravityField::planet(24.0, 5.0),
        GravityHysteresis { exit_margin: 0.5 },
        Surface::default(),
        Mesh3d(meshes.add(Sphere::new(4.0).mesh().ico(5).expect("valid sphere"))),
        MeshMaterial3d(ground(materials, Color::srgb(0.52, 0.35, 0.24))),
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
        TraceEntity,
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
            TraceEntity,
            Collider::cuboid(1.0, 1.0, 1.0),
            ColliderDensity(density),
            Mesh3d(cube.clone()),
            MeshMaterial3d(material(materials, color)),
            Transform::from_translation(position),
        ));
    }
}

fn spawn_player(commands: &mut Commands) -> Entity {
    // The capsule is only the physics body; the visible player is the rig that follows it.
    let body = commands
        .spawn((
            Name::new("Player"),
            Player,
            TraceEntity,
            CharacterController::default(),
            PlayerControlled,
            CameraTarget,
            Transform::from_xyz(0.0, 0.9, 8.0),
        ))
        .id();
    let rig = spawn_rig(commands, body, humanoid::rig(), LocomotionParams::default())
        .expect("the built-in humanoid is a valid rig");
    commands
        .entity(rig)
        .insert((Name::new("Player rig"), Visibility::default()));
    body
}

/// Gives new rigs a body made of simple shapes attached to their joint entities.
fn dress_rigs(
    mut commands: Commands,
    rigs: Query<(&Rig, &RigJoints), Added<RigJoints>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (rig, joints) in &rigs {
        let skin = material(&mut materials, Color::srgb(0.96, 0.86, 0.44));
        let dark = material(&mut materials, Color::srgb(0.30, 0.27, 0.34));
        let defs = rig.skeleton.joints();
        for (def, &joint) in defs.iter().zip(&joints.0) {
            commands.entity(joint).insert(Visibility::default());
            let name = def.name.as_str();
            let decoration = if name == "head" {
                Some((
                    meshes.add(Sphere::new(0.11)),
                    skin.clone(),
                    Transform::from_xyz(0.0, 0.08, 0.0),
                ))
            } else if name.starts_with("hand") {
                Some((
                    meshes.add(Sphere::new(0.04)),
                    skin.clone(),
                    Transform::from_xyz(0.0, -0.03, 0.0),
                ))
            } else if name.starts_with("foot") {
                // Toes point forward (-Z) from the ankle, with the sole on the ground.
                Some((
                    meshes.add(Cuboid::new(0.09, humanoid::ANKLE_HEIGHT, 0.24)),
                    dark.clone(),
                    Transform::from_xyz(0.0, -humanoid::ANKLE_HEIGHT * 0.5, -0.06),
                ))
            } else {
                None
            };
            if let Some((mesh, material, transform)) = decoration {
                commands.spawn((
                    Mesh3d(mesh),
                    MeshMaterial3d(material),
                    transform,
                    ChildOf(joint),
                ));
            }

            // A limb segment from the parent joint to this one, owned by the parent so it turns
            // with it.
            let Some(parent) = def.parent.filter(|&p| p != rig.root) else {
                continue;
            };
            let offset = def.rest.translation;
            let length = offset.length();
            let radius = segment_radius(&defs[parent].name);
            if length < 1e-3 || radius == 0.0 {
                continue;
            }
            let legs = ["thigh", "shin"]
                .iter()
                .any(|l| defs[parent].name.starts_with(l));
            commands.spawn((
                Mesh3d(meshes.add(Capsule3d::new(radius, (length - radius).max(0.01)))),
                MeshMaterial3d(if legs { dark.clone() } else { skin.clone() }),
                Transform::from_translation(offset * 0.5)
                    .with_rotation(Quat::from_rotation_arc(Vec3::Y, offset / length)),
                ChildOf(joints.0[parent]),
            ));
        }
    }
}

/// Thickness of the segment that starts at a joint; zero draws nothing.
fn segment_radius(parent: &str) -> f32 {
    match parent.split('_').next().unwrap_or(parent) {
        "hips" => 0.09,
        "spine" | "chest" => 0.11,
        "neck" => 0.05,
        "upper" | "thigh" => 0.06,
        "forearm" | "shin" => 0.045,
        "fingers" | "thumb" | "hand" => 0.012,
        _ => 0.0,
    }
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

/// The first-person camera sits inside the head, so the player's own rig is not drawn.
fn hide_rig_in_first_person(
    cameras: Query<&PlayerCamera>,
    mut rigs: Query<(&RigOf, &mut Visibility)>,
) {
    for camera in &cameras {
        for (rig, mut visibility) in &mut rigs {
            if rig.0 == camera.target {
                visibility.set_if_neq(match camera.view {
                    ViewMode::FirstPerson => Visibility::Hidden,
                    ViewMode::ThirdPerson => Visibility::Inherited,
                });
            }
        }
    }
}

fn count_footfalls(rigs: Query<&Locomotor>, mut options: ResMut<PlaygroundOptions>) {
    options.footfalls += rigs.iter().map(|l| l.state.output().footfalls).sum::<u32>();
}

fn update_hud(
    player: Query<(&CharacterState, &Submersion, &InCameraZones), With<Player>>,
    camera: Query<&PlayerCamera>,
    mut hud: Query<&mut Text, With<Hud>>,
    time: Res<Time>,
    options: Res<PlaygroundOptions>,
) {
    let (Ok((state, submersion, zones)), Ok(mut text)) = (player.single(), hud.single_mut()) else {
        return;
    };
    let view = match camera.single().map(|camera| camera.view) {
        Ok(ViewMode::FirstPerson) => "first person",
        _ if zones.active().is_some() => "third person (overhead)",
        _ => "third person",
    };
    let fps = if time.delta_secs() > 0.0 {
        1.0 / time.delta_secs()
    } else {
        0.0
    };
    **text = format!(
        "STRUCTION / PLAYGROUND\nWASD move  |  Space jump / swim  |  M mouse look [{}]  |  V or wheel: third / first person  |  Esc release / quit\nGrounded: {}   Swimming: {} ({:.0}%)   Camera: {}   Steps: {}   FPS: {:.0}\nBlue tile: slippery   |   Orange cube: push   |   Right: water   |   Ahead: gravity planet",
        if options.cursor_grabbed { "on" } else { "off" },
        state.grounded,
        state.swimming,
        submersion.0 * 100.0,
        view,
        options.footfalls,
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
            info!(
                "Smoke test character position: {:?}, footfalls: {}",
                position.0, options.footfalls
            );
        }
        exit.write(AppExit::Success);
    } else if keys.just_pressed(KeyCode::Escape) && !options.escape_released_cursor {
        exit.write(AppExit::Success);
    }
}
