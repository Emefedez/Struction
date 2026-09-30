mod common;

use bevy::prelude::*;
use common::*;
use struction_core::*;
use struction_world::*;

#[test]
fn definition_rename_preserves_comments_rewrites_references_and_loads_old_saves() {
    let dir = fixture_copy();
    let mut before = app(dir.path());
    step(&mut before);
    let save = save_world(before.world_mut());
    let report = rename_path(dir.path(), "minions/ogre", "creatures/brute").unwrap();
    assert_eq!(
        report.moved,
        [("minions/ogre".into(), "creatures/brute".into())]
    );
    assert_eq!(
        report.renamed_actions,
        [("minions/ogre/die".into(), "creatures/brute/die".into())]
    );
    let text = std::fs::read_to_string(dir.path().join("creatures/brute/entity.jsonc")).unwrap();
    assert!(text.contains("// A big dumb brute."));
    assert!(text.contains("// After my master's `die` completes"));
    let mut after = app_with(
        dir.path(),
        Options {
            ogre: "creatures/brute",
            ..default()
        },
    );
    let report = load_world(after.world_mut(), &save).unwrap();
    assert_eq!(report, LoadReport::default());
    assert_no_errors(&mut after);
    let brute = by_path(&after, BRUTE);
    assert_eq!(
        get::<Definition>(&after, brute).path.as_str(),
        "creatures/brute"
    );
    assert!(has::<Emboldened>(&after, brute));
    assert_eq!(
        save_world(after.world_mut())
            .entities
            .iter()
            .map(|e| &e.id)
            .collect::<Vec<_>>(),
        save.entities.iter().map(|e| &e.id).collect::<Vec<_>>()
    );
}

#[test]
fn renaming_live_and_removed_spawns_keeps_their_save_bookkeeping() {
    for removed in [false, true] {
        let dir = fixture_copy();
        let mut before = app(dir.path());
        step(&mut before);
        let fireman = by_path(&before, FIREMAN1);
        let id = id_of(&before, fireman);
        if removed {
            before.world_mut().entity_mut(fireman).despawn();
        }
        let save = save_world(before.world_mut());
        let renamed = format!("{GUARDS}/torchbearer");
        rename_path(dir.path(), FIREMAN1, &renamed).unwrap();
        let mut after = app(dir.path());
        assert_eq!(
            load_world(after.world_mut(), &save).unwrap(),
            LoadReport::default()
        );
        step(&mut after);
        assert_no_errors(&mut after);
        assert!(find(&after, FIREMAN1).is_none());
        if removed {
            assert!(find(&after, &renamed).is_none());
        } else {
            assert_eq!(id_of(&after, by_path(&after, &renamed)), id);
        }
    }
}

#[test]
fn zone_and_spawner_renames_preserve_master_references_in_old_saves() {
    let dir = fixture_copy();
    let mut before = app(dir.path());
    step(&mut before);
    let save = save_world(before.world_mut());
    rename_path(dir.path(), "Fortress", "Citadel").unwrap();
    rename_path(dir.path(), "Citadel/Keep", "Citadel/Tower").unwrap();
    let mut after = app(dir.path());
    assert_eq!(
        load_world(after.world_mut(), &save).unwrap(),
        LoadReport::default()
    );
    assert_no_errors(&mut after);
    let boss = by_path(&after, "Citadel/Tower/ogre_lord");
    let ward = by_path(&after, "Citadel/LeftCourtYard/courtyard_guards/fireman1");
    assert_eq!(master_of(&after, ward), Some(boss));
}

#[test]
fn colliding_spawn_rename_is_rejected_without_writing_sources() {
    let dir = fixture_copy();
    let file = dir.path().join("scenes/fortress.jsonc");
    let before = std::fs::read_to_string(&file).unwrap();
    assert!(matches!(
        rename_path(dir.path(), FIREMAN1, FIREMAN2),
        Err(RenameError::Exists(_))
    ));
    assert_eq!(std::fs::read_to_string(file).unwrap(), before);
    assert!(!dir.path().join("aliases.jsonc").exists());
}

#[test]
fn invalid_paths_are_rejected_before_sources_are_changed() {
    let dir = fixture_copy();
    for new in [
        "../escape",
        "minions/../escape",
        "minions/./escape",
        "/absolute",
        "minions//escape",
    ] {
        assert!(
            matches!(
                rename_path(dir.path(), "minions/ogre", new),
                Err(RenameError::InvalidPath(_))
            ),
            "{new}"
        );
    }
    assert!(dir.path().join("minions/ogre/entity.jsonc").is_file());
    assert!(!dir.path().join("aliases.jsonc").exists());
}

#[test]
fn alias_chains_and_renaming_back_resolve_to_the_current_path() {
    let dir = fixture_copy();
    rename_path(dir.path(), "minions/ogre", "minions/brute").unwrap();
    rename_path(dir.path(), "minions/brute", "minions/giant").unwrap();
    let aliases = PathAliases::load(dir.path()).unwrap();
    assert_eq!(aliases.resolve("minions/ogre/die"), "minions/giant/die");
    assert_eq!(aliases.resolve("minions/ogreish"), "minions/ogreish");
    rename_path(dir.path(), "minions/giant", "minions/ogre").unwrap();
    let aliases = PathAliases::load(dir.path()).unwrap();
    assert_eq!(aliases.resolve("minions/brute/die"), "minions/ogre/die");
    assert_eq!(aliases.resolve("minions/ogre/die"), "minions/ogre/die");
}
