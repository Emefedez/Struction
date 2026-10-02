//! The toolbox: the project's mesh sources and the mesh tool, a separate native window where a
//! source is inspected and its LODs and collision shapes are previewed, adjusted from presets
//! and a few controls, and applied as its recipe, with the tool's own undo. Every change goes
//! through `struction_assets::PrepSession`, the session the CLI and AI clients use; the window
//! holds only a draft of the settings and the latest preview.
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bevy::{
    asset::RenderAssetUsages,
    camera::{CameraOutputMode, RenderTarget, Viewport, visibility::RenderLayers},
    ecs::schedule::ScheduleLabel,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    mesh::PrimitiveTopology,
    prelude::*,
    render::render_resource::{BlendState, Face},
    window::{PrimaryWindow, WindowCloseRequested, WindowFocused, WindowRef, WindowResolution},
};
use bevy_egui::EguiSchedule;
use struction_assets::{
    AssetError, Blender, PrepPreview, PrepSession, PreviewJob, collision::convex_hull,
    compile::PrepareSettings, format::MeshBundle, lod_mesh,
};

use crate::state::Editor;
use crate::viewport::zoom_factor;

/// Render layer of the mesh tool's preview, kept out of the scene view and vice versa.
const TOOL_LAYER: usize = 1;
/// Settings must hold still this long before a new preview starts.
const PREVIEW_DELAY: Duration = Duration::from_millis(120);

#[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ToolWindowPass;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Inspect,
    Lods,
    Collision,
    Poses,
    Uvs,
    Materials,
}

impl Mode {
    pub const ALL: [Self; 6] = [
        Self::Inspect,
        Self::Lods,
        Self::Collision,
        Self::Poses,
        Self::Uvs,
        Self::Materials,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Inspect => "Inspect",
            Self::Lods => "LODs",
            Self::Collision => "Collision",
            Self::Poses => "Poses",
            Self::Uvs => "UVs",
            Self::Materials => "Materials",
        }
    }
}

pub enum Request {
    Open {
        asset: String,
        mode: Mode,
    },
    OpenExternally(String),
    OpenSource(String),
    BlenderWorkspace {
        asset: String,
        workspace: struction_assets::blender::BlenderWorkspace,
        object: String,
        material: String,
    },
}

#[derive(Resource, Default)]
pub struct Toolbox {
    pub programs: crate::programs::Programs,
    /// Project-relative mesh sources.
    pub assets: Vec<String>,
    scanned: Option<PathBuf>,
    rescan: bool,
    pub requests: Vec<Request>,
    pub tool: Option<MeshTool>,
    /// Last "Open in…" failure, shown with the asset.
    pub open_error: Option<String>,
    /// Closed from inside the tool's own egui pass, which still holds its context; despawned
    /// on the next update.
    closed: Vec<MeshTool>,
}

impl Toolbox {
    pub fn rescan(&mut self) {
        self.rescan = true;
    }
}

/// What happens once the close prompt is answered with apply or discard.
#[derive(Clone)]
pub enum After {
    Close,
    Open(String, Mode),
}

pub struct MeshTool {
    pub asset: String,
    pub source: PathBuf,
    pub mode: Mode,
    window: Entity,
    camera: Entity,
    ui_camera: Entity,
    pub state: ToolState,
    pub themed: bool,
    /// Set while a close (or switch) waits for apply, discard or cancel.
    pub closing: Option<After>,
    pub pointer_over_ui: bool,
    orbit: ToolOrbit,
    framed: bool,
    shown: Option<Shown>,
    /// The 3D view's rectangle in logical window pixels.
    pub view: Option<Rect>,
}

pub enum ToolState {
    Opening(JoinHandle<Result<PrepSession, AssetError>>),
    Failed(String),
    Ready(Box<Ready>),
}

pub struct Ready {
    pub session: PrepSession,
    pub draft: PrepareSettings,
    pub preview: Option<PrepPreview>,
    job: Option<(PrepareSettings, JoinHandle<Result<PrepPreview, AssetError>>)>,
    changed: Instant,
    /// Outlines of each preview's convex parts, per mesh.
    parts: Vec<Vec<Hull>>,
    pub preview_error: Option<String>,
    pub error: Option<String>,
    pub status: Option<String>,
    pub view: ViewOptions,
    pub pose: crate::pose_tool::PoseDraft,
    pub mesh_index: usize,
    /// Bumped with each new preview, so the 3D view rebuilds.
    revision: u64,
}

pub struct Hull {
    points: Vec<[f32; 3]>,
    faces: Vec<[u32; 3]>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ViewOptions {
    pub lod: usize,
    pub wireframe: bool,
    pub hull: bool,
    pub trimesh: bool,
    pub parts: bool,
}

impl Default for ViewOptions {
    fn default() -> Self {
        Self {
            lod: 1,
            wireframe: true,
            hull: true,
            trimesh: false,
            parts: true,
        }
    }
}

/// What the 3D view currently shows; it is rebuilt when this changes.
#[derive(Clone, PartialEq, Debug)]
struct Shown {
    revision: u64,
    mode: Mode,
    view: ViewOptions,
    pose: crate::pose_tool::PoseDraft,
}

impl Ready {
    fn new(session: PrepSession) -> Self {
        Self {
            draft: session.applied().clone(),
            session,
            preview: None,
            job: None,
            changed: Instant::now() - PREVIEW_DELAY,
            parts: Vec::new(),
            preview_error: None,
            error: None,
            status: None,
            view: ViewOptions::default(),
            pose: default(),
            mesh_index: 0,
            revision: 0,
        }
    }

    /// The draft differs from what the recipe holds.
    pub fn is_dirty(&self) -> bool {
        self.draft != *self.session.applied()
    }

    /// The preview shows the current draft.
    pub fn is_current(&self) -> bool {
        self.preview
            .as_ref()
            .is_some_and(|preview| preview.settings == self.draft)
    }

    pub fn is_previewing(&self) -> bool {
        self.job.is_some()
    }

    pub fn set_draft(&mut self, draft: PrepareSettings) {
        if draft != self.draft {
            self.draft = draft;
            self.changed = Instant::now();
        }
    }

    pub fn apply(&mut self, label: &str) {
        let Some(preview) = self.preview.as_ref().filter(|p| p.settings == self.draft) else {
            return;
        };
        let result = self
            .session
            .apply(preview, label)
            .map(|applied| applied.label);
        self.finish(result.map(Some));
    }

    pub fn revert(&mut self) {
        self.set_draft(self.session.applied().clone());
    }

    pub fn undo(&mut self) {
        let result = self
            .session
            .undo()
            .map(|a| a.map(|a| format!("Undid {}", a.label)));
        self.finish(result);
        self.draft = self.session.applied().clone();
    }

    pub fn redo(&mut self) {
        let result = self
            .session
            .redo()
            .map(|a| a.map(|a| format!("Redid {}", a.label)));
        self.finish(result);
        self.draft = self.session.applied().clone();
    }

    /// Picks up outside saves of the source or recipe; unapplied edits are kept.
    pub fn refresh(&mut self) {
        let dirty = self.is_dirty();
        match self.session.refresh() {
            Ok(refreshed) => {
                if refreshed.recipe_changed && !dirty {
                    self.draft = self.session.applied().clone();
                }
                if refreshed.reimported {
                    // The old preview belongs to the previous import.
                    self.preview = None;
                    self.changed = Instant::now() - PREVIEW_DELAY;
                    self.status = Some("Imported the saved source again".into());
                }
                if refreshed.recipe_changed {
                    self.status = Some(if dirty {
                        "The recipe changed outside; Revert to use it".into()
                    } else {
                        "Loaded the recipe saved outside".into()
                    });
                }
                self.error = None;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn finish(&mut self, result: Result<Option<String>, AssetError>) {
        match result {
            Ok(status) => {
                self.error = None;
                if status.is_some() {
                    self.status = status;
                }
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Starts a preview once the draft has settled, and collects finished ones.
    fn poll(&mut self) {
        if self.job.as_ref().is_some_and(|(_, job)| job.is_finished()) {
            let (_, job) = self.job.take().expect("checked above");
            match job.join() {
                Ok(Ok(preview)) => {
                    self.parts = part_hulls(&preview.bundle);
                    self.preview = Some(preview);
                    self.preview_error = None;
                    self.revision += 1;
                }
                Ok(Err(error)) => self.preview_error = Some(error.to_string()),
                Err(_) => self.preview_error = Some("the preview crashed".into()),
            }
        }
        let wanted = !self.is_current()
            && self.job.is_none()
            && self.changed.elapsed() >= PREVIEW_DELAY
            && self.preview_error.is_none();
        if wanted {
            let job: PreviewJob = self.session.preview_job(self.draft.clone());
            self.job = Some((self.draft.clone(), std::thread::spawn(move || job.run())));
        } else if self.preview_error.is_some() && self.changed.elapsed() < PREVIEW_DELAY {
            // A new edit retries a failed preview.
            self.preview_error = None;
        }
    }
}

fn part_hulls(bundle: &MeshBundle) -> Vec<Vec<Hull>> {
    bundle
        .meshes
        .iter()
        .map(|mesh| {
            mesh.collision
                .parts
                .iter()
                .filter_map(|points| convex_hull(points))
                .map(|hull| Hull {
                    points: hull.points,
                    faces: hull.faces,
                })
                .collect()
        })
        .collect()
}

impl MeshTool {
    pub fn ready(&self) -> Option<&Ready> {
        match &self.state {
            ToolState::Ready(ready) => Some(ready),
            _ => None,
        }
    }

    pub fn ready_mut(&mut self) -> Option<&mut Ready> {
        match &mut self.state {
            ToolState::Ready(ready) => Some(ready),
            _ => None,
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.ready()
            .is_some_and(|ready| ready.is_dirty() || ready.pose.dirty())
    }

    /// The 3D view shows a preview, or the tool failed to open.
    pub fn is_shown(&self) -> bool {
        self.shown.is_some() || matches!(self.state, ToolState::Failed(_))
    }

    pub fn file_name(&self) -> &str {
        self.asset.rsplit('/').next().unwrap_or(&self.asset)
    }
}

/// The external application "Open in…" hands a source to, by name.
pub fn open_in_label(source: &Path) -> &'static str {
    if source
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("blend"))
    {
        "Open in Blender"
    } else {
        "Open in default app"
    }
}

struct ToolOrbit {
    focus: Vec3,
    yaw: f32,
    pitch: f32,
    distance: f32,
}

impl Default for ToolOrbit {
    fn default() -> Self {
        Self {
            focus: Vec3::ZERO,
            // Models face -Z: start in front, a little to the side.
            yaw: std::f32::consts::PI + 0.6,
            pitch: -0.35,
            distance: 4.0,
        }
    }
}

/// A preview mesh in the tool window, rebuilt as a whole.
#[derive(Component)]
struct ToolPreview;

pub struct ToolboxPlugin;

impl Plugin for ToolboxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Toolbox>().add_systems(
            Update,
            (
                scan_assets,
                handle_requests,
                close_windows,
                poll_tool,
                refresh_tool_on_focus,
                orbit_tool,
                show_preview,
            )
                .chain(),
        );
    }
}

/// The playground keeps portable data in `project/` and model sources in sibling `assets/`.
/// Other projects retain their existing project-relative asset layout.
pub fn asset_root(root: &Path) -> PathBuf {
    if root.file_name().is_some_and(|name| name == "project")
        && let Some(parent) = root.parent()
        && parent.join("assets").is_dir()
    {
        parent.join("assets")
    } else {
        root.to_owned()
    }
}

/// Where an asset listed by [`find_project_assets`] lives: `engine://` names the engine library's
/// models, anything else is relative to the project's asset root.
pub fn asset_source(root: &Path, asset: &str) -> PathBuf {
    match asset.strip_prefix(ENGINE) {
        Some(engine) => struction_scene::engine_assets().join(engine),
        None => asset_root(root).join(asset),
    }
}

const ENGINE: &str = "engine://";

/// The project's mesh sources, then the engine library's under `engine://`.
pub fn find_project_assets(root: &Path) -> Vec<String> {
    let engine = find_assets(&struction_scene::engine_assets());
    let mut found = find_assets(&asset_root(root));
    found.extend(engine.into_iter().map(|asset| format!("{ENGINE}{asset}")));
    found
}

/// Mesh sources the pipeline accepts, relative to `root`, skipping hidden and build folders.
pub fn find_assets(root: &Path) -> Vec<String> {
    fn walk(root: &Path, dir: &Path, found: &mut Vec<String>, depth: usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && depth < 8 {
                walk(root, &path, found, depth + 1);
            } else if kind.is_file()
                && struction_assets::import::SourceKind::of(&path).is_some()
                && let Ok(relative) = path.strip_prefix(root)
            {
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut found = Vec::new();
    walk(root, root, &mut found, 0);
    found.sort();
    found
}

fn scan_assets(
    editor: NonSend<Editor>,
    mut toolbox: ResMut<Toolbox>,
    mut focus: MessageReader<WindowFocused>,
    primary: Query<(), With<PrimaryWindow>>,
) {
    let refocused = focus
        .read()
        .filter(|event| event.focused && primary.contains(event.window))
        .count()
        > 0;
    if toolbox.scanned == editor.root && !toolbox.rescan && !refocused {
        return;
    }
    toolbox.scanned.clone_from(&editor.root);
    toolbox.rescan = false;
    toolbox.assets = editor
        .root
        .as_deref()
        .map(find_project_assets)
        .unwrap_or_default();
}

fn handle_requests(mut commands: Commands, mut toolbox: ResMut<Toolbox>, editor: NonSend<Editor>) {
    for tool in std::mem::take(&mut toolbox.closed) {
        despawn_tool(&mut commands, &tool);
    }
    for request in std::mem::take(&mut toolbox.requests) {
        let Some(root) = editor.root.clone() else {
            continue;
        };
        match request {
            Request::BlenderWorkspace {
                asset,
                workspace,
                object,
                material,
            } => {
                let source = asset_source(&root, &asset);
                toolbox.open_error = toolbox
                    .programs
                    .blender
                    .open_workspace(&source, workspace, &object, &material)
                    .err()
                    .map(|e| e.to_string());
            }
            Request::OpenSource(file) => {
                let result = toolbox.programs.open_in().open(&root.join(file));
                toolbox.open_error = result.err().map(|error| error.to_string());
            }
            Request::OpenExternally(asset) => {
                let result = toolbox
                    .programs
                    .open_in()
                    .open(&asset_source(&root, &asset));
                toolbox.open_error = result.err().map(|error| error.to_string());
            }
            Request::Open { asset, mode } => {
                if let Some(tool) = &mut toolbox.tool {
                    if tool.source == asset_source(&root, &asset) {
                        tool.mode = mode;
                        commands
                            .entity(tool.window)
                            .entry::<Window>()
                            .and_modify(|mut window| window.focused = true);
                        continue;
                    }
                    if tool.is_dirty() {
                        tool.closing = Some(After::Open(asset, mode));
                        continue;
                    }
                }
                let old = toolbox.tool.take();
                if let Some(old) = old {
                    despawn_tool(&mut commands, &old);
                }
                toolbox.tool = Some(spawn_tool(
                    &mut commands,
                    &root,
                    asset,
                    mode,
                    toolbox.programs.blender.clone(),
                ));
            }
        }
    }
}

fn spawn_tool(
    commands: &mut Commands,
    root: &Path,
    asset: String,
    mode: Mode,
    blender: Blender,
) -> MeshTool {
    let source = asset_source(root, &asset);
    let name = asset.rsplit('/').next().unwrap_or(&asset).to_owned();
    let window = commands
        .spawn(Window {
            title: format!("{name} — Struction mesh tool"),
            resolution: WindowResolution::new(1180, 760),
            ..default()
        })
        .id();
    let target = RenderTarget::Window(WindowRef::Entity(window));
    let layer = RenderLayers::layer(TOOL_LAYER);
    let camera = commands
        .spawn((
            Name::new("Mesh tool camera"),
            Camera3d::default(),
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb(0.085, 0.09, 0.105)),
                ..default()
            },
            target.clone(),
            layer.clone(),
            children![(
                DirectionalLight {
                    illuminance: 6_000.0,
                    ..default()
                },
                Transform::from_xyz(-1.0, 2.0, 1.5).looking_at(Vec3::ZERO, Vec3::Y),
                layer.clone(),
            )],
        ))
        .id();
    // egui has its own camera so the 3D view can shrink to the area the panels leave.
    let ui_camera = commands
        .spawn((
            Name::new("Mesh tool UI camera"),
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
            target,
            EguiSchedule::new(ToolWindowPass),
        ))
        .id();
    let opening = source.clone();
    MeshTool {
        asset,
        source,
        mode,
        window,
        camera,
        ui_camera,
        state: ToolState::Opening(std::thread::spawn(move || {
            PrepSession::open(opening, blender)
        })),
        themed: false,
        closing: None,
        pointer_over_ui: false,
        orbit: ToolOrbit::default(),
        framed: false,
        shown: None,
        view: None,
    }
}

fn despawn_tool(commands: &mut Commands, tool: &MeshTool) {
    commands.entity(tool.camera).despawn();
    commands.entity(tool.ui_camera).despawn();
    commands.entity(tool.window).despawn();
}

/// Window close buttons: the editor closes outright, a mesh tool with unapplied edits asks.
fn close_windows(
    mut commands: Commands,
    mut requests: MessageReader<WindowCloseRequested>,
    mut toolbox: ResMut<Toolbox>,
    primary: Query<(), With<PrimaryWindow>>,
) {
    for request in requests.read() {
        if primary.contains(request.window) {
            commands.entity(request.window).despawn();
            continue;
        }
        let Some(tool) = &mut toolbox.tool else {
            commands.entity(request.window).despawn();
            continue;
        };
        if tool.window == request.window {
            if tool.is_dirty() {
                tool.closing = Some(After::Close);
            } else if let Some(tool) = toolbox.tool.take() {
                despawn_tool(&mut commands, &tool);
            }
        }
    }
}

/// Carries out an answered close prompt: closes the tool, or opens the next asset. Runs inside
/// the tool window's egui pass, so the window outlives it until the next update.
pub fn finish_close(toolbox: &mut Toolbox, after: After) {
    if let Some(tool) = toolbox.tool.take() {
        toolbox.closed.push(tool);
    }
    if let After::Open(asset, mode) = after {
        toolbox.requests.push(Request::Open { asset, mode });
    }
}

fn poll_tool(mut toolbox: ResMut<Toolbox>) {
    let Some(tool) = &mut toolbox.tool else {
        return;
    };
    let opened = match &tool.state {
        ToolState::Opening(job) if job.is_finished() => {
            let ToolState::Opening(job) =
                std::mem::replace(&mut tool.state, ToolState::Failed(String::new()))
            else {
                unreachable!()
            };
            Some(job.join())
        }
        _ => None,
    };
    match opened {
        Some(Ok(Ok(session))) => tool.state = ToolState::Ready(Box::new(Ready::new(session))),
        Some(Ok(Err(error))) => tool.state = ToolState::Failed(error.to_string()),
        Some(Err(_)) => tool.state = ToolState::Failed("importing the source crashed".into()),
        None => {}
    }
    if let Some(ready) = tool.ready_mut() {
        ready.poll();
    }
}

fn refresh_tool_on_focus(mut focus: MessageReader<WindowFocused>, mut toolbox: ResMut<Toolbox>) {
    let Some(tool) = &mut toolbox.tool else {
        focus.clear();
        return;
    };
    let window = tool.window;
    let refocused = focus
        .read()
        .filter(|event| event.focused && event.window == window)
        .count()
        > 0;
    if refocused && let Some(ready) = tool.ready_mut() {
        ready.refresh();
    }
}

fn orbit_tool(
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    windows: Query<&Window>,
    mut cameras: Query<&mut Transform>,
    mut toolbox: ResMut<Toolbox>,
    mut held: Local<bool>,
) {
    let Some(tool) = &mut toolbox.tool else {
        return;
    };
    let over = windows
        .get(tool.window)
        .ok()
        .and_then(Window::cursor_position)
        .is_some_and(|cursor| tool.view.is_some_and(|view| view.contains(cursor)))
        && !tool.pointer_over_ui;
    let orbiting = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];
    if buttons.any_just_pressed(orbiting) {
        *held = over;
    } else if !buttons.any_pressed(orbiting) {
        *held = false;
    }
    let orbit = &mut tool.orbit;
    if over {
        orbit.distance = (orbit.distance * zoom_factor(&scroll)).clamp(0.05, 500.0);
    }
    if *held {
        let delta = motion.delta;
        if buttons.pressed(MouseButton::Middle) {
            let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
            let scale = orbit.distance * 0.0016;
            orbit.focus +=
                (rotation * Vec3::NEG_X * delta.x + rotation * Vec3::Y * delta.y) * scale;
        } else {
            orbit.yaw -= delta.x * 0.006;
            orbit.pitch = (orbit.pitch - delta.y * 0.006).clamp(-1.5, 1.5);
        }
    }
    if let Ok(mut transform) = cameras.get_mut(tool.camera) {
        let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
        *transform = Transform::from_translation(orbit.focus + rotation * Vec3::Z * orbit.distance)
            .with_rotation(rotation);
    }
}

/// Places the tool's 3D view in the area its panels leave, given in egui points. Returns that
/// area in logical window coordinates, where cursor positions are measured.
pub fn place_view(camera: &mut Camera, window: &Window, view: Rect, pixels_per_point: f32) -> Rect {
    let physical = UVec2::new(window.physical_width(), window.physical_height());
    let position = (view.min * pixels_per_point).as_uvec2().min(physical);
    let size = (view.size() * pixels_per_point)
        .as_uvec2()
        .min(physical - position);
    camera.viewport = (size.x > 0 && size.y > 0).then(|| Viewport {
        physical_position: position,
        physical_size: size,
        ..default()
    });
    let scale = window.scale_factor();
    Rect::from_corners(
        position.as_vec2() / scale,
        (position + size).as_vec2() / scale,
    )
}

impl MeshTool {
    /// The tool's 3D camera and window.
    pub fn view_entities(&self) -> (Entity, Entity) {
        (self.camera, self.window)
    }
}

#[derive(Clone, Copy)]
struct Look {
    color: Color,
    alpha: bool,
    unlit: bool,
    /// A surface that line overlays should draw over where they coincide.
    under_lines: bool,
}

fn material(look: Look) -> StandardMaterial {
    StandardMaterial {
        base_color: look.color,
        perceptual_roughness: 0.75,
        unlit: look.unlit,
        alpha_mode: if look.alpha {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        },
        cull_mode: if look.alpha { None } else { Some(Face::Back) },
        double_sided: look.alpha,
        // Line lists cannot take a depth bias, so the surfaces under them are pushed back.
        depth_bias: if look.under_lines { -2000.0 } else { 0.0 },
        ..default()
    }
}

const WIRE: Color = Color::srgba(0.95, 0.66, 0.23, 0.9);
const HULL: Color = Color::srgb(0.35, 0.78, 0.95);
const TRIMESH: Color = Color::srgb(0.49, 0.85, 0.45);

/// Distinct hues for convex parts.
fn part_color(index: usize) -> Color {
    Color::hsl((index as f32 * 137.5) % 360.0, 0.7, 0.6)
}

/// Rebuilds the tool's 3D view when the preview, mode or view options change.
fn show_preview(
    mut commands: Commands,
    mut toolbox: ResMut<Toolbox>,
    editor: NonSend<Editor>,
    shown: Query<Entity, With<ToolPreview>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Some(tool) = &mut toolbox.tool else {
        for entity in &shown {
            commands.entity(entity).despawn();
        }
        return;
    };
    let mode = tool.mode;
    let Some(ready) = tool.ready() else {
        return;
    };
    let Some(preview) = &ready.preview else {
        return;
    };
    let want = Shown {
        revision: ready.revision,
        mode,
        view: ready.view,
        pose: ready.pose.clone(),
    };
    if tool.shown.as_ref() == Some(&want) {
        return;
    }
    for entity in &shown {
        commands.entity(entity).despawn();
    }
    let bundle = &preview.bundle;
    let placements = if mode == Mode::Poses {
        ready
            .pose
            .rig(&editor)
            .map(|rig| {
                let transforms = ready.pose.transforms(&rig);
                bundle
                    .nodes
                    .iter()
                    .flat_map(|node| {
                        let joint = rig
                            .skeleton
                            .joint_id(node.name.split('.').next().unwrap_or(&node.name))
                            .ok();
                        let transforms = &transforms;
                        node.meshes.iter().filter_map(move |&mesh| {
                            joint.map(|joint| (mesh as usize, transforms[joint]))
                        })
                    })
                    .collect()
            })
            .unwrap_or_else(|| placements(bundle))
    } else {
        placements(bundle)
    };
    let layer = RenderLayers::layer(TOOL_LAYER);
    let mut spawn = |mesh: Mesh, look: Look, transform: Transform| {
        commands.spawn((
            ToolPreview,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(material(look))),
            transform,
            layer.clone(),
        ));
    };

    let bounds = scene_bounds(bundle, &placements);
    let width = (bounds.1.x - bounds.0.x).max(0.1);
    let view = ready.view;
    let solid = |index: usize| {
        let color = bundle.meshes[index]
            .material
            .as_ref()
            .and_then(|name| bundle.materials.iter().find(|m| &m.name == name))
            .map_or(Color::srgb(0.62, 0.64, 0.68), |m| {
                Color::linear_rgba(m.base_color[0], m.base_color[1], m.base_color[2], 1.0)
            });
        Look {
            color,
            alpha: false,
            unlit: false,
            under_lines: true,
        }
    };
    // Side by side in LOD mode: LOD 0 on the left, the chosen level on the right.
    let lod = view.lod;
    let right = Quat::from_rotation_y(tool.orbit.yaw) * Vec3::X;
    let copies: Vec<(usize, f32)> = match mode {
        Mode::Lods if lod > 0 => vec![(0, -0.6 * width), (lod, 0.6 * width)],
        _ => vec![(0, 0.0)],
    };
    for &(index, transform) in &placements {
        let mesh = &bundle.meshes[index];
        for &(level, offset) in &copies {
            let Some(chosen) = mesh.lods.get(level.min(mesh.lods.len() - 1)) else {
                continue;
            };
            let placed = Transform::from_translation(right * offset) * transform;
            let mut look = solid(index);
            if mode == Mode::Collision {
                look.color = look.color.with_alpha(0.25);
                look.alpha = true;
            }
            spawn(lod_mesh(chosen), look, placed);
            if view.wireframe && !matches!(mode, Mode::Collision | Mode::Poses | Mode::Materials) {
                let triangles = chosen.indices.as_chunks::<3>().0.iter().copied();
                spawn(
                    wire_mesh(&chosen.positions, triangles),
                    Look {
                        color: WIRE,
                        alpha: true,
                        unlit: true,
                        under_lines: false,
                    },
                    placed,
                );
            }
        }
        if mode != Mode::Collision {
            continue;
        }
        let collision = &mesh.collision;
        let line = |color: Color| Look {
            color,
            alpha: false,
            unlit: true,
            under_lines: false,
        };
        let fill = |color: Color| Look {
            color: color.with_alpha(0.16),
            alpha: true,
            unlit: true,
            under_lines: false,
        };
        if view.hull
            && let Some(hull) = &collision.hull
        {
            spawn(
                wire_mesh(&hull.points, hull.faces.iter().copied()),
                line(HULL),
                transform,
            );
            spawn(solid_mesh(&hull.points, &hull.faces), fill(HULL), transform);
        }
        if view.trimesh {
            let trimesh = &collision.trimesh;
            spawn(
                wire_mesh(&trimesh.vertices, trimesh.triangles.iter().copied()),
                line(TRIMESH),
                transform,
            );
        }
        if view.parts {
            for (part, hull) in ready.parts.get(index).into_iter().flatten().enumerate() {
                let color = part_color(part);
                spawn(
                    wire_mesh(&hull.points, hull.faces.iter().copied()),
                    line(color),
                    transform,
                );
                spawn(
                    solid_mesh(&hull.points, &hull.faces),
                    fill(color),
                    transform,
                );
            }
        }
    }

    if !tool.framed {
        tool.framed = true;
        let center = (bounds.0 + bounds.1) / 2.0;
        let radius = (bounds.1 - bounds.0).length().max(0.1) / 2.0;
        tool.orbit.focus = center;
        tool.orbit.distance = radius * 3.2;
    }
    tool.shown = Some(want);
}

/// Each mesh index with its placement in the imported scene; unplaced meshes sit at the origin.
fn placements(bundle: &MeshBundle) -> Vec<(usize, Transform)> {
    let mut globals: Vec<Transform> = Vec::with_capacity(bundle.nodes.len());
    let mut placed = Vec::new();
    let mut used = vec![false; bundle.meshes.len()];
    for node in &bundle.nodes {
        let local = Transform {
            translation: Vec3::from(node.translation),
            rotation: Quat::from_array(node.rotation),
            scale: Vec3::from(node.scale),
        };
        let global = match node.parent {
            Some(parent) => globals[parent as usize] * local,
            None => local,
        };
        globals.push(global);
        for &mesh in &node.meshes {
            placed.push((mesh as usize, global));
            used[mesh as usize] = true;
        }
    }
    placed.extend(
        used.iter()
            .enumerate()
            .filter(|(_, used)| !**used)
            .map(|(index, _)| (index, Transform::IDENTITY)),
    );
    placed
}

fn scene_bounds(bundle: &MeshBundle, placements: &[(usize, Transform)]) -> (Vec3, Vec3) {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for (index, transform) in placements {
        let bounds = &bundle.meshes[*index].bounds;
        for corner in 0..8 {
            let local = Vec3::from_array([0, 1, 2].map(|axis| {
                if corner >> axis & 1 == 1 {
                    bounds.max[axis]
                } else {
                    bounds.min[axis]
                }
            }));
            let point = transform.transform_point(local);
            min = min.min(point);
            max = max.max(point);
        }
    }
    if min.x > max.x {
        return (Vec3::splat(-0.5), Vec3::splat(0.5));
    }
    (min, max)
}

/// Every triangle edge once, as a line list.
fn wire_mesh(positions: &[[f32; 3]], triangles: impl Iterator<Item = [u32; 3]>) -> Mesh {
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    for [a, b, c] in triangles {
        for (from, to) in [(a, b), (b, c), (c, a)] {
            if seen.insert((from.min(to), from.max(to))) {
                lines.push(positions[from as usize]);
                lines.push(positions[to as usize]);
            }
        }
    }
    Mesh::new(PrimitiveTopology::LineList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, lines)
}

fn solid_mesh(positions: &[[f32; 3]], faces: &[[u32; 3]]) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions.to_vec())
    .with_inserted_indices(bevy::mesh::Indices::U32(faces.concat()));
    mesh.compute_normals();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playground_models_are_discovered_beside_its_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path().join("project");
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(assets.join("models")).unwrap();
        std::fs::write(assets.join("models/actor.blend"), "fixture").unwrap();
        assert_eq!(asset_root(&project), assets);
        assert_eq!(find_assets(&asset_root(&project)), ["models/actor.blend"]);
        assert_eq!(asset_root(dir.path()), dir.path());
        assert!(
            find_project_assets(&project).contains(&"engine://models/blood_knight.blend".into())
        );
        assert_eq!(
            asset_source(&project, "engine://models/blood_knight.blend"),
            struction_scene::engine_assets().join("models/blood_knight.blend")
        );
    }

    #[test]
    fn finds_mesh_sources_but_not_hidden_or_build_folders() {
        let dir = tempfile::tempdir().unwrap();
        for file in [
            "models/ogre.blend",
            "models/ball.GLB",
            "props/cup.gltf",
            "notes.txt",
            ".cache/x.glb",
            "target/y.glb",
            "models/ogre.blend.recipe.json",
        ] {
            let path = dir.path().join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        assert_eq!(
            find_assets(dir.path()),
            ["models/ball.GLB", "models/ogre.blend", "props/cup.gltf"]
        );
    }

    /// Closing from the tool's own panel must not despawn the egui context bevy_egui is still
    /// running (it panicked with "previously queried context").
    #[test]
    fn closing_from_the_tool_window_outlives_its_egui_pass() {
        let mut app = App::new();
        app.init_resource::<Toolbox>();
        let world = app.world_mut();
        let window = world.spawn_empty().id();
        let camera = world.spawn_empty().id();
        let ui_camera = world
            .spawn((
                bevy_egui::EguiContext::default(),
                EguiSchedule::new(ToolWindowPass),
            ))
            .id();
        world.resource_mut::<Toolbox>().tool = Some(MeshTool {
            asset: "models/a.glb".into(),
            source: "models/a.glb".into(),
            mode: Mode::Inspect,
            window,
            camera,
            ui_camera,
            state: ToolState::Failed(String::new()),
            themed: true,
            closing: None,
            pointer_over_ui: false,
            orbit: ToolOrbit::default(),
            framed: false,
            shown: None,
            view: None,
        });
        app.add_systems(ToolWindowPass, |mut toolbox: ResMut<Toolbox>| {
            finish_close(&mut toolbox, After::Close)
        });
        app.add_systems(Update, handle_requests);
        app.insert_non_send(Editor::default());
        bevy_egui::run_egui_context_pass_loop_system(app.world_mut()).unwrap();
        // Rendering would consume the pass's texture changes; egui refuses to drop them unread.
        let mut output = app
            .world_mut()
            .get_mut::<bevy_egui::EguiFullOutput>(ui_camera)
            .expect("the context survives its own pass");
        if let Some(output) = output.0.as_mut() {
            output.textures_delta.clear();
        }
        app.world_mut().run_schedule(Update);
        for entity in [window, camera, ui_camera] {
            assert!(app.world().get_entity(entity).is_err());
        }
    }

    #[test]
    fn wireframes_list_each_edge_once() {
        let positions = [[0.0; 3], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let mesh = wire_mesh(&positions, [[0, 1, 2], [0, 2, 3]].into_iter());
        // Five edges: the square's four sides and the shared diagonal.
        assert_eq!(mesh.count_vertices(), 10);
    }
}
