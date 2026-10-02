//! Moves and states play authored [`PoseSequence`]s, driven by simulation phase and never by
//! root motion: a one-shot [`MovePose`] (a roll, a swing) over a looping [`LoopPose`] (a walk
//! cycle).

use bevy::prelude::*;

use crate::{
    base_pose::BasePoseSet,
    error::AnimError,
    pose::Pose,
    rig::Rig,
    sequence::{LOOP_BLEND, PoseSequence},
};

/// The one-shot sequence a move plays, at the move's phase.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct MovePose {
    /// Name in the rig's library, such as `roll` or `swing`.
    pub sequence: String,
    pub phase: f32,
    /// Blend out an interrupted move without snapping back to standing.
    pub weight: f32,
    /// World-space travel or strike direction; the rig may still be turning toward it.
    pub direction: Vec3,
}

/// On a body: the looping sequence it plays while it has this component, typically enabled by a
/// definition's `states` section (`"Walking": { "enable": { "PlaySequence": { "sequence":
/// "arm_swing" } } }`), so leaving the state interrupts it.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct PlaySequence {
    pub sequence: String,
}

/// On a rig: the looping sequence under any move, blended in and out over [`LOOP_BLEND`].
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct LoopPose {
    pub sequence: String,
    /// Seconds since it started.
    pub time: f32,
    pub weight: f32,
}

impl LoopPose {
    /// Follows the body's [`PlaySequence`]: a new sequence starts once the old one blended out.
    pub fn follow(&mut self, wanted: Option<&str>, dt: f32) {
        let step = dt / LOOP_BLEND;
        self.time += dt;
        match wanted {
            Some(wanted) if wanted == self.sequence => self.weight = (self.weight + step).min(1.0),
            Some(wanted) if self.weight <= 0.0 => {
                self.sequence = wanted.to_owned();
                self.time = 0.0;
                self.weight = step.min(1.0);
            }
            _ => self.weight = (self.weight - step).max(0.0),
        }
    }

    fn sequence<'a>(&self, poses: &'a BasePoseSet) -> Option<&'a PoseSequence> {
        poses.sequences.get(&self.sequence)
    }

    pub fn takeover(&self, poses: &BasePoseSet) -> f32 {
        self.sequence(poses).map_or(0.0, |sequence| {
            sequence.takeover * self.weight.clamp(0.0, 1.0)
        })
    }

    pub(crate) fn apply(
        &self,
        rig: &Rig,
        poses: &BasePoseSet,
        root: Transform,
        up: Vec3,
        pose: &mut Pose,
    ) -> Result<(), AnimError> {
        if self.weight <= 0.0 {
            return Ok(());
        }
        let sequence = poses.sequence(&self.sequence)?;
        let phase = sequence.phase_at(self.time);
        let weight = self.weight.clamp(0.0, 1.0);
        play(
            sequence,
            phase,
            weight,
            Vec3::ZERO,
            rig,
            poses,
            root,
            up,
            pose,
        )
    }
}

impl MovePose {
    pub fn sequence<'a>(&self, poses: &'a BasePoseSet) -> Option<&'a PoseSequence> {
        poses.sequences.get(&self.sequence)
    }

    pub fn blend_weight(&self, poses: &BasePoseSet) -> f32 {
        self.sequence(poses).map_or(0.0, |sequence| {
            self.weight.clamp(0.0, 1.0) * sequence.envelope(self.phase)
        })
    }

    /// How much of locomotion, feet and constraints the move suppresses.
    pub fn takeover(&self, poses: &BasePoseSet) -> f32 {
        self.sequence(poses)
            .map_or(0.0, |sequence| sequence.takeover * self.blend_weight(poses))
    }

    pub(crate) fn apply(
        &self,
        rig: &Rig,
        poses: &BasePoseSet,
        root: Transform,
        up: Vec3,
        pose: &mut Pose,
    ) -> Result<(), AnimError> {
        if self.weight <= 0.0 {
            return Ok(());
        }
        let sequence = poses.sequence(&self.sequence)?;
        let weight = self.blend_weight(poses);
        play(
            sequence,
            self.phase,
            weight,
            self.direction,
            rig,
            poses,
            root,
            up,
            pose,
        )
    }
}

/// Blends `sequence` at `phase` into `pose` by `weight`. Keys start from the incoming pose,
/// moved toward rest by the sequence's takeover.
#[allow(clippy::too_many_arguments)]
pub fn play(
    sequence: &PoseSequence,
    phase: f32,
    weight: f32,
    direction: Vec3,
    rig: &Rig,
    poses: &BasePoseSet,
    root: Transform,
    up: Vec3,
    pose: &mut Pose,
) -> Result<(), AnimError> {
    if weight <= 0.0 {
        return Ok(());
    }
    let mut target = pose.clone();
    if sequence.takeover > 0.0 {
        target.blend_in_place(&rig.skeleton.rest_pose(), sequence.takeover, None);
    }
    for (key, key_weight) in sequence.blend(phase) {
        poses
            .resolve(&sequence.keys[key].pose, &rig.skeleton)?
            .apply(&mut target, key_weight, None);
    }
    if let Some((angle, sink)) = sequence.tumble_at(phase) {
        let up = up.normalize_or(Vec3::Y);
        let direction = direction - up * direction.dot(up);
        let axis = root.rotation.inverse() * up.cross(direction).normalize_or(Vec3::NEG_X);
        // The pelvis is the pivot, not the feet or the physics capsule's origin.
        let pelvis = &mut target.locals[rig.pelvis];
        pelvis.translation *= sink;
        pelvis.rotation = Quat::from_axis_angle(axis.normalize(), angle) * pelvis.rotation;
    }
    pose.blend_in_place(&target, weight, None);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid;

    fn standing() -> Pose {
        let rig = humanoid::rig();
        let mut standing = rig.skeleton.rest_pose();
        humanoid::base_poses()
            .resolve("idle", &rig.skeleton)
            .unwrap()
            .apply(&mut standing, 1.0, None);
        standing
    }

    #[test]
    fn endpoints_leave_the_existing_pose_unchanged() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        for sequence in ["roll", "swing"] {
            for phase in [0.0, 1.0] {
                let mut result = standing();
                MovePose {
                    sequence: sequence.into(),
                    phase,
                    weight: 1.0,
                    direction: Vec3::NEG_Z,
                }
                .apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut result)
                .unwrap();
                assert_eq!(result, standing(), "{sequence} at {phase}");
            }
        }
    }

    #[test]
    fn a_swing_moves_the_arm_and_leaves_the_legs() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        let mut result = standing();
        let swing = MovePose {
            sequence: "swing".into(),
            phase: poses.sequences["swing"].event("strike").unwrap(),
            weight: 1.0,
            direction: Vec3::NEG_Z,
        };
        swing
            .apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut result)
            .unwrap();
        let joint = |name| rig.skeleton.joint_id(name).unwrap();
        let before = standing();
        let arm = joint("upper_arm_r");
        assert!(
            result.locals[arm]
                .rotation
                .angle_between(before.locals[arm].rotation)
                > 0.5
        );
        for leg in ["thigh_l", "thigh_r", "shin_l", "shin_r", "hips"] {
            assert_eq!(
                result.locals[joint(leg)],
                before.locals[joint(leg)],
                "{leg}"
            );
        }
        assert_eq!(swing.takeover(&poses), 0.0);
    }

    #[test]
    fn tumbling_is_relative_to_local_gravity() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        let rotation = Quat::from_rotation_z(1.2);
        for phase in [0.1, 0.3, 0.5, 0.7, 0.9] {
            let mut flat = rig.skeleton.rest_pose();
            let mut tilted = flat.clone();
            let roll = MovePose {
                sequence: "roll".into(),
                phase,
                weight: 1.0,
                direction: Vec3::X,
            };
            roll.apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut flat)
                .unwrap();
            MovePose {
                direction: rotation * roll.direction,
                ..roll.clone()
            }
            .apply(
                &rig,
                &poses,
                Transform::from_rotation(rotation),
                rotation * Vec3::Y,
                &mut tilted,
            )
            .unwrap();
            for (a, b) in flat.locals.iter().zip(&tilted.locals) {
                assert!(a.translation.abs_diff_eq(b.translation, 1e-5));
                assert!(a.rotation.abs_diff_eq(b.rotation, 1e-5));
            }
        }
    }

    /// The swing data reproduces the code it replaced: wind-up held, strike eased in from 30%.
    #[test]
    fn the_authored_swing_matches_the_former_procedural_one() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        for phase in [0.2, 0.35, 0.45, 0.6] {
            let mut authored = standing();
            let swing = MovePose {
                sequence: "swing".into(),
                phase,
                weight: 1.0,
                direction: Vec3::NEG_Z,
            };
            swing
                .apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut authored)
                .unwrap();
            let mut target = standing();
            poses
                .resolve("swing_raise", &rig.skeleton)
                .unwrap()
                .apply(&mut target, 1.0, None);
            let strike = crate::sequence::smooth((phase - 0.30) / 0.25);
            poses
                .resolve("swing_strike", &rig.skeleton)
                .unwrap()
                .apply(&mut target, strike, None);
            let mut former = standing();
            former.blend_in_place(&target, swing.blend_weight(&poses), None);
            for (a, b) in authored.locals.iter().zip(&former.locals) {
                assert!(a.rotation.abs_diff_eq(b.rotation, 1e-5), "at {phase}");
            }
        }
    }

    #[test]
    fn a_loop_cycles_its_keys_and_blends_out_when_its_state_ends() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        let arm = rig.skeleton.joint_id("upper_arm_l").unwrap();
        let mut looping = LoopPose::default();
        let mut angles = Vec::new();
        for _ in 0..66 {
            looping.follow(Some("arm_swing"), 1.0 / 60.0);
            let mut pose = standing();
            looping
                .apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut pose)
                .unwrap();
            angles.push(pose.locals[arm].rotation.to_euler(EulerRot::XYZ).0);
        }
        assert_eq!(looping.weight, 1.0);
        // Forward (left key), then back (right key), within one 1.1 s cycle.
        let max = angles.iter().copied().fold(f32::MIN, f32::max);
        let min = angles.iter().copied().fold(f32::MAX, f32::min);
        assert!(max > 0.3 && min < -0.2, "{min}..{max}");
        // Twelve frames make 0.2 s, give or take rounding.
        for _ in 0..13 {
            looping.follow(None, 1.0 / 60.0);
        }
        assert_eq!(looping.weight, 0.0);
        let mut pose = standing();
        looping
            .apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut pose)
            .unwrap();
        assert_eq!(pose, standing());
    }

    #[test]
    fn actor_overrides_replace_sequences_and_are_checked() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        let mut targets = crate::base_pose::PoseTargets::default();
        targets.sequences.insert(
            "swing".into(),
            PoseSequence {
                keys: vec![crate::sequence::SequenceKey::new("aim", 0.0)],
                ..Default::default()
            },
        );
        let merged = targets.resolve(&poses, &rig.skeleton).unwrap();
        assert_eq!(merged.sequences["swing"].keys[0].pose, "aim");
        assert_eq!(merged.sequences["roll"], poses.sequences["roll"]);
        targets.sequences.get_mut("swing").unwrap().keys[0].pose = "nope".into();
        assert!(targets.resolve(&poses, &rig.skeleton).is_err());
    }
}
