//! Built-in humanoid rig and its pose library (`content/humanoid.poses.jsonc`): the reference
//! content for tests and the playground.
//!
//! Y up, -Z forward, +X is the character's right, meters. The rest pose hangs the arms down
//! and stands with straight legs; the origin sits on the ground below the pelvis.

use bevy::math::Vec3;
use bevy::transform::components::Transform;

use crate::base_pose::BasePoseSet;
use crate::rig::{Limb, LimbBinding, Rig};
use crate::skeleton::{JointDef, Skeleton};

const HIPS_HEIGHT: f32 = 1.0;
pub const ANKLE_HEIGHT: f32 = 0.08;

struct Builder(Vec<JointDef>);

impl Builder {
    fn add(&mut self, name: &str, parent: Option<&str>, at: Vec3) -> usize {
        let parent = parent.map(|p| {
            self.0
                .iter()
                .position(|j| j.name == p)
                .expect("parent added first")
        });
        self.0.push(JointDef {
            name: name.to_owned(),
            parent,
            rest: Transform::from_translation(at),
        });
        self.0.len() - 1
    }
}

pub fn rig() -> Rig {
    let mut b = Builder(Vec::new());
    let root = b.add("root", None, Vec3::ZERO);
    let pelvis = b.add("hips", Some("root"), Vec3::new(0.0, HIPS_HEIGHT, 0.0));
    let spine = b.add("spine", Some("hips"), Vec3::new(0.0, 0.12, 0.0));
    let chest = b.add("chest", Some("spine"), Vec3::new(0.0, 0.18, 0.0));
    let neck = b.add("neck", Some("chest"), Vec3::new(0.0, 0.22, 0.0));
    let head = b.add("head", Some("neck"), Vec3::new(0.0, 0.10, 0.0));

    let mut limbs = vec![
        LimbBinding {
            limb: Limb::Head,
            chain: vec![spine, chest, neck, head],
            pole: Vec3::ZERO,
        },
        LimbBinding {
            limb: Limb::Pelvis,
            chain: vec![pelvis],
            pole: Vec3::ZERO,
        },
    ];
    for (side, suffix) in [(-1.0_f32, "l"), (1.0, "r")] {
        let n = |base: &str| format!("{base}_{suffix}");
        let arm = b.add(
            &n("upper_arm"),
            Some("chest"),
            Vec3::new(side * 0.19, 0.16, 0.0),
        );
        let fore = b.add(
            &n("forearm"),
            Some(&n("upper_arm")),
            Vec3::new(0.0, -0.28, 0.0),
        );
        let hand = b.add(&n("hand"), Some(&n("forearm")), Vec3::new(0.0, -0.26, 0.0));
        b.add(
            &n("fingers_0"),
            Some(&n("hand")),
            Vec3::new(0.0, -0.03, 0.0),
        );
        b.add(
            &n("fingers_1"),
            Some(&n("fingers_0")),
            Vec3::new(0.0, -0.04, 0.0),
        );
        b.add(
            &n("fingers_2"),
            Some(&n("fingers_1")),
            Vec3::new(0.0, -0.03, 0.0),
        );
        b.add(
            &n("thumb_0"),
            Some(&n("hand")),
            Vec3::new(-side * 0.03, -0.02, -0.02),
        );
        b.add(
            &n("thumb_1"),
            Some(&n("thumb_0")),
            Vec3::new(0.0, -0.03, 0.0),
        );
        limbs.push(LimbBinding {
            limb: if side < 0.0 {
                Limb::LeftHand
            } else {
                Limb::RightHand
            },
            chain: vec![arm, fore, hand],
            pole: Vec3::new(side * 0.3, -0.2, 1.0).normalize(),
        });
    }
    for (side, suffix) in [(-1.0_f32, "l"), (1.0, "r")] {
        let n = |base: &str| format!("{base}_{suffix}");
        let thigh = b.add(
            &n("thigh"),
            Some("hips"),
            Vec3::new(side * 0.09, -0.04, 0.0),
        );
        let shin = b.add(&n("shin"), Some(&n("thigh")), Vec3::new(0.0, -0.46, 0.0));
        let foot_y = -(HIPS_HEIGHT - 0.04 - 0.46 - ANKLE_HEIGHT);
        let foot = b.add(&n("foot"), Some(&n("shin")), Vec3::new(0.0, foot_y, 0.0));
        limbs.push(LimbBinding {
            limb: if side < 0.0 {
                Limb::LeftFoot
            } else {
                Limb::RightFoot
            },
            chain: vec![thigh, shin, foot],
            pole: Vec3::NEG_Z,
        });
    }
    Rig {
        skeleton: Skeleton::new(b.0).expect("builder keeps topological order"),
        pelvis,
        root,
        limbs,
    }
}

/// The humanoid's pose library, authored in [`POSES_FILE`].
pub fn base_poses() -> BasePoseSet {
    BasePoseSet::from_jsonc(POSES).expect("the humanoid pose library is valid")
}

const POSES: &str = include_str!("../content/humanoid.poses.jsonc");

/// Source of [`base_poses`], for tools that show or open it.
pub const POSES_FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/content/humanoid.poses.jsonc");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_is_consistent() {
        let rig = rig();
        let model = rig.skeleton.model_transforms(&rig.skeleton.rest_pose());
        for foot in rig.feet() {
            let ankle = model[*foot.chain.last().unwrap()].translation;
            assert!((ankle.y - ANKLE_HEIGHT).abs() < 1e-5);
        }
        for binding in &rig.limbs {
            for w in binding.chain.windows(2) {
                assert!(rig.skeleton.is_descendant_of(w[1], w[0]));
            }
        }
    }

    #[test]
    fn the_pose_library_fits_the_rig() {
        let poses = base_poses();
        poses.validate(&rig().skeleton).unwrap();
        for sequence in ["roll", "swing", "arm_swing"] {
            poses.sequence(sequence).unwrap();
        }
    }
}
