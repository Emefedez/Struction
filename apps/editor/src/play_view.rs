//! Window focus and device mapping stop at the structured headless play-input API.
use crate::{
    state::Editor,
    ui::Typing,
    viewport::{SceneCamera, viewport_cursor},
};
use bevy::{
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};
use bevy_egui::input::EguiWantsInput;
use struction_editor::PlayInput;

#[derive(Resource, Default)]
pub struct PlayControl {
    pub captured: bool,
}

pub fn camera_transform(editor: &Editor) -> Option<Transform> {
    editor
        .project
        .as_ref()?
        .play_world()?
        .iter_entities()
        .find_map(|entity| {
            entity
                .contains::<struction_camera::PlayerCamera>()
                .then(|| entity.get::<Transform>().copied())
                .flatten()
        })
}

#[allow(clippy::too_many_arguments)]
pub fn capture_input(
    mut editor: NonSendMut<Editor>,
    mut control: ResMut<PlayControl>,
    window: Single<(&Window, &mut CursorOptions), With<PrimaryWindow>>,
    camera: Single<&Camera, With<SceneCamera>>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    typing: Res<Typing>,
    egui: Res<EguiWantsInput>,
) {
    let (window, mut cursor) = window.into_inner();
    let running = editor.play.as_ref().is_some_and(|play| play.running);
    let was_captured = control.captured;
    if !running
        || !window.focused
        || typing.0
        || !camera.is_active
        || keys.just_pressed(KeyCode::Escape)
    {
        control.captured = false;
    } else if buttons.just_pressed(MouseButton::Left)
        && viewport_cursor(window, &camera, &egui).is_some()
        && camera_transform(&editor).is_some()
    {
        control.captured = true;
    }
    cursor.grab_mode = if control.captured {
        CursorGrabMode::Locked
    } else {
        CursorGrabMode::None
    };
    cursor.visible = !control.captured;
    let Some(project) = &mut editor.project else {
        return;
    };
    if !control.captured {
        if was_captured || !running {
            project.release_play_input();
        }
        return;
    }
    let axis = |positive, alternate, negative, other| {
        f32::from(keys.any_pressed([positive, alternate]))
            - f32::from(keys.any_pressed([negative, other]))
    };
    let input = PlayInput {
        movement: [
            axis(
                KeyCode::KeyD,
                KeyCode::ArrowRight,
                KeyCode::KeyA,
                KeyCode::ArrowLeft,
            ),
            axis(
                KeyCode::KeyW,
                KeyCode::ArrowUp,
                KeyCode::KeyS,
                KeyCode::ArrowDown,
            ),
        ],
        look: if was_captured {
            (motion.delta * 0.003).to_array()
        } else {
            [0.0; 2]
        },
        jump_held: keys.pressed(KeyCode::Space),
        jump_pressed: keys.just_pressed(KeyCode::Space),
        roll_pressed: keys.any_just_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]),
        attack_pressed: keys.just_pressed(KeyCode::KeyF)
            || (was_captured && buttons.just_pressed(MouseButton::Left)),
        toggle_view_pressed: keys.just_pressed(KeyCode::KeyV),
        zoom: scroll.delta.y
            * if scroll.unit == MouseScrollUnit::Pixel {
                0.02
            } else {
                1.0
            },
    };
    if let Err(error) = project.play_input(input) {
        warn!("Play input: {error}");
    }
}
