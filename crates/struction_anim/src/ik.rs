//! Inverse kinematics: analytic two-bone with pole vector, FABRIK for chains, and gaze
//! (look-at with limits). Pure math first, then adapters that turn solved positions into local
//! joint rotations on a `Pose`.
//!
//! Joint rotations are changed by swing only (minimal rotation), so twist along a bone is
//! whatever the pose already had. Assumes uniform positive joint scales along a solved chain.

use bevy::math::{Quat, Vec3};
use bevy::reflect::Reflect;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::pose::Pose;
use crate::skeleton::Skeleton;

const EPS: f32 = 1e-5;

/// Positions of a solved two-bone chain.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwoBoneSolution {
    pub mid: Vec3,
    pub end: Vec3,
    /// False when the target was out of reach and the chain is fully extended toward it.
    pub reached: bool,
}

/// Analytic two-bone solve. `pole` is a direction the middle joint bends toward.
pub fn solve_two_bone(
    root: Vec3,
    upper: f32,
    lower: f32,
    target: Vec3,
    pole: Vec3,
) -> TwoBoneSolution {
    let to_target = target - root;
    let raw = to_target.length();
    let dir = if raw > EPS { to_target / raw } else { Vec3::Y };
    let min_reach = (upper - lower).abs() + EPS;
    let max_reach = upper + lower - EPS;
    let dist = raw.clamp(min_reach, max_reach);
    let reached = raw <= max_reach && raw >= min_reach;

    let along = (upper * upper + dist * dist - lower * lower) / (2.0 * dist);
    let height = (upper * upper - along * along).max(0.0).sqrt();
    let mut bend = pole - dir * pole.dot(dir);
    if bend.length_squared() < EPS {
        bend = dir.any_orthonormal_vector();
    }
    TwoBoneSolution {
        mid: root + dir * along + bend.normalize() * height,
        end: root + dir * dist,
        reached,
    }
}

/// Rotates `joint` by `delta` (model-space rotation about the joint) and refreshes its subtree.
pub fn rotate_joint(
    skeleton: &Skeleton,
    pose: &mut Pose,
    model: &mut [Transform],
    joint: usize,
    delta: Quat,
) {
    let parent_rotation = skeleton.joints()[joint]
        .parent
        .map_or(Quat::IDENTITY, |p| model[p].rotation);
    let rotated = (delta * model[joint].rotation).normalize();
    pose.locals[joint].rotation = (parent_rotation.inverse() * rotated).normalize();
    skeleton.update_subtree(pose, model, joint);
}

fn arc(from: Vec3, to: Vec3) -> Quat {
    if from.length_squared() < EPS || to.length_squared() < EPS {
        Quat::IDENTITY
    } else {
        Quat::from_rotation_arc(from.normalize(), to.normalize())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IkResult {
    /// Distance between the chain end and the (weight-blended) goal after solving.
    pub error: f32,
    pub reached: bool,
}

/// Two-bone IK on `chain = [root, mid, end]`. `weight` blends the goal from the end joint's
/// current position (0) to `target` (1); `model` must be current for `pose`.
pub fn apply_two_bone(
    skeleton: &Skeleton,
    pose: &mut Pose,
    model: &mut [Transform],
    chain: [usize; 3],
    target: Vec3,
    pole: Vec3,
    weight: f32,
) -> IkResult {
    let [a, b, c] = chain;
    let root = model[a].translation;
    let mid = model[b].translation;
    let end = model[c].translation;
    let goal = end.lerp(target, weight.clamp(0.0, 1.0));
    let solution = solve_two_bone(root, root.distance(mid), mid.distance(end), goal, pole);

    rotate_joint(
        skeleton,
        pose,
        model,
        a,
        arc(mid - root, solution.mid - root),
    );
    let mid = model[b].translation;
    let end = model[c].translation;
    rotate_joint(skeleton, pose, model, b, arc(end - mid, solution.end - mid));
    IkResult {
        error: model[c].translation.distance(goal),
        reached: solution.reached,
    }
}

/// FABRIK on `points` (root first, root pinned). Returns whether the end is within
/// `tolerance` of `target`.
pub fn fabrik(points: &mut [Vec3], target: Vec3, tolerance: f32, max_iterations: usize) -> bool {
    let n = points.len();
    if n < 2 {
        return false;
    }
    let lengths: Vec<f32> = points.windows(2).map(|w| w[0].distance(w[1])).collect();
    let total: f32 = lengths.iter().sum();
    let root = points[0];
    if root.distance(target) >= total {
        let dir = (target - root).normalize_or_zero();
        let mut at = root;
        for i in 1..n {
            at += dir * lengths[i - 1];
            points[i] = at;
        }
        return root.distance(target) - total <= tolerance;
    }
    for _ in 0..max_iterations {
        if points[n - 1].distance(target) <= tolerance {
            break;
        }
        points[n - 1] = target;
        for i in (0..n - 1).rev() {
            let dir = (points[i] - points[i + 1]).normalize_or_zero();
            points[i] = points[i + 1] + dir * lengths[i];
        }
        points[0] = root;
        for i in 1..n {
            let dir = (points[i] - points[i - 1]).normalize_or_zero();
            points[i] = points[i - 1] + dir * lengths[i - 1];
        }
    }
    points[n - 1].distance(target) <= tolerance
}

/// FABRIK over a joint chain (root first, at least two joints), then swing each bone onto the
/// solved direction. `weight` blends the goal like `apply_two_bone`.
pub fn apply_chain(
    skeleton: &Skeleton,
    pose: &mut Pose,
    model: &mut [Transform],
    chain: &[usize],
    target: Vec3,
    weight: f32,
) -> IkResult {
    let mut points: Vec<Vec3> = chain.iter().map(|&j| model[j].translation).collect();
    let end = *points.last().expect("chain has joints");
    let goal = end.lerp(target, weight.clamp(0.0, 1.0));
    let reached = fabrik(&mut points, goal, 1e-4, 24);
    for i in 0..chain.len() - 1 {
        let here = model[chain[i]].translation;
        let current = model[chain[i + 1]].translation - here;
        rotate_joint(
            skeleton,
            pose,
            model,
            chain[i],
            arc(current, points[i + 1] - here),
        );
    }
    IkResult {
        error: model[*chain.last().expect("chain has joints")]
            .translation
            .distance(goal),
        reached,
    }
}

/// Limits of the gaze cone around the body's forward direction, radians.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct GazeLimits {
    pub yaw: f32,
    pub pitch_up: f32,
    pub pitch_down: f32,
}

impl Default for GazeLimits {
    fn default() -> Self {
        Self {
            yaw: 80f32.to_radians(),
            pitch_up: 45f32.to_radians(),
            pitch_down: 55f32.to_radians(),
        }
    }
}

/// Clamps a direction given in the body frame (-Z forward, Y up). Returns the clamped
/// direction and whether it was already within limits.
pub fn clamp_gaze(direction: Vec3, limits: GazeLimits) -> (Vec3, bool) {
    let d = direction.normalize_or_zero();
    if d == Vec3::ZERO {
        return (Vec3::NEG_Z, true);
    }
    let yaw = (-d.x).atan2(-d.z);
    let pitch = d.y.clamp(-1.0, 1.0).asin();
    let clamped_yaw = yaw.clamp(-limits.yaw, limits.yaw);
    let clamped_pitch = pitch.clamp(-limits.pitch_down, limits.pitch_up);
    let within = (yaw - clamped_yaw).abs() < EPS && (pitch - clamped_pitch).abs() < EPS;
    let (sy, cy) = clamped_yaw.sin_cos();
    let (sp, cp) = clamped_pitch.sin_cos();
    (Vec3::new(-sy * cp, sp, -cy * cp), within)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GazeResult {
    pub within_limits: bool,
    /// Angle between the head's forward axis and the (clamped) gaze direction afterwards.
    pub error: f32,
}

/// Turns the head chain (`chain` root to head, distributed with increasing share toward the
/// head) so the head's `forward` axis looks along `direction` (model space), limited by
/// `limits` relative to the model's forward. `weight` scales the turn.
#[expect(clippy::too_many_arguments, reason = "plain math entry point")]
pub fn apply_look_at(
    skeleton: &Skeleton,
    pose: &mut Pose,
    model: &mut [Transform],
    chain: &[usize],
    forward: Vec3,
    direction: Vec3,
    limits: GazeLimits,
    weight: f32,
) -> GazeResult {
    let head = *chain.last().expect("chain has joints");
    let (desired, within_limits) = clamp_gaze(direction, limits);
    let current = model[head].rotation * forward;
    let full = arc(current, desired);
    let shares: Vec<f32> = (1..=chain.len()).map(|k| (k * k) as f32).collect();
    let total: f32 = shares.iter().sum();
    for (&joint, share) in chain.iter().zip(&shares) {
        let delta = Quat::IDENTITY.slerp(full, weight.clamp(0.0, 1.0) * share / total);
        rotate_joint(skeleton, pose, model, joint, delta);
    }
    GazeResult {
        within_limits,
        error: (model[head].rotation * forward).angle_between(desired),
    }
}

/// Swings `joint` so its local `axis` points along `direction` (model space).
pub fn align_axis(
    skeleton: &Skeleton,
    pose: &mut Pose,
    model: &mut [Transform],
    joint: usize,
    axis: Vec3,
    direction: Vec3,
    weight: f32,
) {
    let full = arc(model[joint].rotation * axis, direction);
    rotate_joint(
        skeleton,
        pose,
        model,
        joint,
        Quat::IDENTITY.slerp(full, weight.clamp(0.0, 1.0)),
    );
}

/// Blends `joint`'s model-space rotation toward `rotation`.
pub fn align_rotation(
    skeleton: &Skeleton,
    pose: &mut Pose,
    model: &mut [Transform],
    joint: usize,
    rotation: Quat,
    weight: f32,
) {
    let full = rotation * model[joint].rotation.inverse();
    rotate_joint(
        skeleton,
        pose,
        model,
        joint,
        Quat::IDENTITY.slerp(full, weight.clamp(0.0, 1.0)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid;
    use crate::rig::Limb;

    #[test]
    fn two_bone_reaches_reachable_targets_and_keeps_lengths() {
        let root = Vec3::new(0.1, 1.0, -0.2);
        let (u, l) = (0.45, 0.4);
        for i in 0..200 {
            let t = i as f32 * 0.37;
            let target = root
                + Vec3::new(t.sin(), t.cos() * 0.8, (t * 1.7).sin())
                    * (0.1 + (i % 10) as f32 * 0.07);
            if root.distance(target) > u + l - 0.01 || root.distance(target) < 0.06 {
                continue;
            }
            let s = solve_two_bone(root, u, l, target, Vec3::new(0.3, 0.1, -1.0));
            assert!(s.reached);
            assert!(s.end.distance(target) < 1e-4);
            assert!((root.distance(s.mid) - u).abs() < 1e-4);
            assert!((s.mid.distance(s.end) - l).abs() < 1e-4);
        }
    }

    #[test]
    fn pole_selects_bend_side_and_unreachable_extends() {
        let root = Vec3::ZERO;
        let s = solve_two_bone(root, 0.5, 0.5, Vec3::new(0.0, -0.8, 0.0), Vec3::NEG_Z);
        assert!(s.mid.z < -0.1);
        let s = solve_two_bone(root, 0.5, 0.5, Vec3::new(0.0, -0.8, 0.0), Vec3::Z);
        assert!(s.mid.z > 0.1);

        let far = solve_two_bone(root, 0.5, 0.5, Vec3::new(0.0, -3.0, 0.0), Vec3::Z);
        assert!(!far.reached);
        assert!((far.end - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn two_bone_on_skeleton_hits_target_and_weight_blends() {
        let rig = humanoid::rig();
        let sk = &rig.skeleton;
        let arm = rig.binding(Limb::RightHand).unwrap();
        let chain = [arm.chain[0], arm.chain[1], arm.chain[2]];
        let mut pose = sk.rest_pose();
        let mut model = sk.model_transforms(&pose);
        let start = model[chain[2]].translation;
        let target = model[chain[0]].translation + Vec3::new(0.1, -0.1, -0.4);

        let mut half = pose.clone();
        let mut half_model = model.clone();
        apply_two_bone(sk, &mut half, &mut half_model, chain, target, arm.pole, 0.5);
        assert!(
            half_model[chain[2]]
                .translation
                .distance(start.lerp(target, 0.5))
                < 1e-4
        );

        let r = apply_two_bone(sk, &mut pose, &mut model, chain, target, arm.pole, 1.0);
        assert!(r.reached && r.error < 1e-4);
        let fk = sk.model_transforms(&pose);
        assert!(fk[chain[2]].translation.distance(target) < 1e-4);
    }

    #[test]
    fn fabrik_reaches_and_preserves_lengths() {
        let mut pts: Vec<Vec3> = (0..5)
            .map(|i| Vec3::new(0.0, i as f32 * 0.3, 0.0))
            .collect();
        let target = Vec3::new(0.5, 0.6, 0.3);
        assert!(fabrik(&mut pts, target, 1e-3, 32));
        assert!(pts[4].distance(target) < 1e-3);
        assert!(pts[0].distance(Vec3::ZERO) < 1e-6);
        for w in pts.windows(2) {
            assert!((w[0].distance(w[1]) - 0.3).abs() < 1e-3);
        }
        assert!(!fabrik(&mut pts, Vec3::new(0.0, 5.0, 0.0), 1e-3, 32));
    }

    #[test]
    fn chain_ik_on_skeleton_reaches_with_fabrik() {
        let rig = humanoid::rig();
        let sk = &rig.skeleton;
        let head = rig.binding(Limb::Head).unwrap();
        let mut pose = sk.rest_pose();
        let mut model = sk.model_transforms(&pose);
        let target = model[*head.chain.last().unwrap()].translation + Vec3::new(0.15, -0.1, -0.15);
        let r = apply_chain(sk, &mut pose, &mut model, &head.chain, target, 1.0);
        assert!(r.reached && r.error < 1e-3);
    }

    #[test]
    fn gaze_is_clamped_and_reports_limits() {
        let limits = GazeLimits::default();
        let (d, within) = clamp_gaze(Vec3::new(-1.0, 0.0, -1.0), limits);
        assert!(within && d.x < 0.0);
        let (d, within) = clamp_gaze(Vec3::new(0.0, 0.0, 1.0), limits);
        assert!(!within);
        let yaw = (-d.x).atan2(-d.z).abs();
        assert!((yaw - limits.yaw).abs() < 1e-4);
        let (d, within) = clamp_gaze(Vec3::Y, limits);
        assert!(!within && (d.y.asin() - limits.pitch_up).abs() < 1e-4);
    }

    #[test]
    fn look_at_turns_the_head_within_limits() {
        let rig = humanoid::rig();
        let sk = &rig.skeleton;
        let head = rig.binding(Limb::Head).unwrap();
        let head_joint = *head.chain.last().unwrap();
        let mut pose = sk.rest_pose();
        let mut model = sk.model_transforms(&pose);
        let dir = Vec3::new(-0.5, 0.2, -1.0).normalize();
        let r = apply_look_at(
            sk,
            &mut pose,
            &mut model,
            &head.chain,
            Vec3::NEG_Z,
            dir,
            GazeLimits::default(),
            1.0,
        );
        assert!(r.within_limits && r.error < 1e-3);
        assert!((model[head_joint].rotation * Vec3::NEG_Z).angle_between(dir) < 1e-3);

        let mut pose = sk.rest_pose();
        let mut model = sk.model_transforms(&pose);
        let r = apply_look_at(
            sk,
            &mut pose,
            &mut model,
            &head.chain,
            Vec3::NEG_Z,
            Vec3::Z,
            GazeLimits::default(),
            1.0,
        );
        assert!(!r.within_limits && r.error < 1e-3);
    }
}
