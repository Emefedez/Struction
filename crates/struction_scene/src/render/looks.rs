//! Shapes and looks as meshes and materials, redrawn whenever live reload edits either.

use bevy::{light::NotShadowCaster, prelude::*};
use struction_physics::{Volume, VolumeShape};

use super::sight_fade::{FadeMaterial, FadesWith, fade_material};
use crate::{Finish, Look, Shape};

/// The mesh of a box or sphere; humanoids are drawn by their rig.
pub fn shape_mesh(shape: &Shape) -> Option<Mesh> {
    match *shape {
        Shape::Box { size } => Some(Cuboid::from_size(size).into()),
        Shape::Sphere { radius } => Some(Sphere::new(radius).mesh().ico(5).expect("valid sphere")),
        Shape::Humanoid { .. } => None,
    }
}

fn matte(color: Color) -> StandardMaterial {
    StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.88,
        ..default()
    }
}

type Dressed<'a> = (
    Entity,
    &'a Look,
    Option<&'a Shape>,
    Option<&'a Volume>,
    Option<&'a WaterSurface>,
);

type Redrawn = (
    With<Look>,
    Or<(Changed<Look>, Changed<Shape>, Changed<Volume>)>,
);

/// The surface drawn for a water volume, replaced when live reload changes the volume's look.
#[derive(Component)]
pub struct WaterSurface(Entity);

/// Gives authored entities their mesh and material from their `Shape` and `Look`, again whenever
/// live reload edits either.
pub(crate) fn dress_looks(
    mut commands: Commands,
    looks: Query<Dressed, Redrawn>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut fading: ResMut<Assets<FadeMaterial>>,
) {
    for (entity, look, shape, volume, surface) in &looks {
        if let Some(surface) = surface {
            commands.entity(surface.0).despawn();
        }
        commands.entity(entity).remove::<(
            WaterSurface,
            Mesh3d,
            MeshMaterial3d<StandardMaterial>,
            MeshMaterial3d<FadeMaterial>,
        )>();
        let [red, green, blue] = look.color;
        let color = Color::srgba(red, green, blue, look.opacity);
        if look.finish == Finish::Water {
            let Some(VolumeShape::Box { half_extents }) = volume.map(|v| v.shape) else {
                warn!("a water look needs a box volume; {entity} is not drawn");
                continue;
            };
            // A closed transparent box overlaps the pool floor and blends its own unsorted faces.
            let size = half_extents.xz() * 2.0;
            let surface = commands
                .spawn((
                    Name::new("Water surface"),
                    Transform::from_xyz(0.0, half_extents.y, 0.0),
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x, size.y))),
                    NotShadowCaster,
                    // A swimmer below the surface stays visible through it.
                    FadesWith(entity),
                    MeshMaterial3d(fade_material(
                        &mut fading,
                        StandardMaterial {
                            base_color: color,
                            alpha_mode: AlphaMode::Blend,
                            perceptual_roughness: 0.24,
                            cull_mode: None,
                            double_sided: true,
                            ..default()
                        },
                    )),
                    ChildOf(entity),
                ))
                .id();
            commands
                .entity(entity)
                .insert((Visibility::default(), WaterSurface(surface)));
            continue;
        }
        // Humanoids are drawn by their rig (see `figures`).
        let Some(mesh) = shape.and_then(shape_mesh) else {
            if shape.is_none() {
                warn!("{entity} has a look but no shape to draw");
            }
            continue;
        };
        let mesh = Mesh3d(meshes.add(mesh));
        let mut entity = commands.entity(entity);
        match look.finish {
            // Ground that can hide the player gets a cut-out along the camera's line of sight.
            Finish::Ground => entity.insert((
                mesh,
                MeshMaterial3d(fade_material(&mut fading, matte(color))),
            )),
            _ => entity.insert((mesh, MeshMaterial3d(materials.add(matte(color))))),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reloaded_water_look_is_redrawn_once() {
        let mut app = App::new();
        app.init_resource::<Assets<Mesh>>()
            .init_resource::<Assets<StandardMaterial>>()
            .init_resource::<Assets<FadeMaterial>>()
            .add_systems(Update, dress_looks);
        let pool = app
            .world_mut()
            .spawn((
                Look {
                    finish: Finish::Water,
                    ..default()
                },
                Volume {
                    shape: VolumeShape::Box {
                        half_extents: Vec3::ONE,
                    },
                },
            ))
            .id();
        app.update();
        // What live reload does to an edited field.
        app.world_mut().get_mut::<Look>(pool).unwrap().color = [0.1, 0.2, 0.3];
        app.update();

        let mut surfaces = app
            .world_mut()
            .query_filtered::<&MeshMaterial3d<FadeMaterial>, With<FadesWith>>();
        let surfaces: Vec<_> = surfaces.iter(app.world()).collect();
        assert_eq!(surfaces.len(), 1);
        let material = app
            .world()
            .resource::<Assets<FadeMaterial>>()
            .get(&surfaces[0].0)
            .unwrap();
        assert_eq!(material.base.base_color, Color::srgba(0.1, 0.2, 0.3, 1.0));
    }
}
