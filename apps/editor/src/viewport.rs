//! The 3D view of the project snapshot: a marker per authored entity, an orbit camera, click
//! selection and drag moves. Moves go through `AuthoringProject::move_spawn`, one history group
//! per drag, so the view never owns positions itself.
use crate::{play_view, scene_view, spatial_guides};
use bevy::camera::primitives::Aabb;
use struction_character::RigOf;

use crate::state::{Command, Editor, Selected};
use crate::ui::Typing;
use bevy::{
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit},
    prelude::*,
    window::PrimaryWindow,
};
use bevy_egui::input::EguiWantsInput;

/// The 3D camera whose viewport fills the space the panels leave.
#[derive(Component)]
pub struct SceneCamera;

#[derive(Resource)]
pub struct Orbit {
    pub focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for Orbit {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            yaw: 0.7,
            pitch: -0.45,
            distance: 22.0,
        }
    }
}

struct Drag {
    path: String,
    group: String,
    start: Vec3,
    grab: Vec3,
    normal: Vec3,
    vertical: bool,
    sent: Vec3,
}

#[derive(Resource, Default)]
struct Dragging(Option<Drag>);

const MARKER_HEIGHT: f32 = 1.8;
const POINT_RADIUS: f32 = 0.5;

#[derive(SystemSet, Clone, Debug, PartialEq, Eq, Hash)]
pub enum ViewportSystems {
    Input,
    Sync,
}

pub struct ViewportPlugin;

impl Plugin for ViewportPlugin {
    fn build(&self, app: &mut App) {
        order_camera_layout(app);
        app.init_resource::<Orbit>()
            .init_resource::<Dragging>()
            .init_resource::<Typing>()
            .init_resource::<play_view::PlayControl>()
            .init_resource::<spatial_guides::GuideSettings>()
            .add_systems(Startup, setup)
            .configure_sets(
                Update,
                (ViewportSystems::Input, ViewportSystems::Sync)
                    .chain()
                    .before(struction_scene::SceneRigSystems),
            )
            .add_systems(
                Update,
                (
                    (
                        frame_project,
                        focus_selection,
                        orbit,
                        pick_and_drag,
                        play_view::capture_input,
                        advance_play,
                    )
                        .chain()
                        .in_set(ViewportSystems::Input),
                    (
                        scene_view::sync_entities,
                        place_camera,
                        draw_guides,
                        spatial_guides::draw,
                        scene_view::selection_outline,
                    )
                        .chain()
                        .in_set(ViewportSystems::Sync),
                ),
            )
            .add_systems(
                Update,
                scene_view::sync_poses.after(struction_scene::render::SceneRenderSystems::Dress),
            );
    }
}

// Egui changes the viewport and can reactivate a compact-layout camera. Do that
// before camera projection, visibility and shadow cascades consume its state.
fn order_camera_layout(app: &mut App) {
    app.configure_sets(
        PostUpdate,
        bevy_egui::EguiPostUpdateSet::EndPass.before(bevy::camera::CameraUpdateSystems),
    );
}

fn setup(mut commands: Commands) {
    commands.spawn((Name::new("Camera"), SceneCamera, Camera3d::default()));
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            shadow_maps_enabled: true,
            ..default()
        },
        Transform::from_xyz(8.0, 16.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 400.0,
        ..default()
    });
}

/// Whether the cursor is over the 3D view rather than a panel, in window coordinates.
pub fn viewport_cursor(window: &Window, camera: &Camera, egui: &EguiWantsInput) -> Option<Vec2> {
    if !camera.is_active {
        return None;
    }
    let cursor = window.cursor_position()?;
    let inside = camera
        .logical_viewport_rect()
        .is_some_and(|rect| rect.contains(cursor));
    (inside && !egui.is_popup_open()).then_some(cursor)
}

/// A newly opened project is framed around its entities.
fn frame_project(
    editor: NonSend<Editor>,
    mut orbit: ResMut<Orbit>,
    mut framed: Local<Option<std::path::PathBuf>>,
) {
    if editor.root == *framed {
        return;
    }
    framed.clone_from(&editor.root);
    let positions: Vec<Vec3> = editor
        .entities
        .iter()
        .filter(|entity| entity.is_instance())
        .filter_map(|entity| entity.position)
        .collect();
    if positions.is_empty() {
        *orbit = Orbit::default();
        return;
    }
    let center = positions.iter().sum::<Vec3>() / positions.len() as f32;
    let extent = positions
        .iter()
        .map(|p| p.distance(center))
        .fold(0.0, f32::max);
    orbit.focus = center + Vec3::Y * MARKER_HEIGHT / 2.0;
    orbit.distance = (extent * 2.5 + 10.0).min(200.0);
}

fn focus_selection(
    keys: Res<ButtonInput<KeyCode>>,
    typing: Res<Typing>,
    editor: NonSend<Editor>,
    mut orbit: ResMut<Orbit>,
) {
    if editor.playing() || typing.0 || !keys.just_pressed(KeyCode::KeyF) {
        return;
    }
    if let Some(Selected::Entity(target)) = &editor.selected
        && let Some(position) = editor.entity(target).and_then(|e| e.position)
    {
        orbit.focus = position + Vec3::Y * MARKER_HEIGHT / 2.0;
        orbit.distance = orbit.distance.min(10.0);
    }
}

#[allow(clippy::too_many_arguments)]
fn orbit(
    editor: NonSend<Editor>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    egui: Res<EguiWantsInput>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &Transform), With<SceneCamera>>,
    mut orbit: ResMut<Orbit>,
    mut held: Local<bool>,
) {
    if editor.playing() {
        return;
    }
    let (camera, transform) = *camera;
    let over = viewport_cursor(&window, camera, &egui).is_some();
    let buttons_down = buttons.any_pressed([MouseButton::Right, MouseButton::Middle]);
    if buttons.any_just_pressed([MouseButton::Right, MouseButton::Middle]) {
        *held = over;
    } else if !buttons_down {
        *held = false;
    }
    if over {
        orbit.distance = (orbit.distance * zoom_factor(&scroll)).clamp(1.5, 400.0);
    }
    if !*held {
        return;
    }
    let delta = motion.delta;
    if buttons.pressed(MouseButton::Middle) {
        let scale = orbit.distance * 0.0016;
        let pan = (transform.right() * -delta.x + transform.up() * delta.y) * scale;
        orbit.focus += pan;
    } else {
        orbit.yaw -= delta.x * 0.006;
        orbit.pitch = (orbit.pitch - delta.y * 0.006).clamp(-1.5, 1.5);
    }
}

/// How much this frame's scrolling scales an orbit distance. Proportional to the amount, since
/// trackpads send many small pixel deltas (with momentum) where a wheel sends whole lines.
pub fn zoom_factor(scroll: &AccumulatedMouseScroll) -> f32 {
    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / 60.0,
    };
    0.88f32.powf(lines.clamp(-3.0, 3.0))
}

fn place_camera(
    editor: NonSend<Editor>,
    orbit: Res<Orbit>,
    mut camera: Single<&mut Transform, With<SceneCamera>>,
) {
    if let Some(transform) = play_view::camera_transform(&editor) {
        **camera = transform;
        return;
    }
    let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    **camera = Transform::from_translation(orbit.focus + rotation * Vec3::Z * orbit.distance)
        .with_rotation(rotation);
}

fn advance_play(time: Res<Time>, mut editor: NonSendMut<Editor>) {
    editor.advance(time.delta_secs());
}

/// Zones and spawners are points drawn as gizmos.
fn point_hit(ray: Ray3d, position: Vec3) -> Option<f32> {
    let t = (position - ray.origin).dot(*ray.direction).max(0.0);
    (ray.get_point(t).distance(position) <= POINT_RADIUS).then_some(t)
}

#[allow(clippy::too_many_arguments)]
fn pick_and_drag(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<KeyCode>>,
    egui: Res<EguiWantsInput>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<SceneCamera>>,
    mut editor: NonSendMut<Editor>,
    mut dragging: ResMut<Dragging>,
    mut drags: Local<u64>,
    meshes: Query<(Entity, &Aabb, &GlobalTransform)>,
    proxies: Query<&scene_view::SceneEntity>,
    parents: Query<&ChildOf>,
    rigs: Query<&RigOf>,
) {
    if editor.playing() {
        return;
    }
    let (camera, camera_transform) = *camera;
    let ray = window
        .cursor_position()
        .and_then(|cursor| camera.viewport_to_world(camera_transform, cursor).ok());

    if buttons.just_released(MouseButton::Left) {
        if dragging.0.take().is_some() {
            editor.apply(Command::EndGroup);
        }
        return;
    }
    if let Some(drag) = &mut dragging.0 {
        let Some(ray) = ray else { return };
        let Some(t) = ray.intersect_plane(drag.grab, InfinitePlane3d::new(drag.normal)) else {
            return;
        };
        let mut delta = ray.get_point(t) - drag.grab;
        if drag.vertical {
            delta = Vec3::Y * delta.y;
        } else {
            delta.y = 0.0;
        }
        let position = drag.start + delta;
        if position.distance(drag.sent) > 1e-3 {
            drag.sent = position;
            let (path, group) = (drag.path.clone(), drag.group.clone());
            editor.apply(Command::Move {
                path,
                position,
                group: Some(group),
            });
        }
        return;
    }

    if !buttons.just_pressed(MouseButton::Left) || viewport_cursor(&window, camera, &egui).is_none()
    {
        return;
    }
    let Some(ray) = ray else { return };
    let mesh_hit = meshes
        .iter()
        .filter_map(|(entity, bounds, transform)| {
            let proxy = scene_view::owner(entity, &proxies, &parents, &rigs)?;
            Some((
                scene_view::bounds_hit(ray, transform, bounds)?,
                editor.entity(&proxy.0)?,
            ))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0));
    let hit = mesh_hit.or_else(|| {
        editor
            .entities
            .iter()
            .filter_map(|entity| Some((point_hit(ray, entity.position?)?, entity)))
            .min_by(|a, b| a.0.total_cmp(&b.0))
    });
    let Some((_, entity)) = hit else {
        editor.apply(Command::Select(None));
        return;
    };
    let (key, position) = (entity.key().to_owned(), entity.position.unwrap_or_default());
    // Only named spawns have an authored offset to move.
    let movable = !editor.playing() && entity.is_named_spawn();
    editor.apply(Command::Select(Some(Selected::Entity(key.clone()))));
    if movable {
        // Shift drags height on a plane facing the camera; otherwise along the ground.
        let vertical = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);
        let normal = if vertical {
            let facing = camera_transform.forward().as_vec3();
            Vec3::new(facing.x, 0.0, facing.z).normalize_or(Vec3::Z)
        } else {
            Vec3::Y
        };
        let grab = ray
            .intersect_plane(position, InfinitePlane3d::new(normal))
            .map_or(position, |t| ray.get_point(t));
        *drags += 1;
        dragging.0 = Some(Drag {
            group: format!("viewport-drag-{}", *drags),
            path: key,
            start: position,
            grab,
            normal,
            vertical,
            sent: position,
        });
    }
}

fn draw_guides(editor: NonSend<Editor>, mut gizmos: Gizmos) {
    if editor.project.is_none() || editor.playing() {
        return;
    }
    gizmos.grid(
        Isometry3d::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        UVec2::splat(60),
        Vec2::ONE,
        Color::srgba(1.0, 1.0, 1.0, 0.06),
    );
    let flat = Quat::from_rotation_x(std::f32::consts::FRAC_PI_2);
    for entity in editor.entities.iter().filter(|e| !e.is_instance()) {
        let Some(Transform {
            translation: position,
            rotation,
            ..
        }) = entity.transform()
        else {
            continue;
        };
        let color = Color::srgba(0.55, 0.58, 0.63, 0.8);
        // Spawners (with a source) are diamonds; zones are squares.
        let turn = if entity.source.is_none() { 0.0 } else { 0.785 };
        let frame = Isometry3d::new(position, rotation * Quat::from_rotation_y(turn) * flat);
        gizmos.rect(frame, Vec2::splat(POINT_RADIUS * 1.6), color);
        gizmos.arrow(position, position + rotation * Vec3::Z * 0.9, color);
    }
    let Some(Selected::Entity(target)) = &editor.selected else {
        return;
    };
    let Some(position) = editor.entity(target).and_then(|e| e.position) else {
        return;
    };
    let axes = [
        (Vec3::X, Color::srgb(0.91, 0.36, 0.36)),
        (Vec3::Y, Color::srgb(0.49, 0.78, 0.35)),
        (Vec3::Z, Color::srgb(0.35, 0.61, 0.91)),
    ];
    for (axis, color) in axes {
        gizmos.arrow(position, position + axis * 1.2, color);
    }
    gizmos.circle(
        Isometry3d::new(position, Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
        0.75,
        Color::srgb(0.95, 0.66, 0.23),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reactivated_camera_has_shadow_cascades_in_the_same_frame() {
        use bevy::light::{
            Cascades, DirectionalLightShadowMap, cascade::build_directional_light_cascades,
        };
        let mut app = App::new();
        order_camera_layout(&mut app);
        app.init_resource::<DirectionalLightShadowMap>();
        app.add_systems(
            PostUpdate,
            build_directional_light_cascades.after(bevy::camera::CameraUpdateSystems),
        );
        app.add_systems(
            PostUpdate,
            (|mut cameras: Query<&mut Camera>| {
                for mut camera in &mut cameras {
                    camera.is_active = true;
                }
            })
            .in_set(bevy_egui::EguiPostUpdateSet::EndPass),
        );
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                Camera {
                    is_active: false,
                    ..default()
                },
                GlobalTransform::IDENTITY,
            ))
            .id();
        let light = app
            .world_mut()
            .spawn((
                DirectionalLight {
                    shadow_maps_enabled: true,
                    ..default()
                },
                GlobalTransform::IDENTITY,
            ))
            .id();
        app.update();
        assert!(
            app.world()
                .get::<Cascades>(light)
                .unwrap()
                .cascades
                .contains_key(&camera)
        );
    }

    #[test]
    fn zoom_follows_the_scroll_amount() {
        let scroll = |unit, y| AccumulatedMouseScroll {
            unit,
            delta: Vec2::new(0.0, y),
        };
        assert_eq!(zoom_factor(&scroll(MouseScrollUnit::Line, 0.0)), 1.0);
        let notch = zoom_factor(&scroll(MouseScrollUnit::Line, 1.0));
        assert!((notch - 0.88).abs() < 1e-6);
        // A small trackpad movement zooms a little, not a whole notch.
        let pixels = zoom_factor(&scroll(MouseScrollUnit::Pixel, 3.0));
        assert!(pixels > 0.99 && pixels < 1.0, "{pixels}");
        // A burst is capped per frame.
        let burst = zoom_factor(&scroll(MouseScrollUnit::Line, 50.0));
        assert!((burst - 0.88f32.powi(3)).abs() < 1e-6);
    }
}
