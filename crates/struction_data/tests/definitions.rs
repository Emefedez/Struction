mod common;

use bevy::prelude::*;
use common::*;
use struction_data::{DefinitionStore, ErrorKind, parse_jsonc};

fn open_fixture() -> (DefinitionStore, bevy::reflect::TypeRegistry) {
    let registry = registry();
    let mut store = DefinitionStore::new(fixture_project());
    store.declare_primordial("Terrain");
    store.load(&registry);
    (store, registry)
}

#[test]
fn fixture_project_loads_without_errors() {
    let (store, _) = open_fixture();
    assert_eq!(store.errors(), vec![]);
    let ids: Vec<_> = store.definitions().collect();
    assert_eq!(
        ids,
        [
            "Actor",
            "Terrain",
            "minions/ogre",
            "minions/ogre_lord",
            "minions/small_ogre",
            "world/lava_pool"
        ]
    );
}

#[test]
fn lineage_is_nearest_ancestor_first() {
    let (store, _) = open_fixture();
    assert_eq!(
        store.get("minions/small_ogre").unwrap().lineage,
        ["minions/ogre", "Actor"]
    );
    assert_eq!(store.get("minions/ogre").unwrap().lineage, ["Actor"]);
    assert!(store.get("Actor").unwrap().lineage.is_empty());
    let mut descendants = store.descendants("minions/ogre");
    descendants.sort();
    assert_eq!(descendants, ["minions/ogre_lord", "minions/small_ogre"]);
}

#[test]
fn defaults_are_inherited_with_field_level_merge() {
    let (store, _) = open_fixture();

    // Actor -> ogre replaces both fields, small_ogre overrides only max.
    let small = store.get("minions/small_ogre").unwrap();
    assert_eq!(
        small.component::<Health>(),
        Some(&Health {
            current: 60.0,
            max: 25.0
        })
    );
    // Untouched Actor default flows through two levels; overridden at the last one.
    assert_eq!(small.component::<Faction>(), Some(&Faction("wild".into())));
    assert_eq!(
        store.get("minions/ogre").unwrap().component::<Faction>(),
        Some(&Faction("neutral".into()))
    );
    // Null on an Option field is a value, not a removal.
    assert_eq!(
        small.component::<Loot>(),
        Some(&Loot {
            items: vec!["club".into()],
            gold: None
        })
    );
    // Stats has no Default: strength comes from the ogre, agility from the small_ogre.
    assert_eq!(
        small.component::<Stats>(),
        Some(&Stats {
            strength: 9,
            agility: 6
        })
    );
}

#[test]
fn transform_section_is_the_transform_component() {
    let (store, _) = open_fixture();
    let ogre = store.get("minions/ogre").unwrap();
    assert_eq!(
        ogre.component::<Transform>().unwrap().scale,
        Vec3::splat(1.5)
    );
    assert_eq!(
        store
            .get("minions/small_ogre")
            .unwrap()
            .component::<Transform>()
            .unwrap()
            .scale,
        Vec3::splat(0.75)
    );
}

#[test]
fn presets_sit_between_inherited_and_own_data() {
    let (store, _) = open_fixture();
    // The ogre applies `flammable`; children inherit the result.
    assert_eq!(
        store
            .get("minions/small_ogre")
            .unwrap()
            .component::<Flammable>(),
        Some(&Flammable {
            ignition_temperature: 300.0
        })
    );
    // A preset can build on another preset: lava = water + damage, with its own drag.
    let pool = store.get("world/lava_pool").unwrap();
    assert_eq!(
        pool.component::<Surface>(),
        Some(&Surface {
            friction: 0.1,
            drag: 8.0
        })
    );
    assert_eq!(
        pool.component::<Damage>(),
        Some(&Damage {
            per_second: 25.0,
            kind: DamageKind::Fire
        })
    );
    assert_eq!(
        pool.component::<Volume>(),
        Some(&Volume {
            half_extents: Vec3::new(4.0, 1.0, 4.0)
        })
    );
}

#[test]
fn null_removes_an_inherited_component() {
    let (store, _) = open_fixture();
    let lord = store.get("minions/ogre_lord").unwrap();
    assert!(lord.component::<Flammable>().is_none());
    assert_eq!(lord.component::<Health>().unwrap().max, 500.0);
    // The sibling keeps it.
    assert!(
        store
            .get("minions/small_ogre")
            .unwrap()
            .component::<Flammable>()
            .is_some()
    );
}

#[test]
fn other_sections_are_kept_raw_and_inherited() {
    let (store, _) = open_fixture();
    let small = store.get("minions/small_ogre").unwrap();
    assert_eq!(
        small.section("brain").unwrap().to_value(),
        serde_json::json!("ai/simple_ogre")
    );
    let reaction = &small.section("reactions").unwrap().as_array().unwrap()[0];
    assert_eq!(
        reaction.get("call").unwrap().as_str(),
        Some("minions/ogre/die")
    );
    // The span still points at the ogre's file, where the value was written.
    assert_eq!(&*reaction.span.file, "minions/ogre/entity.jsonc");
    assert_eq!(reaction.span.start.line, 13);
}

#[test]
fn spawn_overrides_apply_over_the_definition() {
    let (store, registry) = open_fixture();
    let scene = parse_jsonc(
        "scenes/courtyard.jsonc",
        &std::fs::read_to_string(fixture_project().join("scenes/courtyard.jsonc")).unwrap(),
    )
    .unwrap();
    let spawn = scene.get("spawns").unwrap().get("fireman1").unwrap();
    let definition = spawn.get("definition").unwrap().as_str().unwrap();
    let instance = store
        .instantiate(definition, spawn.get("overrides"), &registry)
        .unwrap();

    assert_eq!(
        instance.component::<Health>(),
        Some(&Health {
            current: 5.0,
            max: 25.0
        })
    );
    let transform = instance.component::<Transform>().unwrap();
    assert_eq!(transform.translation, Vec3::new(4.0, 0.0, 6.0));
    assert_eq!(transform.scale, Vec3::splat(0.75));
    assert_eq!(instance.lineage, ["minions/ogre", "Actor"]);
    // The definition itself is untouched.
    assert_eq!(
        store
            .get("minions/small_ogre")
            .unwrap()
            .component::<Health>()
            .unwrap()
            .current,
        60.0
    );
}

#[test]
fn overrides_can_apply_presets_and_remove_components() {
    let (store, registry) = open_fixture();
    let over = parse_jsonc(
        "scene.jsonc",
        r#"{ "presets": ["lava"], "components": { "Flammable": null, "Faction": "hostile" } }"#,
    )
    .unwrap();
    let instance = store
        .instantiate("minions/small_ogre", Some(&over), &registry)
        .unwrap();
    assert!(instance.component::<Flammable>().is_none());
    assert!(instance.component::<Damage>().is_some());
    assert_eq!(
        instance.component::<Faction>(),
        Some(&Faction("hostile".into()))
    );
}

#[test]
fn override_errors_point_into_the_scene_file() {
    let (store, registry) = open_fixture();
    let over = parse_jsonc(
        "scenes/courtyard.jsonc",
        "{\n  \"components\": {\n    \"Health\": { \"hp\": 1 }\n  }\n}",
    )
    .unwrap();
    let errors = store
        .instantiate("minions/ogre", Some(&over), &registry)
        .unwrap_err();
    assert_eq!(
        errors[0].to_string(),
        "scenes/courtyard.jsonc:3: unknown field \"hp\" in Health"
    );
    let over = parse_jsonc("s.jsonc", r#"{ "descendsFrom": "Actor" }"#).unwrap();
    let errors = store
        .instantiate("minions/ogre", Some(&over), &registry)
        .unwrap_err();
    assert_eq!(
        errors[0].kind,
        ErrorKind::UnknownSection("descendsFrom".into(), "override")
    );
}

#[test]
fn instances_get_components_and_lineage() {
    let (store, registry) = open_fixture();
    let mut world = World::new();
    let mut entity = world.spawn_empty();
    store
        .get("minions/small_ogre")
        .unwrap()
        .insert_into(&mut entity, &registry);
    let entity = entity.id();

    assert_eq!(world.get::<Health>(entity).unwrap().max, 25.0);
    assert_eq!(
        world.get::<Flammable>(entity).unwrap().ignition_temperature,
        300.0
    );
    let definition = world.get::<struction_core::Definition>(entity).unwrap();
    assert_eq!(definition.path.as_str(), "minions/small_ogre");
    assert_eq!(definition.validate(), Ok(()));
    for ancestor in ["minions/small_ogre", "minions/ogre", "Actor"] {
        assert!(definition.descends_from(&ancestor.into()), "{ancestor}");
    }
    assert!(!definition.descends_from(&"minions/ogre_lord".into()));
}
