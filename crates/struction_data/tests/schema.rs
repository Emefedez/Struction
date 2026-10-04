mod common;

use common::*;
use serde_json::{Value, json};
use struction_data::DefinitionStore;
use struction_data::schema::{SchemaOptions, entity_schema};

fn refs(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                if k == "$ref" {
                    out.push(v.as_str().unwrap().to_owned());
                } else {
                    refs(v, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|v| refs(v, out)),
        _ => {}
    }
}

fn component<'a>(schema: &'a Value, name: &str) -> &'a Value {
    &schema["properties"]["components"]["properties"][name]
}

/// Follows the `$ref` inside a component's `anyOf` to its definition.
fn definition<'a>(schema: &'a Value, name: &str) -> &'a Value {
    let reference = component(schema, name)["anyOf"][0]["$ref"]
        .as_str()
        .unwrap();
    &schema["$defs"][reference.strip_prefix("#/$defs/").unwrap()]
}

#[test]
fn top_level_sections_follow_the_canonical_fields() {
    let schema = entity_schema(&registry(), &SchemaOptions::default());
    assert_eq!(
        schema["$schema"],
        "https://json-schema.org/draft/2020-12/schema"
    );
    let sections: Vec<_> = schema["properties"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    for key in [
        "descendsFrom",
        "presets",
        "transform",
        "components",
        "constraints",
        "reactions",
        "brain",
    ] {
        assert!(sections.contains(&key.to_owned()), "{key}");
    }
    assert_eq!(schema["additionalProperties"], json!(false));
    // Free-form when no project names are given, but still explained.
    assert_eq!(
        schema["properties"]["descendsFrom"]["type"],
        json!("string")
    );
    assert!(
        schema["properties"]["descendsFrom"]["description"]
            .as_str()
            .is_some_and(|text| text.contains("Inherits"))
    );
}

#[test]
fn every_section_says_what_it_is_for() {
    // The engine's own vocabulary, which no registered type can describe: a client reads it from
    // the schema instead of repeating it.
    let schema = entity_schema(&registry(), &SchemaOptions::default());
    for (section, expected) in [
        ("descendsFrom", "Inherits"),
        ("presets", "Named reusable layers"),
        ("transform", "folded in as its `Transform`"),
        ("components", "Registered Rust components"),
        ("constraints", "no package reads it"),
        ("extensors", "Packages extending this definition"),
        ("states", "Components enabled and disabled"),
        ("reactions", "Instantaneous action hooks"),
        ("grantsToWards", "Capabilities granted by a master"),
    ] {
        assert!(
            schema["properties"][section]["description"]
                .as_str()
                .is_some_and(|text| text.contains(expected)),
            "{section}: {:?}",
            schema["properties"][section]["description"]
        );
    }
}

#[test]
fn values_carry_the_documentation_of_the_type_that_declares_them() {
    // A doc comment on an enum variant or a contributed state is what a completion or a hover
    // shows over that value, so it travels with the schema rather than a client repeating it.
    let schema = entity_schema(
        &registry(),
        &SchemaOptions {
            states: vec![
                (
                    "Resting".to_owned(),
                    "Neither moving nor acting.".to_owned(),
                ),
                ("Rolling".to_owned(), String::new()),
            ],
            ..SchemaOptions::default()
        },
    );
    let damage = definition(&schema, "Damage");
    let reference = damage["properties"]["kind"]["$ref"].as_str().unwrap();
    let kinds = &schema["$defs"][reference.strip_prefix("#/$defs/").unwrap()]["oneOf"][0];
    assert_eq!(kinds["enum"], json!(["Physical", "Fire"]));
    assert_eq!(
        kinds["enumDescriptions"],
        json!(["A blunt impact.", "Burns over time."])
    );
    // States come from the packages that contribute them, docs included.
    let states = &schema["properties"]["states"]["propertyNames"];
    assert_eq!(states["enum"], json!(["Resting", "Rolling"]));
    assert_eq!(
        states["enumDescriptions"],
        json!(["Neither moving nor acting.", ""])
    );
}

#[test]
fn components_come_from_the_registry() {
    let schema = entity_schema(&registry(), &SchemaOptions::default());
    let components = schema["properties"]["components"]["properties"]
        .as_object()
        .unwrap();
    for name in [
        "Health",
        "Flammable",
        "Faction",
        "Loot",
        "Stats",
        "Damage",
        "Volume",
        "Surface",
        "Shape",
        "Transform",
    ] {
        assert!(components.contains_key(name), "{name}");
    }
    // Registered but not a component; and plain enums are only reachable through fields.
    assert!(!components.contains_key("Plain"));
    assert!(!components.contains_key("DamageKind"));
    assert_eq!(
        schema["properties"]["components"]["additionalProperties"],
        json!(false)
    );
    // `null` removes an inherited component.
    assert_eq!(
        component(&schema, "Health")["anyOf"][1],
        json!({ "type": "null" })
    );
}

#[test]
fn struct_fields_types_and_docs() {
    let schema = entity_schema(&registry(), &SchemaOptions::default());
    let health = definition(&schema, "Health");
    assert_eq!(health["type"], "object");
    assert_eq!(health["additionalProperties"], json!(false));
    assert_eq!(health["properties"]["max"], json!({ "type": "number" }));
    assert_eq!(health["properties"]["current"]["type"], "number");
    assert_eq!(
        health["properties"]["current"]["description"],
        "Current hit points."
    );
    assert_eq!(health["description"], "Hit points of a living thing.");
    // Nothing is required: a child definition may set one field.
    assert!(health.get("required").is_none());

    let loot = definition(&schema, "Loot");
    assert_eq!(
        loot["properties"]["items"],
        json!({ "type": "array", "items": { "type": "string" } })
    );
    assert_eq!(
        loot["properties"]["gold"],
        json!({ "anyOf": [{ "type": "null" }, { "type": "integer", "minimum": 0 }] })
    );
    // Newtype: the field itself.
    assert_eq!(
        component(&schema, "Faction")["anyOf"][0],
        json!({ "type": "string" })
    );
}

#[test]
fn vectors_are_arrays_or_objects_and_enums_list_their_variants() {
    let schema = entity_schema(&registry(), &SchemaOptions::default());
    let volume = definition(&schema, "Volume");
    let vec3 = &volume["properties"]["half_extents"]["anyOf"];
    assert_eq!(
        vec3[0],
        json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 })
    );
    assert!(vec3[1]["$ref"].as_str().unwrap().contains("Vec3"));

    let damage = definition(&schema, "Damage");
    let kind_ref = damage["properties"]["kind"]["$ref"].as_str().unwrap();
    let kind = &schema["$defs"][kind_ref.strip_prefix("#/$defs/").unwrap()];
    assert_eq!(
        kind["oneOf"][0],
        json!({
            "enum": ["Physical", "Fire"],
            "enumDescriptions": ["A blunt impact.", "Burns over time."]
        })
    );

    let shape = definition(&schema, "Shape");
    let variants = shape["oneOf"].as_array().unwrap();
    assert_eq!(variants.len(), 3);
    // Unit variants are grouped into one string enum, first.
    assert_eq!(variants[0], json!({ "enum": ["Point"] }));
    assert_eq!(variants[1]["required"], json!(["Sphere"]));
    assert_eq!(
        variants[1]["properties"]["Sphere"]["required"],
        json!(["radius"])
    );
    assert_eq!(variants[2]["properties"]["Box"]["anyOf"][0]["minItems"], 3);
}

#[test]
fn every_reference_resolves() {
    let schema = entity_schema(&registry(), &SchemaOptions::default());
    let mut all = Vec::new();
    refs(&schema, &mut all);
    assert!(all.len() > 10);
    for reference in all {
        let key = reference.strip_prefix("#/$defs/").expect(&reference);
        assert!(schema["$defs"].get(key).is_some(), "dangling {reference}");
        assert!(
            key.chars()
                .all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c)),
            "{key}"
        );
    }
}

#[test]
fn transform_is_offered_as_its_own_section() {
    let schema = entity_schema(&registry(), &SchemaOptions::default());
    let reference = schema["properties"]["transform"]["$ref"].as_str().unwrap();
    let def = &schema["$defs"][reference.strip_prefix("#/$defs/").unwrap()];
    assert!(def["properties"].get("translation").is_some());
    assert!(def["properties"].get("scale").is_some());
}

#[test]
fn the_schema_offers_the_registered_actions_wherever_one_is_named() {
    let registry = registry();
    let mut store = DefinitionStore::new(fixture_project());
    store.declare_primordial("Terrain");
    store.load(&registry);
    let actions = actions(&[("guards/ogre/die", "Die"), ("minions/ogre/hurt", "Hurt")]);
    let schema = store.schema(&registry, &actions);
    let reaction = &schema["properties"]["reactions"]["items"];
    let grant = &schema["properties"]["grantsToWards"]["items"];

    assert_eq!(
        reaction["properties"]["call"]["enum"],
        json!(["guards/ogre/die", "minions/ogre/hurt"])
    );
    assert_eq!(
        reaction["properties"]["after"]["enum"],
        json!(["guards/ogre/die", "minions/ogre/hurt"])
    );
    assert_eq!(
        reaction["properties"]["before"]["enum"],
        json!(["guards/ogre/die", "minions/ogre/hurt"])
    );
    // A completion or hover over a name says which action it is.
    assert_eq!(
        reaction["properties"]["call"]["enumDescriptions"],
        json!(["Die", "Hurt"])
    );
    assert_eq!(
        reaction["properties"]["source"]["enum"],
        json!(["this", "master", "wards"])
    );
    assert_eq!(reaction["required"], json!(["call"]));
    // Exactly one hook, as `reactions_from_node` requires.
    assert_eq!(
        reaction["oneOf"],
        json!([{ "required": ["after"] }, { "required": ["before"] }])
    );
    // A grant names the wards by lineage, so it offers every definition.
    assert_eq!(
        grant["properties"]["to"]["enum"],
        json!([
            "Actor",
            "Terrain",
            "minions/ogre",
            "minions/ogre_lord",
            "minions/small_ogre",
            "world/lava_pool"
        ])
    );
    assert_eq!(
        grant["properties"]["actions"]["items"]["enum"],
        json!(["guards/ogre/die", "minions/ogre/hurt"])
    );
    assert_eq!(grant["required"], json!(["to"]));
    // A grant hands over components, so it offers the same component names as `components`.
    assert_eq!(
        grant["properties"]["components"]["properties"]
            .as_object()
            .map(|components| components.len()),
        schema["properties"]["components"]["properties"]
            .as_object()
            .map(|c| c.len())
    );
    assert!(schema["properties"]["states"]["additionalProperties"]["properties"]["disable"]
        ["description"]
        .as_str()
        .is_some_and(|text| text.contains("Component names")));
}

#[test]
fn project_schema_lists_definitions_and_is_written_as_json() {
    let registry = registry();
    let mut store = DefinitionStore::new(fixture_project());
    store.declare_primordial("Terrain");
    store.load(&registry);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schema.json");
    let actions = actions(&[("guards/ogre/die", "Die")]);
    store.write_schema(&registry, &actions, &path).unwrap();

    let written: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(written, store.schema(&registry, &actions));
    assert_eq!(
        written["properties"]["descendsFrom"]["enum"],
        json!([
            "Actor",
            "Terrain",
            "minions/ogre",
            "minions/ogre_lord",
            "minions/small_ogre",
            "world/lava_pool"
        ])
    );
}
