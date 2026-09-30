//! Blender-free pipeline tests: glTF sources compiled, loaded in Bevy, and
//! recompiled by the source watcher.

mod common;

use std::time::{Duration, Instant};

use bevy::asset::AssetPath;
use bevy::prelude::*;
use struction_assets::compile::MANIFEST_NAME;
use struction_assets::format::to_native;
use struction_assets::{
    AssetError, CompileSettings, CompileStatus, CompiledModel, MappedBundle, SourceRecompiled,
    SourceWatcher, SourceWatcherPlugin, compile_asset, compile_asset_with, lod_label,
};

use common::*;

#[test]
fn gltf_compiles_incrementally() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("ball.gltf");
    let out = dir.path().join("out/ball.smesh");
    write_gltf(&source, &[sphere(32, 16, 1.0)], [0.0; 3]);

    assert_eq!(
        compile_asset(&source, &out).unwrap(),
        CompileStatus::Compiled
    );
    assert!(dir.path().join("out").join(MANIFEST_NAME).is_file());
    assert_eq!(
        compile_asset(&source, &out).unwrap(),
        CompileStatus::UpToDate
    );

    // Settings are part of the key.
    let mut settings = CompileSettings::default();
    settings.prepare.lod.levels = 1;
    assert_eq!(
        compile_asset_with(&source, &out, &settings).unwrap(),
        CompileStatus::Compiled
    );
    assert_eq!(
        compile_asset_with(&source, &out, &settings).unwrap(),
        CompileStatus::UpToDate
    );
    assert_eq!(
        MappedBundle::open(&out).unwrap().bundle().meshes[0]
            .lods
            .len(),
        2
    );

    // Touching without changing content stays up to date; editing does not.
    std::fs::write(&source, std::fs::read(&source).unwrap()).unwrap();
    assert_eq!(
        compile_asset_with(&source, &out, &settings).unwrap(),
        CompileStatus::UpToDate
    );
    write_gltf(&source, &[sphere(16, 8, 1.0)], [0.0; 3]);
    assert_eq!(
        compile_asset_with(&source, &out, &settings).unwrap(),
        CompileStatus::Compiled
    );

    // A missing output is rebuilt; `force` always rebuilds.
    std::fs::remove_file(&out).unwrap();
    assert_eq!(
        compile_asset_with(&source, &out, &settings).unwrap(),
        CompileStatus::Compiled
    );
    settings.force = true;
    assert_eq!(
        compile_asset_with(&source, &out, &settings).unwrap(),
        CompileStatus::Compiled
    );
}

#[test]
fn compiled_bundle_matches_the_source() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("ball.gltf");
    let out = dir.path().join("ball.smesh");
    let input = sphere(32, 16, 1.0);
    write_gltf(&source, std::slice::from_ref(&input), [1.0, 2.0, 3.0]);
    compile_asset(&source, &out).unwrap();

    let mapped = MappedBundle::open(&out).unwrap();
    let bundle = mapped.bundle();
    let lod0 = &bundle.meshes[0].lods[0];
    // Read in place from the memory map, no parsing of the vertex arrays.
    assert!(
        mapped
            .bytes()
            .as_ptr_range()
            .contains(&lod0.positions.as_ptr().cast())
    );
    assert_eq!(to_native(&lod0.positions), input.positions);
    assert_eq!(to_native(&lod0.normals), input.normals);
    assert_eq!(to_native(&lod0.uvs), input.uvs);
    assert_eq!(bundle.meshes[0].material.as_deref(), Some("Metal"));
    assert_eq!(
        bundle.nodes[0].translation.map(|c| c.to_native()),
        [1.0, 2.0, 3.0]
    );
    assert!(bundle.meshes[0].lods.len() > 1);
    assert!(bundle.meshes[0].collision.hull.is_some());
}

#[test]
fn unsupported_and_broken_sources_fail_clearly() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("x.smesh");

    let text = dir.path().join("notes.txt");
    std::fs::write(&text, "hi").unwrap();
    let error = compile_asset(&text, &out).unwrap_err();
    assert!(
        matches!(error, AssetError::UnsupportedSource { .. }),
        "{error}"
    );

    let broken = dir.path().join("broken.gltf");
    std::fs::write(&broken, "{ not json").unwrap();
    let error = compile_asset(&broken, &out).unwrap_err();
    assert!(matches!(error, AssetError::Gltf { .. }), "{error}");
    assert!(error.to_string().starts_with(&broken.display().to_string()));

    let missing = dir.path().join("missing.glb");
    assert!(matches!(
        compile_asset(&missing, &out).unwrap_err(),
        AssetError::Io { .. }
    ));
}

#[test]
fn bevy_loads_meshes_lods_and_collision() {
    let dir = tempfile::tempdir().unwrap();
    let input = sphere(32, 16, 1.0);
    write_gltf(
        &dir.path().join("ball.gltf"),
        std::slice::from_ref(&input),
        [0.0; 3],
    );
    compile_asset(
        &dir.path().join("ball.gltf"),
        &dir.path().join("ball.smesh"),
    )
    .unwrap();

    let mut app = asset_app(dir.path());
    let handle: Handle<CompiledModel> = app.world().resource::<AssetServer>().load("ball.smesh");
    assert!(wait_loaded(&mut app, &handle).is_loaded());

    let models = app.world().resource::<Assets<CompiledModel>>();
    let meshes = app.world().resource::<Assets<Mesh>>();
    let model = models.get(&handle).unwrap();
    let mesh = &model.meshes[0];
    assert_eq!(mesh.name, "Sphere");
    assert_eq!(mesh.lods.len(), mesh.lod_errors.len());
    let vertex_counts: Vec<usize> = mesh
        .lods
        .iter()
        .map(|lod| meshes.get(lod).unwrap().count_vertices())
        .collect();
    assert!(
        vertex_counts.windows(2).all(|pair| pair[1] < pair[0]),
        "{vertex_counts:?}"
    );
    let lod0 = meshes.get(&mesh.lods[0]).unwrap();
    assert_eq!(mesh_positions(lod0), input.positions);
    assert_eq!(
        lod0.indices()
            .unwrap()
            .iter()
            .map(|i| i as u32)
            .collect::<Vec<_>>(),
        input.indices
    );
    assert!(mesh.collision.hull.as_ref().unwrap().points.len() > 4);
    assert!(!mesh.collision.trimesh.triangles.is_empty());

    // LODs are addressable by label.
    let path = format!("ball.smesh#{}", lod_label(0, 1));
    let lod1: Handle<Mesh> = app.world().resource::<AssetServer>().load(path);
    assert_eq!(lod1, mesh.lods[1]);
}

#[test]
fn bevy_rejects_corrupt_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("junk.smesh"),
        b"STRMESH\0\x01\0\0\0\0\0\0\0garbage",
    )
    .unwrap();
    let mut app = asset_app(dir.path());
    let handle: Handle<CompiledModel> = app.world().resource::<AssetServer>().load("junk.smesh");
    assert!(wait_loaded(&mut app, &handle).is_failed());
}

/// Updates until the watcher reports a recompile of the source.
fn wait_recompiled(app: &mut App) -> SourceRecompiled {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        app.update();
        let messages = app.world().resource::<Messages<SourceRecompiled>>();
        if let Some(message) = messages.iter_current_update_messages().last() {
            return message.clone();
        }
        assert!(Instant::now() < deadline, "no recompile reported");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn watcher_recompiles_and_hot_reloads_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("ball.gltf");
    let out = dir.path().join("ball.smesh");
    write_gltf(&source, &[sphere(32, 16, 1.0)], [0.0; 3]);
    compile_asset(&source, &out).unwrap();

    let mut app = asset_app(dir.path());
    app.add_plugins(SourceWatcherPlugin);
    let handle: Handle<CompiledModel> = app.world().resource::<AssetServer>().load("ball.smesh");
    assert!(wait_loaded(&mut app, &handle).is_loaded());
    let mut watcher = app.world_mut().resource_mut::<SourceWatcher>();
    watcher.poll_interval = Duration::ZERO;
    watcher.watch(&source, &out, Some(AssetPath::from("ball.smesh")));

    // Nothing happens until the source changes.
    for _ in 0..5 {
        app.update();
    }
    assert!(!app.world().resource::<SourceWatcher>().is_busy());
    assert_eq!(
        app.world().resource::<Messages<SourceRecompiled>>().len(),
        0
    );

    // "Save" a coarser sphere, as the external application would.
    write_gltf(&source, &[sphere(8, 4, 1.0)], [0.0; 3]);
    let message = wait_recompiled(&mut app);
    assert_eq!(message.source, source);
    assert_eq!(message.result, Ok(CompileStatus::Compiled));

    // The reload system picks up the new file.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        let models = app.world().resource::<Assets<CompiledModel>>();
        let meshes = app.world().resource::<Assets<Mesh>>();
        let vertices = models
            .get(&handle)
            .and_then(|model| meshes.get(&model.meshes[0].lods[0]))
            .map(Mesh::count_vertices);
        if vertices == Some(9 * 5) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "model was not reloaded ({vertices:?})"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    // A broken save reports the error instead of replacing the output.
    std::fs::write(&source, "{ broken").unwrap();
    let message = wait_recompiled(&mut app);
    assert!(message.result.unwrap_err().contains("invalid glTF"));
    assert!(MappedBundle::open(&out).is_ok());
}
