//! Snapshot and transport behavior an editor relies on: unsaved buffers in, structured
//! registrations out, and nothing written to disk.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::path::{Path, PathBuf};

use bevy::prelude::{AppTypeRegistry, *};
use bevy::reflect::TypePath;
use serde_json::{Value, json};
use struction_core::{
    ActionAppExt, ActionCall, ActionMeta, ActionRegistry, ContributedState, CorePlugin,
    ExtensorAppExt, ExtensorMeta, ParamType,
};
use struction_data::{DataPlugin, DefinitionStore};
use struction_editor::AuthoringProject;
use struction_world::WorldPlugin;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Armor {
    rating: f32,
}

fn registrations(app: &mut App) -> &mut App {
    app.register_type::<Health>()
        .register_type::<Armor>()
        .register_extensor(
            ExtensorMeta::inferred("living")
                .doc("Anything that can be hurt")
                .owns::<Health>(),
        )
        .register_extensor(
            ExtensorMeta::opt_in("armor")
                .doc("Protection that reduces incoming hits")
                .supplies::<Armor>()
                .requires("living")
                .state(ContributedState::new(
                    "Guarded",
                    "Standing behind the shield.",
                )),
        )
        .register_action(
            ActionMeta::new("hurt")
                .doc("Lose hit points")
                .requires::<Health>()
                .param_or("amount", ParamType::Float, 1.0),
            |In(call): In<ActionCall>, mut health: Query<&mut Health>| {
                health.get_mut(call.target).unwrap().current -= 1.0;
            },
        )
}

fn factory(root: &Path) -> App {
    build(DataPlugin::new(root).primordial("Actor"))
}

/// The world and core plugins are what the backend requires of any game factory.
fn build(data: DataPlugin) -> App {
    let mut app = App::new();
    app.add_plugins((CorePlugin::default(), data, WorldPlugin::default()));
    registrations(&mut app);
    app
}

thread_local! {
    /// The factory a host is given only receives the project root, so the library root a test
    /// wants built with is left here for it. Each test runs on its own thread.
    static LIBRARY_ROOT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn use_library(library: &Path) {
    LIBRARY_ROOT.with(|root| *root.borrow_mut() = Some(library.to_path_buf()));
}

fn library_factory(root: &Path) -> App {
    let library = LIBRARY_ROOT
        .with(|held| held.borrow().clone())
        .expect("library root");
    build(
        DataPlugin::new(root)
            .library("mod", library)
            .primordial("Actor"),
    )
}

const OGRE: &str = "guards/ogre/entity.jsonc";
const ELITE: &str = "guards/ogre_elite/entity.jsonc";

struct Fixture {
    _dir: tempfile::TempDir,
    project: AuthoringProject,
}

impl Fixture {
    /// `AuthoringProject` canonicalizes its root, so paths are compared through the session.
    fn path(&self, file: &str) -> PathBuf {
        self.project.session().root().join(file)
    }
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    for (file, text) in [
        (
            "Actor/entity.jsonc",
            r#"{"components":{"Health":{"current":50,"max":50}}}"#,
        ),
        (
            OGRE,
            "// The courtyard ogre.\n{\n  \"descendsFrom\": \"Actor\",\n  \"extensors\": [\"armor\"],\n  \"components\": { \"Health\": { \"max\": 60 } }\n}\n",
        ),
        (
            ELITE,
            r#"{"descendsFrom":"guards/ogre","components":{"Health":{"max":120}}}"#,
        ),
        (
            "boss/entity.jsonc",
            r#"{"descendsFrom":"Actor","grantsToWards":[{"to":"guards/ogre","components":{"Armor":{"rating":2}},"actions":["hurt"]}]}"#,
        ),
    ] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let project = AuthoringProject::open(dir.path(), factory).unwrap();
    Fixture { _dir: dir, project }
}

fn snapshot(project: &mut AuthoringProject, sources: &[(&str, &str)]) -> Value {
    let sources = sources
        .iter()
        .map(|(file, text)| (file.to_string(), text.to_string()))
        .collect();
    struction_language::analyze(project, &sources).unwrap()
}

fn definition<'a>(snapshot: &'a Value, path: &str) -> &'a Value {
    snapshot["definitions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["path"] == path)
        .unwrap_or_else(|| panic!("{path} missing from the snapshot"))
}

fn messages(snapshot: &Value) -> Vec<String> {
    snapshot["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["message"].as_str().unwrap().to_string())
        .collect()
}

/// Drives the JSONL transport the way an editor does: one line in, one line out.
fn serve(project: &mut AuthoringProject, requests: &[Value]) -> Vec<Value> {
    let input = requests
        .iter()
        .map(|r| format!("{r}\n"))
        .collect::<String>();
    let mut output = Vec::new();
    struction_language::serve(project, Cursor::new(input), &mut output).unwrap();
    String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn describes_the_protocol_before_analyzing() {
    let mut fixture = fixture();
    let responses = serve(
        &mut fixture.project,
        &[json!({ "id": 7, "command": { "op": "describe" } })],
    );
    assert_eq!(responses.len(), 1);
    assert_eq!(responses[0]["id"], 7);
    assert_eq!(responses[0]["ok"], true);
    assert_eq!(responses[0]["result"]["protocol_version"], 1);
    assert_eq!(
        responses[0]["result"]["commands"],
        json!(["describe", "analyze"])
    );
}

#[test]
fn answers_each_request_by_its_id() {
    let mut fixture = fixture();
    let responses = serve(
        &mut fixture.project,
        &[
            json!({ "id": "a", "command": { "op": "describe" } }),
            json!({ "id": 2, "command": { "op": "analyze" } }),
        ],
    );
    assert_eq!(responses[0]["id"], "a");
    assert_eq!(responses[1]["id"], 2);
    assert!(
        responses[1]["result"]["definitions"]
            .as_array()
            .unwrap()
            .len()
            >= 3
    );
}

#[test]
fn skips_blank_lines_and_keeps_serving_after_bad_input() {
    let mut fixture = fixture();
    let input = format!(
        "\n{}\nnot json\n{}\n",
        json!({ "id": 1, "command": { "op": "describe" } }),
        json!({ "id": 2, "command": { "op": "describe" } })
    );
    let mut output = Vec::new();
    struction_language::serve(&mut fixture.project, Cursor::new(input), &mut output).unwrap();
    let responses: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["ok"], true);
    assert_eq!(responses[1]["ok"], false);
    assert_eq!(responses[1]["id"], Value::Null);
    assert_eq!(responses[1]["error"]["code"], "invalid_request");
    assert_eq!(responses[2]["ok"], true);
}

#[test]
fn rejects_unknown_operations_and_paths_without_failing_the_session() {
    let mut fixture = fixture();
    let responses = serve(
        &mut fixture.project,
        &[
            json!({ "id": 1, "command": { "op": "write" } }),
            json!({ "id": 2, "command": { "op": "analyze", "sources": { "../escape.jsonc": "{}" } } }),
            json!({ "id": 3, "command": { "op": "describe" } }),
        ],
    );
    assert_eq!(responses[0]["ok"], false);
    assert_eq!(responses[0]["error"]["code"], "invalid_request");
    assert_eq!(responses[1]["ok"], false);
    assert_eq!(responses[1]["error"]["code"], "invalid_path");
    assert_eq!(responses[2]["ok"], true);
}

#[test]
fn tells_a_client_what_each_open_file_is() {
    let mut fixture = fixture();
    std::fs::create_dir_all(fixture.path("scenes")).unwrap();
    std::fs::write(fixture.path("scenes/yard.jsonc"), "{\n}\n").unwrap();
    let snapshot = snapshot(
        &mut fixture.project,
        &[
            (OGRE, "{\"descendsFrom\": \"Actor\"}"),
            ("presets/burning.jsonc", "{ \"components\": {} }"),
            ("scenes/yard.jsonc", "{ \"zones\": {} }"),
            // A JSON file in the project that the engine does not read.
            ("notes.jsonc", "{ \"todo\": \"later\" }"),
        ],
    );
    let file = |path: &str| {
        snapshot["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["path"] == path)
            .unwrap_or_else(|| panic!("{path} is not classified: {}", snapshot["files"]))
    };
    // The client's rule is the engine's rule, so it sends everything and asks here.
    assert_eq!(file(OGRE)["kind"], "definition");
    assert_eq!(file(OGRE)["name"], "guards/ogre");
    assert_eq!(file("presets/burning.jsonc")["kind"], "preset");
    assert_eq!(file("presets/burning.jsonc")["name"], "burning");
    assert_eq!(file("scenes/yard.jsonc")["kind"], "scene");
    assert_eq!(file("scenes/yard.jsonc")["name"], Value::Null);
    assert!(
        !snapshot["files"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["path"] == "notes.jsonc"),
        "a file nothing reads has no service: {}",
        snapshot["files"]
    );
}

#[test]
fn reports_the_prose_a_definition_describes_itself_with() {
    let mut fixture = fixture();
    let snapshot = snapshot(&mut fixture.project, &[]);
    assert_eq!(
        definition(&snapshot, "guards/ogre")["doc"],
        "The courtyard ogre."
    );
    assert_eq!(
        definition(&snapshot, "guards/ogre_elite")["doc"],
        Value::Null
    );
}

#[test]
fn reports_every_preset_with_the_file_that_holds_it() {
    let mut fixture = fixture();
    std::fs::create_dir_all(fixture.path("presets")).unwrap();
    std::fs::write(
        fixture.path("presets/burning.jsonc"),
        "{ \"components\": { \"Armor\": { \"rating\": 1 } } }",
    )
    .unwrap();
    let snapshot = snapshot(&mut fixture.project, &[]);
    let preset = snapshot["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "burning")
        .expect("the preset is reported");
    assert_eq!(
        preset["source"],
        fixture
            .path("presets/burning.jsonc")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(preset["library"], Value::Null);
    // A preset can be followed to its source, like a definition can.
    assert_eq!(
        snapshot["schema"]["properties"]["presets"]["items"]["enum"],
        json!(["burning"])
    );
}

#[test]
fn library_presets_report_the_library_that_holds_them() {
    let dir = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(library.path().join("presets")).unwrap();
    std::fs::write(
        library.path().join("presets/burning.jsonc"),
        r#"{"components":{"Armor":{"rating":1}}}"#,
    )
    .unwrap();
    use_library(library.path());
    let mut project = AuthoringProject::open(dir.path(), library_factory).unwrap();
    let snapshot = struction_language::analyze(&mut project, &BTreeMap::new()).unwrap();
    let preset = snapshot["presets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "burning")
        .expect("the library preset is reported");
    assert_eq!(preset["library"], "mod");
    assert_eq!(
        preset["source"],
        library
            .path()
            .join("presets/burning.jsonc")
            .to_string_lossy()
            .as_ref()
    );
}

#[test]
fn reports_resolved_definitions_with_their_sources() {
    let mut fixture = fixture();
    let snapshot = snapshot(&mut fixture.project, &[]);
    assert!(messages(&snapshot).is_empty(), "{:?}", messages(&snapshot));
    let ogre = definition(&snapshot, "guards/ogre");
    assert_eq!(
        ogre["source"],
        fixture.path(OGRE).to_string_lossy().as_ref()
    );
    assert_eq!(ogre["library"], Value::Null);
    assert_eq!(ogre["lineage"], json!(["Actor"]));
    // The primordial type comes first, so a chain reads from the root down to the definition.
    assert_eq!(
        definition(&snapshot, "guards/ogre_elite")["lineage"],
        json!(["Actor", "guards/ogre"])
    );
    assert!(
        ogre["components"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c.as_str().unwrap().ends_with("Health"))
    );
    assert_eq!(ogre["extensors"][0]["name"], "armor");
    assert_eq!(ogre["extensors"][0]["reason"]["named_by"], "guards/ogre");
    assert_eq!(ogre["extensors"][0]["supplied"], json!(["Armor"]));
    // Inferred from the component it owns, without being named.
    let living = ogre["extensors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "living")
        .unwrap();
    assert_eq!(living["reason"]["owns"], "Health");
}

#[test]
fn reports_the_registered_packages_and_actions() {
    let mut fixture = fixture();
    let snapshot = snapshot(&mut fixture.project, &[]);
    let armor = snapshot["extensors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "armor")
        .unwrap();
    assert_eq!(armor["opt_in"], true);
    assert_eq!(armor["doc"], "Protection that reduces incoming hits");
    assert_eq!(armor["requires"], json!(["living"]));
    // A contributed state carries what holds while it does, so a client explains the name.
    assert_eq!(
        armor["states"],
        json!([{ "name": "Guarded", "doc": "Standing behind the shield." }])
    );
    assert_eq!(
        armor["components"],
        json!([{ "name": "Armor", "type_path": Armor::type_path(), "supplied": true }])
    );
    let living = snapshot["extensors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "living")
        .unwrap();
    assert_eq!(living["opt_in"], false);
    let hurt = snapshot["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "hurt")
        .unwrap();
    assert_eq!(hurt["doc"], "Lose hit points");
    assert!(
        hurt["requires"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r.as_str().unwrap().ends_with("Health"))
    );
    assert_eq!(
        hurt["params"],
        json!([{ "name": "amount", "type": "float", "required": false, "default": 1.0 }])
    );
}

#[test]
fn schema_describes_the_definitions_and_components_in_the_snapshot() {
    let mut fixture = fixture();
    let snapshot = snapshot(&mut fixture.project, &[]);
    let parents = snapshot["schema"]["properties"]["descendsFrom"]["enum"]
        .as_array()
        .unwrap();
    assert!(parents.iter().any(|p| p == "guards/ogre"));
    assert!(parents.iter().any(|p| p == "Actor"));
    let components = &snapshot["schema"]["properties"]["components"];
    assert!(
        components["properties"]["Health"]["anyOf"][0]["$ref"]
            .as_str()
            .unwrap()
            .starts_with("#/$defs/")
    );
    assert_eq!(components["additionalProperties"], false);
}

#[test]
fn reports_problems_in_a_buffer_it_never_writes() {
    let mut fixture = fixture();
    let before = std::fs::read_to_string(fixture.path(OGRE)).unwrap();
    let broken = "{\n  \"descendsFrom\": \"Actor\",\n  \"extensors\": [\"armor\"],\n  \"components\": { \"Health\": { \"max\": 60 }, \"Nonsense\": {} }\n}\n";
    let snapshot = snapshot(&mut fixture.project, &[(OGRE, broken)]);
    let unknown = snapshot["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["file"] == OGRE)
        .unwrap_or_else(|| {
            panic!(
                "a mistyped component must be reported: {}",
                snapshot["diagnostics"]
            )
        });
    assert_eq!(unknown["message"], "unknown component \"Nonsense\"");
    assert_eq!(unknown["line"], 4);
    assert_eq!(unknown["column"], 44);
    assert_eq!(std::fs::read_to_string(fixture.path(OGRE)).unwrap(), before);
}

#[test]
fn resolves_definitions_from_buffers_before_they_are_saved() {
    let mut fixture = fixture();
    let before = std::fs::read_to_string(fixture.path(OGRE)).unwrap();
    let edited = "{\n  \"descendsFrom\": \"Actor\",\n  \"extensors\": [\"armor\"],\n  \"components\": { \"Health\": { \"max\": 99 } }\n}\n";
    let snapshot = snapshot(&mut fixture.project, &[(OGRE, edited)]);
    assert!(messages(&snapshot).is_empty(), "{:?}", messages(&snapshot));
    assert_eq!(
        definition(&snapshot, "guards/ogre")["resolved"]["components"]["Health"]["max"],
        99
    );
    assert_eq!(std::fs::read_to_string(fixture.path(OGRE)).unwrap(), before);
}

#[test]
fn a_buffer_replaces_disk_only_for_this_request() {
    let mut fixture = fixture();
    snapshot(
        &mut fixture.project,
        &[(OGRE, "{\n  \"descendsFrom\": \"Actor\"\n}\n")],
    );
    assert!(messages(&snapshot(&mut fixture.project, &[])).is_empty());
}

#[test]
fn a_buffer_can_hold_a_definition_that_is_not_on_disk() {
    let mut fixture = fixture();
    let fresh = "props/statue/entity.jsonc";
    let snapshot = snapshot(
        &mut fixture.project,
        &[(fresh, "{\n  \"descendsFrom\": \"Actor\"\n}\n")],
    );
    let statue = definition(&snapshot, "props/statue");
    assert_eq!(
        statue["source"],
        fixture.path(fresh).to_string_lossy().as_ref()
    );
    assert!(!fixture.path(fresh).exists());
}

#[test]
fn library_definitions_are_read_only_and_labeled() {
    let dir = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(library.path().join("creatures/ogre")).unwrap();
    std::fs::create_dir_all(library.path().join("creatures/troll")).unwrap();
    let ogre_source = r#"{"descendsFrom":"Actor"}"#;
    let troll_source = "{ \"descendsFrom\": }";
    std::fs::write(
        library.path().join("creatures/ogre/entity.jsonc"),
        ogre_source,
    )
    .unwrap();
    std::fs::write(
        library.path().join("creatures/troll/entity.jsonc"),
        troll_source,
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("creatures/ogre")).unwrap();
    std::fs::write(
        dir.path().join("creatures/ogre/entity.jsonc"),
        r#"{"descendsFrom":"Actor","components":{"Health":{"max":30}}}"#,
    )
    .unwrap();
    use_library(library.path());
    let mut project = AuthoringProject::open(dir.path(), library_factory).unwrap();
    let snapshot = struction_language::analyze(&mut project, &BTreeMap::new()).unwrap();

    // A project file overrides the library definition of the same path.
    let ogre = definition(&snapshot, "creatures/ogre");
    assert_eq!(ogre["library"], "mod");
    assert_eq!(
        ogre["source"],
        project
            .session()
            .root()
            .join("creatures/ogre/entity.jsonc")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(ogre["components"].as_array().unwrap().len(), 1);

    // A file that does not resolve is only reachable through its diagnostics, so editors can repair it.
    assert!(
        !snapshot["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["path"] == "creatures/troll")
    );

    // Library problems arrive as paths an editor can open, so a broken library file is repairable.
    let broken = snapshot["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .find(|d| d["message"].as_str().unwrap().contains("syntax error"))
        .unwrap_or_else(|| panic!("no library diagnostic in {}", snapshot["diagnostics"]));
    assert_eq!(
        broken["file"],
        library
            .path()
            .join("creatures/troll/entity.jsonc")
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(broken["line"], 1);

    // Buffers may shadow a library file for one request without writing to it.
    struction_language::analyze(
        &mut project,
        &BTreeMap::from([("creatures/ogre/entity.jsonc".into(), "{".into())]),
    )
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(library.path().join("creatures/ogre/entity.jsonc")).unwrap(),
        ogre_source
    );
    assert_eq!(
        std::fs::read_to_string(library.path().join("creatures/troll/entity.jsonc")).unwrap(),
        troll_source
    );
}

/// A scene file's grammar belongs to `struction_world::scene`; this keeps the advertised schema and
/// the parser that reads scenes from drifting apart.
#[test]
fn the_scene_schema_advertises_exactly_what_the_world_reads() {
    let fixture = fixture();
    let world = fixture.project.preview();
    let types = world.resource::<AppTypeRegistry>().read();
    let store = world.resource::<DefinitionStore>();
    let actions = world.resource::<ActionRegistry>();
    let definition_schema = store.schema(&types, actions);
    let scene = struction_language::scene_schema(store, &definition_schema);
    assert_eq!(
        scene["$defs"]["definition"], definition_schema,
        "overrides use the definition schema"
    );

    // The keys a `zones`, `spawnerList` or `spawns` entry accepts.
    let named = |entry: &Value| {
        entry["properties"]
            .as_object()
            .unwrap_or_else(|| panic!("an entry with named fields is expected: {entry}"))
            .keys()
            .cloned()
            .collect::<Vec<_>>()
    };
    let zones = &scene["properties"]["zones"]["additionalProperties"];
    let spawner = &scene["properties"]["spawnerList"]["additionalProperties"];
    let spawn = &spawner["properties"]["spawns"]["additionalProperties"];
    assert_eq!(named(zones), ["position", "rotation"]);
    assert_eq!(named(spawner), ["zone", "position", "rotation", "spawns"]);
    assert_eq!(
        named(spawn),
        ["definition", "offset", "rotation", "masterIs", "overrides"]
    );
    assert_eq!(spawn["required"], json!(["definition"]));
    assert_eq!(spawner["required"], json!(["zone", "position"]));
    assert!(
        spawn["properties"]["definition"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "Actor"),
        "a spawn lists the definitions it may name"
    );

    let reads = |scene: Value| {
        let source = serde_json::to_string(&scene).unwrap();
        let sources = BTreeMap::from([("scenes/courtyard.jsonc".to_string(), source)]);
        struction_world::SceneCatalog::load_with_sources(store.root(), store, &types, &sources)
            .errors()
            .to_vec()
    };
    let vector = json!([1, 2, 3]);
    let zone = |fields: Value| json!({ "zones": { "Court": fields } });
    let spawner = |fields: Value| {
        let mut guard = json!({ "zone": "Court", "position": vector });
        guard
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        json!({ "spawnerList": { "guards": guard } })
    };
    let spawn = |fields: Value| {
        json!({ "spawnerList": { "guards": { "zone": "Court", "position": vector,
            "spawns": { "ogre": fields } } } })
    };
    for (scene, what) in [
        (zone(json!({ "position": vector })), "zone position"),
        (zone(json!({ "rotation": vector })), "zone rotation"),
        (
            spawner(json!({})),
            "a spawner with only its required fields",
        ),
        (spawner(json!({ "rotation": vector })), "spawner rotation"),
        (
            spawner(json!({ "spawns": { "ogre": { "definition": "Actor" } } })),
            "spawner spawns",
        ),
        (
            spawn(json!({ "definition": "Actor", "offset": vector })),
            "spawn offset",
        ),
        (
            spawn(json!({ "definition": "Actor", "rotation": vector })),
            "spawn rotation",
        ),
        (
            spawn(json!({ "definition": "Actor", "masterIs": "Court/guards/ogre" })),
            "spawn masterIs",
        ),
        (
            spawn(
                json!({ "definition": "Actor", "overrides": { "components": { "Health": { "max": 3 } } } }),
            ),
            "spawn overrides",
        ),
    ] {
        let errors = reads(scene);
        assert!(
            errors.is_empty(),
            "{what} is advertised but rejected: {errors:?}"
        );
    }
    // Fields neither side knows about stay refused, and so does the derived one the engine removed.
    assert_eq!(reads(zone(json!({ "nonsense": 1 }))).len(), 1);
    assert_eq!(reads(spawner(json!({ "nonsense": 1 }))).len(), 1);
    assert_eq!(
        reads(spawn(json!({ "definition": "Actor", "nonsense": 1 }))).len(),
        1
    );
    assert_eq!(reads(spawner(json!({ "tile": [0, 0] }))).len(), 1);
    assert_eq!(reads(json!({ "nonsense": {} })).len(), 1);
}
