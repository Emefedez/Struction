mod common;

use std::collections::BTreeMap;

use common::*;
use struction_core::{ActionRegistry, ContributedState, ExtensorMeta, ExtensorRegistry};
use struction_data::{
    DefinitionStore, DroppedExtensor, ExtensorReason, ExtensorUse, Suggestion, parse_jsonc,
};

/// `Health` belongs to the inferred `living` package; `Flammable` to the opt-in `fire`, which
/// supplies it and builds on `living`, which builds on `physics`.
fn extensors() -> ExtensorRegistry {
    let mut extensors = ExtensorRegistry::default();
    extensors.register(ExtensorMeta::inferred("physics").owns::<Surface>());
    extensors.register(
        ExtensorMeta::inferred("living")
            .owns::<Health>()
            .requires("physics")
            .state(ContributedState::new("Resting", "Neither moving nor acting.")),
    );
    extensors.register(
        ExtensorMeta::opt_in("fire")
            .doc("Burns")
            .supplies::<Flammable>()
            .requires("living")
            .state(ContributedState::new("Burning", "On fire.")),
    );
    extensors.register(ExtensorMeta::opt_in("loot").owns::<Loot>());
    extensors
}

fn load(files: &[(&str, &str)]) -> DefinitionStore {
    let sources: BTreeMap<String, String> = files
        .iter()
        .map(|(path, text)| (path.to_string(), text.to_string()))
        .collect();
    let mut store = DefinitionStore::new("/nonexistent/struction-extensor-test");
    store.set_extensors(extensors());
    store.preview_sources(&sources, &registry())
}

fn errors(store: &DefinitionStore) -> Vec<String> {
    store.errors().iter().map(ToString::to_string).collect()
}

fn named(name: &str, by: &str, components: &[&str], supplied: &[&str]) -> ExtensorUse {
    ExtensorUse {
        name: name.into(),
        reason: ExtensorReason::Named { by: by.into() },
        components: components.iter().map(|c| c.to_string()).collect(),
        supplied: supplied.iter().map(|c| c.to_string()).collect(),
    }
}

#[test]
fn naming_an_extensor_supplies_its_defaults_and_explains_the_rest() {
    let store = load(&[
        (
            "Actor/entity.jsonc",
            r#"{ "components": { "Health": { "max": 5 } } }"#,
        ),
        (
            "torch/entity.jsonc",
            r#"{ "descendsFrom": "Actor", "extensors": ["fire"] }"#,
        ),
    ]);
    assert_eq!(errors(&store), Vec::<String>::new());
    let torch = store.get("torch").unwrap();
    assert_eq!(torch.component::<Flammable>(), Some(&Flammable::default()));
    assert_eq!(
        torch.extensors,
        [
            named("fire", "torch", &["Flammable"], &["Flammable"]),
            ExtensorUse {
                name: "living".into(),
                reason: ExtensorReason::Owns("Health".into()),
                components: vec!["Health".into()],
                supplied: vec![],
            },
            ExtensorUse {
                name: "physics".into(),
                reason: ExtensorReason::RequiredBy("living".into()),
                components: vec![],
                supplied: vec![],
            },
        ]
    );
    // Without the extensor, nothing is supplied and nothing opt-in is used.
    let actor = store.get("Actor").unwrap();
    assert!(actor.component::<Flammable>().is_none());
    assert!(actor.extensors.iter().all(|e| !e.is_named()));
}

#[test]
fn authored_tuning_wins_over_supplied_defaults() {
    let store = load(&[(
        "Torch/entity.jsonc",
        r#"{ "extensors": ["fire"], "components": { "Health": {}, "Flammable": { "ignition_temperature": 90 } } }"#,
    )]);
    assert_eq!(errors(&store), Vec::<String>::new());
    let torch = store.get("Torch").unwrap();
    assert_eq!(
        torch.component::<Flammable>().unwrap().ignition_temperature,
        90.0
    );
    assert!(torch.extensors[0].supplied.is_empty());
}

#[test]
fn opt_in_components_need_their_extensor_named() {
    let store = load(&[(
        "Chest/entity.jsonc",
        "{\n  \"components\": {\n    \"Loot\": { \"items\": [] }\n  }\n}",
    )]);
    assert_eq!(
        errors(&store),
        [
            "Chest/entity.jsonc:3: Loot belongs to the opt-in extensor \"loot\"; add it to \"extensors\""
        ]
    );
}

#[test]
fn unknown_extensors_and_unmet_opt_in_requirements_are_reported() {
    let mut extensors = extensors();
    extensors.register(ExtensorMeta::opt_in("arson").requires("fire"));
    let mut store = DefinitionStore::new("/nonexistent/struction-extensor-test");
    store.set_extensors(extensors);
    let store = store.preview_sources(
        &BTreeMap::from([
            (
                "A/entity.jsonc".to_string(),
                "{\n  \"extensors\": [\"fly\"]\n}".to_string(),
            ),
            (
                "B/entity.jsonc".to_string(),
                "{\n  \"extensors\": [\"arson\"]\n}".to_string(),
            ),
        ]),
        &registry(),
    );
    assert_eq!(
        errors(&store),
        [
            "A/entity.jsonc:2: unknown extensor \"fly\", registered: arson, fire, living, loot, physics",
            "B/entity.jsonc:2: extensor \"arson\" needs \"fire\"; add it to \"extensors\"",
        ]
    );
}

#[test]
fn extensors_accumulate_through_presets_inheritance_and_overrides() {
    let store = load(&[
        ("presets/burning.jsonc", r#"{ "extensors": ["fire"] }"#),
        (
            "Crate/entity.jsonc",
            r#"{ "components": { "Health": {} } }"#,
        ),
        (
            "crates/rich/entity.jsonc",
            r#"{ "descendsFrom": "Crate", "presets": ["burning"], "extensors": ["loot"] }"#,
        ),
        (
            "crates/richer/entity.jsonc",
            r#"{ "descendsFrom": "crates/rich", "extensors": ["fire"] }"#,
        ),
    ]);
    assert_eq!(errors(&store), Vec::<String>::new());
    let named_by = |id: &str| -> Vec<(String, ExtensorReason)> {
        store
            .get(id)
            .unwrap()
            .extensors
            .iter()
            .filter(|e| e.is_named())
            .map(|e| (e.name.clone(), e.reason.clone()))
            .collect()
    };
    let by = |by: &str| ExtensorReason::Named { by: by.into() };
    let expected = vec![
        ("fire".to_string(), by("preset burning")),
        ("loot".to_string(), by("crates/rich")),
    ];
    assert_eq!(named_by("crates/rich"), expected);
    // Naming it again changes nothing: the first declaration explains it.
    assert_eq!(named_by("crates/richer"), expected);

    let over = parse_jsonc("scene.jsonc", r#"{ "extensors": ["fire"] }"#).unwrap();
    let instance = store
        .instantiate("Crate", Some(&over), &registry())
        .unwrap();
    assert!(instance.component::<Flammable>().is_some());
    assert_eq!(
        instance.extensors[0].reason,
        ExtensorReason::Named {
            by: "a scene override".into()
        }
    );
}

#[test]
fn changing_extensors_counts_as_changed_data() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Torch")).unwrap();
    let file = dir.path().join("Torch/entity.jsonc");
    std::fs::write(&file, r#"{ "components": { "Health": {} } }"#).unwrap();
    let registry = registry();
    let mut store = DefinitionStore::new(dir.path());
    store.set_extensors(extensors());
    store.load(&registry);
    std::fs::write(
        &file,
        r#"{ "extensors": ["fire"], "components": { "Health": {} } }"#,
    )
    .unwrap();
    let report = store.reload_file(&file, &registry);
    assert_eq!(report.changed, ["Torch"]);
    assert!(
        store
            .get("Torch")
            .unwrap()
            .component::<Flammable>()
            .is_some()
    );
}

#[test]
fn the_schema_offers_registered_extensors() {
    let store = load(&[]);
    let schema = store.schema(&registry(), &ActionRegistry::default());
    let items = &schema["properties"]["extensors"]["items"]["oneOf"];
    assert!(
        items
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["const"] == "fire" && item["description"] == "Burns"),
        "{items}"
    );
}

#[test]
fn a_descendant_drops_an_inherited_extensor_with_its_components() {
    let store = load(&[
        ("presets/burning.jsonc", r#"{ "extensors": ["fire"] }"#),
        (
            "Crate/entity.jsonc",
            r#"{ "extensors": ["loot", "fire"], "components": { "Health": {}, "Loot": { "items": ["coin"] }, "Flammable": { "ignition_temperature": 90 } } }"#,
        ),
        (
            "crates/wet/entity.jsonc",
            r#"{ "descendsFrom": "Crate", "extensors": ["-fire"] }"#,
        ),
        (
            "crates/relit/entity.jsonc",
            r#"{ "descendsFrom": "crates/wet", "presets": ["burning"] }"#,
        ),
    ]);
    assert_eq!(errors(&store), Vec::<String>::new());
    let wet = store.get("crates/wet").unwrap();
    assert!(wet.component::<Flammable>().is_none());
    assert!(wet.component::<Loot>().is_some());
    assert!(wet.extensors.iter().all(|e| e.name != "fire"));
    assert_eq!(
        wet.dropped,
        [DroppedExtensor {
            name: "fire".into(),
            by: "crates/wet".into()
        }]
    );
    // Naming it again later brings it back with its defaults, not the dropped tuning.
    let relit = store.get("crates/relit").unwrap();
    assert_eq!(relit.component::<Flammable>(), Some(&Flammable::default()));
    assert!(relit.dropped.is_empty());
}

#[test]
fn dropping_an_unknown_extensor_is_reported() {
    let store = load(&[("A/entity.jsonc", "{\n  \"extensors\": [\"-fly\"]\n}")]);
    assert_eq!(
        errors(&store),
        ["A/entity.jsonc:2: unknown extensor \"fly\", registered: fire, living, loot, physics"]
    );
}

#[test]
fn opt_in_extensors_whose_requirements_are_met_are_suggested() {
    let store = load(&[
        (
            "Rock/entity.jsonc",
            r#"{ "components": { "Surface": {} } }"#,
        ),
        (
            "Actor/entity.jsonc",
            r#"{ "components": { "Health": {} } }"#,
        ),
        (
            "actors/torch/entity.jsonc",
            r#"{ "descendsFrom": "Actor", "extensors": ["fire"] }"#,
        ),
        (
            "actors/damp/entity.jsonc",
            r#"{ "descendsFrom": "actors/torch", "extensors": ["-fire"] }"#,
        ),
    ]);
    assert_eq!(errors(&store), Vec::<String>::new());
    let suggested = |id: &str| {
        store
            .get(id)
            .unwrap()
            .suggested_extensors(store.extensors())
    };
    assert_eq!(
        suggested("Actor"),
        [Suggestion {
            name: "fire".into(),
            doc: "Burns".into(),
            because: vec!["living".into()],
            supplies: vec!["Flammable".into()],
        }]
    );
    // Nothing to build on, already used, or deliberately dropped.
    assert!(suggested("Rock").is_empty());
    assert!(suggested("actors/torch").is_empty());
    assert!(suggested("actors/damp").is_empty());
}

#[test]
fn states_switch_components_and_are_validated() {
    let store = load(&[(
        "Actor/entity.jsonc",
        r#"{ "extensors": ["fire"], "components": { "Health": {} }, "states": { "Resting": { "enable": { "Surface": { "friction": 2 } }, "disable": ["Health"] }, "Burning": { "disable": ["Flammable"] } } }"#,
    )]);
    assert_eq!(errors(&store), Vec::<String>::new());
    let rules = store.get("Actor").unwrap().states.clone().unwrap();
    assert_eq!(rules.0.len(), 2);
    assert_eq!(rules.0[0].state, "Resting");
    assert_eq!(
        rules.0[0].enable[0].1.downcast_ref::<Surface>(),
        Some(&Surface {
            friction: 2.0,
            drag: 0.0
        })
    );
    assert_eq!(rules.0[0].disable, [std::any::TypeId::of::<Health>()]);

    let store = load(&[(
        "Actor/entity.jsonc",
        "{\n  \"states\": {\n    \"Sleeping\": {},\n    \"Burning\": {},\n    \"Resting\": { \"enable\": { \"Loot\": {} }, \"disable\": [\"Nope\"], \"other\": 1 }\n  }\n}",
    )]);
    assert_eq!(
        errors(&store),
        [
            "Actor/entity.jsonc:3: unknown state \"Sleeping\", registered: Burning, Resting",
            "Actor/entity.jsonc:4: state Burning comes from the opt-in extensor \"fire\"; add it to \"extensors\"",
            "Actor/entity.jsonc:5: Loot belongs to the opt-in extensor \"loot\"; add it to \"extensors\"",
            "Actor/entity.jsonc:5: unknown component \"Nope\"",
            "Actor/entity.jsonc:5: unknown field \"other\" in state Resting",
        ]
    );
}
