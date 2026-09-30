//! The playground scene is data: definitions and `scenes/milestone1.jsonc` under `project/`,
//! loaded by `struction_data` and spawned by `struction_world`. This module registers the
//! components the data uses and gives the authoring ones effect: a [`Shape`] becomes a collider
//! here, and the rendered host turns [`Look`] and [`Humanoid`] into meshes and a rig. Saved edits
//! to the project apply to the running scene through `struction_world`'s live reload.

use std::path::{Path, PathBuf};

use bevy::{
    ecs::{lifecycle::HookContext, world::DeferredWorld},
    prelude::*,
};
use struction_core::CorePlugin;
use struction_data::DataPlugin;
use struction_physics::avian3d::prelude::*;
use struction_world::{LiveReloadPlugin, LiveReloaded, WorldPlugin, WorldSet};

/// The playground's own project, next to its sources.
pub fn default_project() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("project")
}

/// Geometry of an authored entity in meters: its collider when it has a `RigidBody`, and its mesh
/// in the rendered host.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component)]
#[component(on_insert = insert_shape_collider)]
pub enum Shape {
    Box { size: Vec3 },
    Sphere { radius: f32 },
}

impl Shape {
    pub fn collider(self) -> Collider {
        match self {
            Self::Box { size } => Collider::cuboid(size.x, size.y, size.z),
            Self::Sphere { radius } => Collider::sphere(radius),
        }
    }

    pub fn mesh(self) -> Mesh {
        match self {
            Self::Box { size } => Cuboid::from_size(size).into(),
            Self::Sphere { radius } => Sphere::new(radius).mesh().ico(5).expect("valid sphere"),
        }
    }
}

type ChangedShape = (Changed<Shape>, With<RigidBody>);

/// Live reload edits a `Shape` in place, which the insert hook does not see.
fn refresh_shape_colliders(
    mut commands: Commands,
    shapes: Query<(Entity, Ref<Shape>), ChangedShape>,
) {
    for (entity, shape) in &shapes {
        if !shape.is_added() {
            commands.entity(entity).insert(shape.collider());
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
        if let (Some(&shape), true) = (entity.get::<Shape>(), entity.contains::<RigidBody>()) {
            entity.insert(shape.collider());
        }
    });
}

/// How an authored entity is drawn. Headless apps ignore it.
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

/// The entity the camera follows and the HUD reports on.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct Player;

/// The body is drawn as a procedurally animated humanoid rig.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct Humanoid;

/// Loads the project at `root` and spawns its scenes. Needs the physics, gravity and character
/// plugins for the components the data uses.
pub struct ScenePlugin {
    pub root: PathBuf,
}

impl Plugin for ScenePlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((
            CorePlugin::default(),
            DataPlugin::new(&self.root),
            WorldPlugin::default(),
            LiveReloadPlugin::default(),
        ))
        .add_systems(Update, refresh_shape_colliders)
        .add_systems(First, place_reloaded_bodies.after(WorldSet::Reload))
        // Avian does not register these for reflection.
        .register_type::<RigidBody>()
        .register_type::<ColliderDensity>()
        .register_type::<Shape>()
        .register_type::<Look>()
        .register_type::<Player>()
        .register_type::<Humanoid>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use struction_character::{
        CharacterController, CharacterControllerPlugin, CharacterState, PlayerControlled,
    };
    use struction_gravity::GravityField;
    use struction_physics::{Buoyancy, CameraZone, Submersion, Surface, testing::*};
    use struction_world::{EntityPath, WorldErrors};

    fn scene_app() -> App {
        scene_app_at(default_project())
    }

    fn scene_app_at(root: PathBuf) -> App {
        let mut app = headless_app_with((
            CharacterControllerPlugin,
            // Registers `PlayerControlled` without the input devices of `CharacterPlugins`.
            |app: &mut App| {
                app.register_type::<PlayerControlled>();
            },
            ScenePlugin { root },
        ));
        // Spawners run in the first fixed tick, which the first update does not reach.
        step(&mut app, 2);
        app
    }

    fn entity(app: &mut App, path: &str) -> Entity {
        let mut query = app.world_mut().query::<(Entity, &EntityPath)>();
        query
            .iter(app.world())
            .find(|(_, p)| p.to_string() == path)
            .unwrap_or_else(|| panic!("no entity at {path}"))
            .0
    }

    #[test]
    fn the_project_loads_without_problems() {
        let app = scene_app();
        let errors: Vec<_> = app
            .world()
            .resource::<WorldErrors>()
            .iter()
            .map(ToString::to_string)
            .collect();
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn spawns_keep_the_milestone_one_layout() {
        let mut app = scene_app();
        for (path, translation) in [
            ("Playground/floors/stone", Vec3::new(0.0, -0.25, 5.0)),
            ("Playground/floors/slippery", Vec3::new(0.0, -0.25, -7.0)),
            ("Playground/floors/approach", Vec3::new(0.0, -0.25, -16.5)),
            ("Playground/pool/floor", Vec3::new(10.5, -2.75, -5.0)),
            ("Playground/pool/water", Vec3::new(10.5, -1.0, -5.0)),
            ("Playground/planet/planet", Vec3::new(0.0, 4.0, -20.0)),
            ("Playground/overhead_camera/zone", Vec3::new(0.0, 1.0, -8.0)),
            (
                "Playground/overhead_camera/left_post",
                Vec3::new(-5.8, 1.0, -8.0),
            ),
            (
                "Playground/overhead_camera/right_post",
                Vec3::new(5.8, 1.0, -8.0),
            ),
        ] {
            let entity = entity(&mut app, path);
            let actual = app.world().get::<Transform>(entity).unwrap().translation;
            assert!(actual.abs_diff_eq(translation, 1e-5), "{path}: {actual}");
        }

        let slippery = entity(&mut app, "Playground/floors/slippery");
        let surface = app.world().get::<Surface>(slippery).unwrap();
        assert_eq!(*surface, Surface::slippery());
        // The pool floor removes the surface it would inherit from `Ground`.
        let pool_floor = entity(&mut app, "Playground/pool/floor");
        assert!(app.world().get::<Surface>(pool_floor).is_none());
        let planet = entity(&mut app, "Playground/planet/planet");
        assert_eq!(
            app.world().get::<GravityField>(planet),
            Some(&GravityField::planet(24.0, 5.0))
        );
        let post = entity(&mut app, "Playground/overhead_camera/left_post");
        assert!(app.world().get::<Collider>(post).is_none());
        let zone = entity(&mut app, "Playground/overhead_camera/zone");
        assert!(app.world().get::<CameraZone>(zone).is_some());
        let water = entity(&mut app, "Playground/pool/water");
        assert!(app.world().get::<Buoyancy>(water).is_some());
    }

    #[test]
    fn the_player_lands_on_the_floor_and_the_cube_floats() {
        let mut app = scene_app();
        step(&mut app, 120);
        let player = entity(&mut app, "Playground/start/player");
        let world = app.world();
        assert!(world.get::<CharacterController>(player).is_some());
        assert!(world.get::<CharacterState>(player).unwrap().grounded);
        let position = world.get::<Position>(player).unwrap().0;
        // Dropped from 0.9 m, the capsule rests on its 0.8 m half height.
        assert!(
            position.abs_diff_eq(Vec3::new(0.0, 0.8, 8.0), 0.05),
            "{position}"
        );
        let cube = entity(&mut app, "Playground/pool/floating_cube");
        let submersion = app.world().get::<Submersion>(cube).unwrap().0;
        assert!(submersion > 0.0 && submersion < 1.0, "{submersion}");
    }

    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            if entry.path().is_dir() {
                copy(&entry.path(), &to.join(entry.file_name()));
            } else {
                std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
            }
        }
    }

    fn edit(path: &Path, from: &str, to: &str) {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains(from), "{from} not in {}", path.display());
        std::fs::write(path, text.replace(from, to)).unwrap();
    }

    #[test]
    fn saved_edits_reach_the_running_scene() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        copy(&default_project(), &root);
        let mut app = scene_app_at(root.clone());
        let stone = entity(&mut app, "Playground/floors/stone");
        let planet = entity(&mut app, "Playground/planet/planet");

        edit(
            &root.join("scenes/milestone1.jsonc"),
            r#""offset": [0, 0, 5]"#,
            r#""offset": [0, 0, 6]"#,
        );
        edit(
            &root.join("ground/planet/entity.jsonc"),
            r#""radius": 4 }"#,
            r#""radius": 3 }"#,
        );
        // Past the default scan interval.
        std::thread::sleep(std::time::Duration::from_millis(300));
        step(&mut app, 2);

        let world = app.world();
        let moved = world.get::<Transform>(stone).unwrap().translation;
        assert!(
            moved.abs_diff_eq(Vec3::new(0.0, -0.25, 6.0), 1e-5),
            "{moved}"
        );
        let position = world.get::<Position>(stone).unwrap().0;
        assert!(position.abs_diff_eq(moved, 1e-5), "{position}");
        assert_eq!(
            *world.get::<Shape>(planet).unwrap(),
            Shape::Sphere { radius: 3.0 }
        );
        let radius = world
            .get::<Collider>(planet)
            .unwrap()
            .shape()
            .as_ball()
            .unwrap()
            .radius;
        assert!((radius - 3.0).abs() < 1e-5, "{radius}");
        assert!(world.resource::<WorldErrors>().iter().next().is_none());
    }
}
