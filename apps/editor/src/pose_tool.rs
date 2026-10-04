//! The mesh tool's pose workspace. It edits a rigged definition's overrides of its rig's pose
//! library (`PoseTargets`) in one place: the sequence on a timeline under the view, the key pose
//! selected on it, and that pose's joints, picked by clicking the model and turned with rings in
//! the view or by angle. Names, descriptions and uses come from the library and the definition;
//! saving is one undoable `Set` edit, the operation the JSONL protocol exposes too.
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

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Playback {
    pub playing: bool,
    /// Start one-shots over when they end.
    pub repeat: bool,
    /// Seconds into the sequence.
    pub time: f32,
}

/// Something on the definition that plays a sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub label: String,
    /// The component whose `duration` stretches a one-shot over a move, such as `Attack`.
    pub component: Option<String>,
    pub duration: Option<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PoseDraft {
    pub definition: String,
    pub rig_name: String,
    /// The pose the joint editor edits.
    pub pose: String,
    /// The sequence on the timeline; empty for none.
    pub sequence: String,
    /// The sequence key last selected, whose pose is being edited.
    pub key: Option<usize>,
    /// Show the pose alone instead of the sequence at the playhead.
    pub solo: bool,
    /// Draw the keys around the playhead as faint skeletons.
    pub ghosts: bool,
    pub joint: usize,
    /// The rotation ring under the cursor or being dragged (local X, Y or Z).
    pub ring: Option<usize>,
    pub targets: PoseTargets,
    saved: PoseTargets,
    /// The rig's library before this definition's overrides, as the running game has it.
    defaults: BasePoseSet,
    pub playback: Playback,
    /// What plays the sequence on this definition, refreshed by the panel.
    pub players: Vec<Player>,
    /// Labels of project edits this tool made, for the tool's undo order.
    pub applied: Vec<String>,
    /// A move duration being dragged or typed, applied when let go.
    duration_edit: Option<(String, f32)>,
    new_pose: String,
    new_sequence: String,
    insert_pose: String,
    new_event: String,
    error: Option<String>,
}

impl Default for PoseDraft {
    fn default() -> Self {
        Self {
            definition: String::new(),
            rig_name: String::new(),
            pose: String::new(),
            sequence: String::new(),
            key: None,
            solo: false,
            ghosts: true,
            joint: 0,
            ring: None,
            targets: default(),
            saved: default(),
            defaults: default(),
            playback: default(),
            players: Vec::new(),
            applied: Vec::new(),
            duration_edit: None,
            new_pose: String::new(),
            new_sequence: String::new(),
            insert_pose: String::new(),
            new_event: String::new(),
            error: None,
        }
    }
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

    pub fn load(&mut self, editor: &Editor) {
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
            let base = SolverSettings::default().idle_pose;
            self.pose = if library.poses.contains_key(&base) {
                base
            } else {
                library.poses.keys().next().cloned().unwrap_or_default()
            };
        }
        if !self.sequence.is_empty() && !library.sequences.contains_key(&self.sequence) {
            self.sequence.clear();
        }
        self.key = None;
        self.playback.time = 0.0;
        self.error = None;
    }

    /// Checks the overrides against the rig, as the runtime will.
    pub fn check(&self, editor: &Editor) -> Result<(), String> {
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
        let label = format!("Pose targets for {}", self.definition);
        let value = serde_json::to_value(&self.targets).map_err(|e| e.to_string())?;
        self.edit(editor, &["PoseTargets"], value, &label)?;
        self.saved = self.targets.clone();
        Ok(())
    }

    /// One undoable project edit of a component field on the definition.
    fn edit(
        &mut self,
        editor: &mut Editor,
        path: &[&str],
        value: Value,
        label: &str,
    ) -> Result<(), String> {
        let request = EditRequest::Set {
            file: format!("{}/entity.jsonc", self.definition),
            path: std::iter::once("components")
                .chain(path.iter().copied())
                .map(|key| Field::Key(key.into()))
                .collect(),
            value,
            label: label.into(),
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
        self.applied.push(label.into());
        Ok(())
    }

    /// Retimes the move that plays the sequence: its component's `duration`.
    pub fn set_duration(
        &mut self,
        editor: &mut Editor,
        component: &str,
        seconds: f32,
    ) -> Result<(), String> {
        let label = format!("{component}.duration for {}", self.definition);
        self.edit(
            editor,
            &[component, "duration"],
            serde_json::json!(seconds),
            &label,
        )
    }

    pub fn current_sequence(&self) -> Option<PoseSequence> {
        self.library().sequences.get(&self.sequence).cloned()
    }

    pub fn select_sequence(&mut self, name: String) {
        self.sequence = name;
        self.playback = default();
        self.key = None;
        self.solo = false;
        self.select_key(0);
    }

    /// Choosing a pose while editing a key replaces that key; browsing without a key is solo.
    pub fn select_pose(&mut self, name: String) {
        self.pose = name.clone();
        if let Some(index) = self.key
            && self
                .current_sequence()
                .is_some_and(|s| index < s.keys.len())
        {
            self.sequence_mut().keys[index].pose = name;
            self.select_key(index);
        } else {
            self.key = None;
            self.solo = true;
            self.playback.playing = false;
        }
    }

    pub fn add_key(&mut self, pose: String) {
        if self.current_sequence().is_none() || !self.library().poses.contains_key(&pose) {
            return;
        }
        let at = self.phase();
        let sequence = self.sequence_mut();
        let index = sequence.keys.partition_point(|key| key.at <= at);
        sequence.keys.insert(index, SequenceKey::new(pose, at));
        self.select_key(index);
    }

    pub fn remove_key(&mut self, index: usize) {
        let Some(sequence) = self.current_sequence() else {
            return;
        };
        if sequence.keys.len() <= 1 || index >= sequence.keys.len() {
            return;
        }
        self.sequence_mut().keys.remove(index);
        self.select_key(index.min(sequence.keys.len() - 2));
    }

    pub fn create_sequence(&mut self, name: String, copy: bool) {
        let sequence =
            if copy { self.current_sequence() } else { None }.unwrap_or_else(|| PoseSequence {
                keys: vec![SequenceKey::new(self.pose.clone(), 0.0)],
                ..default()
            });
        self.targets.sequences.insert(name.clone(), sequence);
        self.players.clear();
        self.select_sequence(name);
    }

    pub fn assign_move(&mut self, editor: &mut Editor, component: &str) -> Result<(), String> {
        self.apply(editor)?;
        let label = format!("{component}.sequence for {}", self.definition);
        self.edit(
            editor,
            &[component, "sequence"],
            self.sequence.clone().into(),
            &label,
        )?;
        self.players = sequence_players(
            &editor
                .project
                .as_ref()
                .ok_or("No project")?
                .inspect_definition(&self.definition)
                .map_err(|e| e.to_string())?,
            &self.sequence,
        );
        if let Some(index) = self.key {
            self.select_key(index);
        }
        Ok(())
    }

    pub fn assign_state(&mut self, editor: &mut Editor, state: &str) -> Result<(), String> {
        self.apply(editor)?;
        let label = format!("{state}.PlaySequence for {}", self.definition);
        let request = EditRequest::Set {
            file: format!("{}/entity.jsonc", self.definition),
            path: ["states", state, "enable", "PlaySequence", "sequence"]
                .into_iter()
                .map(|key| Field::Key(key.into()))
                .collect(),
            value: self.sequence.clone().into(),
            label: label.clone(),
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
        self.applied.push(label);
        self.players = sequence_players(
            &editor
                .project
                .as_ref()
                .ok_or("No project")?
                .inspect_definition(&self.definition)
                .map_err(|e| e.to_string())?,
            &self.sequence,
        );
        Ok(())
    }

    /// Seconds the sequence lasts: a loop's cycle, or the duration of the move playing it.
    pub fn length(&self, sequence: &PoseSequence) -> f32 {
        let moving = self.players.iter().find_map(|player| player.duration);
        match moving {
            Some(duration) if !sequence.looping && duration > 0.0 => duration,
            _ => sequence.seconds,
        }
    }

    /// The selected sequence's phase at the playhead.
    pub fn phase(&self) -> f32 {
        self.current_sequence().map_or(0.0, |sequence| {
            let phase = self.playback.time / self.length(&sequence);
            if sequence.looping {
                phase.rem_euclid(1.0)
            } else {
                phase.clamp(0.0, 1.0)
            }
        })
    }

    /// Moves the playhead on by `dt`, stopping at the end of a one-shot unless it repeats.
    pub fn advance(&mut self, dt: f32) {
        if !self.playback.playing {
            return;
        }
        let Some(sequence) = self.current_sequence() else {
            self.playback.playing = false;
            return;
        };
        let length = self.length(&sequence);
        self.playback.time += dt;
        if !sequence.looping && self.playback.time >= length {
            if self.playback.repeat {
                self.playback.time = self.playback.time.rem_euclid(length);
            } else {
                self.playback.time = length;
                self.playback.playing = false;
            }
        }
    }

    /// Edits key `index`: its pose in the joint editor, the playhead on it. A key the sequence
    /// is still fading in or out at is shown alone.
    pub fn select_key(&mut self, index: usize) {
        let Some(sequence) = self.current_sequence() else {
            return;
        };
        let Some(key) = sequence.keys.get(index) else {
            return;
        };
        self.key = Some(index);
        self.pose = key.pose.clone();
        self.playback.playing = false;
        self.playback.time = key.at * self.length(&sequence);
        self.solo = sequence.envelope(key.at) < 0.5;
    }

    /// Whether the view shows the pose alone rather than the sequence at the playhead.
    pub fn shows_pose(&self) -> bool {
        self.solo || self.current_sequence().is_none()
    }

    /// The rig's standing base with `pose` on it.
    fn posed(&self, rig: &Rig, library: &BasePoseSet, pose: &str) -> Pose {
        let mut posed = rig.skeleton.rest_pose();
        let base = SolverSettings::default().idle_pose;
        for name in [base.as_str(), pose] {
            if let Ok(resolved) = library.resolve(name, &rig.skeleton) {
                resolved.apply(&mut posed, 1.0, None);
            }
        }
        posed
    }

    /// What the view shows: the pose, or the sequence at the playhead, over the standing base.
    pub fn preview_pose(&self, rig: &Rig) -> Pose {
        let library = self.library();
        if self.shows_pose() {
            return self.posed(rig, &library, &self.pose);
        }
        let mut pose = self.posed(rig, &library, "");
        if let Some(sequence) = library.sequences.get(&self.sequence) {
            let phase = self.phase();
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
        pose
    }

    /// Model-space transforms of every joint in the preview.
    pub fn transforms(&self, rig: &Rig) -> Vec<Transform> {
        rig.skeleton.model_transforms(&self.preview_pose(rig))
    }

    /// The keys the playhead is between, as joint transforms: where it comes from, where it goes.
    pub fn ghosts(&self, rig: &Rig) -> Vec<Vec<Transform>> {
        if !self.ghosts || self.shows_pose() {
            return Vec::new();
        }
        let library = self.library();
        let Some(sequence) = library.sequences.get(&self.sequence) else {
            return Vec::new();
        };
        let blend = sequence.blend(self.phase());
        let around = match blend.as_slice() {
            [.., (from, _), (to, weight)] if *weight < 1.0 => vec![*from, *to],
            [.., (last, _)] => vec![*last],
            [] => Vec::new(),
        };
        around
            .into_iter()
            .map(|key| {
                let pose = self.posed(rig, &library, &sequence.keys[key].pose);
                rig.skeleton.model_transforms(&pose)
            })
            .collect()
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

    /// The pose's local rotation of `joint`, or the joint's rest rotation when it sets none.
    pub fn joint_rotation(&self, rig: &Rig, joint: usize) -> Quat {
        let name = &rig.skeleton.joints()[joint].name;
        self.library()
            .poses
            .get(&self.pose)
            .and_then(|pose| pose.joints.iter().find(|j| &j.joint == name))
            .and_then(|j| j.euler_deg)
            .map_or(rig.skeleton.joints()[joint].rest.rotation, euler_rotation)
    }

    pub fn set_joint_rotation(&mut self, rig: &Rig, joint: usize, rotation: Quat) {
        let name = rig.skeleton.joints()[joint].name.clone();
        let (x, y, z) = rotation.to_euler(EulerRot::XYZ);
        let angles = Vec3::new(x, y, z) * 180.0 / std::f32::consts::PI;
        let pose = self.pose_mut();
        match pose.joints.iter_mut().find(|j| j.joint == name) {
            Some(existing) => existing.euler_deg = Some(angles),
            None => pose.joints.push(JointPose::rotation(&name, angles)),
        }
    }

    /// Turns `joint` by `angle` radians about its own local `axis` (0, 1, 2: X, Y, Z).
    pub fn rotate_joint(&mut self, rig: &Rig, joint: usize, axis: usize, angle: f32) {
        let turned =
            self.joint_rotation(rig, joint) * Quat::from_axis_angle(Vec3::AXES[axis], angle);
        self.set_joint_rotation(rig, joint, turned);
    }

    /// Copies the pose's joints on the selected joint's side to the other side, mirrored.
    pub fn mirror(&mut self, rig: &Rig) -> usize {
        let selected = &rig.skeleton.joints()[self.joint].name;
        let Some(side) = side_of(selected) else {
            return 0;
        };
        let mirrored: Vec<JointPose> = self
            .library()
            .poses
            .get(&self.pose)
            .map(|pose| {
                pose.joints
                    .iter()
                    .filter(|j| side_of(&j.joint) == Some(side))
                    .filter_map(mirror_joint)
                    .filter(|j| rig.skeleton.joint_id(&j.joint).is_ok())
                    .collect()
            })
            .unwrap_or_default();
        let count = mirrored.len();
        let pose = self.pose_mut();
        for joint in mirrored {
            match pose.joints.iter_mut().find(|j| j.joint == joint.joint) {
                Some(existing) => *existing = joint,
                None => pose.joints.push(joint),
            }
        }
        count
    }

    /// Walks the skeleton: `Up` the parent, `Down` the first child, `Across` the other side.
    pub fn step_joint(&mut self, rig: &Rig, step: Step) {
        let joints = rig.skeleton.joints();
        let next = match step {
            Step::Up => joints[self.joint].parent,
            Step::Down => joints.iter().position(|j| j.parent == Some(self.joint)),
            Step::Across => counterpart(&joints[self.joint].name)
                .and_then(|name| rig.skeleton.joint_id(&name).ok()),
        };
        if let Some(next) = next {
            self.joint = next;
        }
    }

    /// Selects the previous or next key on the timeline.
    pub fn step_key(&mut self, forward: bool) {
        let Some(sequence) = self.current_sequence() else {
            return;
        };
        let count = sequence.keys.len();
        if count == 0 {
            return;
        }
        let phase = self.phase();
        let index = match (self.key, forward) {
            (Some(key), true) => (key + 1) % count,
            (Some(key), false) => (key + count - 1) % count,
            (None, true) => sequence
                .keys
                .iter()
                .position(|key| key.at > phase)
                .unwrap_or(0),
            (None, false) => sequence
                .keys
                .iter()
                .rposition(|key| key.at < phase)
                .unwrap_or(count - 1),
        };
        self.select_key(index);
    }

    pub fn toggle_play(&mut self) {
        let Some(sequence) = self.current_sequence() else {
            return;
        };
        if !self.playback.playing
            && !sequence.looping
            && self.playback.time >= self.length(&sequence)
        {
            self.playback.time = 0.0;
        }
        self.playback.playing = !self.playback.playing;
        self.solo = false;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Across,
}

fn euler_rotation(degrees: Vec3) -> Quat {
    let r = degrees * std::f32::consts::PI / 180.0;
    Quat::from_euler(EulerRot::XYZ, r.x, r.y, r.z)
}

fn side_of(name: &str) -> Option<char> {
    name.strip_suffix("_l")
        .map(|_| 'l')
        .or_else(|| name.strip_suffix("_r").map(|_| 'r'))
}

/// The joint of the same name on the other side: `hand_l` and `hand_r`.
pub fn counterpart(name: &str) -> Option<String> {
    name.strip_suffix("_l")
        .map(|base| format!("{base}_r"))
        .or_else(|| name.strip_suffix("_r").map(|base| format!("{base}_l")))
}

/// A joint's values for the other side, mirrored across the body's YZ plane.
pub fn mirror_joint(joint: &JointPose) -> Option<JointPose> {
    Some(JointPose {
        joint: counterpart(&joint.joint)?,
        translation: joint.translation.map(|t| Vec3::new(-t.x, t.y, t.z)),
        euler_deg: joint.euler_deg.map(|e| Vec3::new(e.x, -e.y, -e.z)),
    })
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
                .map(move |(index, key)| {
                    format!("{name}, key {} at {:.0}%", index + 1, key.at * 100.0)
                })
        })
        .collect()
}

/// Where a definition names `sequence` in a `sequence` field: a component such as a move's
/// tuning (`Attack.sequence`, defaults included, with its `duration`), or a state rule
/// (`While Walking (PlaySequence)`).
pub fn sequence_players(definition: &DefinitionInspection, sequence: &str) -> Vec<Player> {
    fn walk(value: &Value, path: &mut Vec<String>, sequence: &str, found: &mut Vec<Player>) {
        let Value::Object(members) = value else {
            return;
        };
        if members.get("sequence").and_then(Value::as_str) == Some(sequence) {
            found.push(match path.as_slice() {
                [states, state, _, component] if states == "states" => Player {
                    label: format!("While {state} ({component})"),
                    component: None,
                    duration: None,
                },
                [component] => Player {
                    label: format!("{component}.sequence"),
                    component: Some(component.clone()),
                    duration: members
                        .get("duration")
                        .and_then(Value::as_f64)
                        .map(|d| d as f32),
                },
                _ => Player {
                    label: format!("{}.sequence", path.join(".")),
                    component: None,
                    duration: None,
                },
            });
        }
        for (key, value) in members {
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

/// The part under `ray`, as an index into `placed` (each a mesh and where it is).
pub fn pick_part(ray: Ray3d, bundle: &MeshBundle, placed: &[(usize, Transform)]) -> Option<usize> {
    let mut nearest: Option<(f32, usize)> = None;
    for (part, &(mesh, to_world)) in placed.iter().enumerate() {
        let Some(lod) = bundle.meshes[mesh].lods.first() else {
            continue;
        };
        for triangle in lod.indices.as_chunks::<3>().0 {
            let [a, b, c] =
                triangle.map(|i| to_world.transform_point(lod.positions[i as usize].into()));
            if let Some(distance) = ray_triangle(ray, a, b, c)
                && nearest.is_none_or(|(best, _)| distance < best)
            {
                nearest = Some((distance, part));
            }
        }
    }
    nearest.map(|(_, part)| part)
}

/// The joint under `ray`: that of the nearest model part it hits, or else the nearest joint
/// within `reach` of it.
pub fn pick(
    ray: Ray3d,
    bundle: &MeshBundle,
    parts: &[(usize, usize)],
    transforms: &[Transform],
    reach: f32,
) -> Option<usize> {
    let placed: Vec<_> = parts
        .iter()
        .map(|&(mesh, joint)| (mesh, transforms[joint]))
        .collect();
    if let Some(part) = pick_part(ray, bundle, &placed) {
        return Some(parts[part].1);
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

/// The selected joint's rotation rings: its center and world rotation, whose local axes the
/// rings turn about, sized to stay readable at any zoom.
pub fn rings(transforms: &[Transform], joint: usize, camera: Vec3) -> (Vec3, Quat, f32) {
    let at = transforms[joint];
    let radius = (0.07 * camera.distance(at.translation)).clamp(0.04, 0.6);
    (at.translation, at.rotation, radius)
}

/// Points around a ring about `axis` (world) through `center`.
pub fn ring_points(center: Vec3, axis: Vec3, radius: f32) -> impl Iterator<Item = Vec3> {
    let (u, v) = axis.normalize().any_orthonormal_pair();
    (0..48).map(move |i| {
        let t = i as f32 / 48.0 * std::f32::consts::TAU;
        center + (u * t.cos() + v * t.sin()) * radius
    })
}

/// Rotation about a ring's axis for a cursor moving from `before` to `after` around the ring's
/// screen `center` (y down). Turning counterclockwise on screen turns positively about an axis
/// pointing at the viewer.
pub fn ring_drag(before: Vec2, after: Vec2, center: Vec2, toward_viewer: bool) -> f32 {
    let angle = |p: Vec2| (p.y - center.y).atan2(p.x - center.x);
    let mut delta = angle(after) - angle(before);
    if delta > std::f32::consts::PI {
        delta -= std::f32::consts::TAU;
    } else if delta < -std::f32::consts::PI {
        delta += std::f32::consts::TAU;
    }
    if toward_viewer { -delta } else { delta }
}

/// A preview mesh: the part it shows and, in the pose workspace, the joint it rigidly follows.
/// Overlays (wireframes, hulls) are not `solid` and do not light up when selected.
#[derive(Component)]
pub struct PreviewPart {
    pub mesh: usize,
    pub joint: Option<usize>,
    pub solid: bool,
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
const RING_COLORS: [Color; 3] = [
    Color::srgb(0.93, 0.33, 0.33),
    Color::srgb(0.45, 0.82, 0.36),
    Color::srgb(0.36, 0.56, 0.95),
];

/// Plays the pose preview and poses its parts, highlights the selection (the joint while
/// posing, the part otherwise) and draws the skeleton, the key ghosts and the rotation rings.
#[allow(clippy::too_many_arguments)]
pub fn animate_preview(
    time: Res<Time>,
    mut toolbox: ResMut<Toolbox>,
    editor: NonSend<Editor>,
    cameras: Query<&GlobalTransform, With<Camera3d>>,
    mut parts: Query<(
        Ref<PreviewPart>,
        &mut Transform,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut gizmos: Gizmos<PoseGizmos>,
    mut highlighted: Local<Option<(Mode, usize)>>,
) {
    let Some(tool) = &mut toolbox.tool else {
        return;
    };
    let mode = tool.mode;
    let posing = mode == Mode::Poses;
    let camera = cameras
        .get(tool.view_entities().0)
        .map_or(Vec3::Z * 4.0, GlobalTransform::translation);
    let Some(ready) = tool.ready_mut() else {
        return;
    };
    let selected_mesh = ready.mesh_index;
    let draft = &mut ready.pose;
    let rig = posing.then(|| draft.rig(&editor)).flatten();
    if rig.is_some() {
        draft.advance(time.delta_secs());
    }
    let transforms = rig.as_ref().map(|rig| draft.transforms(rig));
    let selection = (mode, if posing { draft.joint } else { selected_mesh });
    let restyle = *highlighted != Some(selection) || parts.iter().any(|(part, ..)| part.is_added());
    *highlighted = Some(selection);
    for (part, mut transform, material) in &mut parts {
        if let (Some(joint), Some(transforms)) = (part.joint, &transforms)
            && let Some(&posed) = transforms.get(joint)
        {
            transform.set_if_neq(posed);
        }
        let lit = if posing {
            part.joint == Some(draft.joint)
        } else {
            part.mesh == selected_mesh
        };
        if restyle
            && part.solid
            && let Some(mut material) = materials.get_mut(&material.0)
        {
            material.emissive = if lit { HIGHLIGHT } else { LinearRgba::BLACK };
        }
    }
    let (Some(rig), Some(transforms)) = (rig, transforms) else {
        return;
    };
    let muted = Color::srgba(0.75, 0.78, 0.85, 0.7);
    let accent = Color::srgb(1.0, 0.66, 0.23);
    let authored: Vec<String> = draft
        .library()
        .poses
        .get(&draft.pose)
        .map(|pose| pose.joints.iter().map(|j| j.joint.clone()).collect())
        .unwrap_or_default();
    for (ghost, color) in draft.ghosts(&rig).iter().zip([
        Color::srgba(0.45, 0.65, 1.0, 0.45),
        Color::srgba(0.5, 1.0, 0.65, 0.45),
    ]) {
        for (index, joint) in rig.skeleton.joints().iter().enumerate() {
            if let Some(parent) = joint.parent {
                gizmos.line(ghost[parent].translation, ghost[index].translation, color);
            }
        }
    }
    for (index, joint) in rig.skeleton.joints().iter().enumerate() {
        let at = transforms[index].translation;
        if let Some(parent) = joint.parent {
            let color = if index == draft.joint || parent == draft.joint {
                accent
            } else {
                muted
            };
            gizmos.line(transforms[parent].translation, at, color);
        }
        let set = authored.contains(&joint.name);
        let (radius, color) = if index == draft.joint {
            (0.035, accent)
        } else if set {
            (0.018, Color::srgb(0.95, 0.85, 0.55))
        } else {
            (0.01, muted)
        };
        gizmos.sphere(Isometry3d::from_translation(at), radius, color);
    }
    let (center, rotation, radius) = rings(&transforms, draft.joint, camera);
    for (axis, color) in RING_COLORS.into_iter().enumerate() {
        let color = if draft.ring == Some(axis) {
            Color::WHITE
        } else {
            color
        };
        let normal = rotation * Vec3::AXES[axis];
        gizmos
            .circle(
                Isometry3d::new(center, Quat::from_rotation_arc(Vec3::Z, normal)),
                radius,
                color,
            )
            .resolution(48);
    }
}

/// A ring being dragged: its axis and the cursor's last position.
#[derive(Default)]
pub struct RingDrag(Option<(usize, Vec2)>);

/// Pointer work in the view: dragging a rotation ring turns the selected joint; a click (not a
/// drag) selects the part under the cursor, and while posing its joint.
#[allow(clippy::too_many_arguments)]
pub fn pointer(
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    cameras: Query<(&Camera, &GlobalTransform)>,
    mut toolbox: ResMut<Toolbox>,
    editor: NonSend<Editor>,
    mut pressed: Local<Option<Vec2>>,
    mut drag: Local<RingDrag>,
) {
    let Some(tool) = &mut toolbox.tool else {
        return;
    };
    let (camera, window) = tool.view_entities();
    let cursor = windows.get(window).ok().and_then(Window::cursor_position);
    let over = cursor.is_some_and(|cursor| tool.view.is_some_and(|view| view.contains(cursor)))
        && !tool.pointer_over_ui;
    let posing = tool.mode == Mode::Poses;
    let Ok((camera, camera_transform)) = cameras.get(camera) else {
        return;
    };
    let Some(ready) = tool.ready_mut() else {
        return;
    };
    let rig = posing.then(|| ready.pose.rig(&editor)).flatten();
    let transforms = rig.as_ref().map(|rig| ready.pose.transforms(rig));

    // The ring under the cursor, by its drawn outline.
    let hovered = match (&rig, &transforms, cursor) {
        (Some(_), Some(transforms), Some(cursor)) if over => {
            let (center, rotation, radius) =
                rings(transforms, ready.pose.joint, camera_transform.translation());
            (0..3)
                .filter_map(|axis| {
                    ring_points(center, rotation * Vec3::AXES[axis], radius)
                        .filter_map(|p| camera.world_to_viewport(camera_transform, p).ok())
                        .map(|p| p.distance(cursor))
                        .min_by(f32::total_cmp)
                        .filter(|distance| *distance < 7.0)
                        .map(|distance| (distance, axis))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, axis)| axis)
        }
        _ => None,
    };

    if buttons.just_pressed(MouseButton::Left) {
        *pressed = cursor.filter(|_| over);
        if let (Some(axis), Some(cursor)) = (hovered, cursor) {
            drag.0 = Some((axis, cursor));
            *pressed = None;
        }
    }
    if let (Some((axis, before)), Some(cursor), Some(rig), Some(transforms)) =
        (drag.0, cursor, &rig, &transforms)
    {
        if buttons.pressed(MouseButton::Left) {
            let joint = ready.pose.joint;
            let center = transforms[joint].translation;
            if let Ok(screen) = camera.world_to_viewport(camera_transform, center) {
                let axis_world = transforms[joint].rotation * Vec3::AXES[axis];
                let toward = axis_world.dot(camera_transform.translation() - center) > 0.0;
                let angle = ring_drag(before, cursor, screen, toward);
                if angle != 0.0 {
                    ready.pose.rotate_joint(rig, joint, axis, angle);
                }
            }
            drag.0 = Some((axis, cursor));
        } else {
            drag.0 = None;
        }
    } else if !buttons.pressed(MouseButton::Left) {
        drag.0 = None;
    }
    ready.pose.ring = drag.0.map(|(axis, _)| axis).or(hovered);
    tool.view_drag = drag.0.is_some();
    let Some(ready) = tool.ready_mut() else {
        return;
    };

    if !buttons.just_released(MouseButton::Left) {
        return;
    }
    let (Some(start), Some(cursor)) = (pressed.take(), cursor) else {
        return;
    };
    if !over || start.distance(cursor) > 4.0 {
        return;
    }
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return;
    };
    let Some(preview) = &ready.preview else {
        return;
    };
    let bundle = &preview.bundle;
    if let (Some(rig), Some(transforms)) = (&rig, &transforms) {
        let parts = parts(bundle, rig);
        let reach = 0.02 * camera_transform.translation().length().max(1.0);
        let placed: Vec<_> = parts
            .iter()
            .map(|&(mesh, joint)| (mesh, transforms[joint]))
            .collect();
        if let Some(part) = pick_part(ray, bundle, &placed) {
            ready.mesh_index = parts[part].0;
        }
        if let Some(joint) = pick(ray, bundle, &parts, transforms, reach) {
            ready.pose.joint = joint;
        }
    } else {
        let placed = crate::tools::placements(bundle);
        if let Some(part) = pick_part(ray, bundle, &placed) {
            ready.mesh_index = placed[part].0;
        }
    }
}

/// Keys of the pose workspace, read before the panels claim them.
pub fn shortcuts(ctx: &egui::Context, draft: &mut PoseDraft, editor: &Editor) {
    use egui::{Key, Modifiers};
    let pressed = |key| ctx.input_mut(|input| input.consume_key(Modifiers::NONE, key));
    if pressed(Key::Space) {
        draft.toggle_play();
    }
    if pressed(Key::Comma) {
        draft.step_key(false);
    }
    if pressed(Key::Period) {
        draft.step_key(true);
    }
    let Some(rig) = draft.rig(editor) else {
        return;
    };
    for (key, step) in [
        (Key::ArrowUp, Step::Up),
        (Key::ArrowDown, Step::Down),
        (Key::Tab, Step::Across),
    ] {
        if pressed(key) {
            draft.step_joint(&rig, step);
        }
    }
}

/// The timeline under the view: keys and events placed on the sequence's length, draggable,
/// and a playhead to scrub.
pub fn timeline(ui: &mut Ui, draft: &mut PoseDraft) {
    let Some(sequence) = draft.current_sequence() else {
        ui.horizontal(|ui| {
            muted(
                ui,
                "Pose library preview. Choose an animation in the sidebar to add pose keys and play it.",
            );
        });
        return;
    };
    let length = draft.length(&sequence);
    let width = ui.available_width();
    ui.horizontal_wrapped(|ui| {
        ui.label(theme::section(&format!("Animation: {}", draft.sequence)));
        muted(ui, "◆ pose key · ▲ gameplay event");
    });
    ui.horizontal_wrapped(|ui| {
        let label = if draft.playback.playing {
            "⏸ Pause"
        } else {
            "▶ Play"
        };
        if ui.button(label).on_hover_text("Space").clicked() {
            draft.toggle_play();
        }
        if !sequence.looping {
            ui.checkbox(&mut draft.playback.repeat, "Repeat");
        }
        if ui
            .checkbox(&mut draft.solo, "Key pose alone")
            .on_hover_text("Show the selected pose by itself instead of the animation")
            .changed()
            && draft.solo
        {
            draft.playback.playing = false;
        }
        ui.checkbox(&mut draft.ghosts, "Ghosts")
            .on_hover_text("Faint skeletons of the keys the playhead is between");
        let source = if sequence.gait {
            "per cycle, steps pace it in game".to_owned()
        } else if sequence.looping {
            "per cycle".to_owned()
        } else {
            draft
                .players
                .iter()
                .find(|player| player.duration.is_some())
                .and_then(|player| player.component.clone())
                .map_or("preview length".into(), |component| {
                    format!("{component}.duration")
                })
        };
        ui.label(
            RichText::new(format!(
                "{:.2} / {length:.2} s · {source} · phase {:.2}",
                draft.phase() * length,
                draft.phase()
            ))
            .monospace()
            .color(theme::MUTED),
        );
        muted(ui, "Space play · , . keys · ↑ ↓ Tab joints");
    });
    if draft.playback.playing {
        ui.ctx().request_repaint();
    }

    ui.horizontal_wrapped(|ui| {
        ui.label("Add pose:");
        let names: Vec<_> = draft.library().poses.keys().cloned().collect();
        if !names.contains(&draft.insert_pose) {
            draft.insert_pose = draft.pose.clone();
        }
        egui::ComboBox::from_id_salt("insert_pose")
            .selected_text(&draft.insert_pose)
            .show_ui(ui, |ui| {
                for name in &names {
                    ui.selectable_value(&mut draft.insert_pose, name.clone(), name);
                }
            });
        if ui.button("Add pose at playhead").clicked() {
            draft.add_key(draft.insert_pose.clone());
        }
        if ui.add_enabled(draft.key.is_some() && sequence.keys.len() > 1,
            egui::Button::new("Remove selected key"))
            .on_hover_text("Removes this occurrence; the pose stays in the library. An animation needs at least one key.")
            .clicked()
            && let Some(index) = draft.key
        {
            draft.remove_key(index);
        }
    });
    let sequence = draft.current_sequence().expect("selected animation");

    let (rect, track) =
        ui.allocate_exact_size(egui::vec2(width, 70.0), egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    let inner = rect.shrink2(egui::vec2(14.0, 6.0));
    let x = |phase: f32| inner.left() + phase.clamp(0.0, 1.0) * inner.width();
    let phase_at = |pos: egui::Pos2| ((pos.x - inner.left()) / inner.width()).clamp(0.0, 1.0);
    let key_y = inner.top() + inner.height() * 0.45;
    let event_y = inner.bottom() - 6.0;
    painter.rect_filled(rect, 4.0, theme::BASE);
    for tick in 0..=10 {
        let at = x(tick as f32 / 10.0);
        let tall = tick % 5 == 0;
        painter.line_segment(
            [
                egui::pos2(at, inner.bottom() - if tall { 14.0 } else { 7.0 }),
                egui::pos2(at, inner.bottom()),
            ],
            egui::Stroke::new(1.0, theme::BORDER),
        );
    }
    if !sequence.looping && sequence.fade_in + sequence.fade_out > 0.0 {
        let fade = egui::Color32::from_rgba_unmultiplied(0x8a, 0x91, 0x9e, 18);
        for (from, to) in [(0.0, sequence.fade_in), (1.0 - sequence.fade_out, 1.0)] {
            painter.rect_filled(
                egui::Rect::from_x_y_ranges(x(from)..=x(to), inner.y_range()),
                0.0,
                fade,
            );
        }
    }
    if let Some(pos) = track.interact_pointer_pos()
        && (track.clicked() || track.dragged())
    {
        draft.playback.playing = false;
        draft.playback.time = phase_at(pos) * length;
        draft.solo = false;
    }

    let mut edited = sequence.clone();
    let count = edited.keys.len();
    for index in 0..count {
        let center = egui::pos2(x(edited.keys[index].at), key_y);
        let id = ui.id().with(("timeline_key", index));
        let marker = ui.interact(
            egui::Rect::from_center_size(center, egui::vec2(16.0, 22.0)),
            id,
            egui::Sense::click_and_drag(),
        );
        if marker.dragged() {
            let low = if index > 0 {
                edited.keys[index - 1].at
            } else {
                0.0
            };
            let high = edited.keys.get(index + 1).map_or(1.0, |key| key.at);
            let at = &mut edited.keys[index].at;
            *at = (*at + marker.drag_delta().x / inner.width()).clamp(low, high);
            draft.key = Some(index);
            draft.pose = edited.keys[index].pose.clone();
            draft.playback.playing = false;
            draft.playback.time = edited.keys[index].at * length;
        } else if marker.clicked() {
            draft.select_key(index);
        }
        let selected = draft.key == Some(index);
        let fill = if selected {
            theme::ACCENT
        } else if marker.hovered() {
            egui::Color32::WHITE
        } else {
            theme::DEFINITION
        };
        let r = 7.0;
        painter.add(egui::Shape::convex_polygon(
            vec![
                center + egui::vec2(0.0, -r),
                center + egui::vec2(r, 0.0),
                center + egui::vec2(0.0, r),
                center + egui::vec2(-r, 0.0),
            ],
            fill,
            egui::Stroke::NONE,
        ));
        painter.text(
            center + egui::vec2(0.0, -r - 2.0),
            egui::Align2::CENTER_BOTTOM,
            &edited.keys[index].pose,
            egui::FontId::proportional(11.0),
            if selected {
                theme::ACCENT
            } else {
                theme::MUTED
            },
        );
        marker.on_hover_text(format!(
            "Pose key {} at {:.2} s: click to edit its pose, drag to retime",
            index + 1,
            edited.keys[index].at * length
        ));
    }
    let names: Vec<String> = edited.events.keys().cloned().collect();
    for name in names {
        let at = edited.events[&name];
        let center = egui::pos2(x(at), event_y);
        let marker = ui.interact(
            egui::Rect::from_center_size(center, egui::vec2(14.0, 14.0)),
            ui.id().with(("timeline_event", &name)),
            egui::Sense::click_and_drag(),
        );
        if marker.dragged() {
            let at = edited.events.get_mut(&name).expect("listed");
            *at = (*at + marker.drag_delta().x / inner.width()).clamp(0.0, 1.0);
            draft.playback.playing = false;
            draft.playback.time = *at * length;
        } else if marker.clicked() {
            draft.playback.playing = false;
            draft.playback.time = at * length;
            draft.solo = false;
        }
        painter.add(egui::Shape::convex_polygon(
            vec![
                center + egui::vec2(0.0, -6.0),
                center + egui::vec2(5.0, 4.0),
                center + egui::vec2(-5.0, 4.0),
            ],
            theme::WARD,
            egui::Stroke::NONE,
        ));
        painter.text(
            center + egui::vec2(8.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &name,
            egui::FontId::proportional(11.0),
            theme::WARD,
        );
        marker.on_hover_text(format!(
            "Gameplay event {name} at {:.2} s: drag to retime. An event signals game logic; it is not a pose.",
            edited.events[&name] * length
        ));
    }
    let head = x(draft.phase());
    painter.line_segment(
        [
            egui::pos2(head, rect.top()),
            egui::pos2(head, rect.bottom()),
        ],
        egui::Stroke::new(2.0, theme::ACCENT),
    );
    if edited != sequence {
        *draft.sequence_mut() = edited;
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
    let definition = editor
        .project
        .as_ref()
        .and_then(|project| project.inspect_definition(&draft.definition).ok());
    draft.players = definition
        .as_ref()
        .map(|definition| sequence_players(definition, &draft.sequence))
        .unwrap_or_default();
    if let Some((component, seconds)) = &draft.duration_edit {
        for player in &mut draft.players {
            if player.component.as_ref() == Some(component) {
                player.duration = Some(*seconds);
            }
        }
    }

    ui.add_space(4.0);
    ui.label(theme::section("Animation"));
    sequence_section(ui, draft, editor);
    if !draft.sequence.is_empty() {
        ui.add_space(6.0);
        ui.label(theme::section("Pose keys"));
        keys_section(ui, draft);
    }
    ui.add_space(6.0);
    ui.label(theme::section("Key pose"));
    pose_section(ui, draft);
    ui.add_space(6.0);
    ui.label(theme::section("Joint"));
    joint_section(ui, draft, &rig);
    if !draft.sequence.is_empty() {
        ui.add_space(6.0);
        egui::CollapsingHeader::new(theme::section("Playback, blending & events"))
            .id_salt("sequence_settings")
            .show(ui, |ui| settings_section(ui, draft));
    }
    ui.separator();
    if let Err(error) = draft.check(editor) {
        ui.colored_label(theme::AXES[0], error);
    }
    if let Some(error) = &draft.error {
        ui.colored_label(theme::AXES[0], error);
    }
}

fn muted(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).small().color(theme::MUTED));
}

/// A name field and button adding a copy of the current selection under a new name.
fn add_copy(ui: &mut Ui, what: &str, names: &[String], new_name: &mut String) -> Option<String> {
    let mut added = None;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(new_name)
                .hint_text(format!("New {what}"))
                .desired_width(140.0),
        );
        let name = new_name.trim().to_owned();
        let valid = !name.is_empty() && !names.contains(&name);
        if ui
            .add_enabled(valid, egui::Button::new(format!("Copy {what}")))
            .on_hover_text(format!("A new {what} starting from the selected one"))
            .clicked()
        {
            added = Some(name);
            new_name.clear();
        }
    });
    added
}

fn overridden(ui: &mut Ui, inherited: bool) -> bool {
    let mut restore = false;
    ui.horizontal(|ui| {
        ui.colored_label(theme::ACCENT, "Overridden on this definition");
        let label = if inherited {
            "Restore library version"
        } else {
            "Remove"
        };
        restore = ui.small_button(label).clicked();
    });
    restore
}

fn sequence_section(ui: &mut Ui, draft: &mut PoseDraft, editor: &mut Editor) {
    let library = draft.library();
    let names: Vec<String> = library.sequences.keys().cloned().collect();
    let before = draft.sequence.clone();
    let shown = if draft.sequence.is_empty() {
        "Pose library only"
    } else {
        draft.sequence.as_str()
    };
    egui::ComboBox::from_id_salt("sequence")
        .selected_text(shown)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut draft.sequence, String::new(), "Pose library only");
            for name in &names {
                ui.selectable_value(&mut draft.sequence, name.clone(), name);
            }
        });
    if before != draft.sequence {
        draft.players = editor
            .project
            .as_ref()
            .and_then(|project| project.inspect_definition(&draft.definition).ok())
            .map(|definition| sequence_players(&definition, &draft.sequence))
            .unwrap_or_default();
        draft.select_sequence(draft.sequence.clone());
    }
    ui.horizontal_wrapped(|ui| {
        ui.add(
            egui::TextEdit::singleline(&mut draft.new_sequence)
                .hint_text("New animation name")
                .desired_width(140.0),
        );
        let name = draft.new_sequence.trim().to_owned();
        let valid = !name.is_empty() && !names.contains(&name);
        let new = ui
            .add_enabled(valid, egui::Button::new("New animation"))
            .on_hover_text(
                "Start with one key using the current pose; add more poses at the playhead",
            )
            .clicked();
        let copy = ui
            .add_enabled(
                valid && draft.current_sequence().is_some(),
                egui::Button::new("Copy animation"),
            )
            .on_hover_text("Duplicate this animation, including keys, events and blending")
            .clicked();
        if new || copy {
            draft.create_sequence(name, copy);
            draft.players.clear();
            draft.new_sequence.clear();
        }
    });
    let Some(sequence) = draft.current_sequence() else {
        muted(
            ui,
            "A pose is a body configuration. An animation plays a list of pose keys over time.",
        );
        return;
    };
    if !sequence.doc.is_empty() {
        ui.label(&sequence.doc);
    }
    if draft.players.is_empty() {
        muted(
            ui,
            "Available to preview. Not assigned to gameplay on this definition.",
        );
    }
    if let Some(definition) = editor
        .project
        .as_ref()
        .and_then(|project| project.inspect_definition(&draft.definition).ok())
    {
        for (path, value) in &definition.components {
            let component = path.rsplit("::").next().unwrap_or(path);
            if !sequence.looping && value.get("duration").is_some()
                && let Some(current) = value.get("sequence").and_then(Value::as_str)
                && current != draft.sequence
                && ui.button(format!("Use for {component}"))
                    .on_hover_text(format!("Save pending poses and replace {component}.sequence ({current}) with {}. Undo restores the assignment.", draft.sequence))
                    .clicked()
            {
                draft.error = draft.assign_move(editor, component).err();
            }
        }
        if sequence.looping
            && let Some(states) = definition.resolved.get("states").and_then(Value::as_object)
        {
            for (state, rule) in states {
                if let Some(current) = rule.get("enable")
                    .and_then(|enable| enable.get("PlaySequence"))
                    .and_then(|player| player.get("sequence")).and_then(Value::as_str)
                    && current != draft.sequence
                    && ui.button(format!("Use while {state}"))
                        .on_hover_text(format!("Save pending poses and replace the animation played while {state}: {current}"))
                        .clicked()
                {
                    draft.error = draft.assign_state(editor, state).err();
                }
            }
        }
    }
    for index in 0..draft.players.len() {
        let player = draft.players[index].clone();
        ui.horizontal(|ui| {
            ui.colored_label(theme::WARD, format!("Played by {}", player.label));
            if let (Some(component), Some(mut duration)) = (&player.component, player.duration) {
                let response = ui
                    .add(
                        egui::DragValue::new(&mut duration)
                            .speed(0.01)
                            .range(0.05..=10.0)
                            .suffix(" s"),
                    )
                    .on_hover_text(format!("{component}.duration: how long the move takes"));
                if response.changed() {
                    draft.duration_edit = Some((component.clone(), duration));
                }
                if (response.drag_stopped() || response.lost_focus())
                    && let Some((component, seconds)) = draft.duration_edit.take()
                {
                    draft.error = draft.set_duration(editor, &component, seconds).err();
                }
            }
        });
    }
    if draft.targets.sequences.contains_key(&draft.sequence)
        && overridden(ui, draft.defaults.sequences.contains_key(&draft.sequence))
    {
        draft.targets.sequences.remove(&draft.sequence);
    }
}

fn pose_section(ui: &mut Ui, draft: &mut PoseDraft) {
    let library = draft.library();
    let names: Vec<String> = library.poses.keys().cloned().collect();
    let before = draft.pose.clone();
    egui::ComboBox::from_id_salt("key_pose")
        .selected_text(&draft.pose)
        .show_ui(ui, |ui| {
            for name in &names {
                ui.selectable_value(&mut draft.pose, name.clone(), name);
            }
        });
    if let Some(name) = add_copy(ui, "pose", &names, &mut draft.new_pose) {
        let copy = library.poses.get(&draft.pose).cloned().unwrap_or_default();
        draft.targets.poses.insert(name.clone(), copy);
        draft.pose = name;
    }
    if before != draft.pose {
        draft.select_pose(draft.pose.clone());
    }
    if let (Some(key), Some(sequence)) = (draft.key, draft.current_sequence())
        && let Some(at) = sequence.keys.get(key).map(|key| key.at)
    {
        ui.colored_label(
            theme::ACCENT,
            format!(
                "Editing key {} of {} at {:.2} s",
                key + 1,
                draft.sequence,
                at * draft.length(&sequence)
            ),
        );
        muted(
            ui,
            "Choosing another pose replaces this key. Copy the pose before changing joints to keep other keys unchanged.",
        );
    } else {
        muted(
            ui,
            "Editing a library pose. Use Add pose at playhead to put it in the animation.",
        );
    }
    let library = draft.library();
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
            "Available to add to an animation; it may also be used by body constraints.",
        );
    } else {
        muted(ui, format!("Used by {}", users.join(" · ")));
    }
    if draft.targets.poses.contains_key(&draft.pose)
        && overridden(ui, draft.defaults.poses.contains_key(&draft.pose))
    {
        draft.targets.poses.remove(&draft.pose);
    }
}

fn joint_section(ui: &mut Ui, draft: &mut PoseDraft, rig: &Rig) {
    let library = draft.library();
    let pose = library.poses.get(&draft.pose).cloned().unwrap_or_default();
    let joints = rig.skeleton.joints();
    draft.joint = draft.joint.min(joints.len().saturating_sub(1));
    egui::ComboBox::from_id_salt("joint")
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
        "Click the model or use ↑ parent, ↓ child, Tab other side. Drag a ring to turn the joint.",
    );
    let name = joints[draft.joint].name.clone();
    let existing = pose.joints.iter().find(|j| j.joint == name);
    let (x, y, z) = draft
        .joint_rotation(rig, draft.joint)
        .to_euler(EulerRot::XYZ);
    let mut angles = Vec3::new(x, y, z) * 180.0 / std::f32::consts::PI;
    let before = angles;
    ui.horizontal(|ui| {
        for axis in 0..3 {
            ui.colored_label(theme::AXES[axis], ["X", "Y", "Z"][axis]);
            ui.add(
                egui::DragValue::new(&mut angles[axis])
                    .speed(0.5)
                    .suffix("°")
                    .range(-180.0..=180.0),
            );
        }
    });
    if angles != before {
        draft.set_joint_rotation(rig, draft.joint, euler_rotation(angles));
    }
    ui.horizontal(|ui| {
        if let Some(other) = counterpart(&name).filter(|other| rig.skeleton.joint_id(other).is_ok())
        {
            let side = if name.ends_with("_l") {
                "left"
            } else {
                "right"
            };
            if ui
                .button(format!("Mirror {side} side"))
                .on_hover_text(format!(
                    "Copy this pose's {side} joints to the other side, mirrored ({name} → {other})"
                ))
                .clicked()
            {
                draft.mirror(rig);
            }
        }
        if existing.is_some()
            && ui
                .button("Leave to the body")
                .on_hover_text(
                    "Stop setting this joint; it keeps what walking and constraints give it",
                )
                .clicked()
        {
            draft.pose_mut().joints.retain(|j| j.joint != name);
        }
    });
}

fn keys_section(ui: &mut Ui, draft: &mut PoseDraft) {
    let Some(sequence) = draft.current_sequence() else {
        return;
    };
    let poses: Vec<String> = draft.library().poses.keys().cloned().collect();
    let mut edited = sequence.clone();
    let count = edited.keys.len();
    let mut remove = None;
    let mut swap = None;
    let mut selected = draft.key;
    let length = draft.length(&sequence);
    for (index, key) in edited.keys.iter_mut().enumerate() {
        let low = if index == 0 {
            0.0
        } else {
            sequence.keys[index - 1].at
        };
        let high = sequence.keys.get(index + 1).map_or(1.0, |key| key.at);
        ui.horizontal(|ui| {
            if ui
                .selectable_label(draft.key == Some(index), format!("{}", index + 1))
                .on_hover_text("Select this key to edit its pose's joints")
                .clicked()
            {
                selected = Some(index);
            }
            let before = key.pose.clone();
            egui::ComboBox::from_id_salt(("key_pose", index))
                .selected_text(&key.pose)
                .width(130.0)
                .show_ui(ui, |ui| {
                    for name in &poses {
                        ui.selectable_value(&mut key.pose, name.clone(), name);
                    }
                });
            let mut seconds = key.at * length;
            if ui
                .add(
                    egui::DragValue::new(&mut seconds)
                        .speed(0.005)
                        .range(low * length..=high * length)
                        .suffix(" s"),
                )
                .changed()
            {
                key.at = seconds / length;
                selected = Some(index);
            }
            if before != key.pose {
                selected = Some(index);
            }
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
        selected = selected.map(|key| match key {
            key if key == index => index + 1,
            key if key == index + 1 => index,
            key => key,
        });
    }
    if let Some(index) = remove {
        edited.keys.remove(index);
        selected = Some(index.min(edited.keys.len() - 1));
    }
    let changed = edited != sequence;
    if changed {
        *draft.sequence_mut() = edited;
    }
    if (changed || selected != draft.key)
        && let Some(index) = selected
    {
        draft.select_key(index);
    }
    muted(
        ui,
        "Add poses beneath the timeline. Times are when each pose is reached; transitions are eased between keys.",
    );
}

fn settings_section(ui: &mut Ui, draft: &mut PoseDraft) {
    let Some(sequence) = draft.current_sequence() else {
        return;
    };
    let mut edited = sequence.clone();
    ui.checkbox(&mut edited.looping, "Loop").on_hover_text(
        "Repeat while the state playing it holds; one-shots last their move's duration",
    );
    if edited.looping {
        ui.checkbox(&mut edited.gait, "Paced by the legs").on_hover_text(
            "One cycle per two steps in game: 0 when the left foot lifts, 0.5 when the right one does",
        );
    } else {
        edited.gait = false;
    }
    ui.horizontal(|ui| {
        ui.add(
            egui::DragValue::new(&mut edited.seconds)
                .speed(0.01)
                .range(0.05..=30.0)
                .suffix(" s"),
        );
        ui.label(if edited.gait {
            "per cycle in this preview"
        } else if edited.looping {
            "per cycle"
        } else {
            "preview length without a move"
        });
    });
    ui.add(egui::Slider::new(&mut edited.takeover, 0.0..=1.0).text("Override walking"))
        .on_hover_text("1 replaces walking, feet and constraints; 0 leaves the legs walking");
    if !edited.looping {
        ui.add(egui::Slider::new(&mut edited.fade_in, 0.0..=1.0).text("fade in"));
        ui.add(egui::Slider::new(&mut edited.fade_out, 0.0..=1.0).text("fade out"));
    }
    ui.label(theme::section("Gameplay events"));
    muted(
        ui,
        "Markers signal game logic; they do not move the model. Attack reads strike to time its hit. Other names need a handler in game code.",
    );
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
                egui::Button::new("Add event at playhead"),
            )
            .on_hover_text("Simulation acts on named events, such as `strike` for a hit")
            .clicked()
        {
            edited.events.insert(name, draft.phase());
            draft.new_event.clear();
        }
    });
    let mut dropped = None;
    for name in edited.events.keys() {
        ui.horizontal(|ui| {
            ui.colored_label(theme::WARD, name);
            if ui
                .small_button("×")
                .on_hover_text("Remove the event")
                .clicked()
            {
                dropped = Some(name.clone());
            }
        });
    }
    if let Some(name) = dropped {
        edited.events.remove(&name);
    }
    ui.label(theme::section("Procedural pelvis turn"));
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

    fn inspect(editor: &Editor) -> DefinitionInspection {
        editor
            .project
            .as_ref()
            .unwrap()
            .inspect_definition("characters/player")
            .unwrap()
    }

    #[test]
    fn playback_and_scrubbing_ignore_an_unrelated_library_pose() {
        let rig = struction_anim::humanoid::rig();
        let mut draft = PoseDraft {
            defaults: struction_anim::humanoid::base_poses(),
            pose: "idle".into(),
            sequence: "swing".into(),
            ..default()
        };
        draft.toggle_play();
        draft.advance(0.3);
        assert!(!draft.shows_pose());
        let before = draft.preview_pose(&rig);
        draft.advance(0.25);
        let after = draft.preview_pose(&rig);
        let hand = rig.skeleton.joint_id("upper_arm_r").unwrap();
        assert_ne!(before.locals[hand].rotation, after.locals[hand].rotation);

        draft.select_pose("seated".into());
        assert!(draft.shows_pose());
        assert!(!draft.playback.playing);
        draft.toggle_play();
        assert!(
            !draft.shows_pose(),
            "Play must always preview the animation"
        );
        draft.select_sequence("swing".into());
        assert_eq!(draft.pose, "swing_raise");
        assert_eq!(draft.key, Some(0));
    }

    #[test]
    fn timeline_buttons_add_and_remove_the_chosen_pose_without_removing_it_from_the_library() {
        fn frame(
            ctx: &egui::Context,
            draft: &mut PoseDraft,
            events: Vec<egui::Event>,
        ) -> egui::FullOutput {
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 400.0),
                    )),
                    events,
                    ..default()
                },
                |ui| timeline(ui, draft),
            );
            output.textures_delta.clear();
            output
        }
        fn position(shape: &egui::epaint::Shape, label: &str) -> Option<egui::Pos2> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                egui::epaint::Shape::Vec(shapes) => {
                    shapes.iter().find_map(|shape| position(shape, label))
                }
                _ => None,
            }
        }
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        let mut draft = PoseDraft {
            defaults: struction_anim::humanoid::base_poses(),
            insert_pose: "aim".into(),
            ..default()
        };
        draft.select_sequence("swing".into());
        draft.playback.time = 0.4;
        frame(&ctx, &mut draft, vec![]);
        for label in ["Add pose at playhead", "Remove selected key"] {
            let output = frame(&ctx, &mut draft, vec![]);
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| position(&shape.shape, label))
                .unwrap();
            frame(&ctx, &mut draft, vec![egui::Event::PointerMoved(pos)]);
            for pressed in [true, false] {
                frame(
                    &ctx,
                    &mut draft,
                    vec![egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    }],
                );
            }
            if label == "Add pose at playhead" {
                assert_eq!(draft.current_sequence().unwrap().keys.len(), 3);
                assert_eq!(draft.pose, "aim");
                assert_eq!(draft.key, Some(1));
            }
        }
        assert_eq!(draft.current_sequence().unwrap().keys.len(), 2);
        assert!(draft.library().poses.contains_key("aim"));
    }

    #[test]
    fn added_and_replaced_pose_keys_save_assign_and_undo() {
        let (dir, mut editor) = open_playground();
        let source = "// Keep these animation notes.\n{}\n";
        let file = dir.path().join("characters/player/entity.jsonc");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, source).unwrap();
        editor.apply(Command::Refresh);
        let mut draft = player(&editor);
        draft.select_sequence("swing".into());
        draft.create_sequence("test_swing".into(), true);
        draft.select_pose("aim".into());
        assert_eq!(draft.current_sequence().unwrap().keys[0].pose, "aim");
        assert_eq!(
            draft.defaults.sequences["swing"].keys[0].pose,
            "swing_raise"
        );
        draft.playback.time = 0.4;
        draft.add_key("fist".into());
        assert_eq!(draft.key, Some(1));
        assert_eq!(draft.pose, "fist");
        assert_eq!(draft.current_sequence().unwrap().keys.len(), 3);
        draft.remove_key(2);
        draft.assign_move(&mut editor, "Attack").unwrap();
        assert_eq!(
            sequence_players(&inspect(&editor), "test_swing")[0].label,
            "Attack.sequence"
        );
        assert_eq!(draft.targets.sequences["test_swing"].keys.len(), 2);

        editor.apply(Command::StartPlay);
        editor.project.as_mut().unwrap().step_play(2).unwrap();
        let world = editor.project.as_ref().unwrap().play_world().unwrap();
        assert!(world.iter_entities().any(|entity| {
            entity
                .get::<struction_character::Attack>()
                .is_some_and(|attack| attack.sequence == "test_swing")
        }));
        assert!(world.iter_entities().any(|entity| {
            entity
                .get::<struction_anim::base_pose::RigPoseSet>()
                .is_some_and(|set| set.0.sequences["test_swing"].keys[0].pose == "aim")
        }));
        editor.apply(Command::StopPlay);
        editor.apply(Command::Undo);
        assert_eq!(
            sequence_players(&inspect(&editor), "swing")[0].label,
            "Attack.sequence"
        );
        editor.apply(Command::Undo);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), source);

        draft.select_sequence("swing".into());
        draft.create_sequence("one_pose".into(), false);
        assert_eq!(draft.current_sequence().unwrap().keys.len(), 1);
        assert!(draft.current_sequence().unwrap().events.is_empty());
        draft.remove_key(0);
        assert_eq!(draft.current_sequence().unwrap().keys.len(), 1);
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
        assert_eq!(draft.applied, ["Pose targets for characters/player"]);
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
        let definition = inspect(&editor);
        let swing = sequence_players(&definition, "swing");
        assert_eq!(swing.len(), 1);
        assert_eq!(swing[0].label, "Attack.sequence");
        assert_eq!(swing[0].component.as_deref(), Some("Attack"));
        assert_eq!(swing[0].duration, Some(0.5));
        assert_eq!(
            sequence_players(&definition, "roll")[0].label,
            "Roll.sequence"
        );

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
        draft.sequence = "march".into();
        draft.playback.playing = true;
        draft.advance(0.45);
        assert!((draft.phase() - 0.5).abs() < 1e-4);
        draft.advance(0.6);
        assert!(draft.phase() < 0.2);

        // A state rule plays it while walking; the data layer accepts the rule.
        draft.assign_state(&mut editor, "Walking").unwrap();
        assert_eq!(
            sequence_players(&inspect(&editor), "march")[0].label,
            "While Walking (PlaySequence)"
        );
        editor.apply(Command::Undo);
        assert_eq!(
            sequence_players(&inspect(&editor), "arm_swing")[0].label,
            "While Walking (PlaySequence)"
        );

        let invalid = draft.targets.sequences.get_mut("march").unwrap();
        invalid.keys[1].pose = "nope".into();
        let error = draft.check(&editor).unwrap_err();
        assert!(error.contains("march") && error.contains("nope"), "{error}");
    }

    /// Selecting a key edits its pose at the move's real timing, and the move's duration can be
    /// changed from the tool as its own undoable edit.
    #[test]
    fn keys_are_edited_in_place_at_the_moves_timing() {
        let (dir, mut editor) = open_playground();
        let mut draft = player(&editor);
        draft.sequence = "swing".into();
        draft.players = sequence_players(&inspect(&editor), "swing");
        let swing = draft.current_sequence().unwrap();
        assert_eq!(draft.length(&swing), 0.5);
        draft.select_key(1);
        assert_eq!(draft.pose, "swing_strike");
        assert!((draft.playback.time - 0.55 * 0.5).abs() < 1e-6);
        assert!(
            !draft.shows_pose(),
            "the strike is fully faded in at its key"
        );
        draft.step_key(false);
        assert_eq!(draft.key, Some(0));
        assert_eq!(draft.pose, "swing_raise");

        // The roll's key sits where it is still fading in: shown alone.
        draft.sequence = "roll".into();
        draft.select_key(0);
        assert!(draft.shows_pose());

        draft.set_duration(&mut editor, "Attack", 0.8).unwrap();
        let text =
            std::fs::read_to_string(dir.path().join("characters/player/entity.jsonc")).unwrap();
        assert!(text.contains("0.8"), "{text}");
        assert_eq!(draft.applied, ["Attack.duration for characters/player"]);
        assert_eq!(
            sequence_players(&inspect(&editor), "swing")[0].duration,
            Some(0.8)
        );
    }

    /// Mirroring the left half of the library's symmetric poses reproduces their right half.
    #[test]
    fn mirroring_matches_the_authored_symmetric_poses() {
        let rig = struction_anim::humanoid::rig();
        let library = struction_anim::humanoid::base_poses();
        for name in ["idle", "grip", "fist", "roll", "seated"] {
            let authored = &library.poses[name];
            for joint in authored.joints.iter().filter(|j| j.joint.ends_with("_l")) {
                let mirrored = mirror_joint(joint).unwrap();
                let right = authored
                    .joints
                    .iter()
                    .find(|j| j.joint == mirrored.joint)
                    .unwrap();
                assert_eq!(&mirrored, right, "{name}");
            }
        }
        let mut draft = PoseDraft {
            defaults: library,
            pose: "aim".into(),
            joint: rig.skeleton.joint_id("upper_arm_l").unwrap(),
            ..default()
        };
        assert_eq!(draft.mirror(&rig), 2);
        let aim = &draft.targets.poses["aim"];
        let right = aim
            .joints
            .iter()
            .find(|j| j.joint == "upper_arm_r")
            .unwrap();
        assert_eq!(right.euler_deg, Some(Vec3::new(62.0, 0.0, 14.0)));
    }

    #[test]
    fn joints_turn_about_their_own_axes_and_the_chain_can_be_walked() {
        let rig = struction_anim::humanoid::rig();
        let mut draft = PoseDraft {
            defaults: struction_anim::humanoid::base_poses(),
            pose: "idle".into(),
            joint: rig.skeleton.joint_id("forearm_l").unwrap(),
            ..default()
        };
        draft.rotate_joint(&rig, draft.joint, 0, 10f32.to_radians());
        let forearm = draft.targets.poses["idle"]
            .joints
            .iter()
            .find(|j| j.joint == "forearm_l")
            .unwrap();
        assert!(
            forearm
                .euler_deg
                .unwrap()
                .abs_diff_eq(Vec3::new(28.0, 0.0, 0.0), 1e-3)
        );

        draft.step_joint(&rig, Step::Up);
        assert_eq!(rig.skeleton.joints()[draft.joint].name, "upper_arm_l");
        draft.step_joint(&rig, Step::Across);
        assert_eq!(rig.skeleton.joints()[draft.joint].name, "upper_arm_r");
        draft.step_joint(&rig, Step::Down);
        assert_eq!(rig.skeleton.joints()[draft.joint].name, "forearm_r");
    }

    /// Turning counterclockwise on screen about an axis facing the viewer is a positive turn.
    #[test]
    fn ring_drags_turn_with_the_cursor() {
        let center = Vec2::new(100.0, 100.0);
        let (right, up) = (Vec2::new(110.0, 100.0), Vec2::new(100.0, 90.0));
        let quarter = std::f32::consts::FRAC_PI_2;
        assert!((ring_drag(right, up, center, true) - quarter).abs() < 1e-5);
        assert!((ring_drag(right, up, center, false) + quarter).abs() < 1e-5);
        // Across the ±180° seam the short way round is taken.
        let (left_above, left_below) = (Vec2::new(90.0, 99.0), Vec2::new(90.0, 101.0));
        assert!(ring_drag(left_above, left_below, center, true).abs() < 0.3);
    }

    #[test]
    fn clicking_picks_the_part_under_the_cursor_or_the_nearest_joint() {
        let rig = struction_anim::humanoid::rig();
        let draft = PoseDraft::default();
        let transforms = draft.transforms(&rig);
        // A small quad around each of two joints' origins, facing +Z.
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
        let pick = |ray| pick(ray, &bundle, &parts, &transforms, 0.05);
        assert_eq!(pick(toward(head, Vec3::ZERO)), Some(head));
        assert_eq!(pick(toward(hand, Vec3::ZERO)), Some(hand));
        // Off every part: the nearest joint within reach, else nothing.
        let chest = rig.skeleton.joint_id("chest").unwrap();
        assert_eq!(pick(toward(chest, Vec3::X * 0.01)), Some(chest));
        assert_eq!(pick(toward(chest, Vec3::X * 0.5)), None);
        let placed = [(0, transforms[head]), (1, transforms[hand])];
        assert_eq!(
            pick_part(toward(hand, Vec3::ZERO), &bundle, &placed),
            Some(1)
        );
    }
}
