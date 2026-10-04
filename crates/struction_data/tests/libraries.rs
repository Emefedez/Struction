mod common;

use bevy::prelude::*;
use common::*;
use struction_data::{DefinitionStore, ErrorKind};

fn store(
    library: &[(&str, &str)],
    project: &[(&str, &str)],
) -> (DefinitionStore, Vec<tempfile::TempDir>) {
    let library = write_project(library);
    let project = write_project(project);
    let mut store = DefinitionStore::new(project.path());
    store.add_library("engine", library.path());
    store.load(&registry());
    (store, vec![library, project])
}

fn errors(store: &DefinitionStore) -> Vec<String> {
    store.errors().iter().map(ToString::to_string).collect()
}

const ENGINE: [(&str, &str); 3] = [
    (
        "Actor/entity.jsonc",
        r#"{ "components": { "Health": { "current": 10, "max": 10 } } }"#,
    ),
    (
        "creatures/ogre/entity.jsonc",
        r#"{ "descendsFrom": "Actor", "components": { "Faction": "hostile" } }"#,
    ),
    (
        "presets/burning.jsonc",
        r#"{ "components": { "Flammable": {} } }"#,
    ),
];

#[test]
fn projects_descend_from_library_definitions() {
    let (store, _dirs) = store(
        &ENGINE,
        &[(
            "ogres/chief/entity.jsonc",
            r#"{ "descendsFrom": "creatures/ogre", "presets": ["burning"] }"#,
        )],
    );
    assert_eq!(errors(&store), Vec::<String>::new());
    let chief = store.get("ogres/chief").unwrap();
    assert_eq!(chief.lineage, ["Actor", "creatures/ogre"]);
    assert_eq!(chief.component::<Health>().unwrap().max, 10.0);
    assert!(chief.component::<Flammable>().is_some());
    assert_eq!(store.library_of("creatures/ogre"), Some("engine"));
    assert_eq!(store.library_of("ogres/chief"), None);
    assert!(!store.in_project("Actor"));
}

#[test]
fn a_project_file_overrides_a_library_definition_for_every_descendant() {
    let (store, _dirs) = store(
        &ENGINE,
        &[
            (
                "Actor/entity.jsonc",
                r#"{ "components": { "Health": { "max": 25 } } }"#,
            ),
            ("hero/entity.jsonc", r#"{ "descendsFrom": "Actor" }"#),
        ],
    );
    assert_eq!(errors(&store), Vec::<String>::new());
    for id in ["Actor", "creatures/ogre", "hero"] {
        let health = store.get(id).unwrap().component::<Health>().unwrap();
        assert_eq!((health.current, health.max), (10.0, 25.0), "{id}");
    }
    assert_eq!(store.library_of("Actor"), Some("engine"));
    assert!(store.in_project("Actor"));
}

#[test]
fn overrides_keep_the_library_parent_and_presets_are_not_shadowed() {
    let (store, _dirs) = store(
        &ENGINE,
        &[
            (
                "creatures/ogre/entity.jsonc",
                "{\n  \"descendsFrom\": \"hero\"\n}",
            ),
            ("hero/entity.jsonc", r#"{ "descendsFrom": "Actor" }"#),
            ("presets/burning.jsonc", "{}"),
        ],
    );
    let kinds: Vec<_> = store.errors().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&ErrorKind::OverrideParent {
        id: "creatures/ogre".into(),
        library: "engine".into()
    }));
    assert!(kinds.contains(&ErrorKind::LibraryPreset {
        name: "burning".into(),
        library: "engine".into()
    }));
}

#[test]
fn library_errors_name_the_library_file() {
    let (store, _dirs) = store(
        &[(
            "Broken/entity.jsonc",
            "{\n  \"components\": { \"Nope\": {} }\n}",
        )],
        &[],
    );
    assert_eq!(
        errors(&store),
        ["engine:Broken/entity.jsonc:2: unknown component \"Nope\""]
    );
}

#[test]
fn deleting_an_override_falls_back_to_the_library() {
    let (mut store, dirs) = store(
        &ENGINE,
        &[(
            "Actor/entity.jsonc",
            r#"{ "components": { "Health": { "max": 25 } } }"#,
        )],
    );
    let file = dirs[1].path().join("Actor/entity.jsonc");
    std::fs::remove_file(&file).unwrap();
    let report = store.reload_file(&file, &registry());
    assert!(report.removed.is_empty());
    assert!(report.changed.contains(&"creatures/ogre".to_string()));
    assert_eq!(
        store
            .get("Actor")
            .unwrap()
            .component::<Health>()
            .unwrap()
            .max,
        10.0
    );
}

#[test]
fn a_client_asks_what_a_file_is_instead_of_guessing() {
    use struction_data::{SourceFile, classify_source};

    assert_eq!(
        classify_source("creatures/ogre/entity.jsonc"),
        Some(SourceFile::Definition("creatures/ogre".into()))
    );
    assert_eq!(
        classify_source("presets/burning.jsonc"),
        Some(SourceFile::Preset("burning".into()))
    );
    assert_eq!(classify_source("scenes/yard.jsonc"), None);
    assert_eq!(classify_source("notes.md"), None);

    let (loaded, dirs) = store(
        &[],
        &[
            (
                "ogres/chief/entity.jsonc",
                "// The chief of the yard, who never blinks.\n\
                 { \"descendsFrom\": \"Actor\", \"presets\": [\"burning\"] }\n",
            ),
            (
                "presets/burning.jsonc",
                "// Everything here is on fire.\n{ \"components\": { \"Flammable\": {} } }\n",
            ),
        ],
    );
    // The prose a file starts with is its description, which no schema field can carry.
    assert_eq!(
        loaded.doc("ogres/chief"),
        Some("The chief of the yard, who never blinks.")
    );
    assert_eq!(
        loaded.doc("presets/burning"),
        None,
        "presets are layers, not definitions"
    );
    assert_eq!(loaded.doc("Actor"), None);

    let (file, library) = loaded.preset_source("burning").unwrap();
    assert_eq!(file, dirs[1].path().join("presets/burning.jsonc"));
    assert_eq!(library, None);

    let (loaded, dirs) = store(&ENGINE, &[]);
    let (file, library) = loaded.preset_source("burning").unwrap();
    assert_eq!(file, dirs[0].path().join("presets/burning.jsonc"));
    assert_eq!(library, Some("engine"));
    assert_eq!(loaded.preset_source("nonexistent"), None);
}
