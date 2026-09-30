mod common;

use std::path::Path;

use bevy::prelude::*;
use common::*;
use struction_world::*;

fn live(root: &Path) -> App {
    let mut app = app_with(
        root,
        Options {
            live: true,
            ..default()
        },
    );
    step(&mut app);
    assert_no_errors(&mut app);
    app
}

fn edit(root: &Path, file: &str, from: &str, to: &str) {
    let path = root.join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{file} has no {from}");
    std::fs::write(path, text.replace(from, to)).unwrap();
}

const FIREMAN: &str = "minions/fireman/entity.jsonc";
const OGRE: &str = "minions/ogre/entity.jsonc";
const SCENE: &str = "scenes/fortress.jsonc";

#[test]
fn definition_edits_rewrite_only_changed_fields() {
    let dir = fixture_copy();
    let mut app = live(dir.path());
    let fireman = by_path(&app, FIREMAN1);
    let wounded = by_path(&app, FIREMAN2);
    let brute = by_path(&app, BRUTE);
    app.world_mut().get_mut::<Health>(fireman).unwrap().current = 12.0;
    app.world_mut().entity_mut(fireman).insert(Scratch(3));

    edit(dir.path(), FIREMAN, r#""max": 40.0"#, r#""max": 80.0"#);
    let report = reload_sources(app.world_mut(), &[FIREMAN.into()]);
    assert_no_errors(&mut app);

    assert_eq!(report.definitions, vec!["minions/fireman"]);
    assert_eq!(report.refreshed, 2);
    assert_eq!(
        get::<Health>(&app, fireman),
        Health {
            current: 12.0,
            max: 80.0
        }
    );
    assert_eq!(get::<Health>(&app, wounded).current, 5.0);
    assert_eq!(get::<Health>(&app, wounded).max, 80.0);
    assert_eq!(get::<Health>(&app, brute).max, 60.0);
    assert!(has::<Scratch>(&app, fireman));
}

#[test]
fn inherited_transform_and_removed_components_reach_live_descendants() {
    let dir = fixture_copy();
    let mut app = live(dir.path());
    let fireman = by_path(&app, FIREMAN1);
    let moved = Vec3::new(40.0, 1.0, -3.0);
    app.world_mut()
        .get_mut::<Transform>(fireman)
        .unwrap()
        .translation = moved;

    edit(dir.path(), OGRE, "[1.5, 1.5, 1.5]", "[2.0, 2.0, 2.0]");
    edit(dir.path(), FIREMAN, r#""presets": ["flammable"],"#, "");
    let report = reload_sources(app.world_mut(), &[OGRE.into(), FIREMAN.into()]);
    assert_no_errors(&mut app);

    assert_eq!(report.refreshed, 3, "{report:?}");
    let transform = get::<Transform>(&app, fireman);
    assert!(transform.translation.abs_diff_eq(moved, 1e-4));
    assert!(transform.scale.abs_diff_eq(Vec3::splat(2.0), 1e-4));
    assert!(!has::<Flammable>(&app, fireman));
    assert!(
        get::<Transform>(&app, by_path(&app, BRUTE))
            .scale
            .abs_diff_eq(Vec3::splat(2.0), 1e-4)
    );
}

#[test]
fn scene_edits_move_add_and_remove_spawns() {
    let dir = fixture_copy();
    let mut app = live(dir.path());
    let fireman = by_path(&app, FIREMAN1);
    let brute = by_path(&app, BRUTE);
    let before = get::<Transform>(&app, brute);
    app.world_mut().get_mut::<Health>(brute).unwrap().current = 7.0;

    edit(
        dir.path(),
        SCENE,
        r#""offset": [0, 0, 2]"#,
        r#""offset": [0, 0, 5]"#,
    );
    edit(
        dir.path(),
        SCENE,
        r#""fireman2": {"#,
        r#""fireman3": { "definition": "minions/fireman" },
        "fireman2_gone": {"#,
    );
    let report = reload_sources(app.world_mut(), &[SCENE.into()]);
    assert_no_errors(&mut app);

    assert_eq!(report.moved, 1);
    assert_eq!(report.despawned, 1);
    assert_eq!(report.restart_needed.len(), 2, "{report:?}");
    assert!(find(&app, FIREMAN2).is_none());
    let after = get::<Transform>(&app, brute);
    // The spawner is rotated 90° about y, so the extra 3 m of offset z lands on world x.
    assert!(
        (after.translation - before.translation).abs_diff_eq(Vec3::new(3.0, 0.0, 0.0), 1e-4),
        "{before:?} -> {after:?}"
    );
    assert_eq!(get::<Health>(&app, brute).current, 7.0);
    assert_eq!(by_path(&app, FIREMAN1), fireman);
}

#[test]
fn the_plugin_applies_saved_files_between_ticks() {
    let dir = fixture_copy();
    let mut app = live(dir.path());
    // `App::update` would run `Startup` again; the poll alone is what matters here.
    app.world_mut().run_schedule(First);
    let fireman = by_path(&app, FIREMAN1);

    edit(dir.path(), FIREMAN, r#""max": 40.0"#, r#""max": 45.5"#);
    app.world_mut().run_schedule(First);

    assert_eq!(get::<Health>(&app, fireman).max, 45.5);
    let reloads = app.world().resource::<Messages<LiveReloaded>>();
    assert_eq!(reloads.len(), 1);
}
