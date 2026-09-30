//! Affordances objects expose to characters. They are plain data on the object; characters
//! solve against them and gameplay never names a bone.

use bevy::ecs::component::Component;
use bevy::ecs::reflect::ReflectComponent;
use bevy::math::Vec3;
use bevy::reflect::Reflect;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::rig::Limb;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Reflect, Serialize, Deserialize)]
pub enum HandPreference {
    #[default]
    Any,
    Left,
    Right,
}

impl HandPreference {
    pub fn accepts(self, limb: Limb) -> bool {
        matches!(
            (self, limb),
            (HandPreference::Any, Limb::LeftHand | Limb::RightHand)
                | (HandPreference::Left, Limb::LeftHand)
                | (HandPreference::Right, Limb::RightHand)
        )
    }
}

fn default_grip_pose() -> String {
    "grip".into()
}

/// A place on an object a hand can take hold of.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct GripTarget {
    /// Desired hand frame relative to the object.
    pub local: Transform,
    #[serde(default)]
    pub hand: HandPreference,
    /// Base pose that shapes the hand while gripping.
    #[serde(default = "default_grip_pose")]
    pub pose: String,
}

#[derive(Component, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Grabbable {
    pub grips: Vec<GripTarget>,
}

#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Sittable {
    /// Pelvis frame relative to the object.
    pub seat: Transform,
    /// Whole-body base pose while seated.
    pub pose: String,
}

impl Default for Sittable {
    fn default() -> Self {
        Self {
            seat: Transform::IDENTITY,
            pose: "seated".into(),
        }
    }
}

/// A hold on a climbable surface. Data only for now: no climbing solver exists yet.
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct ClimbHold {
    pub local: Transform,
    #[serde(default)]
    pub hand: HandPreference,
}

#[derive(Component, Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Climbable {
    pub holds: Vec<ClimbHold>,
}

/// Greedy grip assignment: repeatedly pairs the closest (grip, free hand) that the grip's
/// preference accepts, so a one-grip object goes to the nearest hand and a two-grip object
/// to both. `grips` holds (preference, world position); `hands` (limb, world position).
pub fn assign_grips(
    grips: &[(HandPreference, Vec3)],
    hands: &[(Limb, Vec3)],
) -> Vec<(usize, Limb)> {
    let mut pairs: Vec<(f32, usize, Limb)> = grips
        .iter()
        .enumerate()
        .flat_map(|(g, (pref, pos))| {
            hands
                .iter()
                .filter(|(limb, _)| pref.accepts(*limb))
                .map(move |(limb, hand)| (pos.distance(*hand), g, *limb))
        })
        .collect();
    pairs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out: Vec<(usize, Limb)> = Vec::new();
    for (_, g, limb) in pairs {
        if out.iter().all(|(og, ol)| *og != g && *ol != limb) {
            out.push((g, limb));
        }
    }
    out.sort_by_key(|(g, _)| *g);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const HANDS: [(Limb, Vec3); 2] = [
        (Limb::LeftHand, Vec3::new(-0.3, 1.0, 0.0)),
        (Limb::RightHand, Vec3::new(0.3, 1.0, 0.0)),
    ];

    #[test]
    fn single_grip_goes_to_nearest_hand() {
        let grips = [(HandPreference::Any, Vec3::new(0.5, 1.0, -0.2))];
        assert_eq!(assign_grips(&grips, &HANDS), vec![(0, Limb::RightHand)]);
        let grips = [(HandPreference::Any, Vec3::new(-0.5, 1.0, -0.2))];
        assert_eq!(assign_grips(&grips, &HANDS), vec![(0, Limb::LeftHand)]);
    }

    #[test]
    fn preference_overrides_distance_and_two_grips_use_both_hands() {
        let grips = [(HandPreference::Left, Vec3::new(0.5, 1.0, 0.0))];
        assert_eq!(assign_grips(&grips, &HANDS), vec![(0, Limb::LeftHand)]);

        // Both grips are nearest to the right hand; the second falls back to the left.
        let grips = [
            (HandPreference::Any, Vec3::new(0.5, 1.0, 0.0)),
            (HandPreference::Any, Vec3::new(0.7, 1.0, 0.0)),
        ];
        let out = assign_grips(&grips, &HANDS);
        assert_eq!(out, vec![(0, Limb::RightHand), (1, Limb::LeftHand)]);
    }

    #[test]
    fn affordances_round_trip_as_data() {
        let json = r#"{"grips":[{"local":{"translation":[0,0.1,0],"rotation":[0,0,0,1],"scale":[1,1,1]}}]}"#;
        let g: Grabbable = serde_json::from_str(json).unwrap();
        assert_eq!(g.grips[0].pose, "grip");
        assert_eq!(g.grips[0].hand, HandPreference::Any);
    }
}
