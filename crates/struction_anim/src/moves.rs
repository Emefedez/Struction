//! Procedural moves driven by simulation phase, never root motion: a full-body roll and an
//! upper-body swing.

use bevy::prelude::*;

use crate::{base_pose::BasePoseSet, error::AnimError, pose::Pose, rig::Rig};

/// Phase at which a swing's strike passes in front of the body. Simulation lands the hit here.
pub const SWING_STRIKE: f32 = 0.45;

#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MoveKind {
    /// A tumble around the pelvis using the `roll` base pose. Replaces locomotion while it lasts.
    #[default]
    Roll,
    /// The right arm raises (`swing_raise`) and strikes (`swing_strike`); the legs keep walking.
    Swing,
}

#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct MovePose {
    pub kind: MoveKind,
    pub phase: f32,
    /// Blend out an interrupted move without snapping back to standing.
    pub weight: f32,
    /// World-space travel or strike direction; the rig may still be turning toward it.
    pub direction: Vec3,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl MovePose {
    pub fn blend_weight(&self) -> f32 {
        let (fade_in, fade_out) = match self.kind {
            MoveKind::Roll => (0.15, 0.2),
            MoveKind::Swing => (0.1, 0.35),
        };
        self.weight.clamp(0.0, 1.0)
            * smooth(self.phase / fade_in)
            * smooth((1.0 - self.phase) / fade_out)
    }

    /// How much of locomotion, feet and constraints the move suppresses.
    pub fn takeover(&self) -> f32 {
        match self.kind {
            MoveKind::Roll => self.blend_weight(),
            MoveKind::Swing => 0.0,
        }
    }

    pub(crate) fn apply(
        &self,
        rig: &Rig,
        poses: &BasePoseSet,
        root: Transform,
        up: Vec3,
        pose: &mut Pose,
    ) -> Result<(), AnimError> {
        let weight = self.blend_weight();
        if weight <= 0.0 {
            return Ok(());
        }
        let target = match self.kind {
            MoveKind::Roll => self.tumble(rig, poses, root, up)?,
            MoveKind::Swing => {
                let mut swing = pose.clone();
                poses
                    .resolve("swing_raise", &rig.skeleton)?
                    .apply(&mut swing, 1.0, None);
                let strike = smooth((self.phase - (SWING_STRIKE - 0.15)) / 0.25);
                poses
                    .resolve("swing_strike", &rig.skeleton)?
                    .apply(&mut swing, strike, None);
                swing
            }
        };
        pose.blend_in_place(&target, weight, None);
        Ok(())
    }

    fn tumble(
        &self,
        rig: &Rig,
        poses: &BasePoseSet,
        root: Transform,
        up: Vec3,
    ) -> Result<Pose, AnimError> {
        let mut tucked = rig.skeleton.rest_pose();
        poses
            .resolve("roll", &rig.skeleton)?
            .apply(&mut tucked, 1.0, None);
        let up = up.normalize_or(Vec3::Y);
        let direction = self.direction - up * self.direction.dot(up);
        let axis = root.rotation.inverse() * up.cross(direction).normalize_or(Vec3::NEG_X);
        let angle = core::f32::consts::TAU * smooth((self.phase - 0.15) / 0.65);
        // The pelvis is the pivot, not the feet or the physics capsule's origin.
        let pelvis = &mut tucked.locals[rig.pelvis];
        pelvis.translation *= 0.62;
        pelvis.rotation = Quat::from_axis_angle(axis.normalize(), angle) * pelvis.rotation;
        Ok(tucked)
    }
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
        for kind in [MoveKind::Roll, MoveKind::Swing] {
            for phase in [0.0, 1.0] {
                let mut result = standing();
                MovePose {
                    kind,
                    phase,
                    weight: 1.0,
                    direction: Vec3::NEG_Z,
                }
                .apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut result)
                .unwrap();
                assert_eq!(result, standing(), "{kind:?} at {phase}");
            }
        }
    }

    #[test]
    fn a_swing_moves_the_arm_and_leaves_the_legs() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        let mut result = standing();
        let swing = MovePose {
            kind: MoveKind::Swing,
            phase: SWING_STRIKE,
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
        assert_eq!(swing.takeover(), 0.0);
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
                kind: MoveKind::Roll,
                phase,
                weight: 1.0,
                direction: Vec3::X,
            };
            roll.apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut flat)
                .unwrap();
            MovePose {
                direction: rotation * roll.direction,
                ..roll
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
}
