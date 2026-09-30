//! Fade only the instances obstructing the player; shared source materials stay untouched.
use std::collections::{HashMap, HashSet};

use bevy::{light::NotShadowCaster, prelude::*};
use struction_gravity::LocalUp;
use struction_physics::{CameraOcclusion, Surface, avian3d::prelude::*, camera_obstructions};

use crate::{Player, PlaygroundSystems};

pub struct GroundTransparencyPlugin;

#[derive(Resource, Default)]
struct FadingSurfaces(HashMap<Entity, FadingSurface>);

struct FadingSurface {
    original: Handle<StandardMaterial>,
    faded: Handle<StandardMaterial>,
    alpha: f32,
    was_not_shadow_caster: bool,
}

impl Plugin for GroundTransparencyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FadingSurfaces>().add_systems(
            Update,
            fade_obstructions
                .after(PlaygroundSystems::Camera)
                .before(PlaygroundSystems::Hud),
        );
    }
}

type SurfaceData = (
    Entity,
    &'static mut MeshMaterial3d<StandardMaterial>,
    Has<NotShadowCaster>,
);

#[allow(clippy::too_many_arguments)]
fn fade_obstructions(
    mut commands: Commands,
    camera: Query<(&Transform, &CameraOcclusion), With<Camera3d>>,
    player: Query<(Entity, &Transform, &LocalUp), With<Player>>,
    spatial: SpatialQuery,
    mut surfaces: Query<SurfaceData, With<Surface>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fading: ResMut<FadingSurfaces>,
    time: Res<Time>,
) {
    let (blocked, settings) = match (camera.single(), player.single()) {
        (Ok((camera, settings)), Ok((player, transform, up))) => (
            camera_obstructions(
                &spatial,
                transform.translation + *up.0 * 0.5,
                camera.translation,
                settings.probe_radius,
                &SpatialQueryFilter::default().with_excluded_entities([player]),
            )
            .into_iter()
            .collect::<HashSet<_>>(),
            *settings,
        ),
        _ => (HashSet::new(), CameraOcclusion::default()),
    };
    let opacity = if settings.blocked_opacity.is_finite() {
        settings.blocked_opacity.clamp(0.0, 1.0)
    } else {
        0.12
    };
    let speed = if settings.fade_speed.is_finite() {
        settings.fade_speed.max(0.1)
    } else {
        16.0
    };
    let blend = 1.0 - (-speed * time.delta_secs()).exp();
    for (entity, mut handle, not_shadow_caster) in &mut surfaces {
        if blocked.contains(&entity)
            && !fading.0.contains_key(&entity)
            && let Some(original) = materials.get(&handle.0)
            && matches!(original.alpha_mode, AlphaMode::Opaque | AlphaMode::Mask(_))
        {
            let mut material = original.clone();
            material.alpha_mode = AlphaMode::Blend;
            let faded = materials.add(material);
            fading.0.insert(
                entity,
                FadingSurface {
                    original: handle.0.clone(),
                    faded: faded.clone(),
                    alpha: 1.0,
                    was_not_shadow_caster: not_shadow_caster,
                },
            );
            handle.0 = faded;
            commands.entity(entity).insert(NotShadowCaster);
        }
        let Some(state) = fading.0.get_mut(&entity) else {
            continue;
        };
        // An external material change wins over a pending fade restoration.
        if handle.0 != state.faded {
            if !state.was_not_shadow_caster {
                commands.entity(entity).remove::<NotShadowCaster>();
            }
            materials.remove(state.faded.id());
            fading.0.remove(&entity);
            continue;
        }
        let target = if blocked.contains(&entity) {
            opacity
        } else {
            1.0
        };
        state.alpha += (target - state.alpha) * blend;
        if let Some(mut material) = materials.get_mut(&state.faded) {
            material.base_color.set_alpha(state.alpha);
        }
        if !blocked.contains(&entity) && state.alpha > 0.995 {
            handle.0 = state.original.clone();
            if !state.was_not_shadow_caster {
                commands.entity(entity).remove::<NotShadowCaster>();
            }
            materials.remove(state.faded.id());
            fading.0.remove(&entity);
        }
    }
    // Do not retain material assets for despawned surfaces.
    fading.0.retain(|entity, state| {
        if surfaces.contains(*entity) {
            true
        } else {
            materials.remove(state.faded.id());
            false
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use struction_physics::testing::*;

    #[test]
    fn blocking_planet_fades_and_restores_without_changing_shared_materials() {
        let mut app = headless_app_with(GroundTransparencyPlugin);
        app.init_resource::<Assets<StandardMaterial>>();
        let original = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let planet = app
            .world_mut()
            .spawn((
                RigidBody::Static,
                Collider::sphere(4.0),
                Surface::default(),
                MeshMaterial3d(original.clone()),
                Transform::default(),
            ))
            .id();
        let other = app
            .world_mut()
            .spawn((
                Surface::default(),
                MeshMaterial3d(original.clone()),
                Transform::from_xyz(50.0, 0.0, 0.0),
            ))
            .id();
        app.world_mut()
            .spawn((Player, Transform::from_xyz(0.0, 0.0, 5.0), LocalUp(Dir3::Z)));
        let camera = app
            .world_mut()
            .spawn((
                Camera3d::default(),
                CameraOcclusion::default(),
                Transform::from_xyz(0.0, 0.0, -5.0),
            ))
            .id();
        step(&mut app, 40);
        let faded = app
            .world()
            .get::<MeshMaterial3d<StandardMaterial>>(planet)
            .unwrap()
            .0
            .clone();
        assert_ne!(faded, original);
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(other)
                .unwrap()
                .0,
            original
        );
        let materials = app.world().resource::<Assets<StandardMaterial>>();
        assert_eq!(
            materials.get(&original).unwrap().alpha_mode,
            AlphaMode::Opaque
        );
        assert!(materials.get(&faded).unwrap().base_color.alpha() < 0.13);
        assert!(app.world().get::<NotShadowCaster>(planet).is_some());
        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = Vec3::Z * 10.0;
        step(&mut app, 40);
        assert_eq!(
            app.world()
                .get::<MeshMaterial3d<StandardMaterial>>(planet)
                .unwrap()
                .0,
            original
        );
        assert!(app.world().get::<NotShadowCaster>(planet).is_none());
        assert!(
            app.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&faded)
                .is_none()
        );
    }
}
