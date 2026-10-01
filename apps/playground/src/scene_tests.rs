use std::path::{Path, PathBuf};

use bevy::prelude::*;
use struction_character::{CharacterController, CharacterControllerPlugin, CharacterState};
use struction_gravity::GravityField;
use struction_physics::{
    Buoyancy, CameraZone, Submersion, Surface, avian3d::prelude::*, testing::*,
};
use struction_scene::{ScenePlugin, Shape};
use struction_world::{EntityPath, WorldErrors};

use crate::default_project;

fn scene_app() -> App {
    scene_app_at(default_project())
}

fn scene_app_at(root: PathBuf) -> App {
    let mut app = headless_app_with((CharacterControllerPlugin, ScenePlugin { root }));
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
fn renamed_definitions_keep_save_aliases() {
    let aliases = struction_world::PathAliases::load(&default_project()).unwrap();
    assert_eq!(aliases.resolve("ground/stone"), "terrain/stone");
    assert_eq!(aliases.resolve("gravity/scene"), "fields/scene");
    assert_eq!(aliases.resolve("Ground"), "Ground");
    assert_eq!(aliases.resolve("Gravity"), "Gravity");
    assert_eq!(aliases.resolve("Player"), "characters/player");
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

#[test]
fn authored_player_can_roll_through_the_registered_action() {
    use struction_character::{Roll, Rolling};
    use struction_core::{ActionArgs, ActionInvocation, ActionQueue};

    let mut app = scene_app();
    step(&mut app, 60);
    let player = entity(&mut app, "Playground/start/player");
    // Supplied by the `dodge` extensor the player names.
    assert_eq!(app.world().get::<Roll>(player), Some(&Roll::default()));
    let start = app.world().get::<Position>(player).unwrap().0;
    app.world_mut()
        .resource_mut::<ActionQueue>()
        .invoke(ActionInvocation::new(
            "dodge/roll",
            player,
            ActionArgs::new(),
        ));
    step(&mut app, 20);
    assert!(app.world().get::<Rolling>(player).is_some());
    let at = app.world().get::<Position>(player).unwrap().0;
    assert!(start.z - at.z > 2.0, "{start} -> {at}");
}

#[test]
fn actors_share_the_humanoid_shape_and_opt_into_moves() {
    use struction_character::{Attack, Roll};
    use struction_data::{DefinitionStore, ExtensorReason};

    let mut app = scene_app();
    let model = Shape::Humanoid {
        model: Some("models/blood_knight.blend".into()),
    };
    let player = entity(&mut app, "Playground/start/player");
    let sentry = entity(&mut app, "Playground/guard/sentry");
    let world = app.world();
    assert_eq!(world.get::<Shape>(player), Some(&model));
    assert_eq!(world.get::<Shape>(sentry), Some(&model));
    assert!(world.get::<CharacterController>(sentry).is_some());
    // The sentry names no extensors, so it can neither roll nor attack.
    assert!(world.get::<Roll>(sentry).is_none());
    assert!(world.get::<Attack>(sentry).is_none());
    assert!(world.get::<Attack>(player).is_some());

    let store = world.resource::<DefinitionStore>();
    let explain = |id: &str| -> Vec<(String, ExtensorReason)> {
        store
            .get(id)
            .unwrap()
            .extensors
            .iter()
            .map(|e| (e.name.clone(), e.reason.clone()))
            .collect()
    };
    let named = ExtensorReason::Named {
        by: "characters/player".into(),
    };
    assert_eq!(
        explain("characters/player"),
        [
            ("dodge".into(), named.clone()),
            ("combat".into(), named),
            (
                "character".into(),
                ExtensorReason::Owns("CharacterController".into())
            ),
            (
                "physics".into(),
                ExtensorReason::RequiredBy("character".into())
            ),
        ]
    );
    assert_eq!(
        explain("characters/sentry"),
        [
            (
                "character".into(),
                ExtensorReason::Owns("CharacterController".into())
            ),
            (
                "physics".into(),
                ExtensorReason::RequiredBy("character".into())
            ),
        ]
    );
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
        &root.join("terrain/planet/entity.jsonc"),
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

/// Every piece of the blood knight rides on a humanoid joint and is modeled around that
/// joint's rest position, so parenting it to the joint without an offset puts it in place.
#[test]
fn blood_knight_pieces_sit_on_humanoid_joints() {
    use struction_anim::humanoid;
    use struction_assets::{Blender, import_source};

    let blender = Blender::default();
    if !blender.is_available() {
        eprintln!("skipping: Blender not found (install it or set STRUCTION_BLENDER)");
        return;
    }
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/models/blood_knight.blend");
    let scene = import_source(&source, &blender, false).unwrap();
    let rig = humanoid::rig();
    let defs = rig.skeleton.joints();
    let mut rest = Vec::<Vec3>::new();
    for def in defs {
        let parent = def.parent.map_or(Vec3::ZERO, |parent| rest[parent]);
        rest.push(parent + def.rest.translation);
    }

    assert!(scene.nodes.len() > 40, "{} pieces", scene.nodes.len());
    for node in &scene.nodes {
        let joint = node.name.split('.').next().unwrap();
        let index = defs
            .iter()
            .position(|def| def.name == joint)
            .unwrap_or_else(|| panic!("{} names no joint", node.name));
        assert!(node.parent.is_none(), "{} is nested", node.name);
        let at = Vec3::from(node.translation);
        assert!(
            at.distance(rest[index]) < 1e-4,
            "{} sits at {at}, its joint rests at {}",
            node.name,
            rest[index]
        );
    }
    for piece in ["chest.mark_drop", "hand_r.blade", "head.helm"] {
        assert!(scene.nodes.iter().any(|node| node.name == piece), "{piece}");
    }
}
