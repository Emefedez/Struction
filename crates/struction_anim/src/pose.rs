//! Poses (local joint transforms) and bone masks.

use bevy::reflect::Reflect;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};

/// Local transform of every joint of a skeleton, indexed like `Skeleton::joints`.
#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct Pose {
    pub locals: Vec<Transform>,
}

pub fn blend_transform(a: &Transform, b: &Transform, weight: f32) -> Transform {
    Transform {
        translation: a.translation.lerp(b.translation, weight),
        rotation: a.rotation.slerp(b.rotation, weight),
        scale: a.scale.lerp(b.scale, weight),
    }
}

impl Pose {
    pub fn len(&self) -> usize {
        self.locals.len()
    }

    pub fn is_empty(&self) -> bool {
        self.locals.is_empty()
    }

    /// Moves `self` toward `other` by `weight`, optionally scaled per joint by `mask`.
    pub fn blend_in_place(&mut self, other: &Pose, weight: f32, mask: Option<&BoneMask>) {
        debug_assert_eq!(self.len(), other.len());
        for (i, (a, b)) in self.locals.iter_mut().zip(&other.locals).enumerate() {
            let w = weight * mask.map_or(1.0, |m| m.weight(i));
            if w > 0.0 {
                *a = blend_transform(a, b, w.min(1.0));
            }
        }
    }

    pub fn blended(&self, other: &Pose, weight: f32, mask: Option<&BoneMask>) -> Pose {
        let mut out = self.clone();
        out.blend_in_place(other, weight, mask);
        out
    }

    /// Largest translation distance and rotation angle between corresponding joints.
    pub fn max_difference(&self, other: &Pose) -> (f32, f32) {
        self.locals
            .iter()
            .zip(&other.locals)
            .fold((0.0, 0.0), |(d, a), (x, y)| {
                (
                    d.max(x.translation.distance(y.translation)),
                    a.max(x.rotation.angle_between(y.rotation)),
                )
            })
    }
}

/// Per-joint weight in `0..=1`.
#[derive(Clone, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct BoneMask {
    pub weights: Vec<f32>,
}

impl BoneMask {
    pub fn uniform(len: usize, weight: f32) -> Self {
        Self {
            weights: vec![weight; len],
        }
    }

    pub fn weight(&self, joint: usize) -> f32 {
        self.weights.get(joint).copied().unwrap_or(0.0)
    }

    pub fn inverted(&self) -> Self {
        Self {
            weights: self.weights.iter().map(|w| 1.0 - w).collect(),
        }
    }

    /// Joint-wise maximum, so overlapping masks do not exceed one.
    pub fn union(&self, other: &BoneMask) -> Self {
        Self {
            weights: self
                .weights
                .iter()
                .zip(&other.weights)
                .map(|(a, b)| a.max(*b))
                .collect(),
        }
    }
}
