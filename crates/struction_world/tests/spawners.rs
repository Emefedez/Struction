mod common;

use std::collections::HashSet;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use common::*;
use struction_core::*;
use struction_world::*;

fn close(a: Vec3, b: Vec3) -> bool {
    a.distance(b) < 1e-4
}

#[test]
fn zones_spawners_and_named_spawns_are_identified_by_path() {
    let mut app = app(&fixture());
    // Before the first tick only zones and spawners exist; spawners are pending.
    assert!(instance_paths(&mut app).is_empty());
    for zone in ["Fortress", "Fortress/LeftCourtYard"] {
        assert!(has::<Zone>(&app, by_path(&mut app, zone)), "{zone}");
    }
    let guards = by_path(&mut app, GUARDS);
    assert!(!app.world().get::<Spawner>(guards).unwrap().has_run());

    step(&mut app);
    assert_no_errors(&mut app);
    assert_eq!(
        instance_paths(&mut app),
        [BOSS, BRUTE, FIREMAN1, FIREMAN2, PLAYER]
    );
    let fireman = by_path(&mut app, FIREMAN1);
    let definition = get::<Definition>(&app, fireman);
    assert_eq!(definition.path.as_str(), "minions/fireman");
    assert!(definition.descends_from(&"minions/ogre".into()));
    assert_eq!(get::<Name>(&app, fireman).as_str(), FIREMAN1);
    // Preset, inherited reactions and scene overrides all arrive through instantiate.
    assert!(has::<Flammable>(&app, fireman));
    assert_eq!(get::<Reactions>(&app, fireman).0.len(), 1);
    assert_eq!(get::<Health>(&app, by_path(&mut app, FIREMAN2)).current, 5.0);
    assert_eq!(get::<Health>(&app, fireman).current, 40.0);

    // Every instance has a stable id, and its spawner records it.
    let spawner = app.world().get::<Spawner>(guards).unwrap();
    assert!(spawner.has_run());
    let created: Vec<_> = spawner.created().map(|(name, id)| (name.to_owned(), id)).collect();
    assert_eq!(created.len(), 3);
    assert!(created.contains(&("fireman1".into(), id_of(&app, fireman))));
    assert_eq!(spawner.removed().count(), 0);
}

#[test]
fn offsets_are_rotated_by_the_spawner_in_zone_coordinates() {
    let mut app = started();
    // Zone at x = 10, spawner at [4, 0, 6] in the zone, turned 90 degrees to the left.
    let spawner = get::<Transform>(&app, by_path(&mut app, GUARDS));
    assert!(close(spawner.translation, Vec3::new(14.0, 0.0, 6.0)));

    let fireman = get::<Transform>(&app, by_path(&mut app, FIREMAN1));
    assert!(close(fireman.translation, Vec3::new(14.0, -0.2, 5.5)), "{fireman:?}");
    // The spawn faces where the spawner faces, and keeps its definition's scale.
    assert!(fireman.rotation.angle_between(spawner.rotation) < 1e-4);
    assert!(close(fireman.scale, Vec3::splat(1.5)));

    let brute = get::<Transform>(&app, by_path(&mut app, BRUTE));
    assert!(close(brute.translation, Vec3::new(16.0, 0.0, 6.0)), "{brute:?}");
    let boss = get::<Transform>(&app, by_path(&mut app, BOSS));
    assert!(close(boss.translation, Vec3::new(200.0, 0.0, 0.0)));
}

#[test]
fn cells_are_derived_from_positions_and_follow_movement() {
    let mut app = started();
    let boss = by_path(&mut app, BOSS);
    assert_eq!(get::<Cell>(&app, by_path(&mut app, GUARDS)).0, IVec3::ZERO);
    assert_eq!(get::<Cell>(&app, boss).0, IVec3::new(4, 0, 0));
    // The README offset dips 0.2 m below the spawner, into the cell below.
    assert_eq!(
        get::<Cell>(&app, by_path(&mut app, FIREMAN1)).0,
        IVec3::new(0, -1, 0)
    );

    app.world_mut().get_mut::<Transform>(boss).unwrap().translation.x = -30.0;
    step(&mut app);
    assert_eq!(get::<Cell>(&app, boss).0, IVec3::new(-1, 0, 0));

    // Another cell size re-derives every cell from the same authored positions.
    let mut small = app_with(
        &fixture(),
        Options {
            cell_size: 10.0,
            ..default()
        },
    );
    step(&mut small);
    assert_eq!(get::<Cell>(&small, by_path(&mut small, GUARDS)).0, IVec3::new(1, 0, 0));
    assert_eq!(get::<Cell>(&small, by_path(&mut small, BOSS)).0, IVec3::new(20, 0, 0));
}

#[test]
fn master_is_resolves_by_path_and_applies_grants() {
    let mut app = started();
    let boss = by_path(&mut app, BOSS);
    for ward in [FIREMAN1, FIREMAN2, BRUTE] {
        let ward = by_path(&mut app, ward);
        assert_eq!(master_of(&app, ward), Some(boss));
        assert!(!has::<PendingMaster>(&app, ward));
        assert_eq!(get_emboldened(&app, ward), Some(2.0));
        assert!(app.world().get::<ActionSet>(ward).unwrap().contains(&"rally".into()));
    }
    assert_eq!(app.world().get::<Wards>(boss).unwrap().len(), 3);
    assert_eq!(master_of(&app, by_path(&mut app, PLAYER)), None);
}

fn get_emboldened(app: &App, entity: Entity) -> Option<f32> {
    app.world().get::<Emboldened>(entity).map(|e| e.bonus)
}

#[test]
fn the_master_may_spawn_later() {
    let mut app = app(&fixture());
    // The keep's cell is not loaded: its spawner waits, the courtyard spawns first.
    assert_eq!(unload_cell(app.world_mut(), IVec3::new(4, 0, 0)), 1);
    step(&mut app);
    assert_no_errors(&mut app);
    assert!(find(&mut app, BOSS).is_none());
    let fireman = by_path(&mut app, FIREMAN1);
    assert_eq!(
        get_pending(&app, fireman),
        Some(EntityRef::Path(EntityPath::new(BOSS)))
    );
    assert_eq!(master_of(&app, fireman), None);

    load_cell(app.world_mut(), IVec3::new(4, 0, 0));
    step(&mut app);
    let boss = by_path(&mut app, BOSS);
    assert_eq!(master_of(&app, fireman), Some(boss));
    assert_eq!(get_pending(&app, fireman), None);
    assert_eq!(get_emboldened(&app, fireman), Some(2.0));
}

fn get_pending(app: &App, entity: Entity) -> Option<EntityRef> {
    app.world().get::<PendingMaster>(entity).map(|p| p.0.clone())
}

#[test]
fn spawned_entities_move_independently() {
    let mut app = started();
    let guards = by_path(&mut app, GUARDS);
    let fireman1 = by_path(&mut app, FIREMAN1);
    let fireman2 = by_path(&mut app, FIREMAN2);
    let before = get::<Transform>(&app, fireman2);

    app.world_mut().get_mut::<Transform>(guards).unwrap().translation += Vec3::X * 5.0;
    app.world_mut().get_mut::<Transform>(fireman1).unwrap().translation += Vec3::Y * 5.0;
    step(&mut app);

    assert_eq!(get::<Transform>(&app, fireman2), before);
    assert!(app.world().get::<ChildOf>(fireman1).is_none());
}

#[test]
fn despawning_an_instance_records_its_removal() {
    let mut app = started();
    let fireman2 = by_path(&mut app, FIREMAN2);
    app.world_mut().entity_mut(fireman2).despawn();
    let spawner = app.world().get::<Spawner>(by_path(&mut app, GUARDS)).unwrap();
    assert_eq!(spawner.removed().collect::<Vec<_>>(), ["fireman2"]);
    assert!(spawner.created().all(|(name, _)| name != "fireman2"));
    // The spawner already ran; nothing comes back.
    step(&mut app);
    assert!(find(&mut app, FIREMAN2).is_none());
}

#[test]
fn unloading_is_not_removal() {
    let mut app = started();
    let boss = by_path(&mut app, BOSS);
    unload_cell(app.world_mut(), IVec3::new(4, 0, 0));
    step(&mut app);
    assert!(has::<Disabled>(&app, boss));
    let keep = app.world().get::<Spawner>(by_path(&mut app, "Fortress/Keep")).unwrap();
    assert_eq!(keep.removed().count(), 0);
    assert_eq!(master_of(&app, by_path(&mut app, BRUTE)), Some(boss));
}

#[test]
fn ids_come_from_the_seeded_generator() {
    let ids = |seed| {
        let mut app = app_with(
            &fixture(),
            Options {
                seed,
                ..default()
            },
        );
        step(&mut app);
        let mut ids: Vec<_> = [BOSS, FIREMAN1, PLAYER]
            .map(|path| id_of(&app, by_path(&mut app, path)))
            .into();
        let runtime = spawn_runtime(app.world_mut(), "minions/ogre", Transform::IDENTITY).unwrap();
        assert!(has::<RuntimeCreated>(&app, runtime));
        assert!(app.world().get::<EntityPath>(runtime).is_none());
        ids.push(id_of(&app, runtime));
        ids
    };
    let a = ids(1);
    assert_eq!(a, ids(1));
    assert_ne!(a, ids(2));
    assert_eq!(a.iter().collect::<HashSet<_>>().len(), a.len());
    assert!(a.iter().all(|id| id.0.get_version_num() == 4));
}

#[test]
fn scene_errors_point_at_the_source() {
    let dir = fixture_copy();
    std::fs::write(
        dir.path().join("scenes/broken.jsonc"),
        r#"{
  "spawnerList": {
    "tiled": { "zone": "Fortress", "tile": [0, 2, 3], "position": [0, 0, 0] },
    "odd": {
      "zone": "Fortress",
      "position": [1, 2],
      "spawns": {}
    },
    "camp": {
      "zone": "Fortress/Camp",
      "position": [0, 0, 0],
      "spawns": {
        "ghost": { "definition": "minions/ghost" },
        "loyal": { "definition": "minions/ogre", "masterIs": "Fortress/Keep/nobody" },
        "hurt": { "definition": "minions/ogre", "overrides": { "components": { "Health": { "hp": 1 } } } },
        "lost": { "definition": "minions/ogre", "speed": 3 }
      }
    }
  },
  "props": {}
}"#,
    )
    .unwrap();
    let mut app = app(dir.path());
    let errors: Vec<String> = app
        .world_mut()
        .resource_mut::<WorldErrors>()
        .drain()
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(
        errors,
        [
            "scenes/broken.jsonc:3: invalid value for spawner: \"tile\" is derived from the position; remove it",
            "scenes/broken.jsonc:6: expected array of 3 numbers, found array",
            "scenes/broken.jsonc:16: unknown field \"speed\" in spawn",
            "scenes/broken.jsonc:19: unknown field \"props\" in scene file",
            "scenes/broken.jsonc:13: \"definition\" refers to missing definition \"minions/ghost\"",
            "scenes/broken.jsonc:14: invalid value for masterIs: no spawn has the path \"Fortress/Keep/nobody\"",
            "scenes/broken.jsonc:15: unknown field \"hp\" in Health",
        ]
    );
    // The valid parts of the project still spawn.
    step(&mut app);
    assert!(find(&mut app, BOSS).is_some());
    assert!(find(&mut app, "Fortress/Camp/camp/ghost").is_none());
}
