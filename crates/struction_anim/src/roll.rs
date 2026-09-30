//! A full-body procedural tumble driven by simulation phase, never root motion.

use bevy::prelude::*;

use crate::{base_pose::BasePoseSet, error::AnimError, pose::Pose, rig::Rig};

#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct RollPose {
    pub phase: f32,
    /// Blend out a cancelled roll without snapping back to standing.
    pub weight: f32,
    /// World-space travel direction; the rig may still be turning toward it.
    pub direction: Vec3,
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl RollPose {
    pub fn blend_weight(&self) -> f32 {
        self.weight.clamp(0.0, 1.0) * smooth(self.phase / 0.15) * smooth((1.0 - self.phase) / 0.2)
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
        pose.blend_in_place(&tucked, weight, None);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid;

    #[test]
    fn endpoints_leave_the_existing_pose_unchanged() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        let mut standing = rig.skeleton.rest_pose();
        poses
            .resolve("idle", &rig.skeleton)
            .unwrap()
            .apply(&mut standing, 1.0, None);
        for phase in [0.0, 1.0] {
            let mut result = standing.clone();
            RollPose {
                phase,
                weight: 1.0,
                direction: Vec3::NEG_Z,
            }
            .apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut result)
            .unwrap();
            assert_eq!(result, standing);
        }
    }

    #[test]
    fn tumbling_is_relative_to_local_gravity() {
        let rig = humanoid::rig();
        let poses = humanoid::base_poses();
        let rotation = Quat::from_rotation_z(1.2);
        for phase in [0.1, 0.3, 0.5, 0.7, 0.9] {
            let mut flat = rig.skeleton.rest_pose();
            let mut tilted = flat.clone();
            let roll = RollPose {
                phase,
                weight: 1.0,
                direction: Vec3::X,
            };
            roll.apply(&rig, &poses, Transform::IDENTITY, Vec3::Y, &mut flat)
                .unwrap();
            RollPose {
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
