//! Presentation mirrors of the authoring or play world. They own no simulation state.
use crate::{
    state::Editor,
    tools::{Toolbox, asset_root},
};
use bevy::{camera::primitives::Aabb, prelude::*};
use std::collections::HashMap;
use struction_anim::plugin::{RigJoints, SolvedPose};
use struction_character::RigOf;
use struction_physics::{
    Volume,
    avian3d::prelude::{Collider, SimpleCollider},
};
use struction_scene::{Figure, Look, Shape};

#[derive(Component)]
pub struct SceneEntity(pub String, pub Entity);

pub fn source_world(editor: &Editor) -> Option<&World> {
    let project = editor.project.as_ref()?;
    Some(project.play_world().unwrap_or_else(|| project.preview()))
}

#[allow(clippy::type_complexity)]
pub fn sync_entities(
    mut commands: Commands,
    editor: NonSend<Editor>,
    toolbox: Res<Toolbox>,
    mut models: ResMut<struction_scene::render::Models>,
    mut watcher: ResMut<struction_assets::SourceWatcher>,
    mut proxies: Query<(
        Entity,
        &mut SceneEntity,
        &mut Transform,
        &mut Visibility,
        Option<&Shape>,
        Option<&Look>,
        Option<&Volume>,
    )>,
    mut synced: Local<Option<u64>>,
) {
    models.set_blender(toolbox.programs.blender.clone());
    watcher.settings.blender = toolbox.programs.blender.clone();
    if *synced == Some(editor.generation) {
        return;
    }
    *synced = Some(editor.generation);
    let Some(world) = source_world(&editor) else {
        for (entity, ..) in &proxies {
            commands.entity(entity).despawn();
        }
        return;
    };
    let mut wanted: HashMap<_, _> = editor
        .entities
        .iter()
        .filter(|e| e.is_instance())
        .map(|e| (e.key(), e))
        .collect();
    let shape_for = |entity: Entity| {
        world.get::<Shape>(entity).cloned().map(|mut shape| {
            if let Shape::Rigged {
                model: Some(path), ..
            } = &mut shape
                && !path.starts_with("engine://")
                && let Some(root) = &editor.root
            {
                *path = asset_root(root).join(&*path).to_string_lossy().into_owned();
            }
            shape
        })
    };
    for (entity, mut proxy, mut transform, mut visibility, shape, look, volume) in &mut proxies {
        let Some(entry) = wanted.remove(proxy.0.as_str()) else {
            commands.entity(entity).despawn();
            continue;
        };
        proxy.1 = entry.entity;
        *transform = entry.transform().unwrap_or_default();
        *visibility = if entry.disabled {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let next = shape_for(entry.entity);
        if shape != next.as_ref() {
            if let Some(next) = next {
                commands.entity(entity).insert(next);
            } else {
                commands.entity(entity).remove::<Shape>().remove::<Mesh3d>();
            }
        }
        let next = world.get::<Look>(entry.entity);
        if look != next {
            if let Some(next) = next {
                commands.entity(entity).insert(*next);
            } else {
                commands.entity(entity).remove::<Look>().remove::<Mesh3d>();
            }
        }
        let next = world.get::<Volume>(entry.entity);
        if volume != next {
            if let Some(next) = next {
                commands.entity(entity).insert(*next);
            } else {
                commands.entity(entity).remove::<Volume>();
            }
        }
    }
    for (key, entry) in wanted {
        let mut proxy = commands.spawn((
            SceneEntity(key.to_owned(), entry.entity),
            entry.transform().unwrap_or_default(),
            if entry.disabled {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            },
        ));
        if let Some(shape) = shape_for(entry.entity) {
            proxy.insert(shape);
        }
        if let Some(look) = world.get::<Look>(entry.entity) {
            proxy.insert(*look);
        }
        if let Some(volume) = world.get::<Volume>(entry.entity) {
            proxy.insert(*volume);
        }
    }
}

/// Solved joints are copied, not solved a second time in the editor's rendered world.
#[allow(clippy::type_complexity)]
pub fn sync_poses(
    editor: NonSend<Editor>,
    proxies: Query<(&SceneEntity, &Transform, &Visibility), Without<RigOf>>,
    mut rigs: Query<(&RigOf, &RigJoints, &mut Transform, &mut Visibility), Without<SceneEntity>>,
    mut joints: Query<&mut Transform, (Without<RigOf>, Without<SceneEntity>)>,
) {
    let Some(world) = source_world(&editor) else {
        return;
    };
    let camera = world
        .iter_entities()
        .find_map(|e| e.get::<struction_camera::PlayerCamera>());
    for (of, rig_joints, mut root, mut visible) in &mut rigs {
        let Ok((proxy, transform, visibility)) = proxies.get(of.0) else {
            continue;
        };
        *visible = if camera.is_some_and(|c| {
            editor.playing()
                && c.target == proxy.1
                && c.view == struction_camera::ViewMode::FirstPerson
        }) {
            Visibility::Hidden
        } else {
            *visibility
        };
        let source_rig = world.get::<Figure>(proxy.1).map(|f| f.rig);
        if let Some(rig) = source_rig
            && let Some(pose) = world.get::<SolvedPose>(rig)
        {
            *root = *world.get::<Transform>(rig).unwrap_or(transform);
            for (&joint, local) in rig_joints.0.iter().zip(&pose.0.locals) {
                if let Ok(mut current) = joints.get_mut(joint) {
                    *current = *local;
                }
            }
        } else {
            *root = *transform;
            if let Some(collider) = world.get::<Collider>(proxy.1) {
                root.translation -= transform.rotation
                    * Vec3::Y
                    * (-collider.aabb(Vec3::ZERO, Quat::IDENTITY).min.y);
            }
        }
    }
}

/// Parameter along the original world ray; inverse scaling must not normalize its direction.
pub fn bounds_hit(ray: Ray3d, transform: &GlobalTransform, bounds: &Aabb) -> Option<f32> {
    let inverse = transform.affine().inverse();
    let origin = inverse.transform_point3(ray.origin);
    let direction = inverse.transform_vector3(*ray.direction);
    if !origin.is_finite() || !direction.is_finite() {
        return None;
    }
    let min: Vec3 = (bounds.center - bounds.half_extents).into();
    let max: Vec3 = (bounds.center + bounds.half_extents).into();
    let (mut near, mut far) = (0.0f32, f32::INFINITY);
    for axis in 0..3 {
        if direction[axis].abs() < 1e-8 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - origin[axis]) / direction[axis];
            let b = (max[axis] - origin[axis]) / direction[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return None;
            }
        }
    }
    Some(near)
}

pub fn owner<'a>(
    mut entity: Entity,
    proxies: &'a Query<&SceneEntity>,
    parents: &Query<&ChildOf>,
    rigs: &Query<&RigOf>,
) -> Option<&'a SceneEntity> {
    for _ in 0..64 {
        if let Ok(proxy) = proxies.get(entity) {
            return Some(proxy);
        }
        entity = if let Ok(rig) = rigs.get(entity) {
            rig.0
        } else {
            parents.get(entity).ok()?.parent()
        };
    }
    None
}

pub fn selection_outline(
    editor: NonSend<Editor>,
    mut gizmos: Gizmos,
    meshes: Query<(Entity, &Aabb, &GlobalTransform)>,
    proxies: Query<&SceneEntity>,
    parents: Query<&ChildOf>,
    rigs: Query<&RigOf>,
) {
    if editor.playing() {
        return;
    }
    let Some(crate::state::Selected::Entity(selected)) = &editor.selected else {
        return;
    };
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for (entity, bounds, transform) in &meshes {
        if !owner(entity, &proxies, &parents, &rigs).is_some_and(|proxy| &proxy.0 == selected) {
            continue;
        }
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    let corner: Vec3 = bounds.center.into();
                    let point = transform.transform_point(
                        corner + Vec3::from(bounds.half_extents) * Vec3::new(x, y, z),
                    );
                    min = min.min(point);
                    max = max.max(point);
                }
            }
        }
    }
    if min.is_finite() {
        gizmos.cube(
            Transform::from_translation((min + max) * 0.5)
                .with_scale(max - min + Vec3::splat(0.02)),
            Color::srgb(0.95, 0.66, 0.23),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picking_respects_floor_extents_rotation_and_scale() {
        let bounds = Aabb::from_min_max(Vec3::splat(-0.5), Vec3::splat(0.5));
        let floor = GlobalTransform::from(
            Transform::from_xyz(0.0, -1.0, 0.0).with_scale(Vec3::new(30.0, 2.0, 30.0)),
        );
        let ray = Ray3d::new(Vec3::new(12.0, 5.0, 8.0), Dir3::NEG_Y);
        assert!((bounds_hit(ray, &floor, &bounds).unwrap() - 5.0).abs() < 1e-5);
        assert!(
            bounds_hit(
                Ray3d::new(Vec3::new(16.0, 5.0, 0.0), Dir3::NEG_Y),
                &floor,
                &bounds
            )
            .is_none()
        );
        let rotated = GlobalTransform::from(
            Transform::from_rotation(Quat::from_rotation_z(std::f32::consts::FRAC_PI_2))
                .with_scale(Vec3::new(4.0, 1.0, 1.0)),
        );
        assert!(
            bounds_hit(
                Ray3d::new(Vec3::new(0.0, 1.8, 3.0), Dir3::NEG_Z),
                &rotated,
                &bounds
            )
            .is_some()
        );
    }
}
