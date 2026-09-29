//! Damped springs with an analytic (closed-form) integrator.
//!
//! The exact solution of the damped harmonic oscillator is applied per step, so springs are
//! stable for any `dt` and stiffness, unlike explicit Euler. Frame-rate independence follows
//! from the solution being exact for a constant target: stepping `2 * dt` equals stepping `dt`
//! twice.

use bevy::math::{Quat, Vec3};
use bevy::reflect::Reflect;
use serde::{Deserialize, Serialize};

/// Spring tuning in designer units: `frequency` in Hz (undamped), `damping_ratio` 1 = critical,
/// below 1 overshoots, above 1 creeps.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SpringParams {
    pub frequency: f32,
    pub damping_ratio: f32,
}

impl SpringParams {
    pub const fn new(frequency: f32, damping_ratio: f32) -> Self {
        Self {
            frequency,
            damping_ratio,
        }
    }

    /// Fastest approach without overshoot.
    pub const fn critical(frequency: f32) -> Self {
        Self::new(frequency, 1.0)
    }

    /// Transition coefficients for a step of `dt` seconds.
    pub fn coefficients(self, dt: f32) -> SpringCoefficients {
        let omega = core::f32::consts::TAU * self.frequency.max(0.0);
        let zeta = self.damping_ratio.max(0.0);
        if omega < 1e-6 || dt <= 0.0 {
            // No restoring force: the value coasts.
            return SpringCoefficients {
                pos_pos: 1.0,
                pos_vel: dt.max(0.0),
                vel_pos: 0.0,
                vel_vel: 1.0,
            };
        }
        const EPS: f32 = 1e-4;
        if zeta > 1.0 + EPS {
            let za = -omega * zeta;
            let zb = omega * (zeta * zeta - 1.0).sqrt();
            let (z1, z2) = (za - zb, za + zb);
            let (e1, e2) = ((z1 * dt).exp(), (z2 * dt).exp());
            let inv_two_zb = 1.0 / (2.0 * zb);
            let (e1o, e2o) = (e1 * inv_two_zb, e2 * inv_two_zb);
            let (z1e1, z2e2) = (z1 * e1o, z2 * e2o);
            SpringCoefficients {
                pos_pos: e1o * z2 - z2e2 + e2,
                pos_vel: e2o - e1o,
                vel_pos: (z1e1 - z2e2 + e2) * z2,
                vel_vel: z2e2 - z1e1,
            }
        } else if zeta < 1.0 - EPS {
            let omega_zeta = omega * zeta;
            let alpha = omega * (1.0 - zeta * zeta).sqrt();
            let exp = (-omega_zeta * dt).exp();
            let (sin, cos) = (alpha * dt).sin_cos();
            SpringCoefficients {
                pos_pos: exp * (cos + omega_zeta / alpha * sin),
                pos_vel: exp * sin / alpha,
                vel_pos: -exp * sin * (alpha + omega_zeta * omega_zeta / alpha),
                vel_vel: exp * (cos - omega_zeta / alpha * sin),
            }
        } else {
            let exp = (-omega * dt).exp();
            let time_exp = dt * exp;
            let time_exp_freq = time_exp * omega;
            SpringCoefficients {
                pos_pos: time_exp_freq + exp,
                pos_vel: time_exp,
                vel_pos: -omega * time_exp_freq,
                vel_vel: exp - time_exp_freq,
            }
        }
    }
}

impl Default for SpringParams {
    fn default() -> Self {
        Self::critical(3.0)
    }
}

/// State transition of a spring over one step: `[x', v'] = M * [x - target, v]`.
#[derive(Clone, Copy, Debug)]
pub struct SpringCoefficients {
    pos_pos: f32,
    pos_vel: f32,
    vel_pos: f32,
    vel_vel: f32,
}

impl SpringCoefficients {
    fn step(self, displacement: f32, velocity: f32) -> (f32, f32) {
        (
            self.pos_pos * displacement + self.pos_vel * velocity,
            self.vel_pos * displacement + self.vel_vel * velocity,
        )
    }

    fn step3(self, displacement: Vec3, velocity: Vec3) -> (Vec3, Vec3) {
        (
            displacement * self.pos_pos + velocity * self.pos_vel,
            displacement * self.vel_pos + velocity * self.vel_vel,
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SpringF32 {
    pub value: f32,
    pub velocity: f32,
}

impl SpringF32 {
    pub fn at(value: f32) -> Self {
        Self {
            value,
            velocity: 0.0,
        }
    }

    pub fn step(&mut self, target: f32, params: SpringParams, dt: f32) {
        let (x, v) = params
            .coefficients(dt)
            .step(self.value - target, self.velocity);
        self.value = target + x;
        self.velocity = v;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SpringVec3 {
    pub value: Vec3,
    pub velocity: Vec3,
}

impl SpringVec3 {
    pub fn at(value: Vec3) -> Self {
        Self {
            value,
            velocity: Vec3::ZERO,
        }
    }

    pub fn step(&mut self, target: Vec3, params: SpringParams, dt: f32) {
        let (x, v) = params
            .coefficients(dt)
            .step3(self.value - target, self.velocity);
        self.value = target + x;
        self.velocity = v;
    }
}

/// Rotation spring. The displacement is the shortest rotation vector from the target, so the
/// spring never takes the long way around.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SpringQuat {
    pub value: Quat,
    /// Angular velocity, radians per second, in the frame of `value`'s parent.
    pub angular_velocity: Vec3,
}

impl Default for SpringQuat {
    fn default() -> Self {
        Self::at(Quat::IDENTITY)
    }
}

impl SpringQuat {
    pub fn at(value: Quat) -> Self {
        Self {
            value,
            angular_velocity: Vec3::ZERO,
        }
    }

    pub fn step(&mut self, target: Quat, params: SpringParams, dt: f32) {
        let mut error = (self.value * target.inverse()).normalize();
        if error.w < 0.0 {
            error = -error;
        }
        let (x, v) = params
            .coefficients(dt)
            .step3(error.to_scaled_axis(), self.angular_velocity);
        self.value = (Quat::from_scaled_axis(x) * target).normalize();
        self.angular_velocity = v;
    }
}

/// Volume-preserving squash and stretch along local Y, driven by acceleration and impacts.
#[derive(Clone, Copy, Debug, PartialEq, Reflect, Serialize, Deserialize)]
pub struct SquashStretch {
    pub params: SpringParams,
    /// Stretch per m/s^2 of acceleration along the axis (inertia: pushing up compresses).
    pub acceleration_gain: f32,
    /// Stretch removed per m/s of impact speed.
    pub impact_gain: f32,
    /// Hard limit on the stretch amount, in either direction.
    pub max_stretch: f32,
    pub state: SpringF32,
}

impl Default for SquashStretch {
    fn default() -> Self {
        Self {
            params: SpringParams::new(4.0, 0.35),
            acceleration_gain: 0.004,
            impact_gain: 0.6,
            max_stretch: 0.25,
            state: SpringF32::default(),
        }
    }
}

impl SquashStretch {
    /// `axis_acceleration` is the body's acceleration along its up axis (positive = up).
    pub fn update(&mut self, axis_acceleration: f32, dt: f32) {
        let target = (-self.acceleration_gain * axis_acceleration)
            .clamp(-self.max_stretch, self.max_stretch);
        self.state.step(target, self.params, dt);
        self.state.value = self.state.value.clamp(-self.max_stretch, self.max_stretch);
    }

    /// A landing at `speed` m/s compresses the body; the spring rebounds into a stretch.
    pub fn impact(&mut self, speed: f32) {
        self.state.velocity -= self.impact_gain * speed.max(0.0) * core::f32::consts::TAU * self.params.frequency
            * 0.25;
    }

    pub fn stretch(&self) -> f32 {
        self.state.value
    }

    /// Local scale: `1 + stretch` along Y and the inverse square root across.
    pub fn scale(&self) -> Vec3 {
        let along = (1.0 + self.state.value).max(0.05);
        let across = along.sqrt().recip();
        Vec3::new(across, along, across)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn simulate(params: SpringParams, x0: f32, v0: f32, dt: f32, time: f32) -> SpringF32 {
        let mut s = SpringF32 {
            value: x0,
            velocity: v0,
        };
        for _ in 0..(time / dt).round() as usize {
            s.step(0.0, params, dt);
        }
        s
    }

    /// Reference: tiny-step semi-implicit Euler.
    fn reference(params: SpringParams, x0: f32, v0: f32, time: f32) -> (f32, f32) {
        let omega = core::f32::consts::TAU * params.frequency;
        let (mut x, mut v) = (x0 as f64, v0 as f64);
        let dt = 1e-5_f64;
        for _ in 0..(time as f64 / dt) as usize {
            let a = -(omega as f64).powi(2) * x - 2.0 * params.damping_ratio as f64 * omega as f64 * v;
            v += a * dt;
            x += v * dt;
        }
        (x as f32, v as f32)
    }

    #[test]
    fn matches_reference_in_all_regimes() {
        for zeta in [0.2, 0.7, 1.0, 1.5, 4.0] {
            let params = SpringParams::new(2.0, zeta);
            let s = simulate(params, 1.0, 0.5, 1.0 / 60.0, 0.5);
            let (x, v) = reference(params, 1.0, 0.5, 0.5);
            assert!((s.value - x).abs() < 2e-3, "zeta {zeta}: {} vs {x}", s.value);
            assert!((s.velocity - v).abs() < 2e-2, "zeta {zeta}: {} vs {v}", s.velocity);
        }
    }

    #[test]
    fn frame_rate_independent() {
        for zeta in [0.3, 1.0, 2.0] {
            let params = SpringParams::new(3.0, zeta);
            let a = simulate(params, 1.0, 0.0, 1.0 / 30.0, 1.0);
            let b = simulate(params, 1.0, 0.0, 1.0 / 240.0, 1.0);
            assert!((a.value - b.value).abs() < 1e-4);
            assert!((a.velocity - b.velocity).abs() < 1e-3);
        }
    }

    #[test]
    fn stable_for_huge_steps_and_stiffness() {
        let params = SpringParams::new(500.0, 0.05);
        let mut s = SpringF32::at(10.0);
        for _ in 0..100 {
            s.step(0.0, params, 0.5);
            assert!(s.value.is_finite() && s.value.abs() <= 10.0 + 1e-3);
        }
    }

    #[test]
    fn critical_does_not_overshoot_and_underdamped_does() {
        let mut crit = SpringF32::at(1.0);
        let mut under = SpringF32::at(1.0);
        let mut crossed = false;
        for _ in 0..240 {
            crit.step(0.0, SpringParams::critical(3.0), 1.0 / 120.0);
            under.step(0.0, SpringParams::new(3.0, 0.2), 1.0 / 120.0);
            assert!(crit.value >= -1e-5);
            crossed |= under.value < -0.01;
        }
        assert!(crossed);
        assert!(crit.value.abs() < 1e-3);
    }

    #[test]
    fn zero_frequency_coasts() {
        let mut s = SpringF32 {
            value: 0.0,
            velocity: 2.0,
        };
        s.step(5.0, SpringParams::new(0.0, 1.0), 0.5);
        assert!((s.value - 1.0).abs() < 1e-6);
    }

    #[test]
    fn vec3_converges_to_target() {
        let mut s = SpringVec3::at(Vec3::ZERO);
        for _ in 0..300 {
            s.step(Vec3::new(1.0, -2.0, 3.0), SpringParams::critical(4.0), 1.0 / 60.0);
        }
        assert!((s.value - Vec3::new(1.0, -2.0, 3.0)).length() < 1e-3);
    }

    #[test]
    fn quat_takes_shortest_path_and_converges() {
        let target = Quat::from_rotation_y(0.2);
        // Same rotation as the target rotated by almost a full turn, expressed with w < 0.
        let mut s = SpringQuat::at(-Quat::from_rotation_y(0.2 + 0.1));
        s.step(target, SpringParams::critical(3.0), 1.0 / 60.0);
        assert!(s.angular_velocity.length() < 1.0, "spun the long way around");
        for _ in 0..300 {
            s.step(target, SpringParams::critical(3.0), 1.0 / 60.0);
        }
        assert!(s.value.angle_between(target) < 1e-3);
    }

    #[test]
    fn squash_stretch_preserves_volume_and_rebounds() {
        let mut ss = SquashStretch::default();
        ss.impact(6.0);
        let (mut min, mut max) = (0.0_f32, 0.0_f32);
        for _ in 0..120 {
            ss.update(0.0, 1.0 / 60.0);
            let s = ss.scale();
            assert!((s.x * s.y * s.z - 1.0).abs() < 1e-4);
            min = min.min(ss.stretch());
            max = max.max(ss.stretch());
        }
        assert!(min < -0.02, "squash on impact");
        assert!(max > 0.0, "rebound overshoots into stretch");
        assert!(ss.stretch().abs() < 0.01, "settles");
    }
}
