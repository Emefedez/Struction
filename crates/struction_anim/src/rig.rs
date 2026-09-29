//! Role bindings: gameplay talks about limbs, never joint names.

use bevy::ecs::component::Component;
use bevy::ecs::reflect::ReflectComponent;
use bevy::reflect::Reflect;
use serde::{Deserialize, Serialize};

use crate::error::AnimError;
use crate::pose::BoneMask;
use crate::skeleton::Skeleton;

/// Body part a constraint or intent addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Reflect, Serialize, Deserialize)]
pub enum Limb {
    Head,
    Pelvis,
    LeftHand,
    RightHand,
    LeftFoot,
    RightFoot,
    /// Additional legs of quadrupeds and other creatures.
    ExtraFoot(u8),
}

impl Limb {
    pub fn is_foot(self) -> bool {
        matches!(self, Limb::LeftFoot | Limb::RightFoot | Limb::ExtraFoot(_))
    }

    pub fn is_hand(self) -> bool {
        matches!(self, Limb::LeftHand | Limb::RightHand)
    }
}

/// Joint chain of a limb, root first: three joints for two-bone limbs (thigh, shin, foot), one
/// for the pelvis, any number for the head chain (spine ... head).
#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct LimbBinding {
    pub limb: Limb,
    pub chain: Vec<usize>,
    /// Model-space direction the middle joint bends toward (knee forward, elbow back).
    pub pole: bevy::math::Vec3,
}

#[derive(Component, Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Rig {
    pub skeleton: Skeleton,
    /// Joint that carries body bob, lean and crouch.
    pub pelvis: usize,
    /// Joint that carries the final squash and stretch scale.
    pub root: usize,
    pub limbs: Vec<LimbBinding>,
}

impl Rig {
    pub fn binding(&self, limb: Limb) -> Result<&LimbBinding, AnimError> {
        self.limbs
            .iter()
            .find(|b| b.limb == limb)
            .ok_or(AnimError::UnknownLimb(limb))
    }

    pub fn feet(&self) -> impl Iterator<Item = &LimbBinding> {
        self.limbs.iter().filter(|b| b.limb.is_foot())
    }

    /// Weight 1 on the end joint of the limb and everything below it (fingers, toes).
    pub fn limb_end_mask(&self, limb: Limb) -> Result<BoneMask, AnimError> {
        let end = *self.binding(limb)?.chain.last().expect("limb chains are not empty");
        Ok(self.skeleton.subtree_mask(end))
    }
}
