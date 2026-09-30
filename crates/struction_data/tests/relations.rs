//! `reactions` and `grantsToWards` become core components, with action names resolved against the
//! registry and errors at `file:line`.

mod common;

use std::path::Path;

use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
use common::*;
use struction_core::*;
use struction_data::{DefinitionStore, Resolved};

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Follower {
    distance: f32,
}

#[derive(Resource, Default)]
struct Log(Vec<String>);

const ACTIONS: [&str; 5] = [
    "bosses/ogre_lord/die",
    "minions/ogre/die",
    "fetch",
    "guard",
    "rally",
];

fn types() -> TypeRegistry {
    let mut types = registry();
    types.register::<Follower>();
    types
}

/// An app whose type registry knows the test components and whose actions log `action@path`.
fn app() -> App {
    let mut app = App::new();
    app.add_plugins(CorePlugin::default())
        .init_resource::<Log>();
    app.register_type::<Health>()
        .register_type::<Follower>()
        .register_type::<Transform>();
    for action in ACTIONS {
        app.register_action(
            ActionMeta::new(action).param_or("loud", ParamType::Bool, false),
            move |In(call): In<ActionCall>, mut log: ResMut<Log>, defs: Query<&Definition>| {
                let who = defs
                    .get(call.target)
                    .map_or("?".into(), |d| d.path.to_string());
                log.0.push(format!("{action}@{who}"));
            },
        );
    }
    app
}

fn store(root: &Path) -> DefinitionStore {
    DefinitionStore::open(root, &types())
}

fn encounter() -> DefinitionStore {
    store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/encounter"))
}

/// Spawns an instance the way a runtime does: components and `Definition`, then the sections
/// that need actions.
fn spawn(app: &mut App, resolved: &Resolved) -> Entity {
    let types = app.world().resource::<AppTypeRegistry>().clone();
    let types = types.read();
    let (grants, reactions) = {
        let actions = app.world().resource::<ActionRegistry>();
        (
            resolved.grants_to_wards(&types, actions).unwrap(),
            resolved.reactions(actions).unwrap(),
        )
    };
    let mut entity = app.world_mut().spawn_empty();
    resolved.insert_into(&mut entity, &types);
    if let Some(grants) = grants {
        entity.insert(grants);
    }
    if let Some(reactions) = reactions {
        entity.insert(reactions);
    }
    entity.id()
}

#[test]
fn readme_examples_convert_and_check_clean() {
    let store = encounter();
    assert_eq!(store.errors(), vec![]);
    let app = app();
    let actions = app.world().resource::<ActionRegistry>();
    assert_eq!(store.check_references(&types(), actions), vec![]);

    let grants = store
        .get("player")
        .unwrap()
        .grants_to_wards(&types(), actions)
        .unwrap()
        .unwrap();
    let [rule] = grants.0.as_slice() else {
        panic!("one grant rule")
    };
    assert_eq!(rule.to.as_str(), "minions/ogre");
    assert_eq!(rule.actions, [ActionName::new("fetch"), "guard".into()]);
    let follower = rule.components[0].try_downcast_ref::<Follower>().unwrap();
    assert_eq!(follower.distance, 3.0);

    // Inherited from minions/ogre.
    let reactions = store
        .get("minions/small_ogre")
        .unwrap()
        .reactions(actions)
        .unwrap()
        .unwrap();
    assert_eq!(
        reactions.0,
        [Reaction::after(
            ReactionSource::Master,
            "bosses/ogre_lord/die",
            "minions/ogre/die"
        )]
    );
    assert!(
        store
            .get("player")
            .unwrap()
            .reactions(actions)
            .unwrap()
            .is_none()
    );
}

#[test]
fn spawned_instances_have_one_lineage_component_and_working_relations() {
    let store = encounter();
    let mut app = app();
    let player = spawn(&mut app, store.get("player").unwrap());
    let boss = spawn(&mut app, store.get("bosses/ogre_lord").unwrap());
    let follower = spawn(&mut app, store.get("minions/small_ogre").unwrap());
    let minion = spawn(&mut app, store.get("minions/ogre").unwrap());
    app.world_mut()
        .entity_mut(follower)
        .insert(MasterIs(player));
    app.world_mut().entity_mut(minion).insert(MasterIs(boss));
    app.world_mut().flush();

    let world = app.world();
    assert!(
        world
            .get::<Definition>(follower)
            .unwrap()
            .descends_from(&"minions/ogre".into())
    );
    // Lineage filtering: small_ogre gets what the player grants to minions/ogre.
    assert_eq!(world.get::<Follower>(follower).unwrap().distance, 3.0);
    let granted: Vec<_> = world.get::<ActionSet>(follower).unwrap().iter().collect();
    assert_eq!(granted, [&ActionName::new("fetch"), &"guard".into()]);
    assert!(world.get::<Follower>(minion).is_none());
    assert!(
        world
            .get::<ActionSet>(minion)
            .unwrap()
            .contains(&"rally".into())
    );

    invoke_action(
        app.world_mut(),
        "bosses/ogre_lord/die",
        boss,
        ActionArgs::new(),
    );
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(
        app.world().resource::<Log>().0,
        [
            "bosses/ogre_lord/die@bosses/ogre_lord",
            "minions/ogre/die@minions/ogre"
        ]
    );
    assert!(app.world().resource::<ActionErrors>().is_empty());
}

fn reference_errors(files: &[(&str, &str)]) -> Vec<String> {
    let dir = write_project(files);
    let store = store(dir.path());
    assert_eq!(store.errors(), vec![]);
    let app = app();
    store
        .check_references(&types(), app.world().resource::<ActionRegistry>())
        .iter()
        .map(ToString::to_string)
        .collect()
}

const ACTOR: (&str, &str) = ("Actor/entity.jsonc", "{}");

#[test]
fn bad_reactions_point_at_the_value() {
    let errors = reference_errors(&[
        ACTOR,
        (
            "ogre/entity.jsonc",
            r#"{
  "descendsFrom": "Actor",
  "reactions": [
    { "source": "master", "after": "bosses/ogre_lord/dies", "call": "minions/ogre/die" },
    { "after": "fetch",
      "call": "minions/ogre/diee" },
    { "after": "fetch", "call": "guard", "args": { "loud": "yes" } },
    { "source": "boss", "after": "fetch", "call": "guard" },
    { "after": "fetch", "before": "guard", "call": "guard" },
    { "call": "guard" }
  ]
}"#,
        ),
    ]);
    assert_eq!(
        errors,
        [
            "ogre/entity.jsonc:10: missing field after (or before) in reaction",
            "ogre/entity.jsonc:4: unknown action `bosses/ogre_lord/dies`",
            "ogre/entity.jsonc:6: unknown action `minions/ogre/diee`",
            "ogre/entity.jsonc:7: parameter `loud` of action `guard` expects Bool, got Str",
            "ogre/entity.jsonc:8: unknown variant \"boss\" in reaction source (this, master, wards)",
            "ogre/entity.jsonc:9: invalid value for reaction: give only one of \"after\" and \"before\"",
        ]
    );
}

#[test]
fn bad_grants_point_at_the_value() {
    let errors = reference_errors(&[
        ACTOR,
        (
            "player/entity.jsonc",
            r#"{
  "descendsFrom": "Actor",
  "grantsToWards": [
    { "to": "minions/orge", "actions": ["fetch"] },
    { "to": "Actor",
      "components": { "Folower": {} },
      "actions": ["fetch", "juggle"] },
    { "actions": [] }
  ]
}"#,
        ),
    ]);
    assert_eq!(
        errors,
        [
            "player/entity.jsonc:4: \"to\" refers to missing definition \"minions/orge\"",
            "player/entity.jsonc:6: unknown component \"Folower\"",
            "player/entity.jsonc:7: unknown action `juggle`",
            "player/entity.jsonc:8: missing field to in grant",
        ]
    );
}
