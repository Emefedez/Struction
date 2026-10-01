//! Native physics playground: the README scene, authored in `project/` with the engine's base
//! definitions and drawn with its scene vocabulary (`struction_scene`). This host adds the window, lights, HUD,
//! cursor handling and the smoke-test script.

#[cfg(test)]
mod planet_tests;
#[cfg(test)]
mod scene_tests;

use std::path::{Path, PathBuf};

use bevy::{
    app::AppExit,
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use struction_anim::plugin::Locomotor;
use struction_camera::{CameraSystems, PlayerCamera, PlayerCameraPlugin, ViewMode};
use struction_character::{
    Attacking, CharacterAnimationPlugin, CharacterLook, CharacterMove, CharacterState,
    InputActions, InputMap, InputSystems, PlayerControlled, Rolling,
};
use struction_debug::{DebugTracePlugin, TraceAppExt, TraceWriter};
use struction_gravity::{GravityInfluences, LocalUp};
use struction_physics::{
    CameraOcclusion, CameraTarget, InCameraZones, PhysicsPlugin, Submersion, avian3d::prelude::*,
};
use struction_scene::{
    ScenePlugin,
    render::{EngineAssetsPlugin, SceneRenderPlugin, SceneRenderSystems},
};

/// The playground's own project, next to its sources.
pub fn default_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("project")
}

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
    let mut project = default_project();
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
    .add_plugins(EngineAssetsPlugin)
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
        ScenePlugin { root: project },
        SceneRenderPlugin,
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
        (
            SceneRenderSystems::Dress.in_set(PlaygroundSystems::Dress),
            CameraSystems::Follow.in_set(PlaygroundSystems::Camera),
            SceneRenderSystems::View
                .after(PlaygroundSystems::Camera)
                .before(PlaygroundSystems::Hud),
        ),
    )
    .add_systems(Startup, setup)
    .add_systems(PreUpdate, cursor_controls.in_set(PlaygroundSystems::Cursor))
    .add_systems(PreUpdate, scripted_input.in_set(PlaygroundSystems::Script))
    .add_systems(Update, attach_camera.in_set(PlaygroundSystems::Dress))
    .add_systems(Update, count_footfalls.in_set(PlaygroundSystems::Camera))
    .add_systems(Update, update_hud.in_set(PlaygroundSystems::Hud))
    .add_systems(Update, exit_control.in_set(PlaygroundSystems::Exit))
    .run();
    Ok(())
}

/// The scene itself is data; this adds what only the rendered host needs.
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
                root: default_project(),
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
}
