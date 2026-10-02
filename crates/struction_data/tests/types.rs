//! Value shapes the reflect builder understands: enums, options, vectors, maps, tuples.

mod common;

use std::collections::HashMap;

use bevy::prelude::*;
use common::*;
use struction_data::DefinitionStore;

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Kitchen {
    tags: HashMap<String, u8>,
    pair: (i32, bool),
    fixed: [f32; 2],
    pos: Vec3,
    rot: Quat,
    letter: char,
    maybe: Option<Vec3>,
    nested: Option<Faction>,
    entries: Vec<Stats>,
    objects: HashMap<String, Stats>,
    defaults: Vec<Health>,
}

#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component)]
enum Sign {
    Blank,
    Labeled { text: String, color: Option<Vec3> },
}

fn load(components: &str) -> (Result<(), Vec<String>>, DefinitionStore) {
    let mut registry = registry();
    registry.register::<Kitchen>();
    registry.register::<Sign>();
    let body = format!("{{ \"descendsFrom\": \"Actor\", \"components\": {{ {components} }} }}");
    let dir = write_project(&[("a/entity.jsonc", &body)]);
    let mut store = DefinitionStore::new(dir.path());
    store.declare_primordial("Actor");
    store.load(&registry);
    let errors = store
        .errors()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    (
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        },
        store,
    )
}

#[test]
fn enums_in_all_three_variant_shapes() {
    let (result, store) =
        load(r#""Shape": { "Sphere": { "radius": 2.5 } }, "Damage": { "kind": "Fire" }"#);
    result.unwrap();
    let a = store.get("a").unwrap();
    assert_eq!(a.component::<Shape>(), Some(&Shape::Sphere { radius: 2.5 }));
    assert_eq!(a.component::<Damage>().unwrap().kind, DamageKind::Fire);

    let (result, store) = load(r#""Shape": { "Box": [1, 2, 3] }"#);
    result.unwrap();
    assert_eq!(
        store.get("a").unwrap().component::<Shape>(),
        Some(&Shape::Box(Vec3::new(1.0, 2.0, 3.0)))
    );

    let (result, store) = load(r#""Shape": "Point""#);
    result.unwrap();
    assert_eq!(
        store.get("a").unwrap().component::<Shape>(),
        Some(&Shape::Point)
    );
}

#[test]
fn optional_variant_fields_may_be_left_out() {
    let (result, store) = load(r#""Sign": { "Labeled": { "text": "exit" } }"#);
    result.unwrap();
    assert_eq!(
        store.get("a").unwrap().component::<Sign>(),
        Some(&Sign::Labeled {
            text: "exit".into(),
            color: None
        })
    );
    let (result, _) = load(r#""Sign": { "Labeled": { "color": [1, 0, 0] } }"#);
    assert_eq!(
        result.unwrap_err(),
        ["a/entity.jsonc:1: missing field text in Sign::Labeled"]
    );
    assert!(load(r#""Sign": "Blank""#).0.is_ok());
}

#[test]
fn enum_errors() {
    let (result, _) = load(r#""Shape": { "Sphere": { "diameter": 2 } }"#);
    assert_eq!(
        result.unwrap_err(),
        ["a/entity.jsonc:1: unknown field \"diameter\" in Shape::Sphere"]
    );
    let (result, _) = load(r#""Shape": { "Sphere": {} }"#);
    assert_eq!(
        result.unwrap_err(),
        ["a/entity.jsonc:1: missing field radius in Shape::Sphere"]
    );
    let (result, _) = load(r#""Shape": "Sphere""#);
    assert!(result.unwrap_err()[0].contains("carries data"));
}

#[test]
fn a_bit_of_everything() {
    let (result, store) = load(
        r#""Kitchen": {
            "tags": { "a": 1, "b": 2 },
            "pair": [-4, true],
            "fixed": [0.5, 1.5],
            "pos": [1, 2, 3],
            "rot": [0, 0, 0, 1],
            "letter": "x",
            "maybe": { "x": 9, "y": 8, "z": 7 },
            "nested": "orc"
        }"#,
    );
    result.unwrap();
    let k = store.get("a").unwrap().component::<Kitchen>().unwrap();
    assert_eq!(k.tags.get("b"), Some(&2));
    assert_eq!(k.pair, (-4, true));
    assert_eq!(k.fixed, [0.5, 1.5]);
    assert_eq!(k.pos, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(k.rot, Quat::IDENTITY);
    assert_eq!(k.letter, 'x');
    assert_eq!(k.maybe, Some(Vec3::new(9.0, 8.0, 7.0)));
    assert_eq!(k.nested, Some(Faction("orc".into())));
}

#[test]
fn partial_components_take_missing_fields_from_default() {
    let (result, store) = load(r#""Kitchen": { "letter": "z", "maybe": null }"#);
    result.unwrap();
    let k = store.get("a").unwrap().component::<Kitchen>().unwrap();
    assert_eq!(k.letter, 'z');
    assert_eq!(k.maybe, None);
    assert_eq!(k.pos, Vec3::ZERO);
}

#[test]
fn shape_errors_inside_containers() {
    let (result, _) = load(r#""Kitchen": { "fixed": [1.0] }"#);
    assert_eq!(
        result.unwrap_err(),
        ["a/entity.jsonc:1: invalid value for [f32; 2]: expected 2 elements, found 1"]
    );
    let (result, _) = load(r#""Kitchen": { "pos": [1, 2] }"#);
    assert!(result.unwrap_err()[0].contains("invalid value for Vec3"));
    let (result, _) = load(r#""Kitchen": { "letter": "xy" }"#);
    assert!(result.unwrap_err()[0].contains("expected single-character string"));
    let (result, _) = load(r#""Kitchen": { "tags": { "a": 300 } }"#);
    assert!(result.unwrap_err()[0].contains("invalid value for u8: 300 is out of range"));
}

#[test]
fn incomplete_collection_entries_report_source_errors_instead_of_panicking() {
    for field in [
        r#""entries": [{"strength": 4}]"#,
        r#""objects": {"first": {"strength": 4}}"#,
    ] {
        let (result, _) = load(&format!(r#""Kitchen": {{{field}}}"#));
        assert_eq!(
            result.unwrap_err(),
            ["a/entity.jsonc:1: missing field agility in Stats"]
        );
    }
    let (result, store) = load(r#""Kitchen": {"defaults": [{"current": 7}]}"#);
    result.unwrap();
    assert_eq!(
        store
            .get("a")
            .unwrap()
            .component::<Kitchen>()
            .unwrap()
            .defaults,
        [Health {
            current: 7.0,
            max: 0.0
        }]
    );
}
