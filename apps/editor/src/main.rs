//! The Struction editor: egui panels around a live Bevy viewport, bound to
//! `struction_editor::AuthoringProject` so the GUI edits, validates, undoes and plays through the
//! same operations AI tools use. `struction-editor [project-directory]`.

mod game;
mod state;
mod theme;
mod ui;
mod viewport;

use bevy::{
    camera::{CameraOutputMode, visibility::RenderLayers},
    prelude::*,
    render::render_resource::BlendState,
    window::WindowFocused,
};
use bevy_egui::{EguiGlobalSettings, EguiPlugin, EguiPrimaryContextPass, PrimaryEguiContext};

use crate::state::{Command, Editor};

fn main() {
    let mut editor = Editor::default();
    if let Some(root) = std::env::args_os().nth(1) {
        editor.apply(Command::Open(root.into()));
    }
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.07, 0.075, 0.09)))
        .insert_non_send(editor)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Struction".into(),
                resolution: (1440, 900).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins((EguiPlugin::default(), viewport::ViewportPlugin))
        .add_systems(Startup, ui_camera)
        .add_systems(
            Update,
            refresh_on_focus.before(viewport::ViewportSystems::Input),
        )
        .add_systems(EguiPrimaryContextPass, ui::editor_ui)
        .run();
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
