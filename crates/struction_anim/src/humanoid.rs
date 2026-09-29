//! Built-in humanoid rig and base poses: the reference content for tests and the playground.
//!
//! Y up, -Z forward, +X is the character's right, meters. The rest pose hangs the arms down
//! and stands with straight legs; the origin sits on the ground below the pelvis.

use bevy::math::Vec3;
use bevy::transform::components::Transform;

use crate::base_pose::{BasePose, BasePoseSet, JointPose};
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
        let arm = b.add(&n("upper_arm"), Some("chest"), Vec3::new(side * 0.19, 0.16, 0.0));
        let fore = b.add(&n("forearm"), Some(&n("upper_arm")), Vec3::new(0.0, -0.28, 0.0));
        let hand = b.add(&n("hand"), Some(&n("forearm")), Vec3::new(0.0, -0.26, 0.0));
        b.add(&n("fingers_0"), Some(&n("hand")), Vec3::new(0.0, -0.03, 0.0));
        b.add(&n("fingers_1"), Some(&n("fingers_0")), Vec3::new(0.0, -0.04, 0.0));
        b.add(&n("fingers_2"), Some(&n("fingers_1")), Vec3::new(0.0, -0.03, 0.0));
        b.add(&n("thumb_0"), Some(&n("hand")), Vec3::new(-side * 0.03, -0.02, -0.02));
        b.add(&n("thumb_1"), Some(&n("thumb_0")), Vec3::new(0.0, -0.03, 0.0));
        limbs.push(LimbBinding {
            limb: if side < 0.0 { Limb::LeftHand } else { Limb::RightHand },
            chain: vec![arm, fore, hand],
            pole: Vec3::new(side * 0.3, -0.2, 1.0).normalize(),
        });
    }
    for (side, suffix) in [(-1.0_f32, "l"), (1.0, "r")] {
        let n = |base: &str| format!("{base}_{suffix}");
        let thigh = b.add(&n("thigh"), Some("hips"), Vec3::new(side * 0.09, -0.04, 0.0));
        let shin = b.add(&n("shin"), Some(&n("thigh")), Vec3::new(0.0, -0.46, 0.0));
        let foot_y = -(HIPS_HEIGHT - 0.04 - 0.46 - ANKLE_HEIGHT);
        let foot = b.add(&n("foot"), Some(&n("shin")), Vec3::new(0.0, foot_y, 0.0));
        limbs.push(LimbBinding {
            limb: if side < 0.0 { Limb::LeftFoot } else { Limb::RightFoot },
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

pub fn base_poses() -> BasePoseSet {
    let mut set = BasePoseSet::default();
    let euler = |joint: &str, x: f32, y: f32, z: f32| JointPose::rotation(joint, Vec3::new(x, y, z));
    let both = |f: &dyn Fn(&str, f32) -> Vec<JointPose>| -> Vec<JointPose> {
        [("l", -1.0), ("r", 1.0)]
            .into_iter()
            .flat_map(|(s, side)| f(s, side))
            .collect()
    };
    let hand = |curl: [f32; 3], thumb: [f32; 2]| {
        both(&|s, side| {
            vec![
                euler(&format!("fingers_0_{s}"), curl[0], 0.0, 0.0),
                euler(&format!("fingers_1_{s}"), curl[1], 0.0, 0.0),
                euler(&format!("fingers_2_{s}"), curl[2], 0.0, 0.0),
                euler(&format!("thumb_0_{s}"), thumb[0], 0.0, -side * 0.7 * thumb[0]),
                euler(&format!("thumb_1_{s}"), thumb[1], 0.0, 0.0),
            ]
        })
    };

    let mut idle = hand([15.0, 20.0, 15.0], [5.0, 10.0]);
    idle.extend(both(&|s, side| {
        vec![
            euler(&format!("upper_arm_{s}"), -4.0, 0.0, side * 8.0),
            euler(&format!("forearm_{s}"), 18.0, 0.0, 0.0),
            euler(&format!("thigh_{s}"), 6.0, 0.0, 0.0),
            euler(&format!("shin_{s}"), -10.0, 0.0, 0.0),
        ]
    }));
    set.poses.insert("idle".into(), BasePose { joints: idle });
    set.poses.insert(
        "grip".into(),
        BasePose {
            joints: hand([55.0, 70.0, 50.0], [15.0, 30.0]),
        },
    );
    set.poses.insert(
        "fist".into(),
        BasePose {
            joints: hand([85.0, 100.0, 70.0], [25.0, 40.0]),
        },
    );

    let mut seated = vec![
        JointPose {
            joint: "hips".into(),
            translation: Some(Vec3::new(0.0, 0.54, 0.04)),
            euler_deg: None,
        },
        euler("spine", -4.0, 0.0, 0.0),
    ];
    seated.extend(both(&|s, side| {
        vec![
            euler(&format!("thigh_{s}"), 90.0, 0.0, side * 4.0),
            euler(&format!("shin_{s}"), -90.0, 0.0, 0.0),
            euler(&format!("upper_arm_{s}"), 25.0, 0.0, side * 6.0),
            euler(&format!("forearm_{s}"), 55.0, 0.0, 0.0),
        ]
    }));
    set.poses.insert("seated".into(), BasePose { joints: seated });

    set.poses.insert(
        "aim".into(),
        BasePose {
            joints: vec![
                euler("spine", 0.0, -12.0, 0.0),
                euler("head", 0.0, 12.0, 0.0),
                euler("upper_arm_r", 88.0, 0.0, 4.0),
                euler("forearm_r", 4.0, 0.0, 0.0),
                euler("upper_arm_l", 62.0, 0.0, -14.0),
                euler("forearm_l", 50.0, 0.0, 0.0),
            ],
        },
    );
    set
}

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
    fn every_base_pose_resolves() {
        let rig = rig();
        for name in ["idle", "grip", "fist", "seated", "aim"] {
            base_poses().resolve(name, &rig.skeleton).unwrap();
        }
    }
}
