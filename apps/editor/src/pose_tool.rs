//! The mesh tool's pose mode. It edits a rigged definition's overrides of its rig's pose library
//! (`PoseTargets`): key poses joint by joint, with joints picked from a list or by clicking the
//! model, and the sequences that play them, previewed and played in the tool's view. Names,
//! descriptions and uses all come from the library data and the definition; saving is one
//! undoable `Set` edit, the operation the JSONL protocol exposes too.
use std::collections::BTreeMap;

use crate::{
    state::{Command, Editor},
    theme,
    tools::{Mode, Toolbox},
};
use bevy::prelude::*;
use bevy_egui::egui::{self, RichText, Ui};
use serde_json::Value;
use struction_anim::{
    base_pose::{BasePose, BasePoseSet, JointPose, PoseSequence, PoseTargets, SequenceKey, Tumble},
    moves,
    pose::Pose,
    rig::Rig,
    solve::SolverSettings,
};
use struction_assets::format::MeshBundle;
use struction_editor::{DefinitionInspection, EditRequest, Field};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    Poses,
    Sequences,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playback {
    pub playing: bool,
    /// Start one-shots over when they end.
    pub repeat: bool,
    /// Seconds into the sequence.
    pub time: f32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PoseDraft {
    pub definition: String,
    pub rig_name: String,
    pub tab: Tab,
    pub pose: String,
    pub sequence: String,
    pub joint: usize,
    pub targets: PoseTargets,
    saved: PoseTargets,
    /// The rig's library before this definition's overrides, as the running game has it.
    defaults: BasePoseSet,
    pub playback: Playback,
    new_name: String,
    new_event: String,
    error: Option<String>,
}

impl PoseDraft {
    pub fn dirty(&self) -> bool {
        self.targets != self.saved
    }

    pub fn rig(&self, editor: &Editor) -> Option<Rig> {
        editor
            .project
            .as_ref()?
            .preview()
            .resource::<struction_scene::Rigs>()
            .build(&self.rig_name)
    }

    /// The library with this draft's overrides, unchecked so a half-edited draft still shows.
    pub fn library(&self) -> BasePoseSet {
        let mut library = self.defaults.clone();
        library.poses.extend(self.targets.poses.clone());
        library.sequences.extend(self.targets.sequences.clone());
        library
    }

    fn load(&mut self, editor: &Editor) {
        let Some(project) = &editor.project else {
            return;
        };
        self.defaults = project
            .preview()
            .get_resource::<BasePoseSet>()
            .cloned()
            .unwrap_or_default();
        if let Ok(definition) = project.inspect_definition(&self.definition) {
            self.targets = stored_targets(&definition.components);
            self.saved = self.targets.clone();
        }
        let library = self.library();
        if !library.poses.contains_key(&self.pose) {
            self.pose = library.poses.keys().next().cloned().unwrap_or_default();
        }
        if !library.sequences.contains_key(&self.sequence) {
            self.sequence = library.sequences.keys().next().cloned().unwrap_or_default();
        }
        self.playback.time = 0.0;
        self.error = None;
    }

    /// Checks the overrides against the rig, as the runtime will.
    fn check(&self, editor: &Editor) -> Result<(), String> {
        let rig = self.rig(editor).ok_or("Unknown skeleton")?;
        self.targets
            .resolve(&self.defaults, &rig.skeleton)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    pub fn apply(&mut self, editor: &mut Editor) -> Result<(), String> {
        if !self.dirty() {
            return Ok(());
        }
        let current = editor
            .project
            .as_ref()
            .ok_or("No project")?
            .inspect_definition(&self.definition)
            .map_err(|e| e.to_string())?;
        if stored_targets(&current.components) != self.saved {
            return Err(
                "Pose targets changed outside this draft. Revert to reload them before applying."
                    .into(),
            );
        }
        self.check(editor)?;
        let request = EditRequest::Set {
            file: format!("{}/entity.jsonc", self.definition),
            path: vec![
                Field::Key("components".into()),
                Field::Key("PoseTargets".into()),
            ],
            value: serde_json::to_value(&self.targets).map_err(|e| e.to_string())?,
            label: format!("Pose targets for {}", self.definition),
            group: None,
            revision: None,
        };
        editor
            .project
            .as_mut()
            .ok_or("No project")?
            .edit(request)
            .map_err(|e| e.to_string())?;
        editor.apply(Command::Refresh);
        self.saved = self.targets.clone();
        Ok(())
    }

    /// The selected sequence's phase at the playback time.
    pub fn phase(&self) -> f32 {
        self.library()
            .sequences
            .get(&self.sequence)
            .map_or(0.0, |sequence| sequence.phase_at(self.playback.time))
    }

    /// Moves playback on by `dt`, stopping at the end of a one-shot unless it repeats.
    pub fn advance(&mut self, dt: f32) {
        if !self.playback.playing || self.tab != Tab::Sequences {
            return;
        }
        let Some(sequence) = self.library().sequences.get(&self.sequence).cloned() else {
            return;
        };
        self.playback.time += dt;
        if !sequence.looping && self.playback.time >= sequence.seconds {
            if self.playback.repeat {
                self.playback.time = self.playback.time.rem_euclid(sequence.seconds);
            } else {
                self.playback.time = sequence.seconds;
                self.playback.playing = false;
            }
        }
    }

    /// What the view shows: the selected pose, or the selected sequence at its playback phase,
    /// both over the rig's standing base.
    pub fn preview_pose(&self, rig: &Rig) -> Pose {
        let library = self.library();
        let mut pose = rig.skeleton.rest_pose();
        let base = SolverSettings::default().idle_pose;
        if let Ok(idle) = library.resolve(&base, &rig.skeleton) {
            idle.apply(&mut pose, 1.0, None);
        }
        match self.tab {
            Tab::Poses => {
                if let Ok(target) = library.resolve(&self.pose, &rig.skeleton) {
                    target.apply(&mut pose, 1.0, None);
                }
            }
            Tab::Sequences => {
                if let Some(sequence) = library.sequences.get(&self.sequence) {
                    let phase = sequence.phase_at(self.playback.time);
                    // Invalid keys only stop the preview; the sidebar reports why.
                    let _ = moves::play(
                        sequence,
                        phase,
                        sequence.envelope(phase),
                        Vec3::NEG_Z,
                        rig,
                        &library,
                        Transform::IDENTITY,
                        Vec3::Y,
                        &mut pose,
                    );
                }
            }
        }
        pose
    }

    /// Model-space transforms of every joint in the preview.
    pub fn transforms(&self, rig: &Rig) -> Vec<Transform> {
        rig.skeleton.model_transforms(&self.preview_pose(rig))
    }

    /// The selected pose, copied into this definition's overrides on the first change.
    fn pose_mut(&mut self) -> &mut BasePose {
        let inherited = self.library().poses.get(&self.pose).cloned();
        self.targets
            .poses
            .entry(self.pose.clone())
            .or_insert_with(|| inherited.unwrap_or_default())
    }

    fn sequence_mut(&mut self) -> &mut PoseSequence {
        let inherited = self.library().sequences.get(&self.sequence).cloned();
        self.targets
            .sequences
            .entry(self.sequence.clone())
            .or_insert_with(|| inherited.unwrap_or_default())
    }
}

fn stored_targets(components: &BTreeMap<String, Value>) -> PoseTargets {
    components
        .iter()
        .find(|(key, _)| key.ends_with("::PoseTargets"))
        .and_then(|(_, value)| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

/// Sequences that reach `pose`, with the key that does.
pub fn pose_users(library: &BasePoseSet, pose: &str) -> Vec<String> {
    library
        .sequences
        .iter()
        .flat_map(|(name, sequence)| {
            sequence
                .keys
                .iter()
                .enumerate()
                .filter(|(_, key)| key.pose == pose)
                .map(move |(index, key)| format!("{name}, key {} at {:.2}", index + 1, key.at))
        })
        .collect()
}

/// Where a definition names `sequence` in a `sequence` field: a component such as a move's
/// tuning (`Attack.sequence`, defaults included), or a state rule (`While Walking (PlaySequence)`).
pub fn sequence_players(definition: &DefinitionInspection, sequence: &str) -> Vec<String> {
    fn walk(value: &Value, path: &mut Vec<String>, sequence: &str, found: &mut Vec<String>) {
        let Value::Object(members) = value else {
            return;
        };
        for (key, value) in members {
            if key == "sequence" && value.as_str() == Some(sequence) {
                found.push(match path.as_slice() {
                    [states, state, _, component] if states == "states" => {
                        format!("While {state} ({component})")
                    }
                    _ => format!("{}.sequence", path.join(".")),
                });
            }
            path.push(key.clone());
            walk(value, path, sequence, found);
            path.pop();
        }
    }
    let mut found = Vec::new();
    for (type_path, value) in &definition.components {
        let short = type_path.rsplit("::").next().unwrap_or(type_path);
        walk(value, &mut vec![short.to_owned()], sequence, &mut found);
    }
    if let Some(states) = definition.resolved.get("states") {
        walk(states, &mut vec!["states".into()], sequence, &mut found);
    }
    found
}

/// Which preview part shows which joint: the model's nodes are named after joints.
pub fn parts(bundle: &MeshBundle, rig: &Rig) -> Vec<(usize, usize)> {
    bundle
        .nodes
        .iter()
        .flat_map(|node| {
            let joint = rig
                .skeleton
                .joint_id(node.name.split('.').next().unwrap_or(&node.name))
                .ok();
            node.meshes
                .iter()
                .filter_map(move |&mesh| joint.map(|joint| (mesh as usize, joint)))
        })
        .collect()
}

/// The joint under `ray`: the nearest model part it hits, or else the nearest joint within
/// `reach` of it.
pub fn pick(
    ray: Ray3d,
    bundle: &MeshBundle,
    parts: &[(usize, usize)],
    transforms: &[Transform],
    reach: f32,
) -> Option<usize> {
    let mut nearest: Option<(f32, usize)> = None;
    for &(mesh, joint) in parts {
        let Some(lod) = bundle.meshes[mesh].lods.first() else {
            continue;
        };
        let to_world = transforms[joint];
        for triangle in lod.indices.as_chunks::<3>().0 {
            let [a, b, c] =
                triangle.map(|i| to_world.transform_point(lod.positions[i as usize].into()));
            if let Some(distance) = ray_triangle(ray, a, b, c)
                && nearest.is_none_or(|(best, _)| distance < best)
            {
                nearest = Some((distance, joint));
            }
        }
    }
    if let Some((_, joint)) = nearest {
        return Some(joint);
    }
    transforms
        .iter()
        .enumerate()
        .map(|(joint, transform)| {
            let offset = transform.translation - ray.origin;
            let along = offset.dot(*ray.direction).max(0.0);
            (offset.distance(*ray.direction * along), joint)
        })
        .filter(|(distance, _)| *distance <= reach)
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, joint)| joint)
}

/// Möller–Trumbore: distance along `ray` to the triangle, if it is hit from either side.
fn ray_triangle(ray: Ray3d, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let (ab, ac) = (b - a, c - a);
    let p = ray.direction.cross(ac);
    let determinant = ab.dot(p);
    if determinant.abs() < 1e-9 {
        return None;
    }
    let inverse = 1.0 / determinant;
    let t = ray.origin - a;
    let u = t.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = t.cross(ab);
    let v = ray.direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let distance = ac.dot(q) * inverse;
    (distance > 0.0).then_some(distance)
}

/// A preview part rigidly following a joint.
#[derive(Component)]
pub struct PosedPart {
    pub joint: usize,
}

/// The skeleton overlay of the pose view, drawn over the model in the tool window only.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct PoseGizmos;

pub fn configure_gizmos(mut store: ResMut<GizmoConfigStore>) {
    let (config, _) = store.config_mut::<PoseGizmos>();
    config.render_layers = bevy::camera::visibility::RenderLayers::layer(crate::tools::TOOL_LAYER);
    // Drawn over the model's surfaces.
    config.depth_bias = -1.0;
}

const HIGHLIGHT: LinearRgba = LinearRgba::rgb(0.9, 0.45, 0.05);

/// Plays the preview, poses its parts, highlights the selected joint and draws the skeleton.
pub fn animate_preview(
    time: Res<Time>,
    mut toolbox: ResMut<Toolbox>,
    editor: NonSend<Editor>,
    mut parts: Query<(
        Ref<PosedPart>,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmos: Gizmos<PoseGizmos>,
    mut highlighted: Local<Option<usize>>,
) {
    let Some(tool) = &mut toolbox.tool else {
        return;
    };
    if tool.mode != Mode::Poses {
        return;
    }
    let Some(ready) = tool.ready_mut() else {
        return;
    };
    let draft = &mut ready.pose;
    draft.advance(time.delta_secs());
    let Some(rig) = draft.rig(&editor) else {
        return;
    };
    let transforms = draft.transforms(&rig);
    let selected = draft.joint;
    let restyle = *highlighted != Some(selected) || parts.iter().any(|(part, ..)| part.is_added());
    *highlighted = Some(selected);
    for (part, mut transform, material) in &mut parts {
        if let Some(&posed) = transforms.get(part.joint) {
            transform.set_if_neq(posed);
        }
        if restyle && let Some(mut material) = materials.get_mut(&material.0) {
            material.emissive = if part.joint == selected {
                HIGHLIGHT
            } else {
                LinearRgba::BLACK
            };
        }
    }
    let muted = Color::srgba(0.75, 0.78, 0.85, 0.7);
    let accent = Color::srgb(1.0, 0.66, 0.23);
    for (index, joint) in rig.skeleton.joints().iter().enumerate() {
        let at = transforms[index].translation;
        if let Some(parent) = joint.parent {
            let color = if index == selected || parent == selected {
                accent
            } else {
                muted
            };
            gizmos.line(transforms[parent].translation, at, color);
        }
        let (radius, color) = if index == selected {
            (0.035, accent)
        } else {
            (0.012, muted)
        };
        gizmos.sphere(Isometry3d::from_translation(at), radius, color);
    }
}

/// A click (not a drag) on the model in the pose view selects the joint under the cursor.
pub fn pick_joint(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut toolbox: ResMut<Toolbox>,
    editor: NonSend<Editor>,
    mut pressed: Local<Option<Vec2>>,
) {
    let Some(tool) = &mut toolbox.tool else {
        return;
    };
    let (camera, window) = tool.view_entities();
    let cursor = windows.get(window).ok().and_then(Window::cursor_position);
    let over = cursor.is_some_and(|cursor| tool.view.is_some_and(|view| view.contains(cursor)))
        && !tool.pointer_over_ui;
    if buttons.just_pressed(MouseButton::Left) {
        *pressed = cursor.filter(|_| over);
    }
    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let (Some(start), Some(cursor)) = (pressed.take(), cursor) else {
        return;
    };
    if tool.mode != Mode::Poses || !over || start.distance(cursor) > 4.0 {
        return;
    }
    let Ok((camera, camera_transform)) = cameras.get(camera) else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return;
    };
    let Some(ready) = tool.ready_mut() else {
        return;
    };
    let (Some(rig), Some(preview)) = (ready.pose.rig(&editor), &ready.preview) else {
        return;
    };
    let transforms = ready.pose.transforms(&rig);
    let parts = parts(&preview.bundle, &rig);
    let reach = 0.02 * camera_transform.translation().length().max(1.0);
    if let Some(joint) = pick(ray, &preview.bundle, &parts, &transforms, reach) {
        ready.pose.joint = joint;
    }
}

pub fn panel(ui: &mut Ui, draft: &mut PoseDraft, editor: &mut Editor, asset: &str) {
    let Some(project) = &editor.project else {
        return;
    };
    let choices: Vec<_> = project
        .definitions()
        .into_iter()
        .filter_map(|name| {
            let definition = project.inspect_definition(&name).ok()?;
            let shape = definition
                .components
                .iter()
                .find(|(k, _)| k.ends_with("::Shape"))?
                .1
                .get("Rigged")?;
            (shape.get("model")?.as_str()? == asset).then(|| {
                let rig = shape
                    .get("rig")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                (name, rig.to_owned())
            })
        })
        .collect();
    let Some(first) = choices.first() else {
        ui.label("No rigged definition uses this model. Assign it to a rigged actor before editing its poses.");
        return;
    };
    if draft.definition.is_empty() {
        draft.definition = first.0.clone();
        draft.rig_name = first.1.clone();
        draft.load(editor);
    }
    let before = draft.definition.clone();
    ui.add_enabled_ui(!draft.dirty(), |ui| {
        egui::ComboBox::from_label("Actor definition")
            .selected_text(&draft.definition)
            .show_ui(ui, |ui| {
                for (name, rig) in &choices {
                    if ui
                        .selectable_value(&mut draft.definition, name.clone(), name)
                        .clicked()
                    {
                        draft.rig_name = rig.clone();
                    }
                }
            });
    });
    if before != draft.definition {
        draft.joint = 0;
        draft.load(editor);
    }
    muted(
        ui,
        "Saved on this definition; inherited by its descendants.",
    );
    let Some(rig) = draft.rig(editor) else {
        ui.colored_label(theme::AXES[0], format!("Unknown rig {:?}", draft.rig_name));
        return;
    };
    ui.horizontal(|ui| {
        ui.selectable_value(&mut draft.tab, Tab::Poses, "Key poses");
        ui.selectable_value(&mut draft.tab, Tab::Sequences, "Sequences");
    });
    ui.separator();
    match draft.tab {
        Tab::Poses => poses_tab(ui, draft, &rig),
        Tab::Sequences => {
            let definition = editor
                .project
                .as_ref()
                .and_then(|project| project.inspect_definition(&draft.definition).ok());
            sequences_tab(ui, draft, definition.as_ref());
        }
    }
    ui.separator();
    let checked = draft.check(editor);
    if let Err(error) = &checked {
        ui.colored_label(theme::AXES[0], error);
    }
    if let Some(error) = &draft.error {
        ui.colored_label(theme::AXES[0], error);
    }
    ui.add_enabled_ui(!editor.playing(), |ui| {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    draft.dirty() && checked.is_ok(),
                    egui::Button::new("Apply pose targets"),
                )
                .clicked()
            {
                draft.error = draft.apply(editor).err();
            }
            if ui
                .add_enabled(draft.dirty(), egui::Button::new("Revert"))
                .clicked()
            {
                draft.load(editor);
            }
        });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!draft.dirty(), egui::Button::new("Undo project edit"))
                .clicked()
            {
                editor.apply(Command::Undo);
                draft.load(editor);
            }
            if ui
                .add_enabled(!draft.dirty(), egui::Button::new("Redo project edit"))
                .clicked()
            {
                editor.apply(Command::Redo);
                draft.load(editor);
            }
        });
    });
}

fn muted(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).small().color(theme::MUTED));
}

/// A combo box over `names`, plus a name field and button that add a copy of the selection.
fn choose_or_add(
    ui: &mut Ui,
    label: &str,
    names: &[String],
    selected: &mut String,
    new_name: &mut String,
) -> Option<String> {
    egui::ComboBox::from_label(label)
        .selected_text(selected.as_str())
        .show_ui(ui, |ui| {
            for name in names {
                ui.selectable_value(selected, name.clone(), name);
            }
        });
    let mut added = None;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(new_name)
                .hint_text("New name")
                .desired_width(140.0),
        );
        let name = new_name.trim().to_owned();
        let valid = !name.is_empty() && !names.contains(&name);
        if ui
            .add_enabled(valid, egui::Button::new("Add as copy"))
            .on_hover_text(format!("A new {label} starting from the selected one"))
            .clicked()
        {
            added = Some(name);
            new_name.clear();
        }
    });
    added
}

fn poses_tab(ui: &mut Ui, draft: &mut PoseDraft, rig: &Rig) {
    let library = draft.library();
    let names: Vec<String> = library.poses.keys().cloned().collect();
    if let Some(name) = choose_or_add(ui, "Key pose", &names, &mut draft.pose, &mut draft.new_name)
    {
        let copy = library.poses.get(&draft.pose).cloned().unwrap_or_default();
        draft.targets.poses.insert(name.clone(), copy);
        draft.pose = name;
    }
    let Some(pose) = library.poses.get(&draft.pose) else {
        return;
    };
    if !pose.doc.is_empty() {
        ui.label(&pose.doc);
    }
    let users = pose_users(&library, &draft.pose);
    if users.is_empty() {
        muted(
            ui,
            "No sequence plays it; constraints and solvers may still name it.",
        );
    } else {
        ui.colored_label(theme::WARD, format!("Played by {}", users.join(" · ")));
    }
    if draft.targets.poses.contains_key(&draft.pose) {
        ui.horizontal(|ui| {
            ui.colored_label(theme::ACCENT, "Overridden on this definition");
            let inherited = draft.defaults.poses.contains_key(&draft.pose);
            let label = if inherited {
                "Restore library pose"
            } else {
                "Remove pose"
            };
            if ui.small_button(label).clicked() {
                draft.targets.poses.remove(&draft.pose);
            }
        });
    }
    ui.add_space(6.0);
    let joints = rig.skeleton.joints();
    draft.joint = draft.joint.min(joints.len().saturating_sub(1));
    egui::ComboBox::from_label("Joint")
        .selected_text(&joints[draft.joint].name)
        .show_ui(ui, |ui| {
            for (index, joint) in joints.iter().enumerate() {
                let authored = pose.joints.iter().any(|j| j.joint == joint.name);
                let text = if authored {
                    RichText::new(&joint.name).color(theme::ACCENT)
                } else {
                    RichText::new(&joint.name)
                };
                ui.selectable_value(&mut draft.joint, index, text);
            }
        });
    muted(
        ui,
        "Click the model to pick a joint. Joints the pose sets are highlighted in the list.",
    );
    let name = joints[draft.joint].name.clone();
    let existing = pose.joints.iter().find(|j| j.joint == name);
    let (x, y, z) = joints[draft.joint].rest.rotation.to_euler(EulerRot::XYZ);
    let mut angles = existing.and_then(|j| j.euler_deg).unwrap_or(Vec3::new(
        x.to_degrees(),
        y.to_degrees(),
        z.to_degrees(),
    ));
    let before = angles;
    for axis in 0..3 {
        ui.horizontal(|ui| {
            ui.colored_label(theme::AXES[axis], ["X", "Y", "Z"][axis]);
            ui.add(
                egui::DragValue::new(&mut angles[axis])
                    .speed(0.5)
                    .suffix("°")
                    .range(-180.0..=180.0),
            );
        });
    }
    if angles != before {
        let target = draft.pose_mut();
        if let Some(joint) = target.joints.iter_mut().find(|j| j.joint == name) {
            joint.euler_deg = Some(angles);
        } else {
            target.joints.push(JointPose::rotation(&name, angles));
        }
    }
    if existing.is_some() && ui.button("Leave this joint to the body").clicked() {
        draft.pose_mut().joints.retain(|j| j.joint != name);
    }
}

fn sequences_tab(ui: &mut Ui, draft: &mut PoseDraft, definition: Option<&DefinitionInspection>) {
    let library = draft.library();
    let names: Vec<String> = library.sequences.keys().cloned().collect();
    let before = draft.sequence.clone();
    if let Some(name) = choose_or_add(
        ui,
        "Sequence",
        &names,
        &mut draft.sequence,
        &mut draft.new_name,
    ) {
        let copy = library
            .sequences
            .get(&draft.sequence)
            .cloned()
            .unwrap_or_else(|| PoseSequence {
                keys: vec![SequenceKey::new(draft.pose.clone(), 0.0)],
                ..default()
            });
        draft.targets.sequences.insert(name.clone(), copy);
        draft.sequence = name;
    }
    if before != draft.sequence {
        draft.playback.time = 0.0;
    }
    let Some(sequence) = draft.library().sequences.get(&draft.sequence).cloned() else {
        muted(
            ui,
            "The library has no sequences yet: add one by name above.",
        );
        return;
    };
    if !sequence.doc.is_empty() {
        ui.label(&sequence.doc);
    }
    let players = definition
        .map(|definition| sequence_players(definition, &draft.sequence))
        .unwrap_or_default();
    if players.is_empty() {
        muted(
            ui,
            "Nothing on this definition plays it. Name it in a move's `sequence`, or enable a `PlaySequence` from a state.",
        );
    } else {
        ui.colored_label(theme::WARD, format!("Played by {}", players.join(" · ")));
    }
    if draft.targets.sequences.contains_key(&draft.sequence) {
        ui.horizontal(|ui| {
            ui.colored_label(theme::ACCENT, "Overridden on this definition");
            let label = if draft.defaults.sequences.contains_key(&draft.sequence) {
                "Restore library sequence"
            } else {
                "Remove sequence"
            };
            if ui.small_button(label).clicked() {
                draft.targets.sequences.remove(&draft.sequence);
            }
        });
    }

    ui.add_space(6.0);
    ui.label(theme::section("Playback"));
    ui.horizontal(|ui| {
        let label = if draft.playback.playing {
            "⏸ Pause"
        } else {
            "▶ Play"
        };
        if ui.button(label).clicked() {
            if !draft.playback.playing
                && !sequence.looping
                && draft.playback.time >= sequence.seconds
            {
                draft.playback.time = 0.0;
            }
            draft.playback.playing = !draft.playback.playing;
        }
        if !sequence.looping {
            ui.checkbox(&mut draft.playback.repeat, "Repeat");
        }
    });
    let mut phase = draft.phase();
    if ui
        .add(egui::Slider::new(&mut phase, 0.0..=1.0).text("phase"))
        .changed()
    {
        draft.playback.playing = false;
        draft.playback.time = phase * sequence.seconds;
    }
    if draft.playback.playing {
        ui.ctx().request_repaint();
    }

    ui.add_space(6.0);
    ui.label(theme::section("Keys"));
    let poses: Vec<String> = library.poses.keys().cloned().collect();
    let mut edited = sequence.clone();
    let count = edited.keys.len();
    let mut remove = None;
    let mut swap = None;
    for (index, key) in edited.keys.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}", index + 1));
            egui::ComboBox::from_id_salt(("key_pose", index))
                .selected_text(&key.pose)
                .width(130.0)
                .show_ui(ui, |ui| {
                    for name in &poses {
                        ui.selectable_value(&mut key.pose, name.clone(), name);
                    }
                });
            ui.add(
                egui::DragValue::new(&mut key.at)
                    .speed(0.005)
                    .range(0.0..=1.0)
                    .prefix("at "),
            );
            if ui
                .add_enabled(index > 0, egui::Button::new("↑").small())
                .clicked()
            {
                swap = Some(index - 1);
            }
            if ui
                .add_enabled(index + 1 < count, egui::Button::new("↓").small())
                .clicked()
            {
                swap = Some(index);
            }
            if ui
                .add_enabled(count > 1, egui::Button::new("×").small())
                .clicked()
            {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = swap {
        // Reordering swaps the poses; the times stay in order.
        let pose = edited.keys[index].pose.clone();
        edited.keys[index].pose = std::mem::replace(&mut edited.keys[index + 1].pose, pose);
    }
    if let Some(index) = remove {
        edited.keys.remove(index);
    }
    if ui
        .button("Add key")
        .on_hover_text("The selected key pose, after the last key")
        .clicked()
    {
        let at = edited
            .keys
            .last()
            .map_or(0.0, |key| ((key.at + 1.0) / 2.0).min(1.0));
        edited.keys.push(SequenceKey::new(draft.pose.clone(), at));
    }

    ui.add_space(6.0);
    ui.label(theme::section("Timing and blending"));
    ui.checkbox(&mut edited.looping, "Loop").on_hover_text(
        "Repeat while the state playing it holds; one-shots last their move's duration",
    );
    ui.horizontal(|ui| {
        ui.add(
            egui::DragValue::new(&mut edited.seconds)
                .speed(0.01)
                .range(0.05..=30.0)
                .suffix(" s"),
        );
        ui.label(if edited.looping {
            "per cycle"
        } else {
            "preview length"
        });
    });
    ui.add(egui::Slider::new(&mut edited.takeover, 0.0..=1.0).text("takeover"))
        .on_hover_text("1 replaces walking, feet and constraints; 0 leaves the legs walking");
    if !edited.looping {
        ui.add(egui::Slider::new(&mut edited.fade_in, 0.0..=1.0).text("fade in"));
        ui.add(egui::Slider::new(&mut edited.fade_out, 0.0..=1.0).text("fade out"));
    }

    ui.add_space(6.0);
    ui.label(theme::section("Events"));
    let mut dropped = None;
    for (name, at) in edited.events.iter_mut() {
        ui.horizontal(|ui| {
            ui.label(name);
            ui.add(
                egui::DragValue::new(at)
                    .speed(0.005)
                    .range(0.0..=1.0)
                    .prefix("at "),
            );
            if ui.small_button("×").clicked() {
                dropped = Some(name.clone());
            }
        });
    }
    if let Some(name) = dropped {
        edited.events.remove(&name);
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.new_event)
                .hint_text("strike")
                .desired_width(100.0),
        );
        let name = draft.new_event.trim().to_owned();
        if ui
            .add_enabled(
                !name.is_empty() && !edited.events.contains_key(&name),
                egui::Button::new("Add event"),
            )
            .on_hover_text("Simulation acts on named events, such as `strike` for a hit")
            .clicked()
        {
            edited.events.insert(name, phase);
            draft.new_event.clear();
        }
    });

    let mut tumbles = edited.tumble.is_some();
    if ui.checkbox(&mut tumbles, "Tumble the pelvis").changed() {
        edited.tumble = tumbles.then(Tumble::default);
    }
    if let Some(tumble) = &mut edited.tumble {
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut tumble.from)
                    .speed(0.005)
                    .range(0.0..=1.0)
                    .prefix("from "),
            );
            ui.add(
                egui::DragValue::new(&mut tumble.to)
                    .speed(0.005)
                    .range(0.0..=1.0)
                    .prefix("to "),
            );
            ui.add(
                egui::DragValue::new(&mut tumble.sink)
                    .speed(0.01)
                    .range(0.0..=2.0)
                    .prefix("sink "),
            );
        });
    }
    if edited != sequence {
        *draft.sequence_mut() = edited;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use struction_assets::format::{
        Bounds, BundleMesh, CollisionShapes, MeshLod, SceneNode, TriMesh,
    };

    fn node(name: &str, mesh: u32) -> SceneNode {
        SceneNode {
            name: name.into(),
            parent: None,
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0; 3],
            meshes: vec![mesh],
        }
    }

    fn open_playground() -> (tempfile::TempDir, Editor) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scenes")).unwrap();
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../playground/project/scenes/milestone1.jsonc"),
            dir.path().join("scenes/milestone1.jsonc"),
        )
        .unwrap();
        let mut editor = Editor::default();
        editor.apply(Command::Open(dir.path().into()));
        (dir, editor)
    }

    fn player(editor: &Editor) -> PoseDraft {
        let mut draft = PoseDraft {
            definition: "characters/player".into(),
            rig_name: "humanoid".into(),
            pose: "roll".into(),
            ..default()
        };
        draft.load(editor);
        draft
    }

    #[test]
    fn targets_save_as_project_overrides_and_drive_the_play_rig() {
        let (dir, mut editor) = open_playground();
        let mut draft = player(&editor);
        draft.targets.poses.insert(
            "roll".into(),
            BasePose {
                joints: vec![JointPose::rotation("head", Vec3::new(25.0, 0.0, 0.0))],
                ..default()
            },
        );
        draft.apply(&mut editor).unwrap();
        let file = dir.path().join("characters/player/entity.jsonc");
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("PoseTargets")
        );
        editor.apply(Command::StartPlay);
        editor.project.as_mut().unwrap().step_play(2).unwrap();
        let world = editor.project.as_ref().unwrap().play_world().unwrap();
        assert!(world.iter_entities().any(|e| {
            e.get::<struction_anim::base_pose::RigPoseSet>()
                .is_some_and(|set| {
                    set.0.poses["roll"].joints.iter().any(|j| {
                        j.joint == "head" && j.euler_deg == Some(Vec3::new(25.0, 0.0, 0.0))
                    })
                })
        }));
        editor.apply(Command::StopPlay);
        editor.apply(Command::Undo);
        draft.load(&editor);
        assert!(draft.targets.poses.is_empty());
    }

    /// The tool lists what the library and definition say, and a new looping sequence it saves
    /// plays on the rig of an actor whose state enables it.
    #[test]
    fn sequences_come_from_data_and_new_ones_save_and_play() {
        let (dir, mut editor) = open_playground();
        let mut draft = player(&editor);
        let library = draft.library();
        assert!(library.sequences.contains_key("swing"));
        assert!(
            pose_users(&library, "swing_raise")
                .iter()
                .any(|use_| use_.starts_with("swing"))
        );
        let definition = editor
            .project
            .as_ref()
            .unwrap()
            .inspect_definition("characters/player")
            .unwrap();
        assert_eq!(sequence_players(&definition, "swing"), ["Attack.sequence"]);
        assert_eq!(sequence_players(&definition, "roll"), ["Roll.sequence"]);

        draft.targets.sequences.insert(
            "march".into(),
            PoseSequence {
                keys: vec![
                    SequenceKey::new("arm_swing_left", 0.0),
                    SequenceKey::new("aim", 1.0 / 3.0),
                    SequenceKey::new("arm_swing_right", 2.0 / 3.0),
                ],
                looping: true,
                seconds: 0.9,
                ..default()
            },
        );
        draft.apply(&mut editor).unwrap();
        let text =
            std::fs::read_to_string(dir.path().join("characters/player/entity.jsonc")).unwrap();
        assert!(text.contains("march"), "{text}");
        draft.load(&editor);
        assert_eq!(draft.targets.sequences["march"].keys.len(), 3);

        // Playback follows the keys in order and wraps.
        draft.tab = Tab::Sequences;
        draft.sequence = "march".into();
        draft.playback.playing = true;
        draft.advance(0.45);
        assert!((draft.phase() - 0.5).abs() < 1e-4);
        draft.advance(0.6);
        assert!(draft.phase() < 0.2);

        // A state rule plays it while walking; the data layer accepts the rule.
        editor
            .project
            .as_mut()
            .unwrap()
            .edit(EditRequest::Set {
                file: "characters/player/entity.jsonc".into(),
                path: ["states", "Walking", "enable", "PlaySequence"]
                    .map(|key| Field::Key(key.into()))
                    .into(),
                value: serde_json::json!({ "sequence": "march" }),
                label: "Walk with the march".into(),
                group: None,
                revision: None,
            })
            .unwrap();
        let definition = editor
            .project
            .as_ref()
            .unwrap()
            .inspect_definition("characters/player")
            .unwrap();
        assert_eq!(
            sequence_players(&definition, "march"),
            ["While Walking (PlaySequence)"]
        );

        let invalid = draft.targets.sequences.get_mut("march").unwrap();
        invalid.keys[1].pose = "nope".into();
        let error = draft.check(&editor).unwrap_err();
        assert!(error.contains("march") && error.contains("nope"), "{error}");
    }

    #[test]
    fn clicking_picks_the_part_under_the_cursor_or_the_nearest_joint() {
        let rig = struction_anim::humanoid::rig();
        let draft = PoseDraft::default();
        let transforms = draft.transforms(&rig);
        // A unit quad around each of two joints' origins, facing +Z.
        let quad = MeshLod {
            positions: vec![
                [-0.05, -0.05, 0.0],
                [0.05, -0.05, 0.0],
                [0.05, 0.05, 0.0],
                [-0.05, 0.05, 0.0],
            ],
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 1, 2, 0, 2, 3],
            error: 0.0,
        };
        let mesh = BundleMesh {
            name: "part".into(),
            material: None,
            bounds: Bounds {
                min: [-0.05, -0.05, 0.0],
                max: [0.05, 0.05, 0.0],
            },
            lods: vec![quad],
            collision: CollisionShapes {
                hull: None,
                trimesh: TriMesh {
                    vertices: Vec::new(),
                    triangles: Vec::new(),
                },
                parts: Vec::new(),
            },
        };
        let head = rig.skeleton.joint_id("head").unwrap();
        let hand = rig.skeleton.joint_id("hand_r").unwrap();
        let bundle = MeshBundle {
            meshes: vec![mesh.clone(), mesh],
            nodes: vec![node("head", 0), node("hand_r.001", 1)],
            materials: Vec::new(),
        };
        let parts = parts(&bundle, &rig);
        assert_eq!(parts, [(0, head), (1, hand)]);
        let toward = |joint: usize, offset: Vec3| Ray3d {
            origin: transforms[joint].translation + offset + Vec3::Z * 3.0,
            direction: Dir3::NEG_Z,
        };
        assert_eq!(
            pick(toward(head, Vec3::ZERO), &bundle, &parts, &transforms, 0.05),
            Some(head)
        );
        assert_eq!(
            pick(toward(hand, Vec3::ZERO), &bundle, &parts, &transforms, 0.05),
            Some(hand)
        );
        // Off every part: the nearest joint within reach, else nothing.
        let chest = rig.skeleton.joint_id("chest").unwrap();
        assert_eq!(
            pick(
                toward(chest, Vec3::X * 0.01),
                &bundle,
                &parts,
                &transforms,
                0.05
            ),
            Some(chest)
        );
        assert_eq!(
            pick(
                toward(chest, Vec3::X * 0.5),
                &bundle,
                &parts,
                &transforms,
                0.05
            ),
            None
        );
    }
}
