use std::path::Path;

use bevy::prelude::*;
use serde_json::json;
use struction_core::*;
use struction_data::{DataPlugin, DefinitionStore};
use struction_editor::*;
use struction_world::*;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health {
    current: f32,
    max: f32,
}
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Bonus {
    amount: f32,
}

fn factory(root: &Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        CorePlugin { seed: 7 },
        DataPlugin::new(root).primordial("Actor"),
        WorldPlugin::default(),
    ))
    .register_type::<Health>()
    .register_type::<Bonus>()
    .register_action(
        ActionMeta::new("hurt"),
        |In(call): In<ActionCall>, mut health: Query<&mut Health>| {
            health.get_mut(call.target).unwrap().current -= 1.0;
        },
    );
    app.add_systems(
        FixedUpdate,
        (|mut health: Query<&mut Health>| {
            for mut h in &mut health {
                h.current -= 0.5;
            }
        })
        .in_set(CoreSet::Invoke),
    );
    app
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Armor {
    rating: f32,
}

fn armored_factory(root: &Path) -> App {
    let mut app = factory(root);
    app.register_type::<Armor>()
        .register_extensor(ExtensorMeta::inferred("living").owns::<Health>())
        .register_extensor(ExtensorMeta::opt_in("armor").supplies::<Armor>());
    app
}

const GUARD: &str = "guards/ogre/entity.jsonc";
const SCENE: &str = "scenes/courtyard.jsonc";
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (file, text) in [
        (
            "Actor/entity.jsonc",
            r#"{"components":{"Health":{"current":50,"max":50}}}"#,
        ),
        (
            GUARD,
            "// Ogre guard.\n{\n  \"descendsFrom\": \"Actor\",\n  \"components\": { \"Health\": { \"max\": 60 } }\n}\n",
        ),
        (
            "boss/entity.jsonc",
            r#"{"descendsFrom":"Actor","grantsToWards":[{"to":"guards/ogre","components":{"Bonus":{"amount":2}},"actions":["hurt"]}]}"#,
        ),
        (
            SCENE,
            r#"{"zones":{"Court":{"position":[10,0,0]}},"spawnerList":{"guards":{"zone":"Court","position":[4,0,6],"rotation":[0,90,0],"spawns":{"ogre":{"definition":"guards/ogre","offset":[1,0,0],"masterIs":"Court/guards/boss"},"boss":{"definition":"boss"}}}}}"#,
        ),
    ] {
        let path = dir.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}
fn set(file: &str, path: &[&str], value: serde_json::Value) -> EditRequest {
    EditRequest::Set {
        file: file.into(),
        path: path.iter().map(|key| Field::Key((*key).into())).collect(),
        value,
        label: "Edit".into(),
        group: None,
        revision: None,
    }
}
fn health(project: &AuthoringProject, play: bool) -> f32 {
    let world = if play {
        project.play_world().unwrap()
    } else {
        project.preview()
    };
    let mut query = world.try_query::<(&EntityPath, &Health)>().unwrap();
    query
        .iter(world)
        .find(|(path, _)| path.as_str() == "Court/guards/ogre")
        .unwrap()
        .1
        .current
}

#[test]
fn authoring_loop_validates_inheritance_and_preserves_exact_undo() {
    let dir = fixture();
    let original = std::fs::read_to_string(dir.path().join(GUARD)).unwrap();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    assert!(project.validate().is_empty());
    assert!(project.schema().to_string().contains("Health"));
    assert_eq!(project.actions().len(), 1);
    assert_eq!(health(&project, false), 50.0);
    let inspected = project.inspect_definition("guards/ogre").unwrap();
    let health_of = &inspected.components[Health::type_path()];
    assert_eq!(health_of["max"], 60.0);
    assert_eq!(health_of["current"], 50.0);
    project
        .edit(set(GUARD, &["components", "Health", "current"], json!(30)))
        .unwrap();
    assert_eq!(health(&project, false), 30.0);
    assert!(
        std::fs::read_to_string(dir.path().join(GUARD))
            .unwrap()
            .contains("// Ogre guard.")
    );
    project.undo().unwrap();
    assert_eq!(health(&project, false), 50.0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join(GUARD)).unwrap(),
        original
    );
    project.redo().unwrap();
    assert_eq!(health(&project, false), 30.0);
}

#[test]
fn bad_types_and_scene_references_never_reach_disk_or_history() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    for request in [
        set(GUARD, &["components", "Health", "current"], json!("bad")),
        set(
            SCENE,
            &["spawnerList", "guards", "spawns", "ogre", "masterIs"],
            json!("missing"),
        ),
    ] {
        assert!(matches!(
            project.edit(request),
            Err(SessionError::Validation(_))
        ));
        assert!(!project.session().history().can_undo());
        assert!(project.validate().is_empty());
    }
    assert_eq!(health(&project, false), 50.0);
}

#[test]
fn preview_rebuild_applies_new_grants_to_existing_authored_wards() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    let bonus = |p: &AuthoringProject| {
        let mut query = p.preview().try_query::<&Bonus>().unwrap();
        query.single(p.preview()).unwrap().amount
    };
    assert_eq!(bonus(&project), 2.0);
    let mut request = set("boss/entity.jsonc", &[], json!(9));
    if let EditRequest::Set { path, .. } = &mut request {
        *path = vec![
            Field::Key("grantsToWards".into()),
            Field::Index(0),
            Field::Key("components".into()),
            Field::Key("Bonus".into()),
            Field::Key("amount".into()),
        ];
    }
    project.edit(request).unwrap();
    assert_eq!(bonus(&project), 9.0);
    project.undo().unwrap();
    assert_eq!(bonus(&project), 2.0);
}

#[test]
fn play_is_a_separate_world_and_cannot_write_sources_or_history() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    project
        .edit(set(GUARD, &["components", "Health", "current"], json!(30)))
        .unwrap();
    let text = std::fs::read_to_string(dir.path().join(GUARD)).unwrap();
    project.start_play().unwrap();
    assert_eq!(health(&project, true), 30.0);
    project.step_play(4).unwrap();
    assert_eq!(health(&project, true), 28.0);
    assert_eq!(health(&project, false), 30.0);
    assert!(matches!(
        project.edit(set(GUARD, &["components", "Health", "current"], json!(1))),
        Err(SessionError::Playing)
    ));
    assert!(matches!(
        project.create_definition("guards/new", "Actor"),
        Err(SessionError::Playing)
    ));
    assert!(matches!(project.undo(), Err(SessionError::Playing)));
    project.stop_play();
    assert!(project.play_world().is_none());
    assert_eq!(
        std::fs::read_to_string(dir.path().join(GUARD)).unwrap(),
        text
    );
    project.undo().unwrap();
    assert_eq!(health(&project, false), 50.0);
}

#[test]
fn broken_external_edits_keep_the_last_good_preview_and_report_sources() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    project
        .edit(set(GUARD, &["components", "Health", "current"], json!(30)))
        .unwrap();
    std::fs::write(dir.path().join(GUARD), "{ broken").unwrap();
    assert!(project.refresh().is_err());
    assert_eq!(health(&project, false), 30.0);
    assert!(!project.session().history().can_undo());
    let errors = project.validate();
    let source_error = errors
        .iter()
        .find(|e| e.file.as_deref() == Some(GUARD))
        .unwrap();
    assert_eq!(source_error.line, Some(1));
    assert!(project.start_play().is_err());
}

#[test]
fn templates_are_validated_and_never_overwrite_existing_files() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    assert!(project.create_definition("guards/new", "missing").is_err());
    assert!(!dir.path().join("guards/new/entity.jsonc").exists());
    project
        .create_definition("guards/new", "guards/ogre")
        .unwrap();
    assert!(project.definitions().contains(&"guards/new".into()));
    assert!(project.create_definition("guards/new", "Actor").is_err());
    assert!(
        project
            .preview()
            .resource::<DefinitionStore>()
            .get("guards/new")
            .unwrap()
            .lineage
            .contains(&"guards/ogre".into())
    );
}

#[test]
fn an_invalid_project_can_open_for_inspection_and_repair() {
    let dir = fixture();
    std::fs::write(
        dir.path().join(GUARD),
        r#"{"descendsFrom":"Actor","components":{"Health":{"current":"wrong"}}}"#,
    )
    .unwrap();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    assert!(project.definitions().contains(&"guards/ogre".to_owned()));
    assert!(matches!(
        project.inspect_definition("guards/ogre"),
        Err(SessionError::Validation(_))
    ));
    let tree = project.definition_hierarchy();
    assert!(
        tree[0]
            .children
            .iter()
            .any(|node| node.key == "guards/ogre")
    );
    assert!(!project.validate().is_empty());
    assert!(project.start_play().is_err());
    project
        .edit(set(GUARD, &["components", "Health", "current"], json!(15)))
        .unwrap();
    assert!(project.validate().is_empty());
    assert_eq!(health(&project, false), 15.0);
}

#[test]
fn gizmo_moves_account_for_all_parent_frames_and_keep_identity() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    project
        .edit(set(GUARD, &["transform", "translation"], json!([2, 1, 0])))
        .unwrap();
    project
        .edit(set(
            SCENE,
            &["zones", "Court", "rotation"],
            json!([0, 30, 0]),
        ))
        .unwrap();
    project
        .edit(set(
            SCENE,
            &["spawnerList", "guards", "spawns", "ogre", "rotation"],
            json!([0, 45, 0]),
        ))
        .unwrap();
    let original = std::fs::read_to_string(dir.path().join(SCENE)).unwrap();
    let path = "Court/guards/ogre";
    let before = project.inspect_entity(path, false).unwrap();
    for position in [[3.0, 4.0, 5.0], [8.0, 7.0, 6.0]] {
        project
            .move_spawn(path, Vec3::from_array(position), Some("drag".into()))
            .unwrap();
        let after = project.inspect_entity(path, false).unwrap();
        assert_eq!(after.entity.stable_id, before.entity.stable_id);
        let actual = after.entity.position.unwrap();
        assert!(actual.distance(Vec3::from_array(position)) < 1e-4);
    }
    project.end_group();
    project.undo().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(SCENE)).unwrap(),
        original
    );
    project.redo().unwrap();
    assert!(project.move_spawn(path, Vec3::NAN, None).is_err());
}

#[test]
fn command_stream_survives_bad_requests_and_reports_runtime_state() {
    use struction_editor::protocol::serve;
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    let requests = [
        "{bad".to_string(),
        json!({"id":1,"command":{"op":"describe"}}).to_string(),
        json!({"id":2,"command":{"op":"edit","edit":{"op":"set","file":GUARD,"path":["components","Health","current"],"value":"bad","label":"Bad"}}}).to_string(),
        json!({"id":3,"command":{"op":"start_play"}}).to_string(),
        json!({"id":4,"command":{"op":"step_play","ticks":2}}).to_string(),
        json!({"id":5,"command":{"op":"inspect_entity","target":"Court/guards/ogre","playing":true}}).to_string(),
        json!({"id":6,"command":{"op":"edit","edit":{"op":"set","file":GUARD,"path":["components","Health","current"],"value":20,"label":"Blocked"}}}).to_string(),
        json!({"id":7,"command":{"op":"step_play","ticks":10001}}).to_string(),
        json!({"id":8,"command":{"op":"stop_play"}}).to_string(),
        json!({"id":9,"command":{"op":"entities","playing":true}}).to_string(),
        json!({"id":10,"command":{"op":"validate","typo":true}}).to_string(),
        json!({"id":11,"command":{"op":"actions"}}).to_string(),
        json!({"id":12,"command":{"op":"entities"}}).to_string(),
    ].join("\n");
    let mut output = Vec::new();
    serve(&mut project, requests.as_bytes(), &mut output).unwrap();
    let replies: Vec<serde_json::Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 13);
    assert_eq!(replies[0]["error"]["code"], "invalid_request");
    assert_eq!(replies[1]["result"]["protocol_version"], 1);
    assert_eq!(replies[2]["error"]["code"], "validation");
    assert!(
        !replies[2]["error"]["diagnostics"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        replies[5]["result"]["components"][Health::type_path()]["current"],
        49.0
    );
    assert_eq!(replies[6]["error"]["code"], "playing");
    assert_eq!(replies[7]["error"]["code"], "invalid_operation");
    assert_eq!(replies[9]["error"]["code"], "invalid_operation");
    assert_eq!(replies[10]["id"], 10);
    assert_eq!(replies[10]["error"]["code"], "invalid_request");
    assert_eq!(replies[11]["result"][0]["name"], "hurt");
    let ogre = replies[12]["result"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entity| entity["path"] == "Court/guards/ogre")
        .unwrap();
    assert!(ogre["entity"].is_string());
    assert_eq!(ogre["definition"], "guards/ogre");
    assert_eq!(ogre["master"], "Court/guards/boss");
    assert_eq!(
        ogre["source"]["path"],
        json!(["spawnerList", "guards", "spawns", "ogre"])
    );
    assert_eq!(ogre["position"].as_array().unwrap().len(), 3);
    assert_eq!(ogre["rotation"].as_array().unwrap().len(), 4);
    assert_eq!(health(&project, false), 50.0);
}

#[test]
fn sparse_projects_and_bad_hosts_are_handled_without_query_panics() {
    let dir = tempfile::tempdir().unwrap();
    assert!(AuthoringProject::open(dir.path(), |_| App::new()).is_err());
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    assert!(project.entities(false).unwrap().is_empty());
    project.create_definition("guard", "Actor").unwrap();
    assert!(project.create_definition("presets/guard", "Actor").is_err());
    assert!(project.create_definition("scenes/guard", "Actor").is_err());
    assert!(!dir.path().join("presets/guard/entity.jsonc").exists());
}

#[test]
fn hierarchy_reports_authored_sources_and_disabled_play_entities() {
    use bevy::ecs::entity_disabling::Disabled;
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), |root| {
        let mut app = factory(root);
        app.add_systems(
            FixedUpdate,
            (|mut commands: Commands, entities: Query<Entity, With<Health>>| {
                for entity in &entities {
                    commands.entity(entity).insert(Disabled);
                }
            })
            .after(CoreSet::Invoke),
        );
        app
    })
    .unwrap();
    let before = project.inspect_entity("Court/guards/ogre", false).unwrap();
    let source = before.entity.source.unwrap();
    assert_eq!(source.file, SCENE);
    assert_eq!(source.path, ["spawnerList", "guards", "spawns", "ogre"]);
    project.start_play().unwrap();
    project.step_play(1).unwrap();
    let after = project.inspect_entity("Court/guards/ogre", true).unwrap();
    assert!(after.entity.disabled);
    assert_eq!(after.components[Health::type_path()]["current"], 49.5);
}

#[test]
fn master_tree_creation_and_reparenting_are_undoable() {
    let dir = fixture();
    let original = std::fs::read_to_string(dir.path().join(SCENE)).unwrap();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    let tree = project.master_hierarchy(false).unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].key, "Court/guards/boss");
    assert_eq!(tree[0].children[0].key, "Court/guards/ogre");
    let definitions = project.definition_hierarchy();
    assert_eq!(definitions[0].key, "Actor");
    assert_eq!(definitions[0].children.len(), 2);

    project
        .create_spawn(
            "Court/guards",
            "new_guard",
            "guards/ogre",
            Some("Court/guards/boss"),
            Vec3::X,
        )
        .unwrap();
    assert_eq!(
        project.master_hierarchy(false).unwrap()[0].children.len(),
        2
    );
    let created = std::fs::read_to_string(dir.path().join(SCENE)).unwrap();
    project
        .set_master("Court/guards/new_guard", Some("Court/guards/ogre"))
        .unwrap();
    let tree = project.master_hierarchy(false).unwrap();
    assert_eq!(
        tree[0].children[0].children[0].key,
        "Court/guards/new_guard"
    );
    project.undo().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(SCENE)).unwrap(),
        created
    );
    project.undo().unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.path().join(SCENE)).unwrap(),
        original
    );
    project.redo().unwrap();
    assert_eq!(
        project.master_hierarchy(false).unwrap()[0].children.len(),
        2
    );
    project.set_master("Court/guards/new_guard", None).unwrap();
    assert_eq!(project.master_hierarchy(false).unwrap().len(), 2);
}

#[test]
fn invalid_relationship_edits_do_not_write_or_record_history() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    let original = std::fs::read_to_string(dir.path().join(SCENE)).unwrap();
    assert!(
        project
            .edit(set(
                SCENE,
                &["spawnerList", "guards", "spawns", "boss", "masterIs"],
                json!("Court/guards/ogre")
            ))
            .is_err()
    );
    for (path, master) in [
        ("Court/guards/boss", "Court/guards/ogre"),
        ("Court/guards/boss", "Court/guards/boss"),
        ("Court/guards/ogre", "missing"),
    ] {
        assert!(project.set_master(path, Some(master)).is_err());
    }
    for (spawner, name, definition) in [
        ("Court/guards", "ogre", "guards/ogre"),
        ("Court/guards", "new", "missing"),
        ("Court/guards", "bad/name", "guards/ogre"),
        ("missing", "new", "guards/ogre"),
    ] {
        assert!(
            project
                .create_spawn(spawner, name, definition, None, Vec3::ZERO)
                .is_err()
        );
    }
    assert_eq!(
        std::fs::read_to_string(dir.path().join(SCENE)).unwrap(),
        original
    );
    assert!(!project.session().history().can_undo());
    project.start_play().unwrap();
    assert!(matches!(
        project.set_master("Court/guards/ogre", None),
        Err(SessionError::Playing)
    ));
}

#[test]
fn protocol_exposes_relationship_authoring() {
    use struction_editor::protocol::{Request, execute};
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), factory).unwrap();
    for command in [
        json!({"op":"master_hierarchy"}),
        json!({"op":"definition_hierarchy"}),
        json!({"op":"create_spawn", "spawner":"Court/guards", "name":"new", "definition":"guards/ogre", "master":"Court/guards/boss"}),
        json!({"op":"set_master", "path":"Court/guards/new", "master":null}),
    ] {
        let request: Request = serde_json::from_value(json!({"id":1,"command":command})).unwrap();
        let response = execute(&mut project, request);
        assert!(response.ok, "{response:?}");
    }
}

#[test]
fn definitions_explain_their_extensors() {
    let dir = fixture();
    let mut project = AuthoringProject::open(dir.path(), armored_factory).unwrap();
    assert!(
        project
            .inspect_definition("guards/ogre")
            .unwrap()
            .extensors
            .iter()
            .all(|e| e.name != "armor")
    );
    // Opting in is an ordinary, undoable edit of the `extensors` section.
    project
        .edit(set(GUARD, &["extensors"], json!(["armor"])))
        .unwrap();
    assert!(
        std::fs::read_to_string(dir.path().join(GUARD))
            .unwrap()
            .contains("// Ogre guard.")
    );
    let inspected = project.inspect_definition("guards/ogre").unwrap();
    assert_eq!(
        inspected.extensors,
        [
            ExtensorEntry {
                name: "armor".into(),
                reason: ExtensorWhy::NamedBy("guards/ogre".into()),
                components: vec!["Armor".into()],
                supplied: vec!["Armor".into()],
            },
            ExtensorEntry {
                name: "living".into(),
                reason: ExtensorWhy::Owns("Health".into()),
                components: vec!["Health".into()],
                supplied: vec![],
            },
        ]
    );
    assert!(inspected.components.contains_key(Armor::type_path()));
    let actor = project.inspect_definition("Actor").unwrap();
    assert_eq!(actor.available_extensors, ["armor"]);
    assert_eq!(
        serde_json::to_value(&inspected.extensors[0].reason).unwrap(),
        json!({ "named_by": "guards/ogre" })
    );
    project.undo().unwrap();
    assert!(
        project
            .inspect_definition("guards/ogre")
            .unwrap()
            .extensors
            .iter()
            .all(|e| e.name != "armor")
    );
}
