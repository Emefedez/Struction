//! Optional dodge capability. Requests are consumed by the controller, not by animation.

use bevy::prelude::*;
use struction_core::{ActionAppExt, ActionCall, ActionMeta};

use crate::{CharacterController, CharacterIntent};

#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
#[require(RollRecovery)]
pub struct RollAbility {
    /// Seconds including anticipation and getting back up.
    pub duration: f32,
    /// Peak of the sinusoidal speed profile, in m/s.
    pub peak_speed: f32,
    /// Seconds after finishing or cancelling before another roll can start.
    pub recovery: f32,
}

impl Default for RollAbility {
    fn default() -> Self {
        Self {
            duration: 0.65,
            peak_speed: 10.0,
            recovery: 0.2,
        }
    }
}

impl RollAbility {
    /// Headless authoring validation; invalid live edits cannot start a roll.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return Err("RollAbility.duration must be finite and greater than zero");
        }
        if !self.peak_speed.is_finite() || self.peak_speed <= 0.0 {
            return Err("RollAbility.peak_speed must be finite and greater than zero");
        }
        if !self.recovery.is_finite() || self.recovery < 0.0 {
            return Err("RollAbility.recovery must be finite and nonnegative");
        }
        Ok(())
    }
}

/// Snapshot the tuning at takeoff so live edits affect the next roll, not its timing mid-turn.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
pub struct Rolling {
    pub elapsed: f32,
    pub direction: Vec3,
    pub up: Vec3,
    pub ability: RollAbility,
}

impl Rolling {
    pub fn phase(&self) -> f32 {
        (self.elapsed / self.ability.duration).clamp(0.0, 1.0)
    }

    /// Average speed over a tick, including a partial final tick. Integrating the curve keeps
    /// unobstructed distance independent of the chosen fixed timestep.
    pub(crate) fn advance(&mut self, dt: f32, up: Vec3) -> f32 {
        let direction = Quat::from_rotation_arc(self.up, up) * self.direction;
        self.direction =
            (direction - up * direction.dot(up)).normalize_or(up.any_orthonormal_vector());
        self.up = up;
        let before = self.phase() * core::f32::consts::PI;
        self.elapsed = (self.elapsed + dt).min(self.ability.duration);
        let after = self.phase() * core::f32::consts::PI;
        self.ability.peak_speed * self.ability.duration * (before.cos() - after.cos())
            / (core::f32::consts::PI * dt)
    }
}

#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component)]
pub struct RollRecovery {
    pub remaining: f32,
}

/// Optional adapter for games using `struction_core`. Add alongside `CorePlugin`.
/// Reactions to `character/roll` observe the request, not successful entry or completion.
pub struct RollActionsPlugin;

impl Plugin for RollActionsPlugin {
    fn build(&self, app: &mut App) {
        app.register_action(
            ActionMeta::new("character/roll")
                .doc("Request a ground roll in the character's movement direction, or facing when idle")
                .requires::<CharacterController>()
                .requires::<CharacterIntent>()
                .requires::<RollAbility>(),
            request_roll,
        );
    }
}

fn request_roll(In(call): In<ActionCall>, mut characters: Query<&mut CharacterIntent>) {
    if let Ok(mut intent) = characters.get_mut(call.target) {
        intent.roll_requested = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_integral_includes_partial_final_ticks() {
        for hz in [30.0, 60.0, 144.0] {
            let mut roll = Rolling {
                elapsed: 0.0,
                direction: Vec3::NEG_Z,
                up: Vec3::Y,
                ability: RollAbility {
                    duration: 0.637,
                    ..default()
                },
            };
            let mut distance = 0.0;
            while roll.phase() < 1.0 {
                distance += roll.advance(1.0 / hz, Vec3::Y) / hz;
            }
            let expected =
                2.0 * roll.ability.peak_speed * roll.ability.duration / core::f32::consts::PI;
            assert!((distance - expected).abs() < 1e-4, "{hz} Hz: {distance}");
        }
    }

    #[test]
    fn direction_transports_across_a_right_angle_gravity_change() {
        let mut roll = Rolling {
            elapsed: 0.0,
            direction: Vec3::NEG_Z,
            up: Vec3::Y,
            ability: default(),
        };
        roll.advance(1.0 / 60.0, Vec3::NEG_Z);
        assert!(roll.direction.abs_diff_eq(Vec3::NEG_Y, 1e-5));
        assert!(roll.direction.dot(roll.up).abs() < 1e-5);
    }

    #[test]
    fn invalid_tuning_reports_the_field() {
        assert!(
            RollAbility {
                duration: 0.0,
                ..default()
            }
            .validate()
            .unwrap_err()
            .contains("duration")
        );
        assert!(
            RollAbility {
                peak_speed: f32::INFINITY,
                ..default()
            }
            .validate()
            .unwrap_err()
            .contains("peak_speed")
        );
        assert!(
            RollAbility {
                recovery: -1.0,
                ..default()
            }
            .validate()
            .unwrap_err()
            .contains("recovery")
        );
        assert!(
            RollAbility {
                recovery: 0.0,
                ..default()
            }
            .validate()
            .is_ok()
        );
    }
}
