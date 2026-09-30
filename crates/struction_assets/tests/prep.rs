//! Collision/LOD tool sessions and recipes, on Blender-free glTF sources.

mod common;

use std::time::{Duration, Instant};

use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use struction_assets::compile::PrepareSettings;
use struction_assets::recipe::recipe_json;
use struction_assets::{
    AssetError, Blender, CollisionPreset, CompileStatus, LodPreset, MappedBundle, PrepSession,
    SourceRecompiled, SourceWatcher, SourceWatcherPlugin, compile_asset, read_recipe, recipe_path,
};

use common::*;

fn ball(dir: &std::path::Path) -> std::path::PathBuf {
    let source = dir.join("ball.gltf");
    write_gltf(&source, &[sphere(48, 24, 1.0)], [0.0; 3]);
    source
}

fn lod_count(out: &std::path::Path) -> usize {
    MappedBundle::open(out).unwrap().bundle().meshes[0]
        .lods
        .len()
}

fn with_lods(preset: LodPreset) -> PrepareSettings {
    PrepareSettings {
        lod: preset.settings(),
        ..PrepareSettings::default()
    }
}

#[test]
fn preview_apply_undo_redo() {
    let dir = tempfile::tempdir().unwrap();
    let source = ball(dir.path());
    let out = dir.path().join("ball.smesh");
    let mut session = PrepSession::open(&source, Blender::default()).unwrap();
    assert!(!session.has_recipe());
    assert_eq!(session.applied(), &PrepareSettings::default());

    // A preview reports the result without touching any file.
    let settings = with_lods(LodPreset::Gentle);
    let preview = session.preview(&settings).unwrap();
    let report = &preview.meshes[0];
    assert_eq!(report.lods.len(), 3);
    assert_eq!(report.lods[0].share, 1.0);
    assert!(report.lods[2].triangles < report.lods[1].triangles);
    assert!(report.hull.is_some());
    assert!(report.trimesh.triangles < report.lods[0].triangles);
    assert!(!out.exists() && !recipe_path(&source).exists());

    session.apply(&preview, "Gentle LODs").unwrap();
    assert_eq!(read_recipe(&source).unwrap(), Some(settings.clone()));
    assert_eq!(lod_count(&out), 3);
    // The tool's output is what a compile would produce: nothing to redo.
    assert_eq!(
        compile_asset(&source, &out).unwrap(),
        CompileStatus::UpToDate
    );
    assert_eq!(session.undo_label(), Some("Gentle LODs"));

    session
        .apply_settings(&with_lods(LodPreset::Off), "No LODs")
        .unwrap();
    assert_eq!(lod_count(&out), 1);

    session.undo().unwrap().unwrap();
    assert_eq!(session.applied(), &settings);
    assert_eq!(lod_count(&out), 3);
    // Undoing the first apply removes the recipe again: defaults, four levels.
    session.undo().unwrap().unwrap();
    assert!(!recipe_path(&source).exists());
    assert_eq!(lod_count(&out), 4);
    assert!(session.undo().unwrap().is_none());

    session.redo().unwrap().unwrap();
    assert_eq!(read_recipe(&source).unwrap(), Some(settings));
    assert_eq!(session.redo_label(), Some("No LODs"));

    // Re-applying the settings in effect records nothing.
    session
        .apply_settings(&with_lods(LodPreset::Gentle), "Again")
        .unwrap();
    assert_eq!(session.undo_label(), Some("Gentle LODs"));
}

#[test]
fn collision_presets_change_the_shapes() {
    let dir = tempfile::tempdir().unwrap();
    let session = PrepSession::open(ball(dir.path()), Blender::default()).unwrap();
    let preview = |preset: CollisionPreset| {
        let settings = PrepareSettings {
            collision: preset.settings(),
            ..PrepareSettings::default()
        };
        session.preview(&settings).unwrap().meshes.remove(0)
    };
    let simple = preview(CollisionPreset::SimpleProp);
    let scenery = preview(CollisionPreset::Scenery);
    assert!(simple.trimesh.triangles < scenery.trimesh.triangles);
    assert!(simple.parts.is_empty());
    let concave = preview(CollisionPreset::ConcaveProp);
    assert!(!concave.parts.is_empty());
    assert!(concave.parts.len() <= CollisionPreset::ConcaveProp.settings().max_parts as usize);
}

#[test]
fn invalid_settings_are_refused_with_the_field() {
    let dir = tempfile::tempdir().unwrap();
    let mut session = PrepSession::open(ball(dir.path()), Blender::default()).unwrap();
    let mut settings = PrepareSettings::default();
    settings.collision.trimesh_ratio = 0.0;
    let error = session
        .apply_settings(&settings, "Broken")
        .unwrap_err()
        .to_string();
    assert!(error.contains("collision.trimeshRatio"), "{error}");
    assert!(!recipe_path(session.source()).exists());
}

#[test]
fn outside_recipe_edits_are_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let source = ball(dir.path());
    let mut session = PrepSession::open(&source, Blender::default()).unwrap();
    session
        .apply_settings(&with_lods(LodPreset::Gentle), "Gentle LODs")
        .unwrap();

    // Another client (an AI tool, a text editor) writes the recipe.
    let theirs = with_lods(LodPreset::Aggressive);
    std::fs::write(recipe_path(&source), recipe_json(&theirs)).unwrap();

    let error = session
        .apply_settings(&with_lods(LodPreset::Off), "No LODs")
        .unwrap_err();
    assert!(matches!(error, AssetError::Changed { .. }), "{error}");
    assert_eq!(read_recipe(&source).unwrap(), Some(theirs.clone()));
    assert!(session.undo_label().is_none(), "history was dropped");
    assert!(matches!(
        session.undo().unwrap_err(),
        AssetError::Changed { .. }
    ));

    let refreshed = session.refresh().unwrap();
    assert!(refreshed.recipe_changed && !refreshed.reimported);
    assert_eq!(session.applied(), &theirs);
    session
        .apply_settings(&with_lods(LodPreset::Off), "No LODs")
        .unwrap();
}

#[test]
fn source_saves_are_reimported_and_failures_keep_the_last_import() {
    let dir = tempfile::tempdir().unwrap();
    let source = ball(dir.path());
    let mut session = PrepSession::open(&source, Blender::default()).unwrap();
    let stale = session.preview(&PrepareSettings::default()).unwrap();

    // Different content and size, so the stamp changes even within a timestamp tick.
    write_gltf(&source, &[sphere(12, 6, 1.0)], [0.0; 3]);
    assert!(session.is_source_stale());
    let error = session.apply(&stale, "Apply").unwrap_err();
    assert!(matches!(error, AssetError::Changed { .. }), "{error}");

    assert!(session.refresh().unwrap().reimported);
    assert_eq!(session.scene().meshes[0].positions.len(), 13 * 7);
    // A preview of the previous import cannot be applied over the new one.
    let error = session.apply(&stale, "Apply").unwrap_err();
    assert!(error.to_string().contains("preview again"), "{error}");

    std::fs::write(&source, "{ broken").unwrap();
    let error = session.refresh().unwrap_err();
    assert!(error.to_string().contains("invalid glTF"), "{error}");
    assert_eq!(session.scene().meshes[0].positions.len(), 13 * 7);
    assert!(session.preview(&PrepareSettings::default()).is_ok());
}

#[test]
fn compiles_and_watchers_follow_the_recipe() {
    let dir = tempfile::tempdir().unwrap();
    let source = ball(dir.path());
    let out = dir.path().join("ball.smesh");
    std::fs::write(recipe_path(&source), r#"{ "lod": { "levels": 1 } }"#).unwrap();
    assert_eq!(
        compile_asset(&source, &out).unwrap(),
        CompileStatus::Compiled
    );
    assert_eq!(lod_count(&out), 2);

    std::fs::write(recipe_path(&source), r#"{ "lod": { "levls": 1 } }"#).unwrap();
    let error = compile_asset(&source, &out).unwrap_err().to_string();
    assert!(
        error.contains("ball.gltf.recipe.json") && error.contains("levls"),
        "{error}"
    );

    // Saving only the recipe recompiles a watched source.
    std::fs::write(recipe_path(&source), r#"{ "lod": { "levels": 1 } }"#).unwrap();
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, SourceWatcherPlugin));
    let mut watcher = app.world_mut().resource_mut::<SourceWatcher>();
    watcher.poll_interval = Duration::ZERO;
    watcher.watch(&source, &out, None);
    let mut cursor: MessageCursor<SourceRecompiled> = app
        .world()
        .resource::<Messages<SourceRecompiled>>()
        .get_cursor();
    for _ in 0..3 {
        app.update();
    }
    std::fs::write(recipe_path(&source), r#"{ "lod": { "levels": 2 } }"#).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let message = loop {
        app.update();
        let messages = app.world().resource::<Messages<SourceRecompiled>>();
        if let Some(message) = cursor.read(messages).last() {
            break message.clone();
        }
        assert!(Instant::now() < deadline, "no recompile reported");
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(message.result, Ok(CompileStatus::Compiled));
    assert_eq!(lod_count(&out), 3);
}
