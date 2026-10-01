//! The per-frame pose pipeline, pure and Bevy-free:
//! base pose layers -> spring smoothing -> body offset and lean -> constraint solvers
//! (feet, hands, gaze) -> deformation.

use std::collections::HashMap;

use bevy::math::{Quat, Vec3};
use bevy::reflect::Reflect;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::base_pose::{BasePoseSet, ResolvedBasePose};
use crate::constraint::{AnimConstraint, ConstraintProperty, ResolvedConstraint};
use crate::error::AnimError;
use crate::ik::{self, GazeLimits};
use crate::locomotion::LocomotionOutput;
use crate::moves::MovePose;
use crate::pose::{BoneMask, Pose};
use crate::rig::{Limb, Rig};
use crate::spring::{SpringParams, SpringQuat};

/// Joint rotations that chase a target pose with springs: secondary motion and snap-back after
/// something displaced them.
#[derive(Clone, Debug, Default)]
pub struct PoseSpring {
    states: Vec<SpringQuat>,
}

impl PoseSpring {
    pub fn step(&mut self, target: &Pose, params: SpringParams, dt: f32) -> Pose {
        if self.states.len() != target.len() {
            self.states = target
                .locals
                .iter()
                .map(|t| SpringQuat::at(t.rotation))
                .collect();
        }
        let mut out = target.clone();
        for ((state, local), goal) in self
            .states
            .iter_mut()
            .zip(&mut out.locals)
            .zip(&target.locals)
        {
            state.step(goal.rotation, params, dt);
            local.rotation = state.value;
        }
        out
    }

    /// Displaces a joint with an angular velocity (radians per second); the spring returns it.
    pub fn kick(&mut self, joint: usize, angular_velocity: Vec3) {
        if let Some(state) = self.states.get_mut(joint) {
            state.angular_velocity += angular_velocity;
        }
    }
}

#[derive(Clone, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SolverSettings {
    /// Base pose everything starts from.
    pub idle_pose: String,
    /// Follow spring of the joint rotations.
    pub follow: SpringParams,
    pub gaze: GazeLimits,
    /// Head forward axis in the head joint's frame.
    pub head_forward: Vec3,
    /// Forward pitch speed (rad/s) given to the spine per m/s of landing speed; the follow
    /// spring brings it back to the base pose.
    pub landing_kick: f32,
}

impl Default for SolverSettings {
    fn default() -> Self {
        Self {
            idle_pose: "idle".into(),
            follow: SpringParams::new(6.0, 0.8),
            gaze: GazeLimits::default(),
            head_forward: Vec3::NEG_Z,
            landing_kick: 1.5,
        }
    }
}

/// How well a goal was met, in meters (model space).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GoalReport {
    pub limb: Limb,
    pub error: f32,
    pub reached: bool,
}

#[derive(Clone, Debug, Default)]
pub struct SolveReport {
    pub goals: Vec<GoalReport>,
    pub gaze_within_limits: Option<bool>,
    /// World-space end position of every limb after solving.
    pub ends: Vec<(Limb, Vec3)>,
}

pub struct SolveFrame<'a> {
    /// World transform of the rig root.
    pub root: Transform,
    pub up: Vec3,
    pub base_poses: &'a BasePoseSet,
    /// Persistent constraints, for pose layers referenced by `ResolvedConstraint::index`.
    pub constraints: &'a [AnimConstraint],
    /// Arbitrated world-space goals for this frame, including foot contact.
    pub goals: &'a [ResolvedConstraint],
    /// Locomotion result; `body_weight` scales its pelvis motion and deformation.
    pub locomotion: Option<&'a LocomotionOutput>,
    pub body_weight: f32,
    pub moving: Option<&'a MovePose>,
    pub dt: f32,
}

/// Foot-contact goals from locomotion, weighted by `weight` (fades feet to the base pose).
pub fn foot_goals(
    output: &LocomotionOutput,
    up: Vec3,
    root_rotation: Quat,
    weight: f32,
) -> Vec<ResolvedConstraint> {
    output
        .feet
        .iter()
        .map(|f| ResolvedConstraint {
            index: None,
            source: f.limb,
            property: ConstraintProperty::Pose,
            position: f.position,
            // The foot lies flat on the ground: the body's orientation swung onto the normal.
            rotation: Quat::from_rotation_arc(up, f.normal) * root_rotation,
            weight,
            priority: 0,
        })
        .collect()
}

pub struct PoseSolver {
    pub rig: Rig,
    pub settings: SolverSettings,
    pub springs: PoseSpring,
    resolved: HashMap<String, ResolvedBasePose>,
    masks: HashMap<Limb, BoneMask>,
    pub report: SolveReport,
}

impl PoseSolver {
    pub fn new(rig: Rig, settings: SolverSettings) -> Self {
        Self {
            rig,
            settings,
            springs: PoseSpring::default(),
            resolved: HashMap::new(),
            masks: HashMap::new(),
            report: SolveReport::default(),
        }
    }

    fn layer(
        &mut self,
        name: &str,
        set: &BasePoseSet,
        pose: &mut Pose,
        weight: f32,
        limb: Option<Limb>,
    ) -> Result<(), AnimError> {
        if !self.resolved.contains_key(name) {
            self.resolved
                .insert(name.to_owned(), set.resolve(name, &self.rig.skeleton)?);
        }
        let mask = match limb {
            Some(limb) => {
                if !self.masks.contains_key(&limb) {
                    self.masks.insert(limb, self.rig.limb_end_mask(limb)?);
                }
                self.masks.get(&limb)
            }
            None => None,
        };
        self.resolved[name].apply(pose, weight, mask);
        Ok(())
    }

    pub fn solve(&mut self, frame: &SolveFrame) -> Result<Pose, AnimError> {
        let mut pose = self.rig.skeleton.rest_pose();

        // 1. Authored attractors: idle, then hand shapes of active constraints.
        let idle = self.settings.idle_pose.clone();
        self.layer(&idle, frame.base_poses, &mut pose, 1.0, None)?;
        for goal in frame.goals {
            if let Some(name) = goal
                .index
                .and_then(|i| frame.constraints[i].pose.as_deref())
            {
                self.layer(
                    name,
                    frame.base_poses,
                    &mut pose,
                    goal.weight,
                    Some(goal.source),
                )?;
            }
        }

        // 2. Springs pull joints back toward the pose above; impacts displace the spine first.
        if let Some(speed) = frame.locomotion.and_then(|l| l.landed) {
            let spine = &self.rig.binding(Limb::Head)?.chain;
            for &joint in &spine[..spine.len().saturating_sub(1)] {
                let kick = Vec3::NEG_X * self.settings.landing_kick * speed * frame.body_weight;
                self.springs.kick(joint, kick);
            }
        }
        let mut pose = self.springs.step(&pose, self.settings.follow, frame.dt);

        // 3. Body: bob, crouch, lean.
        let skeleton = &self.rig.skeleton;
        let inverse_root = frame.root.compute_affine().inverse();
        let root_rotation_inv = frame.root.rotation.inverse();
        if let Some(loco) = frame.locomotion {
            let up_local = root_rotation_inv * frame.up;
            let w = frame.body_weight.clamp(0.0, 1.0);
            let pelvis = &mut pose.locals[self.rig.pelvis];
            pelvis.translation += up_local * loco.body_offset * w;
            pelvis.rotation = pelvis.rotation.slerp(pelvis.rotation * loco.lean, w);
        }
        let mut model = skeleton.model_transforms(&pose);

        // 4. Constraint solvers.
        let mut report = SolveReport::default();
        // Gaze first: it turns spine joints that carry the arms, and the limb solvers must have
        // the last word on where hands and feet end up.
        let ordered = frame
            .goals
            .iter()
            .filter(|g| g.property == ConstraintProperty::Direction)
            .chain(
                frame
                    .goals
                    .iter()
                    .filter(|g| g.property != ConstraintProperty::Direction),
            );
        for goal in ordered {
            if goal.weight <= 0.0 {
                continue;
            }
            let binding = self.rig.binding(goal.source)?;
            match goal.property {
                ConstraintProperty::Position
                | ConstraintProperty::Pose
                | ConstraintProperty::Orientation
                    if binding.chain.len() == 3 =>
                {
                    let chain = [binding.chain[0], binding.chain[1], binding.chain[2]];
                    let end = chain[2];
                    let mut result = ik::IkResult {
                        error: 0.0,
                        reached: true,
                    };
                    if goal.property != ConstraintProperty::Orientation {
                        let target = inverse_root.transform_point3(goal.position);
                        result = ik::apply_two_bone(
                            skeleton,
                            &mut pose,
                            &mut model,
                            chain,
                            target,
                            binding.pole,
                            goal.weight,
                        );
                    }
                    if goal.property != ConstraintProperty::Position {
                        ik::align_rotation(
                            skeleton,
                            &mut pose,
                            &mut model,
                            end,
                            root_rotation_inv * goal.rotation,
                            goal.weight,
                        );
                    }
                    report.goals.push(GoalReport {
                        limb: goal.source,
                        error: result.error,
                        reached: result.reached,
                    });
                }
                ConstraintProperty::Position
                | ConstraintProperty::Pose
                | ConstraintProperty::Orientation
                    if binding.chain.len() == 1 =>
                {
                    // Single-joint limb (pelvis): move and turn the joint itself.
                    let joint = binding.chain[0];
                    let parent_model = skeleton.joints()[joint]
                        .parent
                        .map_or(Transform::IDENTITY, |p| model[p]);
                    let mut error = 0.0;
                    if goal.property != ConstraintProperty::Orientation {
                        let start = model[joint].translation;
                        let goal_model = inverse_root.transform_point3(goal.position);
                        let target = parent_model
                            .compute_affine()
                            .inverse()
                            .transform_point3(goal_model);
                        let local = &mut pose.locals[joint];
                        local.translation = local.translation.lerp(target, goal.weight);
                        skeleton.update_subtree(&pose, &mut model, joint);
                        error = model[joint]
                            .translation
                            .distance(start.lerp(goal_model, goal.weight));
                    }
                    if goal.property != ConstraintProperty::Position {
                        ik::align_rotation(
                            skeleton,
                            &mut pose,
                            &mut model,
                            joint,
                            root_rotation_inv * goal.rotation,
                            goal.weight,
                        );
                    }
                    report.goals.push(GoalReport {
                        limb: goal.source,
                        error,
                        reached: true,
                    });
                }
                ConstraintProperty::Direction => {
                    let head = *binding.chain.last().expect("limb chains are not empty");
                    let head_world = frame.root.transform_point(model[head].translation);
                    let direction = root_rotation_inv * (goal.position - head_world);
                    let result = ik::apply_look_at(
                        skeleton,
                        &mut pose,
                        &mut model,
                        &binding.chain,
                        self.settings.head_forward,
                        direction,
                        self.settings.gaze,
                        goal.weight,
                    );
                    report.gaze_within_limits = Some(result.within_limits);
                }
                _ => {}
            }
        }

        // 5. Deformation (squash and stretch of the whole body about the root).
        if let Some(loco) = frame.locomotion {
            let w = frame.body_weight.clamp(0.0, 1.0);
            pose.locals[self.rig.root].scale = Vec3::ONE.lerp(loco.scale, w);
        }
        if let Some(moving) = frame.moving {
            moving.apply(&self.rig, frame.base_poses, frame.root, frame.up, &mut pose)?;
        }
        let final_model = skeleton.model_transforms(&pose);
        report.ends = self
            .rig
            .limbs
            .iter()
            .map(|b| {
                (
                    b.limb,
                    frame.root.transform_point(
                        final_model[*b.chain.last().expect("limb chains are not empty")]
                            .translation,
                    ),
                )
            })
            .collect();
        self.report = report;
        Ok(pose)
    }
}
