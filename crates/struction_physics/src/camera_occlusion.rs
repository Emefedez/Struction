//! Camera obstruction queries; rendering hosts decide how to apply transparency.
use avian3d::prelude::*;
use bevy::prelude::*;

/// Presentation policy for surfaces blocking a camera's view of its target. Hosts can expose
/// these reflected values in the same inspector as other camera components.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct CameraOcclusion {
    /// Swept radius around the sight line, covering the near plane instead of a single ray.
    pub probe_radius: f32,
    pub blocked_opacity: f32,
    /// Exponential blend rate per second; the host restores the original material on release.
    pub fade_speed: f32,
}

impl Default for CameraOcclusion {
    fn default() -> Self {
        Self {
            probe_radius: 0.25,
            blocked_opacity: 0.12,
            fade_speed: 16.0,
        }
    }
}

/// All colliders between the target and camera, including a collider containing the camera.
/// Exclude the target body with `filter`; the render host chooses which hits are fadeable.
pub fn camera_obstructions(
    spatial: &SpatialQuery,
    focus: Vec3,
    camera: Vec3,
    radius: f32,
    filter: &SpatialQueryFilter,
) -> Vec<Entity> {
    let displacement = camera - focus;
    let Ok(direction) = Dir3::new(displacement) else {
        return vec![];
    };
    if !radius.is_finite() || radius <= 0.0 || !displacement.is_finite() {
        return vec![];
    }
    let mut hits: Vec<_> = spatial
        .shape_hits(
            &Collider::sphere(radius),
            focus,
            Quat::IDENTITY,
            direction,
            u32::MAX,
            &ShapeCastConfig::from_max_distance(displacement.length()),
            filter,
        )
        .into_iter()
        .map(|hit| hit.entity)
        .collect();
    hits.sort();
    hits.dedup();
    hits
}
