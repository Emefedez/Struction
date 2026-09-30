mod common;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use common::*;
use serde_json::json;
use struction_core::*;
use struction_world::EntityRef;
use struction_world::*;

#[test]
fn saves_restore_state_identity_relations_and_removals() {
    let mut app = started();
    let fireman = by_path(&app, FIREMAN1);
    let removed = by_path(&app, FIREMAN2);
    let brute = by_path(&app, BRUTE);
    let player = by_path(&app, PLAYER);
    let id = id_of(&app, fireman);
    app.world_mut().get_mut::<Health>(fireman).unwrap().current = 12.0;
    let pose = Transform::from_xyz(31.0, 2.0, -17.0).with_scale(Vec3::splat(2.0));
    app.world_mut()
        .entity_mut(fireman)
        .insert((pose, Scratch(42)));
    app.world_mut().entity_mut(brute).insert(MasterIs(player));
    app.world_mut().entity_mut(removed).despawn();
    app.world_mut().flush();
    unload_cell(app.world_mut(), IVec3::new(4, 0, 0));
    let runtime = spawn_runtime(app.world_mut(), "minions/ogre", Transform::IDENTITY).unwrap();
    let runtime_id = id_of(&app, runtime);
    app.world_mut().entity_mut(runtime).insert(MasterIs(player));
    app.world_mut().flush();
    let save = save_world(app.world_mut());
    assert_eq!(SaveData::from_json(&save.to_json()).unwrap(), save);
    let saved = save
        .entities
        .iter()
        .find(|e| e.id == id.to_string())
        .unwrap();
    assert!(!saved.components.contains_key(Emboldened::type_path()));
    assert!(!saved.components.contains_key(Scratch::type_path()));

    let report = load_world(app.world_mut(), &save).unwrap();
    assert_eq!(report, LoadReport::default());
    step(&mut app);
    assert_no_errors(&mut app);
    let fireman = by_path(&app, FIREMAN1);
    let boss = by_path(&app, BOSS);
    assert_eq!(id_of(&app, fireman), id);
    assert_eq!(get::<Health>(&app, fireman).current, 12.0);
    assert_eq!(get::<Transform>(&app, fireman), pose);
    assert!(!has::<Scratch>(&app, fireman));
    assert!(has::<Emboldened>(&app, fireman));
    assert_eq!(master_of(&app, fireman), Some(boss));
    assert!(has::<Disabled>(&app, boss));
    assert!(find(&app, FIREMAN2).is_none());
    let brute = by_path(&app, BRUTE);
    let player = by_path(&app, PLAYER);
    assert_eq!(master_of(&app, brute), Some(player));
    assert!(has::<Follower>(&app, brute));
    assert!(!has::<Emboldened>(&app, brute));
    let runtime = find_entity(app.world_mut(), &EntityRef::Id(runtime_id)).unwrap();
    assert_eq!(master_of(&app, runtime), Some(player));
    assert!(has::<RuntimeCreated>(&app, runtime));
    let next = spawn_runtime(app.world_mut(), "minions/ogre", Transform::IDENTITY).unwrap();
    assert!(
        save.entities
            .iter()
            .all(|e| e.id != id_of(&app, next).to_string())
    );
}

#[test]
fn runtime_masters_and_missing_persistent_components_round_trip() {
    let mut app = started();
    let master = spawn_runtime(app.world_mut(), "player", Transform::IDENTITY).unwrap();
    let id = id_of(&app, master);
    let ward = by_path(&app, FIREMAN1);
    app.world_mut()
        .entity_mut(ward)
        .insert(MasterIs(master))
        .remove::<Health>();
    app.world_mut().flush();
    let save = save_world(app.world_mut());
    load_world(app.world_mut(), &save).unwrap();
    let ward = by_path(&app, FIREMAN1);
    let master = find_entity(app.world_mut(), &EntityRef::Id(id)).unwrap();
    assert_eq!(master_of(&app, ward), Some(master));
    assert!(!has::<Health>(&app, ward));
    assert!(has::<Follower>(&app, ward));
}

#[test]
fn invalid_saves_leave_the_live_world_untouched() {
    let mut app = started();
    let boss = by_path(&app, BOSS);
    let original = save_world(app.world_mut());
    for case in 0..7 {
        let mut save = original.clone();
        match case {
            0 => save.version += 1,
            1 => save.entities[0].id = "broken".into(),
            2 => save.entities[0].definition = "missing".into(),
            3 => {
                save.entities[0]
                    .components
                    .insert("missing::Component".into(), json!({}));
            }
            4 => {
                save.entities[0]
                    .components
                    .insert(Health::type_path().into(), json!({"hp": 5}));
            }
            5 => save.entities.push(save.entities[0].clone()),
            6 => {
                save.spawners.insert(PLAYER.into(), Default::default());
            }
            _ => unreachable!(),
        }
        assert!(load_world(app.world_mut(), &save).is_err(), "case {case}");
        assert_eq!(by_path(&app, BOSS), boss);
        let after = save_world(app.world_mut());
        assert_eq!(after.entities, original.entities);
        assert_eq!(after.spawners, original.spawners);
    }
    assert!(matches!(
        SaveData::from_json(r#"{"version":999}"#),
        Err(SaveError::UnsupportedVersion { .. })
    ));
}

#[test]
fn boss_encounter_survives_save_load_and_keeps_death_distinct_from_removal() {
    let mut app = started();
    let boss = by_path(&app, BOSS);
    let player = by_path(&app, PLAYER);
    let adopted = by_path(&app, FIREMAN1);
    app.world_mut().entity_mut(adopted).insert(MasterIs(player));
    app.world_mut().flush();
    notify_wards(app.world_mut(), player, "fetch", ActionArgs::new());
    step(&mut app);
    assert_eq!(
        app.world().resource::<Log>().0,
        [format!("fetch@{FIREMAN1}")]
    );
    invoke_action(
        app.world_mut(),
        "bosses/ogre_lord/die",
        boss,
        ActionArgs::new(),
    );
    step(&mut app);
    assert_no_errors(&mut app);
    for path in [BOSS, FIREMAN2, BRUTE] {
        assert!(has::<Dead>(&app, by_path(&app, path)), "{path}");
    }
    assert!(!has::<Dead>(&app, adopted));
    let save = save_world(app.world_mut());
    load_world(app.world_mut(), &save).unwrap();
    assert!(has::<Dead>(&app, by_path(&app, BOSS)));
    assert!(has::<Dead>(&app, by_path(&app, BRUTE)));
    assert!(!has::<Dead>(&app, by_path(&app, FIREMAN1)));
    let boss = by_path(&app, BOSS);
    app.world_mut().entity_mut(boss).despawn();
    app.world_mut().flush();
    assert_eq!(master_of(&app, by_path(&app, BRUTE)), None);
    assert!(!has::<Emboldened>(&app, by_path(&app, BRUTE)));
    let save = save_world(app.world_mut());
    load_world(app.world_mut(), &save).unwrap();
    step(&mut app);
    assert!(find(&app, BOSS).is_none());
    assert_eq!(master_of(&app, by_path(&app, BRUTE)), None);
}

#[test]
fn removed_source_spawns_are_dropped_without_leaving_stale_save_records() {
    let dir = fixture_copy();
    let mut before = app(dir.path());
    step(&mut before);
    let id = id_of(&before, by_path(&before, FIREMAN1));
    let save = save_world(before.world_mut());
    let scene = dir.path().join("scenes/fortress.jsonc");
    let source = std::fs::read_to_string(&scene).unwrap();
    let edited = struction_data::edit::remove_value(
        &source,
        &[
            "spawnerList".into(),
            "courtyard_guards".into(),
            "spawns".into(),
            "fireman1".into(),
        ],
    )
    .unwrap();
    std::fs::write(scene, edited.text).unwrap();
    let mut after = app(dir.path());
    let report = load_world(after.world_mut(), &save).unwrap();
    assert_eq!(report.dropped, [format!("instance {id}")]);
    assert!(report.errors.is_empty());
    let save = save_world(after.world_mut());
    assert!(!save.spawners[GUARDS].created.contains_key("fireman1"));
    assert_eq!(
        load_world(after.world_mut(), &save).unwrap(),
        LoadReport::default()
    );
}
