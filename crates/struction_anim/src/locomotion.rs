//! Procedural legs: reactive stepping from velocity, ground contact and local up.
//!
//! Each planted foot is fixed in world space. A foot lifts when it trails its home position (the
//! rest foot position under the body, projected to the ground) by more than half a stride, or
//! when the leg nears full extension; it swings along an arc to a landing spot predicted from the
//! body's velocity, and plants again. Legs of different groups do not swing together unless a
//! leg is about to overstretch, which gives alternating stance and swing without a gait clock.
//! Everything is expressed through the root transform and `up`, so walking on a planet needs no
//! special case. Ground comes from a [`Ground`] query, not from the physics crate.

use bevy::math::{Quat, Vec3};
use bevy::reflect::Reflect;
use bevy::transform::components::Transform;
use serde::{Deserialize, Serialize};

use crate::constraint::smoothstep;
use crate::error::AnimError;
use crate::rig::{Limb, Rig};
use crate::spring::{SpringF32, SpringParams, SpringQuat, SquashStretch};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundHit {
    pub point: Vec3,
    pub normal: Vec3,
}

/// Downward ray query against whatever the game uses as ground. `direction` is a unit vector.
pub trait Ground {
    fn cast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<GroundHit>;
}

impl<F> Ground for F
where
    F: Fn(Vec3, Vec3, f32) -> Option<GroundHit>,
{
    fn cast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<GroundHit> {
        self(origin, direction, max_distance)
    }
}

/// Infinite plane, handy for tests and as a fallback ground.
#[derive(Clone, Copy, Debug)]
pub struct PlaneGround {
    pub point: Vec3,
    pub normal: Vec3,
}

impl Ground for PlaneGround {
    fn cast(&self, origin: Vec3, direction: Vec3, max_distance: f32) -> Option<GroundHit> {
        let n = self.normal.normalize();
        let denom = direction.dot(n);
        if denom.abs() < 1e-6 {
            return None;
        }
        let t = (self.point - origin).dot(n) / denom;
        (0.0..=max_distance).contains(&t).then(|| GroundHit {
            point: origin + direction * t,
            normal: n,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct LocomotionParams {
    /// Half stride at rest and per m/s of speed; the stride is capped at `max_half_stride`.
    pub half_stride: f32,
    pub half_stride_per_speed: f32,
    pub max_half_stride: f32,
    /// Distance from home that starts a step when standing (moving uses the stride).
    pub idle_trigger: f32,
    /// Fraction of leg reach that forces a step.
    pub reach_limit: f32,
    pub step_height: f32,
    pub min_swing: f32,
    pub max_swing: f32,
    /// Swing time when starting from standstill.
    pub idle_swing: f32,
    /// Pelvis drop that keeps the knees slightly bent.
    pub crouch: f32,
    pub bob: SpringParams,
    /// Downward pelvis speed added per footfall, m/s.
    pub footfall_impulse: f32,
    pub lean: SpringParams,
    /// Forward lean per m/s of speed and per m/s^2 of acceleration, radians.
    pub lean_per_speed: f32,
    pub lean_per_acceleration: f32,
    pub max_lean: f32,
    pub probe_up: f32,
    pub probe_down: f32,
    /// Height of the root origin above the ground when standing, for landing prediction.
    pub root_height: f32,
    /// Feet lift toward the pelvis by this much in flight.
    pub air_tuck: f32,
    /// Seconds before touchdown that the feet start reaching for the landing spot.
    pub landing_anticipation: f32,
    pub squash: SquashStretch,
}

impl Default for LocomotionParams {
    fn default() -> Self {
        Self {
            half_stride: 0.10,
            half_stride_per_speed: 0.05,
            max_half_stride: 0.22,
            idle_trigger: 0.15,
            reach_limit: 0.95,
            step_height: 0.10,
            min_swing: 0.10,
            max_swing: 0.32,
            idle_swing: 0.22,
            crouch: 0.06,
            bob: SpringParams::new(4.0, 0.4),
            footfall_impulse: 0.3,
            lean: SpringParams::new(3.0, 0.7),
            lean_per_speed: 0.025,
            lean_per_acceleration: 0.01,
            max_lean: 0.2,
            probe_up: 0.5,
            probe_down: 1.5,
            root_height: 0.0,
            air_tuck: 0.25,
            landing_anticipation: 0.3,
            squash: SquashStretch::default(),
        }
    }
}

/// Static description of one leg, in rig model space at the rest pose.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct LegSpec {
    pub limb: Limb,
    pub hip: Vec3,
    pub rest_foot: Vec3,
    pub reach: f32,
    /// Legs in different groups alternate (left/right of a biped, diagonals of a quadruped).
    pub group: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub enum LegPhase {
    Planted,
    Swing { from: Vec3, elapsed: f32, duration: f32 },
    Air,
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct LegState {
    pub spec: LegSpec,
    pub phase: LegPhase,
    /// World-space ankle position and ground normal the foot currently occupies.
    pub foot: Vec3,
    pub normal: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Landing {
    pub point: Vec3,
    pub normal: Vec3,
    pub time: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct LocomotionInput {
    /// World transform of the rig root; its axes define the body's forward and right.
    pub root: Transform,
    pub velocity: Vec3,
    pub up: Vec3,
    pub grounded: bool,
    /// Gravitational acceleration, for landing prediction.
    pub gravity: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootTarget {
    pub limb: Limb,
    pub position: Vec3,
    pub normal: Vec3,
    pub planted: bool,
}

#[derive(Clone, Debug, Default)]
pub struct LocomotionOutput {
    pub feet: Vec<FootTarget>,
    /// Pelvis offset along up, meters (negative = lower).
    pub body_offset: f32,
    /// Body lean in the root frame.
    pub lean: Quat,
    pub scale: Vec3,
    pub landing: Option<Landing>,
    /// Fall speed of a touchdown that happened this update.
    pub landed: Option<f32>,
    pub footfalls: u32,
}

#[derive(Clone, Debug, Reflect)]
pub struct LocomotionState {
    pub params: LocomotionParams,
    pub legs: Vec<LegState>,
    bob: SpringF32,
    lean: SpringQuat,
    squash: SquashStretch,
    previous_velocity: Option<Vec3>,
    was_grounded: bool,
    initialized: bool,
    #[reflect(ignore)]
    output: LocomotionOutput,
}

fn project(params: &LocomotionParams, ground: &dyn Ground, point: Vec3, up: Vec3, ankle: f32) -> (Vec3, Vec3) {
    let origin = point + up * params.probe_up;
    match ground.cast(origin, -up, params.probe_up + params.probe_down) {
        Some(hit) => (hit.point + hit.normal * ankle, hit.normal),
        None => (point, up),
    }
}

fn tangent(v: Vec3, up: Vec3) -> Vec3 {
    v - up * v.dot(up)
}

impl LocomotionState {
    pub fn new(specs: Vec<LegSpec>, params: LocomotionParams) -> Self {
        let legs = specs
            .into_iter()
            .map(|spec| LegState {
                spec,
                phase: LegPhase::Planted,
                foot: Vec3::ZERO,
                normal: Vec3::Y,
            })
            .collect();
        Self {
            squash: params.squash,
            params,
            legs,
            bob: SpringF32::default(),
            lean: SpringQuat::default(),
            previous_velocity: None,
            was_grounded: true,
            initialized: false,
            output: LocomotionOutput::default(),
        }
    }

    /// Derives leg specs from the rig's foot chains at the skeleton rest pose.
    pub fn from_rig(rig: &Rig, params: LocomotionParams) -> Result<Self, AnimError> {
        let model = rig.skeleton.model_transforms(&rig.skeleton.rest_pose());
        let specs = rig
            .feet()
            .map(|binding| {
                let [hip, knee, foot] = [binding.chain[0], binding.chain[1], binding.chain[2]]
                    .map(|j| model[j].translation);
                LegSpec {
                    limb: binding.limb,
                    hip,
                    rest_foot: foot,
                    reach: hip.distance(knee) + knee.distance(foot),
                    group: match binding.limb {
                        Limb::LeftFoot => 0,
                        Limb::RightFoot => 1,
                        Limb::ExtraFoot(i) => i % 2,
                        _ => 0,
                    },
                }
            })
            .collect();
        Ok(Self::new(specs, params))
    }

    pub fn output(&self) -> &LocomotionOutput {
        &self.output
    }

    /// Advances the legs and secondary motion by `dt` seconds.
    pub fn update(&mut self, input: &LocomotionInput, ground: &dyn Ground, dt: f32) -> &LocomotionOutput {
        let dt = dt.max(1e-4);
        let up = input.up.normalize_or(Vec3::Y);
        let velocity_t = tangent(input.velocity, up);
        let speed = velocity_t.length();
        let dir = (speed > 0.05).then(|| velocity_t / speed);

        if !self.initialized {
            self.initialized = true;
            for leg in &mut self.legs {
                let flat = input.root.transform_point(leg.spec.rest_foot);
                let (foot, normal) = project(&self.params, ground, flat, up, leg.spec.rest_foot.y);
                leg.foot = foot;
                leg.normal = normal;
            }
            self.previous_velocity = Some(input.velocity);
            // Start at rest height instead of easing in, which would overstretch the legs.
            self.bob = SpringF32::at(-self.params.crouch);
        }

        let previous_velocity = self.previous_velocity.unwrap_or(input.velocity);
        let accel = (input.velocity - previous_velocity) / dt;
        self.previous_velocity = Some(input.velocity);
        let accel = accel.clamp_length_max(60.0);

        let mut footfalls = 0;
        let mut landed = None;
        let mut landing = None;
        if input.grounded {
            if !self.was_grounded {
                // The controller has already zeroed the velocity at touchdown; use the last airborne one.
                let impact = (-previous_velocity.dot(up)).max(0.0);
                landed = Some(impact);
                self.bob.velocity -= impact * 0.15;
                self.squash.impact(impact);
                for leg in &mut self.legs {
                    let flat = input.root.transform_point(leg.spec.rest_foot);
                    let (foot, normal) = project(&self.params, ground, flat, up, leg.spec.rest_foot.y);
                    leg.phase = LegPhase::Planted;
                    leg.foot = foot;
                    leg.normal = normal;
                }
            }
            footfalls = self.step_legs(input, ground, up, velocity_t, speed, dir, dt);
        } else {
            landing = predict_landing(
                input.root.translation - up * self.params.root_height,
                input.velocity,
                input.gravity,
                ground,
                3.0,
            );
            self.air_legs(input, ground, up, landing);
        }
        self.was_grounded = input.grounded;

        // Secondary motion.
        for _ in 0..footfalls {
            self.bob.velocity -= self.params.footfall_impulse * (0.5 + 0.25 * speed.min(4.0));
        }
        let ground_follow = self.ground_follow(input, up);
        let bob_target = -self.params.crouch + ground_follow;
        self.bob.step(bob_target, self.params.bob, dt);

        let forward = input.root.rotation * Vec3::NEG_Z;
        let right = input.root.rotation * Vec3::X;
        let pitch = -(input.velocity.dot(forward) * self.params.lean_per_speed
            + accel.dot(forward) * self.params.lean_per_acceleration);
        let roll = -accel.dot(right) * self.params.lean_per_acceleration;
        let max = self.params.max_lean;
        let target = Quat::from_rotation_x(pitch.clamp(-max, max))
            * Quat::from_rotation_z(roll.clamp(-max, max));
        self.lean.step(target, self.params.lean, dt);

        self.squash.update(accel.dot(up), dt);

        self.output = LocomotionOutput {
            feet: self
                .legs
                .iter()
                .map(|l| FootTarget {
                    limb: l.spec.limb,
                    position: l.foot,
                    normal: l.normal,
                    planted: l.phase == LegPhase::Planted,
                })
                .collect(),
            body_offset: self.bob.value,
            lean: self.lean.value,
            scale: self.squash.scale(),
            landing,
            landed,
            footfalls,
        };
        &self.output
    }

    /// Average height of the planted feet above where they would be on flat ground.
    fn ground_follow(&self, input: &LocomotionInput, up: Vec3) -> f32 {
        let (sum, n) = self
            .legs
            .iter()
            .filter(|l| l.phase == LegPhase::Planted)
            .fold((0.0, 0), |(s, n), l| {
                let flat = input.root.transform_point(l.spec.rest_foot);
                (s + (l.foot - flat).dot(up), n + 1)
            });
        if n == 0 { 0.0 } else { 0.5 * sum / n as f32 }
    }

    fn half_stride(&self, speed: f32) -> f32 {
        (self.params.half_stride + self.params.half_stride_per_speed * speed)
            .min(self.params.max_half_stride)
    }

    /// Landing spot of a step that finishes in `remaining` seconds.
    fn landing_spot(
        &self,
        input: &LocomotionInput,
        ground: &dyn Ground,
        up: Vec3,
        spec: &LegSpec,
        velocity_t: Vec3,
        dir: Option<Vec3>,
        stride: f32,
        remaining: f32,
    ) -> (Vec3, Vec3) {
        let flat = input.root.transform_point(spec.rest_foot);
        let mut at = flat + velocity_t * remaining;
        if let Some(d) = dir {
            at += d * stride;
        }
        project(&self.params, ground, at, up, spec.rest_foot.y)
    }

    #[allow(clippy::too_many_arguments)]
    fn step_legs(
        &mut self,
        input: &LocomotionInput,
        ground: &dyn Ground,
        up: Vec3,
        velocity_t: Vec3,
        speed: f32,
        dir: Option<Vec3>,
        dt: f32,
    ) -> u32 {
        let stride = self.half_stride(speed);
        let mut footfalls = 0;

        for i in 0..self.legs.len() {
            let leg = self.legs[i];
            match leg.phase {
                LegPhase::Swing { from, elapsed, duration } => {
                    let elapsed = elapsed + dt;
                    let remaining = (duration - elapsed).max(0.0);
                    let (target, normal) =
                        self.landing_spot(input, ground, up, &leg.spec, velocity_t, dir, stride, remaining);
                    let leg = &mut self.legs[i];
                    if elapsed >= duration {
                        leg.phase = LegPhase::Planted;
                        leg.foot = target;
                        leg.normal = normal;
                        footfalls += 1;
                    } else {
                        let t = elapsed / duration;
                        let length = from.distance(target);
                        let height = self.params.step_height * (0.5 + 0.5 * (length / 0.4).min(1.0));
                        leg.foot = from.lerp(target, smoothstep(t)) + up * (height * (core::f32::consts::PI * t).sin());
                        leg.normal = leg.normal.lerp(normal, 0.3).normalize_or(up);
                        leg.phase = LegPhase::Swing { from, elapsed, duration };
                    }
                }
                LegPhase::Air => {
                    // Landing while the legs were still airborne is handled by the caller.
                }
                LegPhase::Planted => {}
            }
        }

        // Planted legs that want to step, most urgent first.
        let hip_lift = up * self.bob.value;
        let mut candidates: Vec<(usize, f32, bool)> = Vec::new();
        for (i, leg) in self.legs.iter().enumerate() {
            if leg.phase != LegPhase::Planted {
                continue;
            }
            let flat = input.root.transform_point(leg.spec.rest_foot);
            let (home, _) = project(&self.params, ground, flat, up, leg.spec.rest_foot.y);
            let offset = tangent(home - leg.foot, up);
            let behind = dir.map_or(0.0, |d| offset.dot(d));
            let trigger = if dir.is_some() {
                (stride + 0.04).max(self.params.idle_trigger)
            } else {
                self.params.idle_trigger
            };
            let hip = input.root.transform_point(leg.spec.hip) + hip_lift;
            let ratio = hip.distance(leg.foot) / leg.spec.reach;
            let urgent = ratio > self.params.reach_limit;
            let urgency = (behind / stride)
                .max(offset.length() / trigger)
                .max(ratio / self.params.reach_limit);
            if urgent || behind > stride || offset.length() > trigger {
                candidates.push((i, urgency, urgent));
            }
        }
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        for (i, _, urgent) in candidates {
            let group = self.legs[i].spec.group;
            let blocked = self
                .legs
                .iter()
                .any(|l| l.spec.group != group && matches!(l.phase, LegPhase::Swing { .. }));
            if blocked && !urgent {
                continue;
            }
            let duration = if dir.is_some() {
                (0.6 * 2.0 * stride / speed).clamp(self.params.min_swing, self.params.max_swing)
            } else {
                self.params.idle_swing
            };
            let leg = &mut self.legs[i];
            leg.phase = LegPhase::Swing {
                from: leg.foot,
                elapsed: 0.0,
                duration,
            };
        }
        footfalls
    }

    fn air_legs(&mut self, input: &LocomotionInput, ground: &dyn Ground, up: Vec3, landing: Option<Landing>) {
        let approach = landing.map_or(0.0, |l| {
            smoothstep(1.0 - l.time / self.params.landing_anticipation.max(1e-3))
        });
        for leg in &mut self.legs {
            leg.phase = LegPhase::Air;
            let flat = input.root.transform_point(leg.spec.rest_foot);
            let tucked = flat + up * self.params.air_tuck;
            let (reach, normal) = match landing {
                Some(l) => {
                    // Keep the foot's body-relative offset, dropped onto the landing surface.
                    let offset = tangent(flat - input.root.translation, up);
                    let base = l.point + tangent(input.root.translation - l.point, up) + offset;
                    let hit = ground
                        .cast(base + up * self.params.probe_up, -up, self.params.probe_up + self.params.probe_down);
                    match hit {
                        Some(h) => (h.point + h.normal * leg.spec.rest_foot.y, h.normal),
                        None => (l.point + l.normal * leg.spec.rest_foot.y, l.normal),
                    }
                }
                None => (tucked, up),
            };
            leg.foot = tucked.lerp(reach, approach);
            leg.normal = up.lerp(normal, approach).normalize_or(up);
        }
    }
}

/// Predicts where a ballistic body (`position`, `velocity`, constant `gravity`) meets the
/// ground within `max_time` seconds.
pub fn predict_landing(
    position: Vec3,
    velocity: Vec3,
    gravity: Vec3,
    ground: &dyn Ground,
    max_time: f32,
) -> Option<Landing> {
    const STEP: f32 = 0.04;
    let at = |t: f32| position + velocity * t + gravity * (0.5 * t * t);
    let mut t = 0.0;
    while t < max_time {
        let (a, b) = (at(t), at(t + STEP));
        let segment = b - a;
        let length = segment.length();
        if length > 1e-6
            && let Some(hit) = ground.cast(a, segment / length, length)
        {
            // Refine against the parabola: find where it crosses the hit plane.
            let side = |t: f32| (at(t) - hit.point).dot(hit.normal);
            let (mut lo, mut hi) = (t, t + STEP);
            for _ in 0..16 {
                let mid = 0.5 * (lo + hi);
                if side(mid).signum() == side(lo).signum() {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let time = 0.5 * (lo + hi);
            return Some(Landing {
                point: at(time),
                normal: hit.normal,
                time,
            });
        }
        t += STEP;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humanoid;

    #[test]
    fn landing_prediction_matches_analytic_flat_ground() {
        let ground = PlaneGround {
            point: Vec3::ZERO,
            normal: Vec3::Y,
        };
        let (p, v, g) = (Vec3::new(1.0, 4.0, -2.0), Vec3::new(2.0, 3.0, 1.0), Vec3::new(0.0, -9.81, 0.0));
        let landing = predict_landing(p, v, g, &ground, 5.0).unwrap();
        // 4 + 3t - 4.905 t^2 = 0
        let t = (3.0 + (9.0_f32 + 4.0 * 4.905 * 4.0).sqrt()) / (2.0 * 4.905);
        assert!((landing.time - t).abs() < 1e-3, "{} vs {t}", landing.time);
        let expected = Vec3::new(1.0 + 2.0 * t, 0.0, -2.0 + t);
        assert!(landing.point.distance(expected) < 5e-3);
        assert!(predict_landing(p, Vec3::new(0.0, 30.0, 0.0), Vec3::ZERO, &ground, 1.0).is_none());
    }

    #[test]
    fn standing_still_keeps_feet_planted_and_takes_no_steps() {
        let rig = humanoid::rig();
        let mut state = LocomotionState::from_rig(&rig, LocomotionParams::default()).unwrap();
        let ground = PlaneGround {
            point: Vec3::ZERO,
            normal: Vec3::Y,
        };
        let input = LocomotionInput {
            root: Transform::IDENTITY,
            velocity: Vec3::ZERO,
            up: Vec3::Y,
            grounded: true,
            gravity: Vec3::NEG_Y * 9.81,
        };
        let first = state.update(&input, &ground, 1.0 / 60.0).feet.clone();
        for _ in 0..120 {
            state.update(&input, &ground, 1.0 / 60.0);
        }
        let out = state.output();
        assert_eq!(out.footfalls, 0);
        for (a, b) in first.iter().zip(&out.feet) {
            assert!(a.position.distance(b.position) < 1e-6 && b.planted);
            assert!((b.position.y - humanoid::ANKLE_HEIGHT).abs() < 1e-5);
        }
        // Settles at the crouch offset.
        assert!((out.body_offset + LocomotionParams::default().crouch).abs() < 1e-3);
    }

    #[test]
    fn touchdown_reports_impact_and_squashes() {
        let rig = humanoid::rig();
        let mut state = LocomotionState::from_rig(&rig, LocomotionParams::default()).unwrap();
        let ground = PlaneGround {
            point: Vec3::ZERO,
            normal: Vec3::Y,
        };
        let mut input = LocomotionInput {
            root: Transform::from_xyz(0.0, 1.0, 0.0),
            velocity: Vec3::new(0.0, -6.0, 0.0),
            up: Vec3::Y,
            grounded: false,
            gravity: Vec3::NEG_Y * 9.81,
        };
        let out = state.update(&input, &ground, 1.0 / 60.0);
        assert!(out.landing.is_some());
        assert!(out.feet.iter().all(|f| !f.planted));
        input.grounded = true;
        input.root.translation.y = 0.0;
        input.velocity = Vec3::ZERO;
        let out = state.update(&input, &ground, 1.0 / 60.0);
        assert!((out.landed.unwrap() - 6.0).abs() < 1e-4);
        assert!(out.feet.iter().all(|f| f.planted));
        let mut min_stretch = 0.0_f32;
        for _ in 0..30 {
            min_stretch = min_stretch.min(state.update(&input, &ground, 1.0 / 60.0).scale.y - 1.0);
        }
        assert!(min_stretch < -0.03, "landing squashes");
    }
}
