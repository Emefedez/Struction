//! Authored base poses: sparse, data-only joint targets the solvers move away from and the
//! springs return to.

use std::collections::BTreeMap;

use bevy::ecs::resource::Resource;
use bevy::ecs::{
    component::Component,
    reflect::{ReflectComponent, ReflectResource},
};
use bevy::math::{EulerRot, Quat, Vec3};
use bevy::reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::error::AnimError;
use crate::pose::{BoneMask, Pose};
use crate::skeleton::Skeleton;

/// Absolute local values for one joint; unset fields keep the underlying pose.
#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct JointPose {
    pub joint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[reflect(default)]
    pub translation: Option<Vec3>,
    /// XYZ Euler angles in degrees, friendlier to author than quaternions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[reflect(default)]
    pub euler_deg: Option<Vec3>,
}

impl JointPose {
    pub fn rotation(joint: &str, euler_deg: Vec3) -> Self {
        Self {
            joint: joint.to_owned(),
            translation: None,
            euler_deg: Some(euler_deg),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct BasePose {
    pub joints: Vec<JointPose>,
}

/// Named library of base poses (`idle`, `grip`, `fist`, `seated`, `aim`, ...).
#[derive(Resource, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Resource)]
pub struct BasePoseSet {
    pub poses: BTreeMap<String, BasePose>,
}

/// Sparse per-actor targets. Names are those consumed by the animation solvers (`idle`,
/// `roll`, `swing_raise`, `swing_strike`, ...); unspecified poses use the shared library.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct PoseTargets {
    pub poses: BTreeMap<String, BasePose>,
}

/// Effective library on a rig, rebuilt when its body's targets change.
#[derive(Component)]
pub struct RigPoseSet(pub BasePoseSet);

impl PoseTargets {
    pub fn resolve(
        &self,
        defaults: &BasePoseSet,
        skeleton: &Skeleton,
    ) -> Result<BasePoseSet, AnimError> {
        let mut merged = defaults.clone();
        for (name, pose) in &self.poses {
            pose.resolve(skeleton)?;
            merged.poses.insert(name.clone(), pose.clone());
        }
        Ok(merged)
    }
}

impl BasePoseSet {
    pub fn resolve(&self, name: &str, skeleton: &Skeleton) -> Result<ResolvedBasePose, AnimError> {
        self.poses
            .get(name)
            .ok_or_else(|| AnimError::UnknownPose(name.to_owned()))?
            .resolve(skeleton)
    }
}

impl BasePose {
    pub fn resolve(&self, skeleton: &Skeleton) -> Result<ResolvedBasePose, AnimError> {
        let entries = self
            .joints
            .iter()
            .map(|j| {
                Ok(ResolvedJoint {
                    joint: skeleton.joint_id(&j.joint)?,
                    translation: j.translation,
                    rotation: j.euler_deg.map(|e| {
                        Quat::from_euler(
                            EulerRot::XYZ,
                            e.x.to_radians(),
                            e.y.to_radians(),
                            e.z.to_radians(),
                        )
                    }),
                })
            })
            .collect::<Result<_, AnimError>>()?;
        Ok(ResolvedBasePose {
            entries,
            joint_count: skeleton.len(),
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
struct ResolvedJoint {
    joint: usize,
    translation: Option<Vec3>,
    rotation: Option<Quat>,
}

/// A base pose bound to a skeleton, cheap to apply every frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedBasePose {
    entries: Vec<ResolvedJoint>,
    joint_count: usize,
}

impl ResolvedBasePose {
    /// Blends the joints this pose authors toward its values by `weight` (times `mask`).
    pub fn apply(&self, pose: &mut Pose, weight: f32, mask: Option<&BoneMask>) {
        debug_assert_eq!(pose.len(), self.joint_count);
        for e in &self.entries {
            let w = (weight * mask.map_or(1.0, |m| m.weight(e.joint))).clamp(0.0, 1.0);
            if w <= 0.0 {
                continue;
            }
            let local = &mut pose.locals[e.joint];
            if let Some(t) = e.translation {
                local.translation = local.translation.lerp(t, w);
            }
            if let Some(r) = e.rotation {
                local.rotation = local.rotation.slerp(r, w);
            }
        }
    }

    /// Joints this pose touches, as a 0/1 mask.
    pub fn footprint(&self) -> BoneMask {
        let mut mask = BoneMask::uniform(self.joint_count, 0.0);
        for e in &self.entries {
            mask.weights[e.joint] = 1.0;
        }
        mask
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid;

    #[test]
    fn round_trips_through_json_and_applies() {
        let set = humanoid::base_poses();
        let json = serde_json::to_string_pretty(&set).unwrap();
        let back: BasePoseSet = serde_json::from_str(&json).unwrap();
        assert_eq!(set, back);

        let rig = humanoid::rig();
        let mut pose = rig.skeleton.rest_pose();
        back.resolve("fist", &rig.skeleton)
            .unwrap()
            .apply(&mut pose, 1.0, None);
        let finger = rig.skeleton.joint_id("fingers_0_r").unwrap();
        assert!(pose.locals[finger].rotation.angle_between(Quat::IDENTITY) > 1.0);
    }

    #[test]
    fn partial_weight_is_halfway_and_mask_limits_joints() {
        let rig = humanoid::rig();
        let sk = &rig.skeleton;
        let fist = humanoid::base_poses().resolve("fist", sk).unwrap();
        let mut full = sk.rest_pose();
        fist.apply(&mut full, 1.0, None);
        let mut half = sk.rest_pose();
        fist.apply(&mut half, 0.5, None);
        let f = sk.joint_id("fingers_1_l").unwrap();
        let full_angle = full.locals[f].rotation.angle_between(Quat::IDENTITY);
        let half_angle = half.locals[f].rotation.angle_between(Quat::IDENTITY);
        assert!((half_angle - full_angle * 0.5).abs() < 1e-4);

        let mut masked = sk.rest_pose();
        let right = rig.limb_end_mask(crate::rig::Limb::RightHand).unwrap();
        fist.apply(&mut masked, 1.0, Some(&right));
        assert_eq!(masked.locals[f].rotation, Quat::IDENTITY);
        assert!(masked.locals[sk.joint_id("fingers_1_r").unwrap()].rotation != Quat::IDENTITY);
    }

    #[test]
    fn unknown_joint_and_pose_are_errors() {
        let sk = humanoid::rig().skeleton;
        let bad = BasePose {
            joints: vec![JointPose::rotation("tail", Vec3::ZERO)],
        };
        assert_eq!(
            bad.resolve(&sk),
            Err(AnimError::UnknownJoint("tail".into()))
        );
        assert!(BasePoseSet::default().resolve("idle", &sk).is_err());
    }
}
