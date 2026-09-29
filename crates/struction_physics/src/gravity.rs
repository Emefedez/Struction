use avian3d::prelude::*;
use bevy::prelude::*;
use struction_gravity::LocalGravity;

use crate::EnvironmentSystems;

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        FixedPostUpdate,
        apply_gravity.in_set(EnvironmentSystems::ApplyGravity),
    );
}

/// Gravity is an acceleration from the summed fields, scaled by Avian's `GravityScale`.
/// It never wakes a sleeping body: contacts and other forces do that.
fn apply_gravity(mut bodies: Query<(Forces, &RigidBody, &LocalGravity, Option<&GravityScale>)>) {
    for (mut forces, body, gravity, scale) in &mut bodies {
        if body.is_dynamic() {
            let scale = scale.map_or(1.0, |scale| scale.0);
            forces
                .non_waking()
                .apply_linear_acceleration(gravity.0 * scale);
        }
    }
}
