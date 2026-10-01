//! Authoring guides use the simulation's actual volume and field functions.
use crate::{scene_view::source_world, state::Editor};
use bevy::prelude::*;
use struction_gravity::{GravityField, GravityVolume, field_acceleration};
use struction_physics::{CameraMode, CameraZone, Volume, VolumeShape};

#[derive(Resource)]
pub struct GuideSettings {
    pub visible: bool,
}
impl Default for GuideSettings {
    fn default() -> Self {
        Self { visible: true }
    }
}

pub fn draw(editor: NonSend<Editor>, settings: Res<GuideSettings>, mut gizmos: Gizmos) {
    if !settings.visible || editor.playing() {
        return;
    }
    let Some(world) = source_world(&editor) else {
        return;
    };
    for entry in editor.entities.iter().filter(|e| !e.disabled) {
        let Some(transform) = entry.transform() else {
            continue;
        };
        let position = transform.translation;
        if let Some(field) = world.get::<GravityField>(entry.entity) {
            let color = Color::srgba(0.62, 0.48, 1.0, 0.35);
            let extent = match field.volume {
                GravityVolume::Infinite => Vec3::splat(1.0),
                GravityVolume::Sphere { radius } => {
                    gizmos.sphere(Isometry3d::new(position, transform.rotation), radius, color);
                    Vec3::splat(radius)
                }
                GravityVolume::Box { half_extents } => {
                    gizmos.cube(
                        Transform {
                            scale: half_extents * 2.0,
                            ..transform
                        },
                        color,
                    );
                    half_extents
                }
            };
            let n = if matches!(field.volume, GravityVolume::Infinite) {
                0
            } else {
                2
            };
            for x in -n..=n {
                for y in -n..=n {
                    for z in -n..=n {
                        let local = Vec3::new(x as f32, y as f32, z as f32) * extent * 0.38;
                        let at = position + transform.rotation * local;
                        let pull = field_acceleration(field, position, transform.rotation, at);
                        if let Some(direction) = pull.try_normalize() {
                            let length = extent.min_element().mul_add(0.3, 0.1).clamp(0.25, 1.2);
                            gizmos.arrow(
                                at - direction * length * 0.5,
                                at + direction * length * 0.5,
                                color,
                            );
                        }
                    }
                }
            }
        }
        if let Some(zone) = world.get::<CameraZone>(entry.entity) {
            let color = Color::srgba(0.25, 0.8, 0.95, 0.45);
            if let Some(volume) = world.get::<Volume>(entry.entity) {
                match volume.shape {
                    VolumeShape::Box { half_extents } => {
                        gizmos.cube(
                            Transform {
                                scale: half_extents * 2.0 * transform.scale,
                                ..transform
                            },
                            color,
                        );
                    }
                    VolumeShape::Sphere { radius } => {
                        gizmos.sphere(
                            Isometry3d::new(position, transform.rotation),
                            radius * transform.scale.max_element(),
                            color,
                        );
                    }
                }
            }
            if let CameraMode::Fixed { position: fixed } = zone.constraint.mode {
                gizmos.line(position, fixed, color);
                gizmos.cube(
                    Transform::from_translation(fixed).with_scale(Vec3::new(0.4, 0.3, 0.6)),
                    color,
                );
            }
        }
    }
}

pub fn camera_label(zone: &CameraZone) -> String {
    let mode = match zone.constraint.mode {
        CameraMode::Follow { distance, pitch } => {
            format!("Follow · {distance:.1} m · {:.0}°", pitch.to_degrees())
        }
        CameraMode::Fixed { position } => format!(
            "Fixed · {:.1}, {:.1}, {:.1}",
            position.x, position.y, position.z
        ),
    };
    format!(
        "{mode}\nWeight {:.2} · priority {}",
        zone.constraint.weight, zone.constraint.priority
    )
}
