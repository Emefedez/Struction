//! Native physics playground with a procedurally animated player.

mod camera_occlusion;
mod figures;
use struction_playground::scene;

use camera_occlusion::{FadeMaterial, FadesWith, fade_material};
#[cfg(test)]
mod planet_tests;
use scene::{Finish, Look, ScenePlugin, Shape};

use bevy::{
    app::AppExit,
    light::NotShadowCaster,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use struction_anim::plugin::Locomotor;
use struction_camera::{CameraSystems, PlayerCamera, PlayerCameraPlugin, ViewMode};
use struction_character::{
    Attacking, CharacterAnimationPlugin, CharacterLook, CharacterMove, CharacterState,
    InputActions, InputMap, InputSystems, PlayerControlled, RigOf, Rolling,
};
use struction_debug::{DebugTracePlugin, TraceAppExt, TraceWriter};
use struction_gravity::{GravityInfluences, LocalUp};
use struction_physics::{
    CameraOcclusion, CameraTarget, InCameraZones, PhysicsPlugin, Submersion, Volume, VolumeShape,
    avian3d::prelude::*,
};

#[derive(Resource)]
struct PlaygroundOptions {
    smoke: bool,
    jump_sent: bool,
    roll_sent: bool,
    attack_sent: bool,
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
                project = std::path::absolute(
                    args.next()
                        .ok_or("--project requires a project directory")?,
                )?
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
            .trace_component::<Rolling>()
            .trace_component::<Attacking>()
            .trace_component::<CharacterMove>()
            .trace_component::<struction_character::CharacterIntent>()
            .trace_component::<LocalUp>()
            .trace_component::<GravityInfluences>()
            .trace_component::<Submersion>()
            .trace_component::<InCameraZones>();
    }
    app.insert_resource(PlaygroundOptions {
        smoke,
        jump_sent: false,
        roll_sent: false,
        attack_sent: false,
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
        ScenePlugin { root: project },
        figures::FiguresPlugin,
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
    .add_systems(
        Update,
        (
            attach_camera,
            dress_looks,
            figures::attach_rigs,
            figures::request_models,
            figures::finish_compiles,
            figures::dress_figures,
        )
            .chain()
            .in_set(PlaygroundSystems::Dress),
    )
    .add_systems(
        Update,
        (hide_rig_in_first_person, count_footfalls).in_set(PlaygroundSystems::Camera),
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
    spawn_hud(&mut commands);
}

fn matte(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.88,
        ..default()
    }
}

type Dressed<'a> = (
    Entity,
    &'a Look,
    Option<&'a Shape>,
    Option<&'a Volume>,
    Option<&'a WaterSurface>,
);

type Redrawn = (
    With<Look>,
    Or<(Changed<Look>, Changed<Shape>, Changed<Volume>)>,
);

/// The surface drawn for a water volume, replaced when live reload changes the volume's look.
#[derive(Component)]
struct WaterSurface(Entity);

/// Gives authored entities their mesh and material from their `Shape` and `Look`, again whenever
/// live reload edits either.
fn dress_looks(
    mut commands: Commands,
    looks: Query<Dressed, Redrawn>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fading: ResMut<Assets<FadeMaterial>>,
) {
    for (entity, look, shape, volume, surface) in &looks {
        if let Some(surface) = surface {
            commands.entity(surface.0).despawn();
        }
        commands.entity(entity).remove::<(
            WaterSurface,
            Mesh3d,
            MeshMaterial3d<StandardMaterial>,
            MeshMaterial3d<FadeMaterial>,
        )>();
        let [red, green, blue] = look.color;
        let color = Color::srgba(red, green, blue, look.opacity);
        if look.finish == Finish::Water {
            let Some(VolumeShape::Box { half_extents }) = volume.map(|v| v.shape) else {
                warn!("a water look needs a box volume; {entity} is not drawn");
                continue;
            };
            // A closed transparent box overlaps the pool floor and blends its own unsorted faces.
            let size = half_extents.xz() * 2.0;
            let surface = commands
                .spawn((
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
                ))
                .id();
            commands
                .entity(entity)
                .insert((Visibility::default(), WaterSurface(surface)));
            continue;
        }
        // Humanoids are drawn by their rig (see `figures`).
        let Some(mesh) = shape.and_then(Shape::mesh) else {
            if shape.is_none() {
                warn!("{entity} has a look but no shape to draw");
            }
            continue;
        };
        let mesh = Mesh3d(meshes.add(mesh));
        let mut entity = commands.entity(entity);
        match look.finish {
            // Ground that can hide the player gets a cut-out along the camera's line of sight.
            Finish::Ground => entity.insert((
                mesh,
                MeshMaterial3d(fade_material(&mut fading, matte(color))),
            )),
            _ => entity.insert((mesh, MeshMaterial3d(materials.add(matte(color))))),
        };
    }
}

/// The player comes from scene data, so its camera is attached once it spawns; camera zones then
/// reframe the view around it.
fn attach_camera(mut commands: Commands, players: Query<Entity, Added<PlayerControlled>>) {
    for player in &players {
        commands.entity(player).insert(CameraTarget);
        commands.spawn((
            Name::new("Player camera"),
            Camera3d::default(),
            CameraOcclusion::default(),
            PlayerCamera::new(player),
        ));
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
    if seconds >= 3.0 && !options.roll_sent {
        actions.roll.pressed = true;
        options.roll_sent = true;
    }
    if seconds >= 4.5 && !options.attack_sent {
        actions.attack.pressed = true;
        options.attack_sent = true;
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

type Reported<'a> = (
    &'a CharacterState,
    &'a Submersion,
    &'a InCameraZones,
    Has<Rolling>,
    Has<Attacking>,
);

fn update_hud(
    player: Query<Reported, With<PlayerControlled>>,
    camera: Query<&PlayerCamera>,
    mut hud: Query<&mut Text, With<Hud>>,
    time: Res<Time>,
    options: Res<PlaygroundOptions>,
) {
    let (Ok((state, submersion, zones, rolling, attacking)), Ok(mut text)) =
        (player.single(), hud.single_mut())
    else {
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
    let doing = match (rolling, attacking) {
        (true, _) => "rolling",
        (_, true) => "attacking",
        _ => "-",
    };
    **text = format!(
        "STRUCTION / PLAYGROUND\nWASD move  |  Space jump / swim  |  Left Shift roll  |  Left click or F attack  |  M mouse look [{}]\nV or wheel: third / first person  |  Esc release / quit\nGrounded: {}   Swimming: {} ({:.0}%)   Move: {}   Camera: {}   Steps: {}   FPS: {:.0}\nBlue tile: slippery   |   Orange cube: push or hit   |   Left: a knight on guard   |   Right: water   |   Ahead: gravity planet",
        if options.cursor_grabbed { "on" } else { "off" },
        state.grounded,
        state.swimming,
        submersion.0 * 100.0,
        doing,
        view,
        options.footfalls,
        fps
    );
}

fn exit_control(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    options: Res<PlaygroundOptions>,
    player: Query<&Position, With<PlayerControlled>>,
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
    use struction_character::{CharacterControllerPlugin, InputActionsPlugin};
    use struction_physics::testing::*;

    #[test]
    fn the_data_spawned_player_gets_the_player_camera() {
        let mut app = headless_app_with((
            CharacterControllerPlugin,
            InputActionsPlugin,
            PlayerCameraPlugin,
            ScenePlugin {
                root: scene::default_project(),
            },
        ));
        app.add_systems(Update, attach_camera);
        // Spawners run in the first fixed tick; the camera follows in the updates after it.
        step(&mut app, 30);

        let mut players = app
            .world_mut()
            .query_filtered::<(Entity, &Transform), With<PlayerControlled>>();
        let (player, body) = players.single(app.world()).expect("one player");
        let focus = body.translation;
        let mut cameras = app.world_mut().query::<(&PlayerCamera, &Transform)>();
        let cameras: Vec<_> = cameras.iter(app.world()).collect();
        assert_eq!(cameras.len(), 1);
        let (camera, transform) = cameras[0];
        assert_eq!(camera.target, player);
        assert_eq!(camera.view, ViewMode::ThirdPerson);
        let distance = transform.translation.distance(focus);
        assert!((1.0..12.0).contains(&distance), "camera {distance} m away");
    }

    #[test]
    fn a_reloaded_water_look_is_redrawn_once() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<FadeMaterial>>()
            .add_systems(Update, dress_looks);
        let pool = app
            .world_mut()
            .spawn((
                Look {
                    finish: Finish::Water,
                    ..default()
                },
                Volume {
                    shape: VolumeShape::Box {
                        half_extents: Vec3::ONE,
                    },
                },
            ))
            .id();
        app.update();
        // What live reload does to an edited field.
        app.world_mut().get_mut::<Look>(pool).unwrap().color = [0.1, 0.2, 0.3];
        app.update();

        let mut surfaces = app
            .world_mut()
            .query_filtered::<&MeshMaterial3d<FadeMaterial>, With<FadesWith>>();
        let surfaces: Vec<_> = surfaces.iter(app.world()).collect();
        assert_eq!(surfaces.len(), 1);
        let material = app
            .world()
            .resource::<Assets<FadeMaterial>>()
            .get(&surfaces[0].0)
            .unwrap();
        assert_eq!(material.base.base_color, Color::srgba(0.1, 0.2, 0.3, 1.0));
    }
}
