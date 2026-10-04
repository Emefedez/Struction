//! Scene vocabulary: what authored entities are and look like, and the plugin that loads a
//! project into a running world.
//!
//! A project's definitions and scenes are loaded by `struction_data` and spawned by
//! `struction_world`; saved edits reach the running world through live reload. [`Shape`] is an
//! entity's geometry (a box or sphere that is also its collider, or an animated rig) and [`Look`]
//! how it is drawn. Everything here is headless; the `render` feature adds
//! [`render::SceneRenderPlugin`], which draws shapes and looks, dresses rigs with their models and
//! fades surfaces that hide the player.
//!
//! The engine's base definitions (actors, terrain, gravity, props, regions, characters) ship in
//! `content/definitions` and load as the read-only `engine` library every project descends from
//! and may override; their models are under `content/assets`, the `engine://` asset source.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use struction_anim::{humanoid, rig::Rig};
use struction_core::CorePlugin;
use struction_data::DataPlugin;
use struction_physics::avian3d::prelude::*;
use struction_world::{LiveReloadPlugin, LiveReloaded, WorldPlugin, WorldSet};

pub mod rigs;
pub use rigs::{Figure, SceneRigPlugin, SceneRigSystems};

#[cfg(feature = "render")]
pub mod render;

/// The engine's base definitions, loaded as the `engine` library.
pub fn engine_definitions() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("content/definitions")
}

/// The engine's models and other assets, the `engine://` asset source.
pub fn engine_assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("content/assets")
}

/// What an authored entity looks like, in meters. Boxes and spheres are also its collider when
/// it has a `RigidBody`; the rendered host draws them with its [`Look`].
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
#[component(on_insert = insert_shape_collider)]
pub enum Shape {
    Box {
        size: Vec3,
    },
    Sphere {
        radius: f32,
    },
    /// A procedurally animated skeleton following the entity's character body (which brings its
    /// own collider). `rig` names a skeleton in [`Rigs`] (`humanoid`). `model` is an asset source
    /// (such as `engine://models/blood_knight.blend`) whose pieces, named `<joint>.<piece>`, ride
    /// on the rig's joints; without one, the rig is drawn with simple shapes.
    Rigged {
        rig: String,
        model: Option<String>,
    },
}

impl Shape {
    pub fn collider(&self) -> Option<Collider> {
        match *self {
            Self::Box { size } => Some(Collider::cuboid(size.x, size.y, size.z)),
            Self::Sphere { radius } => Some(Collider::sphere(radius)),
            Self::Rigged { .. } => None,
        }
    }
}

/// Skeletons rigged shapes can name, each with the rest pose its models are built around.
#[derive(Resource)]
pub struct Rigs(BTreeMap<String, fn() -> Rig>);

impl Default for Rigs {
    fn default() -> Self {
        Self(BTreeMap::from([(
            "humanoid".to_owned(),
            humanoid::rig as fn() -> Rig,
        )]))
    }
}

impl Rigs {
    pub fn register(&mut self, name: impl Into<String>, rig: fn() -> Rig) -> &mut Self {
        self.0.insert(name.into(), rig);
        self
    }

    pub fn build(&self, name: &str) -> Option<Rig> {
        self.0.get(name).map(|rig| rig())
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }
}

type ChangedShape = (Changed<Shape>, With<RigidBody>);

/// Live reload edits a `Shape` in place, which the insert hook does not see.
fn refresh_shape_colliders(
    mut commands: Commands,
    shapes: Query<(Entity, Ref<Shape>), ChangedShape>,
) {
    for (entity, shape) in &shapes {
        if !shape.is_added()
            && let Some(collider) = shape.collider()
        {
            commands.entity(entity).insert(collider);
        }
    }
}

/// Physics drives a body's `Transform` from its `Position`, so a body the editor moved is
/// teleported there.
fn place_reloaded_bodies(
    mut reloads: MessageReader<LiveReloaded>,
    mut bodies: Query<(&Transform, &mut Position, &mut Rotation), With<RigidBody>>,
) {
    for entity in reloads.read().flat_map(|reload| &reload.placed) {
        if let Ok((transform, mut position, mut rotation)) = bodies.get_mut(*entity) {
            position.0 = transform.translation;
            rotation.0 = transform.rotation;
        }
    }
}

fn insert_shape_collider(mut world: DeferredWorld, context: HookContext) {
    // Deferred: definitions insert their components one at a time, so the body may come later.
    world.commands().queue(move |world: &mut World| {
        let Ok(mut entity) = world.get_entity_mut(context.entity) else {
            return;
        };
        if entity.contains::<RigidBody>()
            && let Some(collider) = entity.get::<Shape>().and_then(Shape::collider)
        {
            entity.insert(collider);
        }
    });
}

/// How an authored entity is drawn; headless apps ignore it.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Look {
    /// sRGB.
    pub color: [f32; 3],
    pub opacity: f32,
    pub finish: Finish,
}

impl Default for Look {
    fn default() -> Self {
        Self {
            color: [0.8, 0.8, 0.8],
            opacity: 1.0,
            finish: Finish::Matte,
        }
    }
}

#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Finish {
    #[default]
    Matte,
    /// Matte, with a cut-out along the camera's sight line while it hides the player.
    Ground,
    /// A translucent surface on the top face of the entity's box `Volume`, fading while the
    /// volume hides the player.
    Water,
}

/// Loads the project at `root`, over the engine's base definitions, and spawns its scenes, with
/// every engine package its definitions can name (the `dodge` and `combat` moves included).
/// Needs the physics and character controller plugins.
pub struct ScenePlugin {
    pub root: PathBuf,
}

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            CorePlugin::default(),
            struction_character::DodgePlugin,
            struction_character::CombatPlugin,
            DataPlugin::new(&self.root).library("engine", engine_definitions()),
            WorldPlugin::default(),
            LiveReloadPlugin::default(),
        ))
        .init_resource::<Rigs>()
        .add_systems(Update, refresh_shape_colliders)
        .add_systems(First, place_reloaded_bodies.after(WorldSet::Reload))
        // Avian does not register these for reflection.
        .register_type::<RigidBody>()
        .register_type::<ColliderDensity>()
        .register_type::<Shape>()
        .register_type::<Look>()
        .register_type::<struction_anim::base_pose::PoseTargets>()
        .register_type::<struction_anim::moves::PlaySequence>();
    }
}

/// Every engine package registered headless, as tools load a project: editor preview, isolated
/// play, validation and tests. Not started; `update` it to spawn the scenes.
pub fn authoring_app(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        struction_physics::PhysicsPlugin::default(),
        struction_character::CharacterControllerPlugin,
        ScenePlugin {
            root: root.to_owned(),
        },
    ));
    app
}
