//! Native physics playground with a procedurally animated player.

mod camera_occlusion;
mod scene;

use camera_occlusion::{FadeMaterial, FadesWith, fade_material};
#[cfg(test)]
mod planet_tests;
use scene::{Finish, Humanoid, Look, Player, ScenePlugin, Shape};

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
use struction_character::{
    CharacterAnimationPlugin, CharacterLook, CharacterState, InputActions, InputMap, InputSystems,
    spawn_rig,
};
use struction_debug::{DebugTracePlugin, TraceAppExt, TraceWriter};
use struction_gravity::{GravityInfluences, LocalUp};
use struction_physics::{
    CameraMode, CameraOcclusion, CameraZone, InCameraZones, PhysicsPlugin, Submersion, Volume,
    VolumeShape, avian3d::prelude::*,
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
struct Hud;

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PlaygroundSystems {
    Cursor,
    Script,
    MovementFrame,
    Dress,
    Camera,
    Hud,
    Exit,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut smoke = false;
    let mut trace = None;
    let mut project = scene::default_project();
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
            "--project" => {
                project = args
                    .next()
                    .ok_or("--project requires a project directory")?
                    .into()
            }
            "--help" | "-h" => {
                println!(
                    "struction-playground [--smoke-test] [--trace FILE.jsonl] [--project DIR]"
                );
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
        camera_occlusion::SightFadePlugin,
        ScenePlugin { root: project },
    ))
    .configure_sets(
        PreUpdate,
        PlaygroundSystems::Cursor.before(InputSystems::Map),
    )
    .configure_sets(
        PreUpdate,
        (PlaygroundSystems::Script, PlaygroundSystems::MovementFrame)
            .chain()
            .after(InputSystems::Map)
            .before(InputSystems::Command),
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
    .add_systems(Startup, setup)
    .add_systems(PreUpdate, cursor_controls.in_set(PlaygroundSystems::Cursor))
    .add_systems(PreUpdate, scripted_input.in_set(PlaygroundSystems::Script))
    .add_systems(
        PreUpdate,
        camera_movement.in_set(PlaygroundSystems::MovementFrame),
    )
    .add_systems(
        Update,
        (attach_rigs, dress_looks, dress_rigs)
            .chain()
            .in_set(PlaygroundSystems::Dress),
    )
    .add_systems(
        Update,
        (follow_camera, count_footfalls).in_set(PlaygroundSystems::Camera),
    )
    .add_systems(Update, update_hud.in_set(PlaygroundSystems::Hud))
    .add_systems(Update, exit_control.in_set(PlaygroundSystems::Exit))
    .run();
    Ok(())
}

/// The scene itself is data (see `scene`); this adds what only the rendered host needs.
fn setup(mut commands: Commands) {
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
    commands.spawn((
        Camera3d::default(),
        CameraOcclusion::default(),
        Transform::from_xyz(0.0, 4.0, 13.0).looking_at(Vec3::new(0.0, 1.4, 8.0), Vec3::Y),
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

type Dressed<'a> = (Entity, &'a Look, Option<&'a Shape>, Option<&'a Volume>);

/// Gives authored entities their mesh and material from their `Shape` and `Look`.
fn dress_looks(
    mut commands: Commands,
    looks: Query<Dressed, Added<Look>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fading: ResMut<Assets<FadeMaterial>>,
) {
    for (entity, look, shape, volume) in &looks {
        let [red, green, blue] = look.color;
        let color = Color::srgba(red, green, blue, look.opacity);
        if look.finish == Finish::Water {
            let Some(VolumeShape::Box { half_extents }) = volume.map(|v| v.shape) else {
                warn!("a water look needs a box volume; {entity} is not drawn");
                continue;
            };
            // A closed transparent box overlaps the pool floor and blends its own unsorted faces.
            let size = half_extents.xz() * 2.0;
            commands.entity(entity).insert(Visibility::default());
            commands.spawn((
                Name::new("Water surface"),
                Transform::from_xyz(0.0, half_extents.y, 0.0),
                Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x, size.y))),
                NotShadowCaster,
                // A swimmer below the surface stays visible through it.
                FadesWith(entity),
                MeshMaterial3d(fade_material(
                    &mut fading,
                    StandardMaterial {
                        base_color: color,
                        alpha_mode: AlphaMode::Blend,
                        perceptual_roughness: 0.24,
                        cull_mode: None,
                        double_sided: true,
                        ..default()
                    },
                )),
                ChildOf(entity),
            ));
            continue;
        }
        let Some(shape) = shape else {
            warn!("{entity} has a look but no shape to draw");
            continue;
        };
        let mesh = Mesh3d(meshes.add(shape.mesh()));
        let mut entity = commands.entity(entity);
        match look.finish {
            // Ground that can hide the player gets a cut-out along the camera's line of sight.
            Finish::Ground => entity.insert((
                mesh,
                MeshMaterial3d(fade_material(&mut fading, matte(color))),
            )),
            _ => entity.insert((mesh, MeshMaterial3d(material(&mut materials, color)))),
        };
    }
}

/// The capsule is only the physics body; a humanoid is drawn as the rig that follows it.
fn attach_rigs(mut commands: Commands, bodies: Query<Entity, Added<Humanoid>>) {
    for body in &bodies {
        let rig = spawn_rig(
            &mut commands,
            body,
            humanoid::rig(),
            LocomotionParams::default(),
        )
        .expect("the built-in humanoid is a valid rig");
        commands
            .entity(rig)
            .insert((Name::new("Humanoid rig"), Visibility::default()));
    }
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
            let angle = (look.pitch + pitch).clamp(-1.3, 1.3);
            focus - look.forward * (distance * angle.cos()) + up * (distance * angle.sin() + 0.7)
        }
        CameraMode::Fixed { position } => position,
    };
    let smoothing = 1.0 - (-8.0 * time.delta_secs()).exp();
    camera.translation = camera.translation.lerp(target, smoothing);
    // Overhead views use the ground heading as screen-up, avoiding the look-at pole.
    let view_up = match mode {
        CameraMode::Fixed { .. } => look.forward,
        CameraMode::Follow { .. } => up,
    };
    let target_rotation = camera.looking_at(focus, view_up).rotation;
    camera.rotation = camera
        .rotation
        .slerp(target_rotation, smoothing)
        .normalize();
}

fn camera_movement(
    camera: Query<&Transform, With<Camera3d>>,
    player: Query<&LocalUp, With<Player>>,
    mut actions: ResMut<InputActions>,
) {
    if let (Ok(camera), Ok(up)) = (camera.single(), player.single()) {
        actions.movement_forward = camera_ground_forward(camera, *up.0);
    }
}

fn camera_ground_forward(camera: &Transform, up: Vec3) -> Option<Vec3> {
    // Screen-right remains defined when a camera points straight down at the player.
    let right = *camera.right();
    let right = (right - up * right.dot(up)).try_normalize()?;
    Some(up.cross(right))
}

fn count_footfalls(rigs: Query<&Locomotor>, mut options: ResMut<PlaygroundOptions>) {
    options.footfalls += rigs.iter().map(|l| l.state.output().footfalls).sum::<u32>();
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
        "STRUCTION / PLAYGROUND\nWASD move  |  Space jump / swim  |  M mouse look [{}]  |  Esc release / quit\nGrounded: {}   Swimming: {} ({:.0}%)   Camera: {}   Steps: {}   FPS: {:.0}\nBlue tile: slippery   |   Orange cube: push   |   Right: water   |   Ahead: gravity planet",
        if options.cursor_grabbed { "on" } else { "off" },
        state.grounded,
        state.swimming,
        submersion.0 * 100.0,
        zone,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use struction_physics::CameraConstraint;

    #[test]
    fn camera_movement_remains_defined_in_overhead_and_planet_views() {
        for (up, forward) in [
            (Vec3::Y, Vec3::NEG_Z),
            (Vec3::Z, Vec3::Y),
            (Vec3::NEG_Y, Vec3::Z),
        ] {
            let overhead = Transform::from_translation(up * 10.0).looking_at(Vec3::ZERO, forward);
            assert!(
                camera_ground_forward(&overhead, up)
                    .unwrap()
                    .abs_diff_eq(forward, 1e-5)
            );
            let follow =
                Transform::from_translation(up * 3.0 - forward * 5.0).looking_at(Vec3::ZERO, up);
            assert!(
                camera_ground_forward(&follow, up)
                    .unwrap()
                    .abs_diff_eq(forward, 1e-5)
            );
        }
    }

    #[test]
    fn passing_under_the_overhead_camera_does_not_reverse_screen_right() {
        let mut app = App::new();
        let mut time = Time::<()>::default();
        time.advance_by(Duration::from_secs_f32(1.0 / 60.0));
        app.insert_resource(time).add_systems(Update, follow_camera);
        let position = Vec3::new(0.0, 14.0, -8.0);
        let zone = app
            .world_mut()
            .spawn(CameraZone {
                constraint: CameraConstraint {
                    mode: CameraMode::Fixed { position },
                    weight: 1.0,
                    priority: 1,
                },
            })
            .id();
        let player = app
            .world_mut()
            .spawn((
                Player,
                Transform::from_xyz(0.0, 0.9, -6.0),
                LocalUp::default(),
                CharacterLook::default(),
                InCameraZones(vec![zone]),
            ))
            .id();
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Transform::from_translation(position)
                    .looking_at(Vec3::new(0.0, 1.4, -6.0), Vec3::NEG_Z),
            ))
            .id();
        for frame in 0..120 {
            app.world_mut()
                .get_mut::<Transform>(player)
                .unwrap()
                .translation
                .z = -6.0 - frame as f32 / 30.0;
            let previous = app.world().get::<Transform>(camera).unwrap().rotation;
            app.world_mut().run_schedule(Update);
            let transform = app.world().get::<Transform>(camera).unwrap();
            assert!(transform.rotation.is_finite());
            assert!(transform.rotation.angle_between(previous) < 0.1);
            assert!(transform.right().dot(Vec3::X) > 0.99);
            assert!(
                camera_ground_forward(transform, Vec3::Y)
                    .unwrap()
                    .dot(Vec3::NEG_Z)
                    > 0.99
            );
        }
    }
}
