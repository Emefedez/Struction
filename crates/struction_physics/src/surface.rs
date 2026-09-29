use avian3d::prelude::*;
use bevy::prelude::*;

use crate::EnvironmentSystems;

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<Surface>().add_systems(
        FixedPostUpdate,
        (sync_surface_material, apply_surface_drag)
            .chain()
            .in_set(EnvironmentSystems::Effects),
    );
}

/// How a collider treats what touches it. No material types: a slippery floor is a `Surface`
/// with low friction, mud one with high drag.
///
/// Friction and restitution are handed to Avian's material components with the `Multiply` combine
/// rule, so they scale the coefficients of the touching body (Avian's default body friction is
/// 0.5): a surface of 1.0 leaves them unchanged, 0.05 makes everything slide.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
#[require(Friction, Restitution)]
pub struct Surface {
    /// Multiplier of the touching body's friction coefficient.
    pub friction: f32,
    /// Multiplier of the touching body's restitution (bounciness).
    pub restitution: f32,
    /// Rate (1/s) at which the velocity of touching bodies along the surface decays.
    pub drag: f32,
}

impl Default for Surface {
    fn default() -> Self {
        Self {
            friction: 1.0,
            restitution: 1.0,
            drag: 0.0,
        }
    }
}

impl Surface {
    pub fn slippery() -> Self {
        Self {
            friction: 0.02,
            ..default()
        }
    }
}

fn sync_surface_material(
    mut surfaces: Query<(&Surface, &mut Friction, &mut Restitution), Changed<Surface>>,
) {
    for (surface, mut friction, mut restitution) in &mut surfaces {
        *friction = Friction::new(surface.friction).with_combine_rule(CoefficientCombine::Multiply);
        *restitution =
            Restitution::new(surface.restitution).with_combine_rule(CoefficientCombine::Multiply);
    }
}

/// Damps the tangential velocity of dynamic bodies touching a surface with drag.
pub(crate) fn apply_surface_drag(
    contacts: Res<ContactGraph>,
    surfaces: Query<&Surface>,
    mut bodies: Query<(Forces, &RigidBody)>,
    time: Res<Time>,
) {
    let dt = time.delta_secs();
    for pair in contacts.iter_active_touching() {
        let Some(manifold) = pair.manifolds.first() else {
            continue;
        };
        for (surface_collider, other_body) in
            [(pair.collider1, pair.body2), (pair.collider2, pair.body1)]
        {
            let (Ok(surface), Some(other_body)) = (surfaces.get(surface_collider), other_body)
            else {
                continue;
            };
            if surface.drag <= 0.0 {
                continue;
            }
            let Ok((mut forces, body)) = bodies.get_mut(other_body) else {
                continue;
            };
            if !body.is_dynamic() {
                continue;
            }
            let velocity = forces.linear_velocity();
            let tangential = velocity - manifold.normal * velocity.dot(manifold.normal);
            // Never decelerate past standstill within one tick.
            let rate = surface.drag.min(1.0 / dt);
            forces.apply_linear_acceleration(-tangential * rate);
        }
    }
}
