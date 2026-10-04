//! The Struction editor: egui panels around a live Bevy viewport, bound to
//! `struction_editor::AuthoringProject` so the GUI edits, validates, undoes and plays through the
//! same operations AI tools use. Mesh sources open in a separate mesh tool window (`tools`).
//!
//! `struction-editor [project-directory] [--mesh <asset> [--mode inspect|lods|collision|poses|uvs|materials]]
//! [--screenshot <directory>]`: `--mesh` opens a project-relative source in the mesh tool;
//! `--screenshot` saves `editor.png` (and `mesh-tool.png`) there once everything has drawn, then
//! quits, for checking the GUI without a person at the screen. `--headless` serves the JSONL
//! authoring protocol on stdin/stdout and `--mcp` the same operations as a Model Context Protocol
//! server, both without a window.

mod assistant;
mod game;
mod inspector_fields;
mod play_view;
mod pose_tool;
mod programs;
mod scene_view;
mod spatial_guides;
mod state;
mod theme;
mod tool_ui;
mod tools;
mod ui;
mod viewport;

use bevy::{
    camera::{CameraOutputMode, visibility::RenderLayers},
    prelude::*,
    render::render_resource::BlendState,
    window::{ExitCondition, WindowFocused},
};
use bevy_egui::{EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext};

use crate::state::{Command, Editor};
use crate::tools::{Mode, Request, Toolbox};

const USAGE: &str = "struction-editor [project-directory] [--mesh <asset> [--mode inspect|lods|collision|poses|uvs|materials]] [--screenshot <directory>] [--blender <executable>] [--headless | --mcp] [--play]";

fn main() {
    let mut editor = Editor::default();
    let (mut mesh, mut mode, mut capture) = (None, Mode::Inspect, None);
    let mut blender = None;
    let mut headless = false;
    let mut mcp = false;
    let mut play = false;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        let mut value = || args.next().unwrap_or_else(|| exit_with_usage());
        match arg.to_str() {
            Some("--headless") => headless = true,
            Some("--mcp") => mcp = true,
            Some("--play") => play = true,
            Some("--blender") => blender = Some(std::path::PathBuf::from(value())),
            Some("--mesh") => mesh = Some(value().to_string_lossy().into_owned()),
            Some("--mode") => {
                mode = match value().to_str() {
                    Some("inspect") => Mode::Inspect,
                    Some("lods") => Mode::Lods,
                    Some("collision") => Mode::Collision,
                    Some("poses") => Mode::Poses,
                    Some("uvs") => Mode::Uvs,
                    Some("materials") => Mode::Materials,
                    _ => exit_with_usage(),
                }
            }
            Some("--screenshot") => capture = Some(std::path::PathBuf::from(value())),
            Some("--help" | "-h") => exit_with_usage(),
            _ if editor.root.is_none() => editor.apply(Command::Open(arg.into())),
            _ => exit_with_usage(),
        }
    }
    if headless || mcp {
        let Some(project) = editor.project.as_mut() else {
            eprintln!("--headless and --mcp require a project directory");
            if let Some(rejection) = &editor.rejection {
                eprintln!("{}", rejection.message);
            }
            std::process::exit(2);
        };
        let (input, output) = (std::io::stdin().lock(), std::io::stdout().lock());
        let served = if mcp {
            struction_editor::mcp::serve(project, input, output)
        } else {
            struction_editor::protocol::serve(project, input, output)
        };
        if let Err(error) = served {
            eprintln!("Authoring protocol: {error}");
            std::process::exit(1);
        }
        return;
    }
    if play {
        editor.apply(Command::StartPlay);
    }
    let mut toolbox = Toolbox::default();
    if let Some(executable) = blender
        && let Err(error) = toolbox.programs.select(executable, false)
    {
        eprintln!("Blender: {error}");
        std::process::exit(2);
    }
    if let Some(asset) = mesh {
        toolbox.requests.push(Request::Open { asset, mode });
    }
    let mut app = App::new();
    if let Some(dir) = capture {
        app.insert_resource(Capture {
            dir,
            frames: 0,
            waited: 0,
        })
        .add_systems(Last, capture_windows);
    }
    app.insert_resource(ClearColor(Color::srgb(0.07, 0.075, 0.09)))
        .insert_non_send(editor)
        .add_plugins(struction_scene::render::EngineAssetsPlugin)
        .add_plugins(
            DefaultPlugins
                .set(bevy::asset::AssetPlugin {
                    unapproved_path_mode: bevy::asset::UnapprovedPathMode::Allow,
                    ..default()
                })
                .set(WindowPlugin {
                    primary_window: Some(Window {
                        title: "Struction".into(),
                        resolution: (1440, 900).into(),
                        ..default()
                    }),
                    // The mesh tool asks before closing with unapplied edits; closing the editor quits.
                    close_when_requested: false,
                    exit_condition: ExitCondition::OnPrimaryClosed,
                    ..default()
                }),
        )
        .add_plugins((
            EguiPlugin::default(),
            struction_scene::render::SceneVisualsPlugin,
            viewport::ViewportPlugin,
            tools::ToolboxPlugin,
            assistant::AssistantPlugin,
        ))
        .insert_resource(toolbox)
        .add_systems(Startup, ui_camera)
        .add_systems(
            Update,
            refresh_on_focus.before(viewport::ViewportSystems::Input),
        )
        .add_systems(EguiPrimaryContextPass, ui::editor_ui)
        .add_systems(tools::ToolWindowPass, tool_ui::tool_ui)
        .run();
}

fn exit_with_usage() -> ! {
    eprintln!("usage: {USAGE}");
    std::process::exit(2)
}

#[derive(Resource)]
struct Capture {
    dir: std::path::PathBuf,
    frames: u32,
    /// Frames spent waiting for the mesh tool; a stuck import still gets captured.
    waited: u32,
}

/// Frames to wait once the windows show what they will show, so rendering has settled.
const SETTLE_FRAMES: u32 = 45;

fn capture_windows(
    mut commands: Commands,
    mut capture: ResMut<Capture>,
    toolbox: Res<Toolbox>,
    models: Res<struction_scene::render::Models>,
    assets: Res<AssetServer>,
    primary: Single<Entity, With<bevy::window::PrimaryWindow>>,
    mut exit: MessageWriter<AppExit>,
) {
    let ready =
        toolbox.tool.as_ref().is_none_or(tools::MeshTool::is_shown) && !models.is_loading(&assets);
    capture.waited += 1;
    if !ready && capture.waited < 1200 {
        return;
    }
    capture.frames += 1;
    if capture.frames == SETTLE_FRAMES {
        use bevy::render::view::window::screenshot::{Screenshot, save_to_disk};
        std::fs::create_dir_all(&capture.dir).expect("screenshot directory");
        commands
            .spawn(Screenshot::window(*primary))
            .observe(save_to_disk(capture.dir.join("editor.png")));
        if let Some(tool) = &toolbox.tool {
            commands
                .spawn(Screenshot::window(tool.view_entities().1))
                .observe(save_to_disk(capture.dir.join("mesh-tool.png")));
        }
    }
    if capture.frames == SETTLE_FRAMES + 30 {
        exit.write(AppExit::Success);
    }
}

fn ui_camera(mut commands: Commands, mut egui_settings: ResMut<EguiGlobalSettings>) {
    // egui gets its own camera so the scene camera's viewport can shrink to the free area.
    egui_settings.auto_create_primary_context = false;
    commands.spawn((
        PrimaryEguiContext,
        Camera2d,
        RenderLayers::none(),
        Camera {
            order: 1,
            output_mode: CameraOutputMode::Write {
                blend_state: Some(BlendState::ALPHA_BLENDING),
                clear_color: ClearColorConfig::None,
            },
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
    ));
}

/// Sources edited in another tool are picked up when the editor regains focus.
fn refresh_on_focus(mut focus: MessageReader<WindowFocused>, mut editor: NonSendMut<Editor>) {
    // Drains every message, unlike `any`, so none is seen again next frame.
    let focused = focus.read().filter(|event| event.focused).count() > 0;
    if focused && editor.project.is_some() && !editor.playing() {
        editor.apply(Command::Refresh);
    }
}
