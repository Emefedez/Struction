//! The `combat` extensor: an optional melee swing. The strike lands at a fixed phase of the
//! swing, matching the procedural pose, and hits what is in front of the character through the
//! `combat/hit` action, so definitions can react to being hit.

use avian3d::prelude::*;
use bevy::prelude::*;
use struction_anim::moves::SWING_STRIKE;
use struction_core::{
    ActionAppExt, ActionArgs, ActionCall, ActionInvocation, ActionMeta, ActionQueue,
    ExtensorAppExt, ExtensorMeta, ParamType,
};
use struction_gravity::LocalUp;

use crate::{
    CharacterCondition, CharacterController, CharacterIntent, CharacterLook, CharacterMove,
    CharacterState, CharacterSystems, controller::transport, dodge,
};

/// Melee tuning; its presence is the capability.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Attack {
    /// Seconds from wind-up to follow-through.
    pub duration: f32,
    /// Distance of the struck sphere's center ahead of the body (m).
    pub reach: f32,
    /// Radius of the struck sphere (m).
    pub radius: f32,
    /// Speed given to struck dynamic bodies, away from the attacker (m/s).
    pub knockback: f32,
    /// Seconds after the swing before another move can start.
    pub recovery: f32,
    /// A swing neither starts nor continues while any of these holds.
    pub blocked_while: Vec<CharacterCondition>,
}

impl Default for Attack {
    fn default() -> Self {
        Self {
            duration: 0.5,
            reach: 1.0,
            radius: 0.6,
            knockback: 4.0,
            recovery: 0.15,
            blocked_while: vec![CharacterCondition::Swimming],
        }
    }
}

impl Attack {
    /// Headless authoring validation; invalid live edits cannot start an attack.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !self.duration.is_finite() || self.duration <= 0.0 {
            return Err("Attack.duration must be finite and greater than zero");
        }
        for (value, error) in [
            (self.reach, "Attack.reach must be finite and nonnegative"),
            (self.radius, "Attack.radius must be finite and nonnegative"),
            (
                self.knockback,
                "Attack.knockback must be finite and nonnegative",
            ),
            (
                self.recovery,
                "Attack.recovery must be finite and nonnegative",
            ),
        ] {
            if !value.is_finite() || value < 0.0 {
                return Err(error);
            }
        }
        Ok(())
    }
}

/// A swing in progress, with the tuning it started with.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct Attacking {
    pub elapsed: f32,
    pub direction: Vec3,
    pub up: Vec3,
    pub tuning: Attack,
    /// The strike has landed (or missed); a swing strikes once.
    pub struck: bool,
}

impl Attacking {
    pub fn phase(&self) -> f32 {
        (self.elapsed / self.tuning.duration).clamp(0.0, 1.0)
    }
}

/// Registers the `combat` extensor, the `combat/attack` and `combat/hit` actions and the swing
/// simulation. Hits land through the action queue, so they need `struction_core::CorePlugin`.
pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Attack>()
            .register_type::<Attacking>()
            .register_extensor(
                ExtensorMeta::opt_in("combat")
                    .doc("Melee swing that knocks back what it hits (left mouse or F, `combat/attack`)")
                    .supplies::<Attack>()
                    .requires("character"),
            )
            .register_action(
                ActionMeta::new("combat/attack")
                    .doc("Request a melee swing in the character's movement direction, or facing when idle")
                    .requires::<CharacterController>()
                    .requires::<Attack>(),
                request_attack,
            )
            .register_action(
                ActionMeta::new("combat/hit")
                    .doc("A melee hit by `by`: knocks a dynamic target back. React to it for damage or breakage")
                    .param("by", ParamType::Entity)
                    .param_or("knockback", ParamType::Float, 0.0),
                hit,
            )
            .add_systems(
                FixedPostUpdate,
                attack
                    .in_set(CharacterSystems::Moves)
                    .after(dodge::roll),
            );
    }
}

fn request_attack(In(call): In<ActionCall>, mut characters: Query<&mut CharacterIntent>) {
    if let Ok(mut intent) = characters.get_mut(call.target) {
        intent.attack_requested = true;
    }
}

/// Pushes the target away from the attacker along the attacker's ground, with a little lift.
fn hit(
    In(call): In<ActionCall>,
    positions: Query<(&Position, Option<&LocalUp>)>,
    mut bodies: Query<(&RigidBody, &mut LinearVelocity)>,
) {
    let (Some(by), Some(knockback)) = (call.args.entity("by"), call.args.float("knockback")) else {
        return;
    };
    let (Ok((from, up)), Ok((to, _))) = (positions.get(by), positions.get(call.target)) else {
        return;
    };
    let Ok((body, mut velocity)) = bodies.get_mut(call.target) else {
        return;
    };
    if !body.is_dynamic() {
        return;
    }
    let up = up.map_or(Vec3::Y, |up| *up.0);
    let away = to.0 - from.0;
    let along_ground = (away - up * away.dot(up)).normalize_or_zero();
    velocity.0 += (along_ground + up * 0.3) * knockback as f32;
}

type Attacker<'a> = (
    Entity,
    &'a mut CharacterIntent,
    &'a CharacterState,
    &'a CharacterLook,
    &'a mut CharacterMove,
    &'a LocalUp,
    &'a Position,
    Option<&'a Attack>,
    Option<&'a mut Attacking>,
);

/// Starts accepted requests and runs swings; the character keeps walking meanwhile, facing the
/// strike. A blocking condition or losing the capability cancels a swing.
fn attack(
    mut commands: Commands,
    time: Res<Time>,
    spatial: SpatialQuery,
    bodies: Query<&ColliderOf>,
    sensors: Query<(), With<Sensor>>,
    mut queue: Option<ResMut<ActionQueue>>,
    mut characters: Query<Attacker>,
) {
    let dt = time.delta_secs();
    for (entity, mut intent, state, look, mut moving, up, position, tuning, attacking) in
        &mut characters
    {
        let up = *up.0;
        let requested = core::mem::take(&mut intent.attack_requested);
        let mut swing = match attacking {
            Some(swing)
                if swing.phase() >= 1.0 || tuning.is_none_or(|t| state.any(&t.blocked_while)) =>
            {
                moving.end(swing.tuning.recovery);
                commands.entity(entity).remove::<Attacking>();
                continue;
            }
            Some(swing) => swing.clone(),
            None => {
                let Some(tuning) = tuning
                    .filter(|t| requested && moving.can_start() && !state.any(&t.blocked_while))
                else {
                    continue;
                };
                if let Err(error) = tuning.validate() {
                    warn!("{entity}: {error}");
                    continue;
                }
                let forward = look.heading(up);
                moving.busy = true;
                Attacking {
                    elapsed: 0.0,
                    direction: intent.wish(forward, up).try_normalize().unwrap_or(forward),
                    up,
                    tuning: tuning.clone(),
                    struck: false,
                }
            }
        };
        swing.direction = transport(swing.direction, swing.up, up);
        swing.up = up;
        swing.elapsed = (swing.elapsed + dt).min(swing.tuning.duration);
        moving.facing = Some(swing.direction);
        if !swing.struck && swing.phase() >= SWING_STRIKE {
            swing.struck = true;
            let center = position.0 + swing.direction * swing.tuning.reach;
            let filter = SpatialQueryFilter::default().with_excluded_entities([entity]);
            let mut struck: Vec<Entity> = spatial
                .shape_intersections(
                    &Collider::sphere(swing.tuning.radius),
                    center,
                    Quat::IDENTITY,
                    &filter,
                )
                .into_iter()
                .filter(|&collider| !sensors.contains(collider))
                .map(|collider| bodies.get(collider).map_or(collider, |of| of.body))
                .filter(|&body| body != entity)
                .collect();
            struck.sort();
            struck.dedup();
            if let Some(queue) = queue.as_mut() {
                for target in struck {
                    queue.invoke(ActionInvocation::new(
                        "combat/hit",
                        target,
                        ActionArgs::new()
                            .with("by", entity)
                            .with("knockback", swing.tuning.knockback),
                    ));
                }
            }
        }
        commands.entity(entity).insert(swing);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_tuning_reports_the_field() {
        let invalid = |attack: Attack| attack.validate().unwrap_err();
        assert!(
            invalid(Attack {
                duration: 0.0,
                ..default()
            })
            .contains("duration")
        );
        assert!(
            invalid(Attack {
                reach: f32::NAN,
                ..default()
            })
            .contains("reach")
        );
        assert!(
            invalid(Attack {
                knockback: -1.0,
                ..default()
            })
            .contains("knockback")
        );
        assert!(Attack::default().validate().is_ok());
    }
}
