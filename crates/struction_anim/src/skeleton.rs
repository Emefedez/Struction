//! Joint hierarchy, forward kinematics and rig roles.

use bevy::reflect::Reflect;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::error::AnimError;
use crate::pose::{BoneMask, Pose};

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct JointDef {
    pub name: String,
    pub parent: Option<usize>,
    pub rest: Transform,
}

/// Joints in topological order: a parent always precedes its children.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[serde(try_from = "SkeletonData")]
pub struct Skeleton {
    joints: Vec<JointDef>,
}

#[derive(Deserialize)]
struct SkeletonData {
    joints: Vec<JointDef>,
}

impl TryFrom<SkeletonData> for Skeleton {
    type Error = AnimError;

    fn try_from(data: SkeletonData) -> Result<Self, AnimError> {
        Skeleton::new(data.joints)
    }
}

impl Skeleton {
    pub fn new(joints: Vec<JointDef>) -> Result<Self, AnimError> {
        for (i, joint) in joints.iter().enumerate() {
            if let Some(parent) = joint.parent
                && parent >= i
            {
                return Err(AnimError::BadParent { joint: i, parent });
            }
        }
        Ok(Self { joints })
    }

    pub fn joints(&self) -> &[JointDef] {
        &self.joints
    }

    pub fn len(&self) -> usize {
        self.joints.len()
    }

    pub fn is_empty(&self) -> bool {
        self.joints.is_empty()
    }

    pub fn joint_id(&self, name: &str) -> Result<usize, AnimError> {
        self.joints
            .iter()
            .position(|j| j.name == name)
            .ok_or_else(|| AnimError::UnknownJoint(name.to_owned()))
    }

    pub fn rest_pose(&self) -> Pose {
        Pose {
            locals: self.joints.iter().map(|j| j.rest).collect(),
        }
    }

    /// Model-space transform of every joint.
    pub fn model_transforms(&self, pose: &Pose) -> Vec<Transform> {
        debug_assert_eq!(pose.len(), self.len());
        let mut model: Vec<Transform> = Vec::with_capacity(self.len());
        for (i, joint) in self.joints.iter().enumerate() {
            let local = pose.locals[i];
            model.push(match joint.parent {
                Some(p) => model[p].mul_transform(local),
                None => local,
            });
        }
        model
    }

    /// Recomputes the model transforms of every descendant of `root` after its local changed.
    pub fn update_subtree(&self, pose: &Pose, model: &mut [Transform], root: usize) {
        let mut dirty = vec![false; self.len()];
        dirty[root] = true;
        model[root] = match self.joints[root].parent {
            Some(p) => model[p].mul_transform(pose.locals[root]),
            None => pose.locals[root],
        };
        for i in root + 1..self.len() {
            if let Some(p) = self.joints[i].parent
                && dirty[p]
            {
                dirty[i] = true;
                model[i] = model[p].mul_transform(pose.locals[i]);
            }
        }
    }

    pub fn is_descendant_of(&self, joint: usize, ancestor: usize) -> bool {
        let mut current = Some(joint);
        while let Some(j) = current {
            if j == ancestor {
                return true;
            }
            current = self.joints[j].parent;
        }
        false
    }

    /// Weight 1 for `root` and everything below it.
    pub fn subtree_mask(&self, root: usize) -> BoneMask {
        BoneMask {
            weights: (0..self.len())
                .map(|j| f32::from(u8::from(self.is_descendant_of(j, root))))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use bevy::math::{Quat, Vec3};

    use super::*;

    fn chain() -> Skeleton {
        let joint = |name: &str, parent, t: Vec3| JointDef {
            name: name.into(),
            parent,
            rest: Transform::from_translation(t),
        };
        Skeleton::new(vec![
            joint("a", None, Vec3::ZERO),
            joint("b", Some(0), Vec3::X),
            joint("c", Some(1), Vec3::X),
            joint("d", Some(0), Vec3::Y),
        ])
        .unwrap()
    }

    #[test]
    fn forward_kinematics_composes_parent_rotation() {
        let sk = chain();
        let mut pose = sk.rest_pose();
        pose.locals[0].rotation = Quat::from_rotation_z(core::f32::consts::FRAC_PI_2);
        let model = sk.model_transforms(&pose);
        assert!((model[2].translation - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-5);
        assert!((model[3].translation - Vec3::new(-1.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn update_subtree_matches_full_fk() {
        let sk = chain();
        let mut pose = sk.rest_pose();
        let mut model = sk.model_transforms(&pose);
        pose.locals[1].rotation = Quat::from_rotation_y(0.7);
        sk.update_subtree(&pose, &mut model, 1);
        let full = sk.model_transforms(&pose);
        for (a, b) in model.iter().zip(&full) {
            assert!(a.translation.distance(b.translation) < 1e-6);
        }
    }

    #[test]
    fn rejects_bad_parent_order_including_from_data() {
        let bad = r#"{"joints":[{"name":"a","parent":1,"rest":{"translation":[0,0,0],"rotation":[0,0,0,1],"scale":[1,1,1]}}]}"#;
        assert!(serde_json::from_str::<Skeleton>(bad).is_err());
    }

    #[test]
    fn subtree_mask_covers_descendants_only() {
        let mask = chain().subtree_mask(1);
        assert_eq!(mask.weights, vec![0.0, 1.0, 1.0, 0.0]);
    }

    #[test]
    fn lookup_by_name() {
        let sk = chain();
        assert_eq!(sk.joint_id("c"), Ok(2));
        assert!(sk.joint_id("zzz").is_err());
    }
}
