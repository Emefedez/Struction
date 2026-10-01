mod common;

use std::fs;
use std::path::Path;

use bevy::prelude::*;
use common::*;
use struction_data::{DataPlugin, DefinitionStore, DefinitionsChanged, reload_definition_file};

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Project {
    dir: tempfile::TempDir,
    store: DefinitionStore,
    registry: bevy::reflect::TypeRegistry,
}

impl Project {
    fn open() -> Self {
        let dir = tempfile::tempdir().unwrap();
        copy_dir(&fixture_project(), dir.path());
        let registry = registry();
        let mut store = DefinitionStore::new(dir.path());
        store.declare_primordial("Terrain");
        assert_eq!(store.load(&registry).errors, vec![]);
        Project {
            dir,
            store,
            registry,
        }
    }

    fn write(&self, rel: &str, content: &str) {
        let path = self.dir.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.dir.path().join(rel)).unwrap()
    }

    fn reload(&mut self, rel: &str) -> struction_data::ReloadReport {
        let path = self.dir.path().join(rel);
        self.store.reload_file(&path, &self.registry)
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

#[test]
fn changing_a_definition_reports_its_descendants() {
    let mut p = Project::open();
    let text = p.read("minions/ogre/entity.jsonc").replace("60.0", "80.0");
    p.write("minions/ogre/entity.jsonc", &text);
    let report = p.reload("minions/ogre/entity.jsonc");

    assert_eq!(report.errors, vec![]);
    // The lord sets both Health fields itself, so the ogre's Health does not reach it.
    assert_eq!(
        sorted(report.changed),
        ["minions/ogre", "minions/small_ogre"]
    );
    // small_ogre overrides only max, so the new current flows through.
    let health = p
        .store
        .get("minions/small_ogre")
        .unwrap()
        .component::<Health>()
        .unwrap();
    assert_eq!((health.current, health.max), (80.0, 25.0));
}

#[test]
fn only_definitions_whose_result_differs_are_reported() {
    let mut p = Project::open();
    let text = p
        .read("minions/ogre/entity.jsonc")
        .replace("\"strength\": 9", "\"strength\": 10");
    p.write("minions/ogre/entity.jsonc", &text);
    let report = p.reload("minions/ogre/entity.jsonc");
    assert_eq!(
        sorted(report.changed),
        ["minions/ogre", "minions/ogre_lord", "minions/small_ogre"]
    );

    // Editing the small_ogre touches nobody else.
    let text = p
        .read("minions/small_ogre/entity.jsonc")
        .replace("\"agility\": 6", "\"agility\": 7");
    p.write("minions/small_ogre/entity.jsonc", &text);
    let report = p.reload("minions/small_ogre/entity.jsonc");
    assert_eq!(report.changed, ["minions/small_ogre"]);
}

#[test]
fn comment_and_formatting_edits_change_nothing() {
    let mut p = Project::open();
    let text = format!("// a new comment\n{}", p.read("minions/ogre/entity.jsonc"))
        .replace("\"club\"", "\"club\"  ");
    p.write("minions/ogre/entity.jsonc", &text);
    let report = p.reload("minions/ogre/entity.jsonc");
    assert!(report.changed.is_empty(), "{:?}", report.changed);
    assert!(report.errors.is_empty());
}

#[test]
fn preset_changes_reach_the_definitions_that_use_them() {
    let mut p = Project::open();
    p.write(
        "presets/flammable.jsonc",
        r#"{ "components": { "Flammable": { "ignition_temperature": 100.0 } } }"#,
    );
    let report = p.reload("presets/flammable.jsonc");
    // The lord removes Flammable, so nothing changes for it.
    assert_eq!(
        sorted(report.changed),
        ["minions/ogre", "minions/small_ogre"]
    );
    assert_eq!(
        p.store
            .get("minions/small_ogre")
            .unwrap()
            .component::<Flammable>()
            .unwrap()
            .ignition_temperature,
        100.0
    );

    // A preset used through another preset.
    p.write(
        "presets/water.jsonc",
        r#"{ "components": { "Surface": { "friction": 0.5, "drag": 2.0 }, "Volume": { "half_extents": [1, 1, 1] } } }"#,
    );
    let report = p.reload("presets/water.jsonc");
    assert_eq!(report.changed, ["world/lava_pool"]);
}

#[test]
fn a_broken_edit_keeps_the_last_good_version_and_reports_file_line() {
    let mut p = Project::open();
    let good = p.read("minions/ogre/entity.jsonc");

    // Semantic error.
    p.write(
        "minions/ogre/entity.jsonc",
        &good.replace("\"gold\": 12", "\"golds\": 12"),
    );
    let report = p.reload("minions/ogre/entity.jsonc");
    assert!(report.changed.is_empty());
    let messages: Vec<_> = report.errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        ["minions/ogre/entity.jsonc:9: unknown field \"golds\" in Loot"]
    );
    assert_eq!(p.store.errors().len(), 1);
    // Old data is still served, for the ogre and its children.
    assert_eq!(
        p.store
            .get("minions/ogre")
            .unwrap()
            .component::<Loot>()
            .unwrap()
            .gold,
        Some(12)
    );
    assert!(p.store.get("minions/small_ogre").is_some());

    // Syntax error.
    p.write(
        "minions/ogre/entity.jsonc",
        &good.replace("\"Actor\",", "\"Actor\" ,,"),
    );
    let report = p.reload("minions/ogre/entity.jsonc");
    assert!(
        report.errors[0]
            .to_string()
            .starts_with("minions/ogre/entity.jsonc:3: syntax error")
    );
    assert!(p.store.get("minions/ogre").is_some());

    // Fixing it clears the errors and reports the change.
    p.write(
        "minions/ogre/entity.jsonc",
        &good.replace("\"gold\": 12", "\"gold\": 13"),
    );
    let report = p.reload("minions/ogre/entity.jsonc");
    assert_eq!(report.errors, vec![]);
    assert_eq!(p.store.errors(), vec![]);
    // small_ogre sets gold to null itself.
    assert_eq!(
        sorted(report.changed),
        ["minions/ogre", "minions/ogre_lord"]
    );
}

#[test]
fn removing_and_adding_files() {
    let mut p = Project::open();
    fs::remove_file(p.dir.path().join("minions/ogre/entity.jsonc")).unwrap();
    let report = p.reload("minions/ogre/entity.jsonc");
    assert_eq!(report.removed, ["minions/ogre"]);
    assert!(p.store.get("minions/ogre").is_none());
    // Children now dangle: they report it and keep serving their last good data.
    let messages: Vec<_> = report.errors.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        [
            "minions/ogre_lord/entity.jsonc:2: descendsFrom refers to missing definition \"minions/ogre\"",
            "minions/small_ogre/entity.jsonc:2: descendsFrom refers to missing definition \"minions/ogre\"",
        ]
    );
    assert!(p.store.get("minions/small_ogre").is_some());

    // Bringing the parent back resolves them again.
    p.write(
        "minions/ogre/entity.jsonc",
        r#"{ "descendsFrom": "Actor", "components": { "Health": { "current": 1.0, "max": 2.0 }, "Stats": { "strength": 1, "agility": 1 } } }"#,
    );
    let report = p.reload("minions/ogre/entity.jsonc");
    assert_eq!(report.errors, vec![]);
    assert_eq!(
        sorted(report.changed),
        ["minions/ogre", "minions/ogre_lord", "minions/small_ogre"]
    );
    assert_eq!(p.store.errors(), vec![]);

    // A brand new definition.
    p.write(
        "minions/imp/entity.jsonc",
        r#"{ "descendsFrom": "minions/small_ogre" }"#,
    );
    let report = p.reload("minions/imp/entity.jsonc");
    assert_eq!(report.changed, ["minions/imp"]);
    assert_eq!(
        p.store.get("minions/imp").unwrap().lineage,
        ["Actor", "minions/ogre", "minions/small_ogre"]
    );
}

#[test]
fn changing_descends_from_updates_lineage_and_dependencies() {
    let mut p = Project::open();
    p.write(
        "minions/small_ogre/entity.jsonc",
        r#"{ "descendsFrom": "Actor" }"#,
    );
    let report = p.reload("minions/small_ogre/entity.jsonc");
    assert_eq!(report.changed, ["minions/small_ogre"]);
    assert_eq!(
        p.store.get("minions/small_ogre").unwrap().lineage,
        ["Actor"]
    );
    assert!(
        p.store
            .descendants("minions/ogre")
            .iter()
            .all(|d| *d != "minions/small_ogre")
    );

    // It no longer depends on the ogre: editing the ogre does not report it.
    let text = p
        .read("minions/ogre/entity.jsonc")
        .replace("\"strength\": 9", "\"strength\": 11");
    p.write("minions/ogre/entity.jsonc", &text);
    let report = p.reload("minions/ogre/entity.jsonc");
    assert_eq!(
        sorted(report.changed),
        ["minions/ogre", "minions/ogre_lord"]
    );
}

#[test]
fn unrelated_files_are_ignored() {
    let mut p = Project::open();
    p.write("scenes/courtyard.jsonc", "not even json");
    let report = p.reload("scenes/courtyard.jsonc");
    assert_eq!(report, Default::default());
}

#[test]
fn schema_reflects_the_project() {
    let p = Project::open();
    let schema = p.store.schema(&p.registry);
    let defs: Vec<_> = schema["properties"]["descendsFrom"]["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(defs.contains(&"minions/ogre") && defs.contains(&"Terrain"));
    assert_eq!(
        schema["properties"]["presets"]["items"]["enum"],
        serde_json::json!(["flammable", "lava", "water"])
    );
}

fn plugin_app(root: &Path) -> App {
    let mut app = App::new();
    app.register_type::<Health>()
        .register_type::<Flammable>()
        .register_type::<Faction>()
        .register_type::<Loot>()
        .register_type::<Stats>()
        .register_type::<Damage>()
        .register_type::<Volume>()
        .register_type::<Surface>()
        .add_plugins(DataPlugin::new(root).primordial("Terrain"));
    app.finish();
    app.cleanup();
    app
}

#[test]
fn plugin_loads_the_store_and_announces_reloads() {
    let dir = tempfile::tempdir().unwrap();
    copy_dir(&fixture_project(), dir.path());
    let mut app = plugin_app(dir.path());

    let store = app.world().resource::<DefinitionStore>();
    assert_eq!(store.errors(), vec![]);
    assert!(store.get("minions/small_ogre").is_some());

    let ogre = dir.path().join("minions/ogre/entity.jsonc");
    let text = fs::read_to_string(&ogre).unwrap().replace("60.0", "70.0");
    fs::write(&ogre, text).unwrap();
    let report = reload_definition_file(app.world_mut(), &ogre);
    assert_eq!(report.changed.len(), 2);

    let mut messages = app
        .world_mut()
        .resource_mut::<Messages<DefinitionsChanged>>();
    let sent: Vec<_> = messages.drain().collect();
    assert_eq!(sent.len(), 1);
    assert_eq!(sorted(sent[0].changed.clone()), sorted(report.changed));

    // A reload with no data change sends nothing.
    let report = reload_definition_file(app.world_mut(), &ogre);
    assert!(report.changed.is_empty());
    assert!(
        app.world_mut()
            .resource_mut::<Messages<DefinitionsChanged>>()
            .drain()
            .next()
            .is_none()
    );
}
