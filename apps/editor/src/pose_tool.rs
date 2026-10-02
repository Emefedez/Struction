//! Edits the same sparse joint targets consumed by the runtime animation solvers.
use crate::{
    state::{Command, Editor},
    theme,
};
use bevy::prelude::*;
use bevy_egui::egui::{self, Ui};
use struction_anim::{
    base_pose::{BasePose, BasePoseSet, JointPose, PoseTargets},
    rig::Rig,
};
use struction_editor::{EditRequest, Field};

#[derive(Clone, Debug, PartialEq)]
pub struct PoseDraft {
    pub definition: String,
    pub rig_name: String,
    pub name: String,
    pub joint: usize,
    pub targets: PoseTargets,
    saved: PoseTargets,
    pub revision: u64,
}
impl Default for PoseDraft {
    fn default() -> Self {
        Self {
            definition: String::new(),
            rig_name: "humanoid".into(),
            name: "idle".into(),
            joint: 0,
            targets: default(),
            saved: default(),
            revision: 0,
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
    pub fn poses(&self) -> BasePoseSet {
        let mut poses = struction_anim::humanoid::base_poses();
        poses.poses.extend(self.targets.poses.clone());
        poses
    }
    fn load(&mut self, editor: &Editor) {
        let Some(project) = &editor.project else {
            return;
        };
        if let Ok(definition) = project.inspect_definition(&self.definition) {
            self.targets = definition
                .components
                .iter()
                .find(|(k, _)| k.ends_with("::PoseTargets"))
                .and_then(|(_, v)| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default();
            self.saved = self.targets.clone();
            self.revision += 1;
        }
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
        let targets: PoseTargets = current
            .components
            .iter()
            .find(|(key, _)| key.ends_with("::PoseTargets"))
            .and_then(|(_, value)| serde_json::from_value(value.clone()).ok())
            .unwrap_or_default();
        if targets != self.saved {
            return Err(
                "Pose targets changed outside this draft. Revert to reload them before applying."
                    .into(),
            );
        }
        let rig = self.rig(editor).ok_or("Unknown skeleton")?;
        self.targets
            .resolve(&struction_anim::humanoid::base_poses(), &rig.skeleton)
            .map_err(|e| e.to_string())?;
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
    pub fn transforms(&self, rig: &Rig) -> Vec<Transform> {
        let mut pose = rig.skeleton.rest_pose();
        if let Ok(target) = self.poses().resolve(&self.name, &rig.skeleton) {
            target.apply(&mut pose, 1.0, None);
        }
        rig.skeleton.model_transforms(&pose)
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
                (
                    name,
                    shape
                        .get("rig")
                        .and_then(|v| v.as_str())
                        .unwrap_or("humanoid")
                        .to_owned(),
                )
            })
        })
        .collect();
    if choices.is_empty() {
        ui.label("No rigged definition uses this model. Assign it to a rigged actor before editing its state targets.");
        return;
    }
    if draft.definition.is_empty() {
        draft.definition = choices[0].0.clone();
        draft.rig_name = choices[0].1.clone();
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
    ui.label(
        egui::RichText::new("Saved on this definition; inherited by its descendants.")
            .small()
            .color(theme::MUTED),
    );
    let poses = draft.poses();
    egui::ComboBox::from_label("Target pose")
        .selected_text(&draft.name)
        .show_ui(ui, |ui| {
            for name in poses.poses.keys() {
                ui.selectable_value(&mut draft.name, name.clone(), name);
            }
        });
    let relation = match draft.name.as_str() {
        "idle" => "Locomotion base · standing and moving",
        "roll" => "Rolling · dodge extensor · tucked target",
        "swing_raise" => "Attacking · combat extensor · wind-up target",
        "swing_strike" => "Attacking · combat extensor · strike target",
        "seated" => "Sit constraint · seated target",
        "grip" | "fist" => "Hand / grip constraint target",
        _ => "Named animation target",
    };
    ui.colored_label(theme::WARD, relation);
    ui.label("The view shows this target pose. Runtime blends it with locomotion and constraints.");
    let Some(rig) = draft.rig(editor) else {
        ui.label("Unknown skeleton");
        return;
    };
    let joints = rig.skeleton.joints();
    draft.joint = draft.joint.min(joints.len().saturating_sub(1));
    egui::ComboBox::from_label("Joint")
        .selected_text(&joints[draft.joint].name)
        .show_ui(ui, |ui| {
            for (index, joint) in joints.iter().enumerate() {
                ui.selectable_value(&mut draft.joint, index, &joint.name);
            }
        });
    let name = &joints[draft.joint].name;
    let pose = poses.poses.get(&draft.name).cloned().unwrap_or_default();
    let existing = pose.joints.iter().find(|j| &j.joint == name);
    let rest = joints[draft.joint].rest;
    let (x, y, z) = rest.rotation.to_euler(EulerRot::XYZ);
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
        let target: &mut BasePose = draft
            .targets
            .poses
            .entry(draft.name.clone())
            .or_insert(pose);
        if let Some(joint) = target.joints.iter_mut().find(|j| &j.joint == name) {
            joint.euler_deg = Some(angles);
        } else {
            target.joints.push(JointPose::rotation(name, angles));
        }
        draft.revision += 1;
    }
    if ui.button("Restore default target").clicked() {
        draft.targets.poses.remove(&draft.name);
        draft.revision += 1;
    }
    ui.separator();
    ui.add_enabled_ui(!editor.playing(), |ui| {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(draft.dirty(), egui::Button::new("Apply pose targets"))
                .clicked()
                && let Err(error) = draft.apply(editor)
            {
                ui.colored_label(theme::AXES[0], error);
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn targets_save_as_project_overrides_and_drive_the_play_rig() {
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
        let mut draft = PoseDraft {
            definition: "characters/player".into(),
            name: "roll".into(),
            ..default()
        };
        draft.load(&editor);
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
}
