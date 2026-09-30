//! Surfaces hiding the player get a soft cut-out along the camera's line of sight instead of
//! vanishing whole, so the ground around the hole stays. The shader does the geometry; this
//! module decides which materials fade and how strongly, and blends opaque ones only while
//! they fade, so they sort and render as opaque otherwise.
use std::collections::{HashMap, HashSet};

use bevy::{
    asset::embedded_asset,
    pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin},
    prelude::*,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::ShaderRef,
};
use struction_gravity::LocalUp;
use struction_physics::{CameraOcclusion, avian3d::prelude::*, camera_obstructions};

use crate::{Player, PlaygroundSystems};

pub type FadeMaterial = ExtendedMaterial<StandardMaterial, SightFade>;

#[derive(Asset, AsBindGroup, Reflect, Clone, Debug, Default)]
pub struct SightFade {
    #[uniform(100)]
    pub sight: SightUniform,
}

/// Mirrors `SightFade` in `sight_fade.wgsl`.
#[derive(ShaderType, Reflect, Clone, Copy, Debug, Default, PartialEq)]
pub struct SightUniform {
    pub focus: Vec3,
    pub radius: f32,
    pub strength: f32,
    pub min_opacity: f32,
}

impl MaterialExtension for SightFade {
    fn fragment_shader() -> ShaderRef {
        "embedded://struction_playground/sight_fade.wgsl".into()
    }
}

/// Fades when another entity blocks the view, e.g. a water surface with its volume's sensor.
#[derive(Component, Clone, Copy, Debug)]
pub struct FadesWith(pub Entity);

pub fn fade_material(
    materials: &mut Assets<FadeMaterial>,
    base: StandardMaterial,
) -> Handle<FadeMaterial> {
    materials.add(ExtendedMaterial {
        base,
        extension: SightFade::default(),
    })
}

/// Alpha modes of opaque materials switched to blending while they fade.
#[derive(Resource, Default)]
struct RestingAlphaModes(HashMap<AssetId<FadeMaterial>, AlphaMode>);

pub struct SightFadePlugin;

impl Plugin for SightFadePlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "sight_fade.wgsl");
        app.add_plugins(MaterialPlugin::<FadeMaterial>::default())
            .init_resource::<RestingAlphaModes>()
            .add_systems(
                Update,
                fade_sight_lines
                    .after(PlaygroundSystems::Camera)
                    .before(PlaygroundSystems::Hud),
            );
    }
}

fn fade_sight_lines(
    camera: Query<(&Transform, &CameraOcclusion), With<Camera3d>>,
    player: Query<(Entity, &Transform, &LocalUp), With<Player>>,
    spatial: SpatialQuery,
    surfaces: Query<(Entity, &MeshMaterial3d<FadeMaterial>, Option<&FadesWith>)>,
    mut materials: ResMut<Assets<FadeMaterial>>,
    mut resting: ResMut<RestingAlphaModes>,
    time: Res<Time>,
) {
    let (Ok((camera, settings)), Ok((player, transform, up))) = (camera.single(), player.single())
    else {
        return;
    };
    let finite_or = |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
    let focus = transform.translation + *up.0 * 0.5;
    let lead = finite_or(settings.anticipation, 1.5).max(0.0);
    let behind = (camera.translation - focus).normalize_or_zero() * lead;
    let blocked: HashSet<_> = camera_obstructions(
        &spatial,
        focus,
        camera.translation + behind,
        settings.probe_radius,
        &SpatialQueryFilter::default().with_excluded_entities([player]),
    )
    .into_iter()
    .collect();
    // Materials may be shared: one blocked user fades them all, only near the sight line.
    let mut wanted: HashMap<AssetId<FadeMaterial>, bool> = HashMap::new();
    for (entity, material, with) in &surfaces {
        let blocker = with.map_or(entity, |with| with.0);
        *wanted.entry(material.id()).or_default() |= blocked.contains(&blocker);
    }

    let opacity = finite_or(settings.blocked_opacity, 0.12).clamp(0.0, 1.0);
    let radius = finite_or(settings.cutout_radius, 1.6).max(0.0);
    let speed = finite_or(settings.fade_speed, 5.0).max(0.1);
    let blend = 1.0 - (-speed * time.delta_secs()).exp();
    for (id, blocked) in wanted {
        let goal = if blocked { 1.0 } else { 0.0 };
        // Leave idle materials alone so they are not re-uploaded every frame.
        let idle = materials
            .get(id)
            .is_none_or(|m| !blocked && m.extension.sight.strength == 0.0);
        if idle {
            continue;
        }
        let Some(mut material) = materials.get_mut(id) else {
            continue;
        };
        let sight = &mut material.extension.sight;
        sight.strength += (goal - sight.strength) * blend;
        if !blocked && sight.strength < 0.01 {
            sight.strength = 0.0;
        }
        sight.focus = focus;
        sight.radius = radius;
        sight.min_opacity = opacity;
        let fading = sight.strength > 0.0;
        let alpha_mode = &mut material.base.alpha_mode;
        if fading && matches!(*alpha_mode, AlphaMode::Opaque | AlphaMode::Mask(_)) {
            resting.0.insert(id, *alpha_mode);
            *alpha_mode = AlphaMode::Blend;
        } else if !fading && let Some(mode) = resting.0.remove(&id) {
            *alpha_mode = mode;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use struction_physics::{VolumeShape, testing::*, water};

    fn app() -> App {
        let mut app = headless_app();
        app.init_resource::<Assets<FadeMaterial>>()
            .init_resource::<RestingAlphaModes>()
            .add_systems(Update, fade_sight_lines);
        app
    }

    fn material(app: &mut App) -> Handle<FadeMaterial> {
        let mut materials = app.world_mut().resource_mut::<Assets<FadeMaterial>>();
        fade_material(&mut materials, StandardMaterial::default())
    }

    fn sight(app: &App, handle: &Handle<FadeMaterial>) -> SightUniform {
        app.world()
            .resource::<Assets<FadeMaterial>>()
            .get(handle)
            .unwrap()
            .extension
            .sight
    }

    fn alpha_mode(app: &App, handle: &Handle<FadeMaterial>) -> AlphaMode {
        let materials = app.world().resource::<Assets<FadeMaterial>>();
        materials.get(handle).unwrap().base.alpha_mode
    }

    fn spawn_viewers(app: &mut App, camera: Vec3) -> Entity {
        app.world_mut()
            .spawn((Player, Transform::from_xyz(0.0, 0.0, 5.0), LocalUp(Dir3::Z)));
        app.world_mut()
            .spawn((
                Camera3d::default(),
                CameraOcclusion::default(),
                Transform::from_translation(camera),
            ))
            .id()
    }

    #[test]
    fn a_blocking_planet_cuts_out_around_the_player_and_recovers() {
        let mut app = app();
        let planet_material = material(&mut app);
        let other_material = material(&mut app);
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::sphere(4.0),
            MeshMaterial3d(planet_material.clone()),
            Transform::default(),
        ));
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::sphere(1.0),
            MeshMaterial3d(other_material.clone()),
            Transform::from_xyz(50.0, 0.0, 0.0),
        ));
        let camera = spawn_viewers(&mut app, Vec3::Z * -5.0);
        step(&mut app, 120);

        let faded = sight(&app, &planet_material);
        assert!(faded.strength > 0.99, "{faded:?}");
        assert_eq!(alpha_mode(&app, &planet_material), AlphaMode::Blend);
        // The cut-out follows the player's focus, half a meter along its up.
        assert!(faded.focus.abs_diff_eq(Vec3::Z * 5.5, 1e-5));
        assert_eq!(faded.radius, CameraOcclusion::default().cutout_radius);
        assert_eq!(sight(&app, &other_material), SightUniform::default());

        app.world_mut()
            .get_mut::<Transform>(camera)
            .unwrap()
            .translation = Vec3::Z * 10.0;
        step(&mut app, 120);
        assert_eq!(sight(&app, &planet_material).strength, 0.0);
        assert_eq!(alpha_mode(&app, &planet_material), AlphaMode::Opaque);
    }

    #[test]
    fn a_surface_just_behind_the_camera_is_already_fading() {
        let mut app = app();
        let wall_material = material(&mut app);
        app.world_mut().spawn((
            RigidBody::Static,
            Collider::cuboid(20.0, 20.0, 0.2),
            MeshMaterial3d(wall_material.clone()),
            Transform::from_xyz(0.0, 0.0, -5.0),
        ));
        // The camera has not crossed the wall yet: it is one meter in front of it.
        spawn_viewers(&mut app, Vec3::Z * -4.0);
        step(&mut app, 120);
        assert!(sight(&app, &wall_material).strength > 0.99);
    }

    #[test]
    fn a_water_surface_fades_while_its_volume_hides_a_sinking_player() {
        let mut app = app();
        let surface_material = material(&mut app);
        // The player is below the surface of a pool; the camera looks down from above it.
        let volume = app
            .world_mut()
            .spawn((
                water(VolumeShape::Box {
                    half_extents: Vec3::new(3.0, 3.0, 8.0),
                }),
                Transform::from_xyz(0.0, 0.0, 5.0),
            ))
            .id();
        app.world_mut().spawn((
            MeshMaterial3d(surface_material.clone()),
            FadesWith(volume),
            Transform::from_xyz(0.0, 3.0, 5.0),
        ));
        spawn_viewers(&mut app, Vec3::new(0.0, 8.0, 12.0));
        step(&mut app, 120);
        assert!(sight(&app, &surface_material).strength > 0.99);
    }
}
