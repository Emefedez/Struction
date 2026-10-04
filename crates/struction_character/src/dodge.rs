//! The `dodge` extensor: an optional ground roll. Requests are consumed in the fixed-step
//! simulation, not by animation.

use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use struction_core::{ActionAppExt, ActionCall, ActionMeta, ExtensorAppExt, ExtensorMeta};
use struction_gravity::LocalUp;

use crate::{
    CancelInto, CharacterAction, CharacterCondition, CharacterController, CharacterIntent,
    CharacterLook, CharacterMove, CharacterState, CharacterSystems, controller::transport,
};

/// Roll tuning; its presence is the capability.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Roll {
    /// Seconds including anticipation and getting back up.
    pub duration: f32,
    /// Peak of the sinusoidal speed profile, in m/s.
    pub peak_speed: f32,
    /// Seconds after finishing or cancelling before another move can start.
    pub recovery: f32,
    /// A roll neither starts nor continues while any of these holds. Without `Airborne`, a
    /// roll started on the ground carries on through the air as a dash.
    pub blocked_while: Vec<CharacterCondition>,
    /// Actions that may cut it short, and from how many seconds in. Empty: it always runs out.
    pub cancel_into: Vec<CancelInto>,
    /// Pose sequence the rig plays over the roll, by name in its pose library.
    pub sequence: String,
}

impl Default for Roll {
    fn default() -> Self {
        Self {
            duration: 0.65,
            peak_speed: 10.0,
            recovery: 0.2,
            blocked_while: vec![
                CharacterCondition::Airborne,
                CharacterCondition::Swimming,
                CharacterCondition::Attacking,
                CharacterCondition::Recovering,
            ],
            cancel_into: Vec::new(),
            sequence: "roll".into(),
        }
    }
}

impl Roll {
    /// Headless authoring validation; invalid live edits cannot start a roll.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return Err("Roll.duration must be finite and greater than zero");
        }
        if !self.peak_speed.is_finite() || self.peak_speed <= 0.0 {
            return Err("Roll.peak_speed must be finite and greater than zero");
        }
        if !self.recovery.is_finite() || self.recovery < 0.0 {
            return Err("Roll.recovery must be finite and nonnegative");
        }
        for window in &self.cancel_into {
            window.validate()?;
        }
        Ok(())
    }
}

/// A roll in progress. Snapshots the tuning at takeoff so live edits affect the next roll, not
/// its timing mid-turn.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct Rolling {
    pub elapsed: f32,
    pub direction: Vec3,
    pub up: Vec3,
    pub tuning: Roll,
}

impl Rolling {
    pub fn phase(&self) -> f32 {
        (self.elapsed / self.tuning.duration).clamp(0.0, 1.0)
    }

    /// Average speed over a tick, including a partial final tick. Integrating the curve keeps
    /// unobstructed distance independent of the chosen fixed timestep.
    pub(crate) fn advance(&mut self, dt: f32, up: Vec3) -> f32 {
        self.direction = transport(self.direction, self.up, up);
        self.up = up;
        let before = self.phase() * core::f32::consts::PI;
        self.elapsed = (self.elapsed + dt).min(self.tuning.duration);
        let after = self.phase() * core::f32::consts::PI;
        self.tuning.peak_speed * self.tuning.duration * (before.cos() - after.cos())
            / (core::f32::consts::PI * dt)
    }
}

const ROLLING: Option<CharacterCondition> = Some(CharacterCondition::Rolling);

/// Registers the `dodge` extensor, its `dodge/roll` action and the roll simulation.
/// Reactions to `dodge/roll` observe the request, not successful entry or completion.
pub struct DodgePlugin;

impl Plugin for DodgePlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Roll>().register_type::<Rolling>();
        let rolling = crate::states::state(&app.world().resource::<AppTypeRegistry>().read(), "Rolling");
        app.register_extensor(
                ExtensorMeta::opt_in("dodge")
                    .doc("Ground roll in the movement direction (Left Shift, `dodge/roll`)")
                    .supplies::<Roll>()
                    .requires("character")
                    .state(rolling),
            )
            .register_action(
                ActionMeta::new("dodge/roll")
                    .doc("Request a ground roll in the character's movement direction, or facing when idle")
                    .requires::<CharacterController>()
                    .requires::<Roll>(),
                request_roll,
            )
            .add_systems(
                FixedPostUpdate,
                roll.in_set(CharacterSystems::Moves),
            );
    }
}

fn request_roll(In(call): In<ActionCall>, mut characters: Query<&mut CharacterIntent>) {
    if let Ok(mut intent) = characters.get_mut(call.target) {
        intent.request(CharacterAction::Roll);
    }
}

type Roller<'a> = (
    Entity,
    &'a mut CharacterIntent,
    &'a CharacterState,
    &'a CharacterLook,
    &'a mut CharacterMove,
    &'a LocalUp,
    Option<&'a Roll>,
    Option<&'a mut Rolling>,
);

/// Starts accepted requests and drives rolls along the ground. A blocking condition or losing
/// the capability cancels a roll; rejected presses are consumed.
pub(crate) fn roll(mut commands: Commands, time: Res<Time>, mut characters: Query<Roller>) {
    let dt = time.delta_secs();
    for (entity, mut intent, state, look, mut moving, up, tuning, rolling) in &mut characters {
        let up = *up.0;
        // A refused request waits in the intent for the controller's `input_buffer`, unless
        // nothing could ever accept it.
        if tuning.is_none() {
            intent.roll_requested = false;
        }
        let requested = intent.roll_requested && moving.allows_request(CharacterAction::Roll);
        let mut active = match rolling {
            Some(rolling)
                if rolling.phase() >= 1.0
                    || tuning.is_none_or(|t| moving.blocked(state, &t.blocked_while, ROLLING)) =>
            {
                moving.end(CharacterCondition::Rolling, rolling.tuning.recovery);
                commands.entity(entity).remove::<Rolling>();
                continue;
            }
            Some(rolling) => rolling,
            None => {
                let Some(tuning) =
                    tuning.filter(|t| requested && !moving.blocked(state, &t.blocked_while, None))
                else {
                    continue;
                };
                if let Err(error) = tuning.validate() {
                    warn!("{entity}: {error}");
                    intent.roll_requested = false;
                    continue;
                }
                let forward = look.heading(up);
                let direction = intent.wish(forward, up).try_normalize().unwrap_or(forward);
                intent.roll_requested = false;
                moving.start(CharacterCondition::Rolling);
                // A roll moves from the tick it starts.
                let mut first = Rolling {
                    elapsed: 0.0,
                    direction,
                    up,
                    tuning: tuning.clone(),
                };
                let speed = first.advance(dt, up);
                drive(&mut moving, &first, speed, state);
                commands.entity(entity).insert(first);
                continue;
            }
        };
        let speed = active.advance(dt, up);
        drive(&mut moving, &active, speed, state);
    }
}

fn drive(moving: &mut CharacterMove, rolling: &Rolling, speed: f32, state: &CharacterState) {
    let normal = state.ground_normal.normalize_or(rolling.up);
    let along_ground = (rolling.direction - normal * rolling.direction.dot(normal))
        .normalize_or(rolling.direction);
    moving.velocity = Some(along_ground * speed);
    moving.facing = Some(rolling.direction);
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
                tuning: Roll {
                    duration: 0.637,
                    ..default()
                },
            };
            let mut distance = 0.0;
            while roll.phase() < 1.0 {
                distance += roll.advance(1.0 / hz, Vec3::Y) / hz;
            }
            let expected =
                2.0 * roll.tuning.peak_speed * roll.tuning.duration / core::f32::consts::PI;
            assert!((distance - expected).abs() < 1e-4, "{hz} Hz: {distance}");
        }
    }

    #[test]
    fn direction_transports_across_a_right_angle_gravity_change() {
        let mut roll = Rolling {
            elapsed: 0.0,
            direction: Vec3::NEG_Z,
            up: Vec3::Y,
            tuning: default(),
        };
        roll.advance(1.0 / 60.0, Vec3::NEG_Z);
        assert!(roll.direction.abs_diff_eq(Vec3::NEG_Y, 1e-5));
        assert!(roll.direction.dot(roll.up).abs() < 1e-5);
    }

    #[test]
    fn invalid_tuning_reports_the_field() {
        let invalid = |roll: Roll| roll.validate().unwrap_err();
        assert!(
            invalid(Roll {
                duration: 0.0,
                ..default()
            })
            .contains("duration")
        );
        assert!(
            invalid(Roll {
                peak_speed: f32::INFINITY,
                ..default()
            })
            .contains("peak_speed")
        );
        assert!(
            invalid(Roll {
                recovery: -1.0,
                ..default()
            })
            .contains("recovery")
        );
        assert!(
            Roll {
                recovery: 0.0,
                ..default()
            }
            .validate()
            .is_ok()
        );
        for after in [-0.1, f32::INFINITY, f32::NAN] {
            assert!(
                invalid(Roll {
                    cancel_into: vec![CancelInto {
                        action: CharacterAction::Jump,
                        after
                    }],
                    ..default()
                })
                .contains("cancel_into.after")
            );
        }
    }
}
