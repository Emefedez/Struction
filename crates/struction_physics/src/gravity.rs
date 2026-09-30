use avian3d::physics_transform::PhysicsTransformSystems;
use avian3d::prelude::*;
use bevy::prelude::*;
use struction_gravity::{GravityPose, GravitySystems, LocalGravity};

use crate::EnvironmentSystems;

pub(crate) fn plugin(app: &mut App) {
    app.register_required_components::<RigidBody, GravityPose>();
    app.add_systems(
        FixedPostUpdate,
        sample_poses
            .in_set(PhysicsSystems::Prepare)
            .before(GravitySystems::Update)
            .after(PhysicsTransformSystems::TransformToPosition),
    );
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

fn sample_poses(mut bodies: Query<(&Position, &Rotation, &mut GravityPose)>) {
    for (position, rotation, mut pose) in &mut bodies {
        pose.set_if_neq(GravityPose {
            translation: position.0,
            rotation: rotation.0,
        });
    }
}
