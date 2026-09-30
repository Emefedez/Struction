#![allow(dead_code)]

use std::path::{Path, PathBuf};

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;
use struction_core::*;
use struction_data::DataPlugin;
use struction_world::*;

#[derive(Component, Reflect, Default, Debug, PartialEq, Clone)]
#[reflect(Component, Default, Persist)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

/// What `die` leaves behind: death is ordinary persistent state.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default, Persist)]
pub struct Dead;

/// Granted by the ogre lord. Persistent, but grants are derived and never saved.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default, Persist)]
pub struct Emboldened {
    pub bonus: f32,
}

/// Granted by the player.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Follower {
    pub distance: f32,
}

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Flammable;

/// Runtime-only state: not persistent, so saves drop it.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Scratch(pub u32);

#[derive(Resource, Default)]
pub struct Log(pub Vec<String>);

pub const LOGGED_ACTIONS: [&str; 3] = ["fetch", "guard", "rally"];

pub fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fortress")
}

/// A copy of the fixture project that tests may edit.
pub fn fixture_copy() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture(), dir.path());
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

pub struct Options {
    pub seed: u64,
    pub cell_size: f32,
    /// Prefix of the action paths the definitions reference (renames change it).
    pub ogre: &'static str,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            seed: 7,
            cell_size: 50.0,
            ogre: "minions/ogre",
        }
    }
}

/// A headless app on the project at `root`, built through `Startup` but with no tick run yet.
pub fn app_with(root: &Path, options: Options) -> App {
    let mut app = App::new();
    app.add_plugins((
        CorePlugin { seed: options.seed },
        DataPlugin::new(root).primordial("Actor"),
        WorldPlugin {
            cell_size: options.cell_size,
        },
    ))
    .init_resource::<Log>()
    .register_type::<Health>()
    .register_type::<Dead>()
    .register_type::<Emboldened>()
    .register_type::<Follower>()
    .register_type::<Flammable>()
    .register_type::<Scratch>();
    for action in LOGGED_ACTIONS {
        app.register_action(
            ActionMeta::new(action),
            move |In(call): In<ActionCall>, mut log: ResMut<Log>, names: Query<&Name>| {
                let who = names.get(call.target).map_or("?", |n| n.as_str());
                log.0.push(format!("{action}@{who}"));
            },
        );
    }
    for die in [
        "bosses/ogre_lord/die".to_owned(),
        format!("{}/die", options.ogre),
    ] {
        app.register_action(
            ActionMeta::new(die),
            |In(call): In<ActionCall>, mut commands: Commands| {
                commands.entity(call.target).insert(Dead);
            },
        );
    }
    app.finish();
    app.cleanup();
    app.world_mut().run_schedule(Startup);
    app
}

pub fn app(root: &Path) -> App {
    app_with(root, Options::default())
}

/// Runs one fixed tick directly, independent of wall-clock time.
pub fn step(app: &mut App) {
    app.world_mut().run_schedule(FixedUpdate);
}

/// The fixture app after its first tick: every spawner has run.
pub fn started() -> App {
    let mut app = app(&fixture());
    step(&mut app);
    assert_no_errors(&mut app);
    app
}

pub fn assert_no_errors(app: &mut App) {
    let errors: Vec<String> = app
        .world_mut()
        .resource_mut::<WorldErrors>()
        .drain()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(errors, Vec::<String>::new());
    let actions = app.world_mut().resource_mut::<ActionErrors>().drain();
    assert_eq!(actions, vec![]);
}

/// Finds an entity by path, loaded or not.
pub fn by_path(app: &App, path: &str) -> Entity {
    find(app, path).unwrap_or_else(|| panic!("no entity at {path}"))
}

pub fn find(app: &App, path: &str) -> Option<Entity> {
    app.world()
        .try_query_filtered::<(Entity, &EntityPath), Allow<Disabled>>()?
        .iter(app.world())
        .find_map(|(entity, candidate)| (candidate.as_str() == path).then_some(entity))
}

pub fn has<C: Component>(app: &App, entity: Entity) -> bool {
    app.world().get::<C>(entity).is_some()
}

pub fn get<C: Component + Clone>(app: &App, entity: Entity) -> C {
    app.world().get::<C>(entity).unwrap().clone()
}

pub fn id_of(app: &App, entity: Entity) -> StableId {
    *app.world().get::<StableId>(entity).unwrap()
}

pub fn master_of(app: &App, entity: Entity) -> Option<Entity> {
    app.world().get::<MasterIs>(entity).map(|m| m.0)
}

/// Paths of every instance, loaded or not, sorted.
pub fn instance_paths(app: &mut App) -> Vec<String> {
    let mut query = app
        .world_mut()
        .query_filtered::<&EntityPath, (With<Spawned>, Allow<Disabled>)>();
    let mut paths: Vec<String> = query.iter(app.world()).map(ToString::to_string).collect();
    paths.sort();
    paths
}

pub const BOSS: &str = "Fortress/Keep/ogre_lord";
pub const FIREMAN1: &str = "Fortress/LeftCourtYard/courtyard_guards/fireman1";
pub const FIREMAN2: &str = "Fortress/LeftCourtYard/courtyard_guards/fireman2";
pub const BRUTE: &str = "Fortress/LeftCourtYard/courtyard_guards/brute";
pub const PLAYER: &str = "Fortress/start/player";
pub const GUARDS: &str = "Fortress/LeftCourtYard/courtyard_guards";
