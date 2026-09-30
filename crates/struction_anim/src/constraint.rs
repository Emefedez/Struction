//! Animation-facing constraints: the README's shared authoring model (source, target,
//! property, weight, priority, falloff) with foot contact, gaze and grip alignment as
//! consumers. Physics/attachment constraints live elsewhere and share only this vocabulary.

use bevy::ecs::entity::Entity;
use bevy::math::{Quat, Vec3};
use bevy::reflect::Reflect;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::rig::Limb;

/// What the constraint drives on its source limb.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Reflect, Serialize, Deserialize)]
pub enum ConstraintProperty {
    /// End of the limb reaches the target point.
    Position,
    /// End of the limb takes the target orientation.
    Orientation,
    /// Position and orientation (grips, planted feet).
    Pose,
    /// Limb looks toward the target (gaze).
    Direction,
}

/// Constraints that compete for the same aspect of a limb are arbitrated together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Slot {
    Reach,
    Orientation,
    Look,
}

impl ConstraintProperty {
    fn slots(self) -> &'static [Slot] {
        match self {
            ConstraintProperty::Position => &[Slot::Reach],
            ConstraintProperty::Orientation => &[Slot::Orientation],
            ConstraintProperty::Pose => &[Slot::Reach, Slot::Orientation],
            ConstraintProperty::Direction => &[Slot::Look],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub enum ConstraintTarget {
    /// World-space point.
    Point(Vec3),
    /// Frame of another entity, with an offset in that entity's local space (a grip point).
    Entity { entity: Entity, offset: Transform },
}

/// Scales the weight by the distance between source and target, so a reach fades out when the
/// target moves away instead of snapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub enum Falloff {
    #[default]
    None,
    /// Full weight up to `start` meters, zero from `end`.
    Linear { start: f32, end: f32 },
    /// Same range with smoothstep easing.
    Smooth { start: f32, end: f32 },
}

impl Falloff {
    pub fn factor(self, distance: f32) -> f32 {
        let ramp = |start: f32, end: f32| {
            if end <= start {
                f32::from(u8::from(distance <= start))
            } else {
                1.0 - ((distance - start) / (end - start)).clamp(0.0, 1.0)
            }
        };
        match self {
            Falloff::None => 1.0,
            Falloff::Linear { start, end } => ramp(start, end),
            Falloff::Smooth { start, end } => smoothstep(ramp(start, end)),
        }
    }
}

pub fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Weight that eases toward a goal over a fixed time, for smooth 0 -> 1 transitions.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct AnimatedWeight {
    pub goal: f32,
    /// Seconds for a full 0 -> 1 transition.
    pub duration: f32,
    progress: f32,
}

impl AnimatedWeight {
    pub fn new(goal: f32, duration: f32) -> Self {
        Self {
            goal,
            duration,
            progress: 0.0,
        }
    }

    pub fn advance(&mut self, dt: f32) {
        let step = if self.duration > 0.0 {
            dt / self.duration
        } else {
            1.0
        };
        let delta = self.goal - self.progress;
        self.progress += delta.clamp(-step, step);
    }

    /// Eased weight in `0..=1`.
    pub fn value(&self) -> f32 {
        smoothstep(self.progress)
    }

    pub fn is_finished_releasing(&self) -> bool {
        self.goal <= 0.0 && self.progress <= 0.0
    }
}

/// Which gameplay request created a constraint, so intents can update or release it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Reflect, Serialize, Deserialize)]
pub enum ConstraintOrigin {
    Hold,
    Gaze,
    Sit,
    Manual,
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct AnimConstraint {
    pub origin: ConstraintOrigin,
    pub source: Limb,
    pub target: ConstraintTarget,
    pub property: ConstraintProperty,
    pub weight: AnimatedWeight,
    /// Higher wins: it takes weight first, lower priorities share what remains.
    pub priority: i32,
    pub falloff: Falloff,
    /// Base pose blended on the source limb's end (hand shape) with the constraint weight.
    pub pose: Option<String>,
}

/// A constraint reduced to a world-space goal for this frame.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedConstraint {
    /// Index into the source list, `None` for transient constraints such as foot contact.
    pub index: Option<usize>,
    pub source: Limb,
    pub property: ConstraintProperty,
    pub position: Vec3,
    pub rotation: Quat,
    pub weight: f32,
    pub priority: i32,
}

/// Resolves targets to world space and applies falloff. Skips constraints with no weight or
/// whose target entity is gone.
pub fn resolve(
    constraints: &[AnimConstraint],
    target_world: &dyn Fn(&ConstraintTarget) -> Option<Transform>,
    source_position: &dyn Fn(Limb) -> Option<Vec3>,
) -> Vec<ResolvedConstraint> {
    constraints
        .iter()
        .enumerate()
        .filter_map(|(index, c)| {
            let base = c.weight.value();
            if base <= 0.0 {
                return None;
            }
            let target = target_world(&c.target)?;
            let distance =
                source_position(c.source).map_or(0.0, |p| p.distance(target.translation));
            Some(ResolvedConstraint {
                index: Some(index),
                source: c.source,
                property: c.property,
                position: target.translation,
                rotation: target.rotation,
                weight: base * c.falloff.factor(distance),
                priority: c.priority,
            })
        })
        .collect()
}

/// Reduces weights so constraints competing for the same slot of a limb never sum above one,
/// highest priority first (stable for equal priorities).
pub fn arbitrate(resolved: &mut [ResolvedConstraint]) {
    let mut order: Vec<usize> = (0..resolved.len()).collect();
    order.sort_by_key(|&i| core::cmp::Reverse(resolved[i].priority));
    let mut remaining: Vec<(Limb, Slot, f32)> = Vec::new();
    for i in order {
        let mut available = 1.0_f32;
        for &slot in resolved[i].property.slots() {
            let key = (resolved[i].source, slot);
            let left = remaining
                .iter()
                .find(|(l, s, _)| (*l, *s) == key)
                .map_or(1.0, |(_, _, r)| *r);
            available = available.min(left);
        }
        let weight = resolved[i].weight.min(available);
        resolved[i].weight = weight;
        for &slot in resolved[i].property.slots() {
            let key = (resolved[i].source, slot);
            let left = remaining
                .iter()
                .find(|(l, s, _)| (*l, *s) == key)
                .map_or(1.0, |(_, _, r)| *r);
            let updated = (left - weight).max(0.0);
            match remaining.iter_mut().find(|(l, s, _)| (*l, *s) == key) {
                Some(entry) => entry.2 = updated,
                None => remaining.push((key.0, key.1, updated)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolved(
        source: Limb,
        property: ConstraintProperty,
        weight: f32,
        priority: i32,
    ) -> ResolvedConstraint {
        ResolvedConstraint {
            index: None,
            source,
            property,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            weight,
            priority,
        }
    }

    #[test]
    fn weight_animates_smoothly_from_zero_to_one_and_back() {
        let mut w = AnimatedWeight::new(1.0, 0.5);
        let mut last = w.value();
        assert_eq!(last, 0.0);
        let mut steps = Vec::new();
        for _ in 0..30 {
            w.advance(1.0 / 60.0);
            assert!(w.value() >= last, "monotonic");
            steps.push(w.value() - last);
            last = w.value();
        }
        assert!((w.value() - 1.0).abs() < 1e-6);
        // Eased: the first and last increments are much smaller than the middle one.
        assert!(steps[0] < steps[15] * 0.2 && steps[29] < steps[15] * 0.2);
        w.goal = 0.0;
        for _ in 0..60 {
            w.advance(1.0 / 60.0);
        }
        assert!(w.is_finished_releasing());
    }

    #[test]
    fn falloff_shapes() {
        assert_eq!(Falloff::None.factor(100.0), 1.0);
        let lin = Falloff::Linear {
            start: 1.0,
            end: 3.0,
        };
        assert_eq!(lin.factor(0.5), 1.0);
        assert!((lin.factor(2.0) - 0.5).abs() < 1e-6);
        assert_eq!(lin.factor(3.5), 0.0);
        let smooth = Falloff::Smooth {
            start: 1.0,
            end: 3.0,
        };
        assert!((smooth.factor(2.0) - 0.5).abs() < 1e-6);
        assert!(smooth.factor(1.5) > lin.factor(1.5));
    }

    #[test]
    fn priority_takes_weight_first() {
        let mut list = vec![
            resolved(Limb::RightHand, ConstraintProperty::Pose, 1.0, 0),
            resolved(Limb::RightHand, ConstraintProperty::Pose, 0.7, 5),
            resolved(Limb::LeftHand, ConstraintProperty::Pose, 1.0, 0),
            resolved(Limb::RightHand, ConstraintProperty::Direction, 1.0, 0),
        ];
        arbitrate(&mut list);
        assert!((list[1].weight - 0.7).abs() < 1e-6);
        assert!((list[0].weight - 0.3).abs() < 1e-6);
        assert_eq!(list[2].weight, 1.0, "other limbs are independent");
        assert_eq!(list[3].weight, 1.0, "different slot is independent");
    }

    #[test]
    fn resolve_applies_falloff_and_skips_missing_targets() {
        let mut c = AnimConstraint {
            origin: ConstraintOrigin::Manual,
            source: Limb::RightHand,
            target: ConstraintTarget::Point(Vec3::new(2.0, 0.0, 0.0)),
            property: ConstraintProperty::Position,
            weight: AnimatedWeight::new(1.0, 0.0),
            priority: 0,
            falloff: Falloff::Linear {
                start: 1.0,
                end: 3.0,
            },
            pose: None,
        };
        c.weight.advance(1.0);
        let to_world = |t: &ConstraintTarget| match t {
            ConstraintTarget::Point(p) => Some(Transform::from_translation(*p)),
            ConstraintTarget::Entity { .. } => None,
        };
        let out = resolve(std::slice::from_ref(&c), &to_world, &|_| Some(Vec3::ZERO));
        assert!((out[0].weight - 0.5).abs() < 1e-6);

        let mut gone = c.clone();
        gone.target = ConstraintTarget::Entity {
            entity: Entity::PLACEHOLDER,
            offset: Transform::IDENTITY,
        };
        assert!(resolve(&[gone], &to_world, &|_| None).is_empty());
    }
}
