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
            reach_limit: 0.97,
            step_height: 0.10,
            min_swing: 0.10,
            max_swing: 0.32,
            idle_swing: 0.22,
            crouch: 0.08,
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
    Swing {
        from: Vec3,
        elapsed: f32,
        duration: f32,
    },
    Air,
}

#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct LegState {
    pub spec: LegSpec,
    pub phase: LegPhase,
    /// World-space ankle position and ground normal the foot currently occupies.
    pub foot: Vec3,
    pub normal: Vec3,
    /// Seconds since the foot was last planted.
    pub stance_time: f32,
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
    /// Where the legs are in their two-step cycle, see [`GaitCycle`].
    pub gait_phase: f32,
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
    cycle: GaitCycle,
    #[reflect(ignore)]
    output: LocomotionOutput,
}

/// The legs' two-step cycle as a phase: 0 when a leg of the first group (the left foot) lifts
/// off, 0.5 when one of the other does, advancing in between at the pace of the last half cycle.
/// It holds while the legs stand still, so sequences paced by it pause with them.
#[derive(Clone, Copy, Debug, PartialEq, Reflect)]
pub struct GaitCycle {
    /// 0 or 0.5: which group lifted last.
    start: f32,
    since: f32,
    /// Seconds between the last two lifts of different groups.
    half: f32,
}

impl Default for GaitCycle {
    fn default() -> Self {
        Self {
            start: 0.5,
            since: f32::INFINITY,
            half: 0.5,
        }
    }
}

impl GaitCycle {
    pub fn phase(&self) -> f32 {
        (self.start + 0.5 * (self.since / self.half).min(1.0)).rem_euclid(1.0)
    }

    fn advance(&mut self, dt: f32) {
        self.since += dt;
    }

    /// A leg of `group` lifted off; the other group's lifts do not restart the half cycle.
    fn lift(&mut self, group: u8, first: u8) {
        let start = if group == first { 0.0 } else { 0.5 };
        if start == self.start && self.since.is_finite() {
            return;
        }
        if self.since.is_finite() {
            self.half = self.since.clamp(0.1, 2.0);
        }
        self.start = start;
        self.since = 0.0;
    }
}

fn project(
    params: &LocomotionParams,
    ground: &dyn Ground,
    point: Vec3,
    up: Vec3,
    ankle: f32,
) -> (Vec3, Vec3) {
    let origin = point + up * params.probe_up;
    match ground.cast(origin, -up, params.probe_up + params.probe_down) {
        Some(hit) => (hit.point + hit.normal * ankle, hit.normal),
        None => (point, up),
    }
}

/// Per-update movement summary shared by the stepping helpers.
#[derive(Clone, Copy)]
struct Gait {
    up: Vec3,
    /// Velocity along the ground (tangent to `up`).
    velocity: Vec3,
    speed: f32,
    dir: Option<Vec3>,
    stride: f32,
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
                stance_time: f32::INFINITY,
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
            cycle: GaitCycle::default(),
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

    /// Forget planted contacts after a full-body move; the next update finds ground anew.
    pub fn reset(&mut self) {
        let specs = self.legs.iter().map(|leg| leg.spec).collect();
        *self = Self::new(specs, self.params);
    }

    /// Advances the legs and secondary motion by `dt` seconds.
    pub fn update(
        &mut self,
        input: &LocomotionInput,
        ground: &dyn Ground,
        dt: f32,
    ) -> &LocomotionOutput {
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
                    let (foot, normal) =
                        project(&self.params, ground, flat, up, leg.spec.rest_foot.y);
                    leg.phase = LegPhase::Planted;
                    leg.stance_time = 0.0;
                    leg.foot = foot;
                    leg.normal = normal;
                }
            }
            let gait = Gait {
                up,
                velocity: velocity_t,
                speed,
                dir,
                stride: self.half_stride(speed),
            };
            footfalls = self.step_legs(input, ground, &gait, dt);
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
        if input.grounded {
            // Hard limit on top of the spring: drop the pelvis until every foot is in reach, so
            // IK never has to leave a planted foot behind. Writing into the spring keeps the
            // motion continuous afterwards.
            let limit = self.pelvis_limit(input, up);
            if self.bob.value > limit {
                self.bob.value = limit;
                self.bob.velocity = self.bob.velocity.min(0.0);
            }
        }

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
            gait_phase: self.cycle.phase(),
        };
        &self.output
    }

    /// Highest pelvis offset along `up` that keeps every foot within `reach_limit` of its hip.
    fn pelvis_limit(&self, input: &LocomotionInput, up: Vec3) -> f32 {
        self.legs.iter().fold(f32::INFINITY, |limit, leg| {
            let reach = leg.spec.reach * self.params.reach_limit;
            let v = input.root.transform_point(leg.spec.hip) - leg.foot;
            let along = v.dot(up);
            let across = (v.length_squared() - along * along).max(0.0);
            // Solve |v + d * up| = reach for the largest d; out of reach sideways, go as low as
            // the foot allows.
            let d = if across < reach * reach {
                -along + (reach * reach - across).sqrt()
            } else {
                -along
            };
            limit.min(d)
        })
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
        spec: &LegSpec,
        gait: &Gait,
        remaining: f32,
    ) -> (Vec3, Vec3) {
        let flat = input.root.transform_point(spec.rest_foot);
        let mut at = flat + gait.velocity * remaining;
        if let Some(d) = gait.dir {
            at += d * gait.stride;
        }
        project(&self.params, ground, at, gait.up, spec.rest_foot.y)
    }

    fn step_legs(
        &mut self,
        input: &LocomotionInput,
        ground: &dyn Ground,
        gait: &Gait,
        dt: f32,
    ) -> u32 {
        let Gait {
            up,
            speed,
            dir,
            stride,
            ..
        } = *gait;
        let mut footfalls = 0;
        self.cycle.advance(dt);

        for i in 0..self.legs.len() {
            let leg = self.legs[i];
            match leg.phase {
                LegPhase::Swing {
                    from,
                    elapsed,
                    duration,
                } => {
                    let elapsed = elapsed + dt;
                    let remaining = (duration - elapsed).max(0.0);
                    let (target, normal) =
                        self.landing_spot(input, ground, &leg.spec, gait, remaining);
                    let leg = &mut self.legs[i];
                    if elapsed >= duration {
                        leg.phase = LegPhase::Planted;
                        leg.stance_time = 0.0;
                        leg.foot = target;
                        leg.normal = normal;
                        footfalls += 1;
                    } else {
                        let t = elapsed / duration;
                        let length = from.distance(target);
                        let height =
                            self.params.step_height * (0.5 + 0.5 * (length / 0.4).min(1.0));
                        leg.foot = from.lerp(target, smoothstep(t))
                            + up * (height * (core::f32::consts::PI * t).sin());
                        leg.normal = leg.normal.lerp(normal, 0.3).normalize_or(up);
                        leg.phase = LegPhase::Swing {
                            from,
                            elapsed,
                            duration,
                        };
                    }
                }
                LegPhase::Air => {
                    // Landing while the legs were still airborne is handled by the caller.
                }
                LegPhase::Planted => self.legs[i].stance_time += dt,
            }
        }

        // Planted legs that want to step, most urgent first.
        // Measure reach where the pelvis will be: last frame's height was limited while swinging
        // feet were still in the air, so a foot landing lower would read as out of reach.
        let hip_lift = up * self.bob.value.min(self.pelvis_limit(input, up));
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
            // The pelvis limit parks the hip exactly at the reach limit; slack absorbs rounding.
            let urgent = ratio > self.params.reach_limit + 1e-3;
            let urgency = (behind / stride)
                .max(offset.length() / trigger)
                .max(ratio / self.params.reach_limit);
            if urgent || behind > stride || offset.length() > trigger {
                candidates.push((i, urgency, urgent));
            }
        }
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
        let duration = if dir.is_some() {
            (0.6 * 2.0 * stride / speed).clamp(self.params.min_swing, self.params.max_swing)
        } else {
            self.params.idle_swing
        };
        // Waiting this long after another group lands spreads the lifts evenly over the cycle;
        // without it the legs settle into a limp (one step right after the other, then a pause).
        let settle = dir.map_or(0.0, |_| (0.5 * (2.0 * stride / speed - duration)).max(0.0));
        for (i, _, urgent) in candidates {
            let group = self.legs[i].spec.group;
            let blocked = self.legs.iter().any(|l| {
                l.spec.group != group
                    && (matches!(l.phase, LegPhase::Swing { .. })
                        || (l.phase == LegPhase::Planted && l.stance_time < settle))
            });
            // Overstretching legs may break the alternation, but never lift the last support.
            let planted = self
                .legs
                .iter()
                .filter(|l| l.phase == LegPhase::Planted)
                .count();
            if (blocked && !urgent) || planted <= 1 {
                continue;
            }
            let first = self.legs[0].spec.group;
            self.cycle.lift(group, first);
            let leg = &mut self.legs[i];
            leg.phase = LegPhase::Swing {
                from: leg.foot,
                elapsed: 0.0,
                duration,
            };
        }
        footfalls
    }

    fn air_legs(
        &mut self,
        input: &LocomotionInput,
        ground: &dyn Ground,
        up: Vec3,
        landing: Option<Landing>,
    ) {
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
                    let hit = ground.cast(
                        base + up * self.params.probe_up,
                        -up,
                        self.params.probe_up + self.params.probe_down,
                    );
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
        let (p, v, g) = (
            Vec3::new(1.0, 4.0, -2.0),
            Vec3::new(2.0, 3.0, 1.0),
            Vec3::new(0.0, -9.81, 0.0),
        );
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

    #[test]
    fn the_gait_phase_follows_the_lifts_and_holds_at_rest() {
        let rig = humanoid::rig();
        let mut state = LocomotionState::from_rig(&rig, LocomotionParams::default()).unwrap();
        let ground = PlaneGround {
            point: Vec3::ZERO,
            normal: Vec3::Y,
        };
        let velocity = Vec3::new(0.0, 0.0, -1.4);
        let mut input = LocomotionInput {
            root: Transform::IDENTITY,
            velocity,
            up: Vec3::Y,
            grounded: true,
            gravity: Vec3::NEG_Y * 9.81,
        };
        let dt = 1.0 / 60.0;
        let mut planted = [true, true];
        let (mut lifts, mut wraps) = (0, 0);
        let mut previous = state.update(&input, &ground, dt).gait_phase;
        for _ in 0..300 {
            input.root.translation += velocity * dt;
            let out = state.update(&input, &ground, dt);
            for foot in &out.feet {
                let side = usize::from(foot.limb == Limb::RightFoot);
                if planted[side] && !foot.planted {
                    lifts += 1;
                    let expected = 0.5 * side as f32;
                    assert!(
                        (out.gait_phase - expected).abs() < 1e-5,
                        "{side} lifts at {}",
                        out.gait_phase
                    );
                }
                planted[side] = foot.planted;
            }
            // Forward only, wrapping once per cycle.
            let delta = (out.gait_phase - previous).rem_euclid(1.0);
            assert!(delta < 0.5, "{previous} -> {}", out.gait_phase);
            wraps += usize::from(out.gait_phase < previous);
            previous = out.gait_phase;
        }
        assert!(lifts >= 6 && wraps >= 3, "{lifts} lifts, {wraps} cycles");

        input.velocity = Vec3::ZERO;
        for _ in 0..120 {
            state.update(&input, &ground, dt);
        }
        let rest = state.output().gait_phase;
        for _ in 0..60 {
            assert_eq!(state.update(&input, &ground, dt).gait_phase, rest);
        }
    }
}
