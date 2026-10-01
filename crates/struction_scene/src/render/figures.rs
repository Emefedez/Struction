//! Rigged shapes: every entity whose `Shape` is `Rigged` gets the named skeleton as a
//! procedurally animated rig that follows its character body, dressed with the shape's model or,
//! without one, with simple shapes. The player, NPCs and anything else that walks share this path.
//!
//! A model is a source in the app's `assets/` or, with an `engine://` prefix, the engine's (such
//! as a `.blend`). It compiles to a `.smesh` beside it on a background thread the first time an
//! entity uses it, and again whenever it is saved while the app runs. Pieces are named `<joint>.<piece>` and modeled around the joint's rest
//! position, so each mesh is parented to its joint as is. A model that cannot be compiled or
//! loaded falls back to simple shapes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crate::{Shape, engine_assets};
use bevy::asset::io::file::FileAssetReader;
use bevy::prelude::*;
use struction_anim::{humanoid, plugin::RigJoints, rig::Rig};
use struction_assets::{
    AssetError, COMPILED_EXTENSION, CompileSettings, CompileStatus, CompiledModel, SourceWatcher,
    SourceWatcherPlugin, StructionAssetsPlugin, compile_asset_with, format::BundleMaterial,
};
use struction_camera::{PlayerCamera, ViewMode};
use struction_character::RigOf;

/// On a rig: the model it is dressed with, or `None` for simple shapes.
#[derive(Component, PartialEq)]
pub struct Dressed(Option<AssetId<CompiledModel>>);

/// A mesh dressing the rig it names, replaced when the rig is dressed again.
#[derive(Component)]
pub struct Piece(Entity);

enum Model {
    Compiling {
        job: JoinHandle<Result<CompileStatus, AssetError>>,
        source: PathBuf,
        compiled: PathBuf,
    },
    Loaded {
        handle: Handle<CompiledModel>,
        /// The model's materials, made once per load.
        palette: Option<Vec<(String, Handle<StandardMaterial>)>>,
    },
    Missing,
}

/// Models by the source path shapes name: relative to the app's `assets/`, or to the engine's
/// with `engine://`.
#[derive(Resource)]
pub struct Models {
    root: PathBuf,
    pub settings: CompileSettings,
    engine: PathBuf,
    models: HashMap<String, Model>,
    pub errors: std::collections::BTreeMap<String, String>,
}

impl Models {
    pub fn set_blender(&mut self, blender: struction_assets::Blender) {
        if self.settings.blender.executable != blender.executable {
            self.models
                .retain(|_, model| !matches!(model, Model::Missing));
            self.errors.clear();
        }
        self.settings.blender = blender;
    }

    pub fn is_loading(&self, assets: &AssetServer) -> bool {
        self.models.values().any(|model| match model {
            Model::Compiling { .. } => true,
            Model::Loaded { handle, .. } => {
                !assets.is_loaded_with_dependencies(handle.id())
                    && !matches!(
                        assets.load_state(handle.id()),
                        bevy::asset::LoadState::Failed(_)
                    )
            }
            Model::Missing => false,
        })
    }

    /// The source file of a model path and the file it compiles to.
    fn files(&self, model: &str) -> (PathBuf, PathBuf) {
        let (root, path) = match model.strip_prefix(ENGINE_SOURCE) {
            Some(path) => (&self.engine, path),
            None => (&self.root, model),
        };
        (root.join(path), root.join(compiled_path(path)))
    }
}

const ENGINE_SOURCE: &str = "engine://";

pub(crate) struct FiguresPlugin;

impl Plugin for FiguresPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((StructionAssetsPlugin, SourceWatcherPlugin))
            .insert_resource(Models {
                settings: CompileSettings::default(),
                root: FileAssetReader::get_base_path().join("assets"),
                engine: engine_assets(),
                models: HashMap::new(),
                errors: default(),
            });
    }
}

/// The first-person camera sits inside the head, so its target's own rig is not drawn.
pub(crate) fn hide_in_first_person(
    cameras: Query<&PlayerCamera>,
    mut rigs: Query<(&RigOf, &mut Visibility)>,
) {
    for camera in &cameras {
        for (rig, mut visibility) in &mut rigs {
            if rig.0 == camera.target {
                visibility.set_if_neq(match camera.view {
                    ViewMode::FirstPerson => Visibility::Hidden,
                    ViewMode::ThirdPerson => Visibility::Inherited,
                });
            }
        }
    }
}

/// The asset path of the `.smesh` a model source compiles to, in the same asset source.
fn compiled_path(model: &str) -> String {
    Path::new(model)
        .with_extension(COMPILED_EXTENSION)
        .to_string_lossy()
        .replace('\\', "/")
}

fn model(shape: &Shape) -> Option<Option<&str>> {
    match shape {
        Shape::Rigged { model, .. } => Some(model.as_deref()),
        _ => None,
    }
}

/// Starts compiling the models new shapes name.
pub(crate) fn request_models(mut models: ResMut<Models>, shapes: Query<&Shape>) {
    let models = &mut *models;
    for path in shapes.iter().filter_map(|shape| model(shape).flatten()) {
        if models.models.contains_key(path) {
            continue;
        }
        let (source, compiled) = models.files(path);
        let job = {
            let (source, compiled) = (source.clone(), compiled.clone());
            let settings = models.settings.clone();
            std::thread::spawn(move || {
                compile_asset_with(&source, &compiled, &settings.with_recipe_of(&source)?)
            })
        };
        models.models.insert(
            path.to_owned(),
            Model::Compiling {
                job,
                source,
                compiled,
            },
        );
    }
}

/// Loads models whose compile finished and watches their sources for hot reload.
pub(crate) fn finish_compiles(
    mut models: ResMut<Models>,
    mut watcher: ResMut<SourceWatcher>,
    asset_server: Res<AssetServer>,
) {
    let models = &mut *models;
    for (path, model) in &mut models.models {
        let Model::Compiling { job, .. } = model else {
            continue;
        };
        if !job.is_finished() {
            continue;
        }
        let Model::Compiling {
            job,
            source,
            compiled,
        } = std::mem::replace(model, Model::Missing)
        else {
            unreachable!("matched above");
        };
        match job.join() {
            Ok(Ok(CompileStatus::Compiled)) => info!("compiled {}", compiled.display()),
            Ok(Ok(CompileStatus::UpToDate)) => {}
            Ok(Err(error)) if compiled.is_file() => {
                warn!("using the last compiled {path}: {error}");
                models
                    .errors
                    .insert(path.clone(), format!("Using cached model: {error}"));
            }
            Ok(Err(error)) => {
                warn!("no {path}, drawing it with shapes: {error}");
                models.errors.insert(path.clone(), error.to_string());
                continue;
            }
            Err(_) => {
                error!("compiling {path} panicked; drawing it with shapes");
                models
                    .errors
                    .insert(path.clone(), "Model compilation panicked".into());
                continue;
            }
        }
        let asset_path = compiled_path(path);
        watcher.watch(&source, &compiled, Some(asset_path.clone().into()));
        *model = Model::Loaded {
            handle: asset_server.load(asset_path),
            palette: None,
        };
    }
}

type RigsToDress<'a> = (
    Entity,
    &'a RigOf,
    &'a Rig,
    &'a RigJoints,
    Option<&'a Dressed>,
);

/// Dresses rigs whose model is ready, again after a hot reload or a changed shape. Models that
/// fail to load are drawn with shapes.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dress_figures(
    mut commands: Commands,
    mut models: ResMut<Models>,
    mut reloads: MessageReader<AssetEvent<CompiledModel>>,
    asset_server: Res<AssetServer>,
    compiled: Res<Assets<CompiledModel>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    bodies: Query<&Shape>,
    rigs: Query<RigsToDress>,
    pieces: Query<(Entity, &Piece)>,
) {
    let reloaded: Vec<_> = reloads
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    for model in models.models.values_mut() {
        if let Model::Loaded { handle, palette } = model {
            if asset_server.load_state(&*handle).is_failed() {
                error!("{:?} failed to load; drawing it with shapes", handle.path());
                *model = Model::Missing;
            } else if reloaded.contains(&handle.id()) {
                *palette = None;
            }
        }
    }

    for (rig_entity, of, rig, joints, dressed) in &rigs {
        let Some(path) = bodies.get(of.0).ok().and_then(model) else {
            continue;
        };
        let look = match path.and_then(|path| models.models.get_mut(path)) {
            None | Some(Model::Missing) => None,
            Some(Model::Compiling { .. }) => continue,
            Some(Model::Loaded { handle, palette }) => {
                let Some(model) = compiled.get(&*handle) else {
                    continue;
                };
                Some((handle.id(), model, palette))
            }
        };
        let id = look.as_ref().map(|(id, ..)| *id);
        if dressed.is_some_and(|d| d.0 == id && id.is_none_or(|id| !reloaded.contains(&id))) {
            continue;
        }
        for (piece, of) in &pieces {
            if of.0 == rig_entity {
                commands.entity(piece).despawn();
            }
        }
        for &joint in &joints.0 {
            commands.entity(joint).insert(Visibility::default());
        }
        match look {
            Some((_, model, palette)) => {
                let palette = palette.get_or_insert_with(|| {
                    model
                        .materials
                        .iter()
                        .map(|m| (m.name.clone(), materials.add(standard(m))))
                        .collect()
                });
                dress_with_model(&mut commands, rig_entity, rig, joints, model, palette);
            }
            None => dress_with_shapes(
                &mut commands,
                rig_entity,
                rig,
                joints,
                &mut meshes,
                &mut materials,
            ),
        }
        commands.entity(rig_entity).insert(Dressed(id));
    }
}

fn dress_with_model(
    commands: &mut Commands,
    rig_entity: Entity,
    rig: &Rig,
    joints: &RigJoints,
    model: &CompiledModel,
    palette: &[(String, Handle<StandardMaterial>)],
) {
    let defs = rig.skeleton.joints();
    for node in &model.nodes {
        let joint_name = node.name.split('.').next().unwrap_or(&node.name);
        let Some(joint) = defs.iter().position(|def| def.name == joint_name) else {
            warn!("model piece {:?} names no rig joint", node.name);
            continue;
        };
        for &mesh in &node.meshes {
            let mesh = &model.meshes[mesh as usize];
            let material = mesh
                .material
                .as_ref()
                .and_then(|name| palette.iter().find(|(n, _)| n == name))
                .map(|(_, handle)| handle.clone())
                .unwrap_or_default();
            commands.spawn((
                Piece(rig_entity),
                Mesh3d(mesh.lods[0].clone()),
                MeshMaterial3d(material),
                Transform::default(),
                ChildOf(joints.0[joint]),
            ));
        }
    }
}

/// A body of simple shapes attached to the joint entities.
fn dress_with_shapes(
    commands: &mut Commands,
    rig_entity: Entity,
    rig: &Rig,
    joints: &RigJoints,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let matte = |color| StandardMaterial {
        base_color: color,
        perceptual_roughness: 0.88,
        ..default()
    };
    let skin = materials.add(matte(Color::srgb(0.96, 0.86, 0.44)));
    let dark = materials.add(matte(Color::srgb(0.30, 0.27, 0.34)));
    let defs = rig.skeleton.joints();
    for (def, &joint) in defs.iter().zip(&joints.0) {
        let name = def.name.as_str();
        let decoration = if name == "head" {
            Some((
                meshes.add(Sphere::new(0.11)),
                skin.clone(),
                Transform::from_xyz(0.0, 0.08, 0.0),
            ))
        } else if name.starts_with("hand") {
            Some((
                meshes.add(Sphere::new(0.04)),
                skin.clone(),
                Transform::from_xyz(0.0, -0.03, 0.0),
            ))
        } else if name.starts_with("foot") {
            // Toes point forward (-Z) from the ankle, with the sole on the ground.
            Some((
                meshes.add(Cuboid::new(0.09, humanoid::ANKLE_HEIGHT, 0.24)),
                dark.clone(),
                Transform::from_xyz(0.0, -humanoid::ANKLE_HEIGHT * 0.5, -0.06),
            ))
        } else {
            None
        };
        if let Some((mesh, material, transform)) = decoration {
            commands.spawn((
                Piece(rig_entity),
                Mesh3d(mesh),
                MeshMaterial3d(material),
                transform,
                ChildOf(joint),
            ));
        }

        // A limb segment from the parent joint to this one, owned by the parent so it turns with
        // it.
        let Some(parent) = def.parent.filter(|&p| p != rig.root) else {
            continue;
        };
        let offset = def.rest.translation;
        let length = offset.length();
        let radius = segment_radius(&defs[parent].name);
        if length < 1e-3 || radius == 0.0 {
            continue;
        }
        let legs = ["thigh", "shin"]
            .iter()
            .any(|l| defs[parent].name.starts_with(l));
        commands.spawn((
            Piece(rig_entity),
            Mesh3d(meshes.add(Capsule3d::new(radius, (length - radius).max(0.01)))),
            MeshMaterial3d(if legs { dark.clone() } else { skin.clone() }),
            Transform::from_translation(offset * 0.5)
                .with_rotation(Quat::from_rotation_arc(Vec3::Y, offset / length)),
            ChildOf(joints.0[parent]),
        ));
    }
}

/// Thickness of the segment that starts at a joint; zero draws nothing.
fn segment_radius(parent: &str) -> f32 {
    match parent.split('_').next().unwrap_or(parent) {
        "hips" => 0.09,
        "spine" | "chest" => 0.11,
        "neck" => 0.05,
        "upper" | "thigh" => 0.06,
        "forearm" | "shin" => 0.045,
        "fingers" | "thumb" | "hand" => 0.012,
        _ => 0.0,
    }
}

/// Emission carries over as is: Bevy's default emissive ignores camera exposure, so strength 1
/// is full brightness on screen, much as in Blender.
fn standard(material: &BundleMaterial) -> StandardMaterial {
    let [r, g, b] = material.emissive;
    StandardMaterial {
        base_color: LinearRgba::from_f32_array(material.base_color).into(),
        metallic: material.metallic,
        perceptual_roughness: material.roughness,
        emissive: LinearRgba::rgb(r, g, b),
        ..default()
    }
}

pub(crate) fn rig_visibility(
    mut commands: Commands,
    rigs: Query<Entity, (With<RigOf>, Without<Visibility>)>,
) {
    for entity in &rigs {
        commands.entity(entity).insert(Visibility::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_compile_beside_their_source() {
        assert_eq!(
            compiled_path("models/blood_knight.blend"),
            "models/blood_knight.smesh"
        );
    }
}
