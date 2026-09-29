mod common;

use common::*;
use struction_data::{DefinitionStore, ErrorKind};

/// Loads a throwaway project and returns every error as displayed, plus the store.
fn load(files: &[(&str, &str)]) -> (Vec<String>, DefinitionStore, tempfile::TempDir) {
    let dir = write_project(files);
    let mut store = DefinitionStore::new(dir.path());
    store.declare_primordial("Actor");
    store.load(&registry());
    let errors = store.errors().iter().map(ToString::to_string).collect();
    (errors, store, dir)
}

const ACTOR: (&str, &str) = ("Actor/entity.jsonc", "{}");

#[test]
fn unknown_field_points_at_the_key() {
    let (errors, store, _dir) = load(&[(
        "ogre/entity.jsonc",
        "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {\n    \"Health\": { \"current\": 1.0, \"hp\": 3 }\n  }\n}\n",
    )]);
    assert_eq!(
        errors,
        ["ogre/entity.jsonc:4: unknown field \"hp\" in Health"]
    );
    assert!(store.get("ogre").is_none());
    let error = &store.errors()[0];
    assert_eq!(error.location.as_ref().unwrap().column, 33);
}

#[test]
fn unknown_component() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "ogre/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n\n  \"components\": {\n    \"Helth\": {}\n  }\n}",
        ),
    ]);
    assert_eq!(errors, ["ogre/entity.jsonc:5: unknown component \"Helth\""]);
}

#[test]
fn registered_non_component_is_reported_as_such() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{ \"descendsFrom\": \"Actor\", \"components\": { \"Plain\": {} } }",
        ),
    ]);
    assert_eq!(
        errors,
        ["a/entity.jsonc:1: \"Plain\" is a registered type but not a component"]
    );
}

#[test]
fn type_mismatch_reports_expected_and_found() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {\n    \"Health\": {\n      \"current\": \"lots\",\n      \"max\": 10\n    }\n  }\n}",
        ),
    ]);
    assert_eq!(errors, ["a/entity.jsonc:5: expected f32, found string"]);
}

#[test]
fn integer_range_and_shape_errors() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {\n    \"Stats\": { \"strength\": -1, \"agility\": 1.5 },\n    \"Loot\": { \"items\": \"club\" },\n    \"Damage\": { \"kind\": \"Frost\" },\n    \"Health\": [1, 2]\n  }\n}",
        ),
    ]);
    assert_eq!(
        errors,
        [
            "a/entity.jsonc:4: invalid value for u32: -1 is out of range",
            "a/entity.jsonc:5: expected array for Vec<String>, found string",
            "a/entity.jsonc:6: unknown variant \"Frost\" in DamageKind",
            "a/entity.jsonc:7: expected object for Health, found array",
        ]
    );
}

#[test]
fn missing_fields_of_a_component_without_default() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {\n    \"Stats\": { \"strength\": 3 }\n  }\n}",
        ),
    ]);
    assert_eq!(errors, ["a/entity.jsonc:4: missing field agility in Stats"]);
}

#[test]
fn missing_parent_points_at_descends_from() {
    let (errors, ..) = load(&[(
        "small_ogre/entity.jsonc",
        "{\n  \"descendsFrom\": \"ogre\"\n}",
    )]);
    assert_eq!(
        errors,
        ["small_ogre/entity.jsonc:2: descendsFrom refers to missing definition \"ogre\""]
    );
}

#[test]
fn missing_preset_points_at_the_name() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"presets\": [\"flammable\", \"soggy\"]\n}",
        ),
        ("presets/flammable.jsonc", "{}"),
    ]);
    assert_eq!(errors, ["a/entity.jsonc:3: unknown preset \"soggy\""]);
}

#[test]
fn descends_from_cycle_is_detected() {
    let (errors, store, _dir) = load(&[
        ("a/entity.jsonc", "{ \"descendsFrom\": \"b\" }"),
        ("b/entity.jsonc", "{\n  \"descendsFrom\": \"c\"\n}"),
        ("c/entity.jsonc", "{\n\n  \"descendsFrom\": \"a\"\n}"),
    ]);
    assert!(store.get("a").is_none());
    assert_eq!(
        errors,
        ["c/entity.jsonc:3: descendsFrom cycle: a -> b -> c -> a"]
    );
    assert!(matches!(
        store.errors()[0].kind,
        ErrorKind::DefinitionCycle(_)
    ));
}

#[test]
fn self_descent_is_a_cycle() {
    let (errors, ..) = load(&[("a/entity.jsonc", "{ \"descendsFrom\": \"a\" }")]);
    assert_eq!(errors, ["a/entity.jsonc:1: descendsFrom cycle: a -> a"]);
}

#[test]
fn preset_cycle_is_detected() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{ \"descendsFrom\": \"Actor\", \"presets\": [\"x\"] }",
        ),
        ("presets/x.jsonc", "{ \"presets\": [\"y\"] }"),
        ("presets/y.jsonc", "{\n  \"presets\": [\"x\"]\n}"),
    ]);
    assert!(
        errors
            .iter()
            .any(|e| e.contains("preset cycle: x -> y -> x")),
        "{errors:?}"
    );
}

#[test]
fn lineage_must_end_in_a_primordial() {
    let (errors, ..) = load(&[
        ("orphan/entity.jsonc", "{\n  \"components\": {}\n}"),
        ("child/entity.jsonc", "{ \"descendsFrom\": \"orphan\" }"),
    ]);
    // The child reports the same problem at the same place; it is listed once.
    assert_eq!(
        errors,
        [
            "orphan/entity.jsonc:1: definition \"orphan\" has no descendsFrom and is not primordial (primordial names are capitalized)"
        ]
    );
}

#[test]
fn undeclared_primordial_is_a_missing_definition() {
    let (errors, ..) = load(&[("a/entity.jsonc", "{ \"descendsFrom\": \"Actr\" }")]);
    assert_eq!(
        errors,
        ["a/entity.jsonc:1: descendsFrom refers to missing definition \"Actr\""]
    );
}

#[test]
fn syntax_errors_carry_line_and_column() {
    let (errors, store, _dir) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {\n    \"Health\": { \"max\": }\n  }\n}",
        ),
    ]);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0].starts_with("a/entity.jsonc:4: syntax error"),
        "{errors:?}"
    );
    assert!(store.errors()[0].location.as_ref().unwrap().column > 1);
}

#[test]
fn duplicate_keys_and_unknown_sections() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"compnents\": {}\n}",
        ),
        (
            "b/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {},\n  \"components\": {}\n}",
        ),
    ]);
    assert_eq!(
        errors,
        [
            "a/entity.jsonc:3: unknown field \"compnents\" in entity definition",
            "b/entity.jsonc:4: duplicate key \"components\"",
        ]
    );
}

#[test]
fn component_given_twice() {
    let full = <common::Health as bevy::reflect::TypePath>::type_path();
    let b = format!(
        "{{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {{\n    \"Health\": {{}},\n    \"{full}\": {{}}\n  }}\n}}"
    );
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"transform\": {},\n  \"components\": {\n    \"Transform\": {}\n  }\n}",
        ),
        ("b/entity.jsonc", &b),
    ]);
    assert_eq!(
        errors[0],
        "a/entity.jsonc:3: component Transform is given twice"
    );
    // Health and its full path are the same component.
    assert_eq!(
        errors[1],
        "b/entity.jsonc:5: component Health is given twice".replace("Health", full)
    );
}

#[test]
fn every_bad_component_is_reported() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "a/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": {\n    \"Nope\": {},\n    \"Health\": { \"x\": 1 }\n  }\n}",
        ),
    ]);
    assert_eq!(errors.len(), 2, "{errors:?}");
}

#[test]
fn errors_in_inherited_data_point_at_the_parent_file() {
    let (errors, ..) = load(&[
        ACTOR,
        (
            "ogre/entity.jsonc",
            "{\n  \"descendsFrom\": \"Actor\",\n  \"components\": { \"Health\": { \"current\": true } }\n}",
        ),
        ("small_ogre/entity.jsonc", "{ \"descendsFrom\": \"ogre\" }"),
    ]);
    // Reported once, at the place the bad value is written.
    assert_eq!(errors, ["ogre/entity.jsonc:3: expected f32, found boolean"]);
}
