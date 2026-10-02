//! Authored base poses: sparse, data-only joint targets the solvers move away from and the
//! springs return to, and the [`PoseSequence`]s that play them in order.

use std::collections::BTreeMap;

use bevy::ecs::resource::Resource;
use bevy::ecs::{
    component::Component,
    reflect::{ReflectComponent, ReflectResource},
};
use bevy::math::{EulerRot, Quat, Vec3};
use bevy::reflect::{Reflect, std_traits::ReflectDefault};
use serde::{Deserialize, Serialize};

use crate::error::AnimError;
use crate::pose::{BoneMask, Pose};
pub use crate::sequence::{PoseSequence, SequenceKey, Tumble};
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
#[reflect(Default)]
pub struct BasePose {
    /// What the pose is for, shown by tools.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    #[reflect(default)]
    pub doc: String,
    pub joints: Vec<JointPose>,
}

/// A rig's library: named key poses (`idle`, `grip`, `seated`, `swing_raise`, ...) and the
/// sequences that play them (`roll`, `swing`, ...). Authored as JSONC, such as the humanoid's
/// `content/humanoid.poses.jsonc`.
#[derive(Resource, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Resource)]
pub struct BasePoseSet {
    pub poses: BTreeMap<String, BasePose>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[reflect(default)]
    pub sequences: BTreeMap<String, PoseSequence>,
}

/// Sparse per-actor overrides of the rig's library, by pose and sequence name; anything left out
/// comes from the library.
#[derive(Component, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct PoseTargets {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[reflect(default)]
    pub poses: BTreeMap<String, BasePose>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[reflect(default)]
    pub sequences: BTreeMap<String, PoseSequence>,
}

/// Effective library on a rig, rebuilt when its body's targets change.
#[derive(Component)]
pub struct RigPoseSet(pub BasePoseSet);

impl PoseTargets {
    /// The library with these overrides, checked against `skeleton`.
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
        for (name, sequence) in &self.sequences {
            merged.sequences.insert(name.clone(), sequence.clone());
        }
        for name in self.sequences.keys() {
            merged.sequences[name].validate(name, &merged)?;
        }
        Ok(merged)
    }

    /// The sequence `name` plays for an actor with these overrides.
    pub fn sequence<'a>(
        targets: Option<&'a Self>,
        defaults: Option<&'a BasePoseSet>,
        name: &str,
    ) -> Option<&'a PoseSequence> {
        targets
            .and_then(|targets| targets.sequences.get(name))
            .or_else(|| defaults?.sequences.get(name))
    }
}

impl BasePoseSet {
    /// Reads a library from JSONC and checks its sequences; joints are checked against a
    /// skeleton by [`Self::validate`].
    pub fn from_jsonc(text: &str) -> Result<Self, AnimError> {
        let set: Self = jsonc_parser::parse_to_serde_value(text, &Default::default())
            .map_err(|e| AnimError::Library(e.to_string()))?;
        for (name, sequence) in &set.sequences {
            sequence.validate(name, &set)?;
        }
        Ok(set)
    }

    /// Every pose binds to `skeleton` and every sequence plays known poses.
    pub fn validate(&self, skeleton: &Skeleton) -> Result<(), AnimError> {
        for pose in self.poses.values() {
            pose.resolve(skeleton)?;
        }
        for (name, sequence) in &self.sequences {
            sequence.validate(name, self)?;
        }
        Ok(())
    }

    pub fn sequence(&self, name: &str) -> Result<&PoseSequence, AnimError> {
        self.sequences
            .get(name)
            .ok_or_else(|| AnimError::UnknownSequence(name.to_owned()))
    }

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
    use bevy::prelude::default;

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
            ..default()
        };
        assert_eq!(
            bad.resolve(&sk),
            Err(AnimError::UnknownJoint("tail".into()))
        );
        assert!(BasePoseSet::default().resolve("idle", &sk).is_err());
    }
}
