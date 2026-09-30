//! Tests that run headless Blender. Each one is skipped, with a message, when
//! Blender is not installed.

mod common;

use std::path::Path;

use bevy::prelude::*;
use struction_assets::format::{Bounds, to_native};
use struction_assets::{
    AssetError, BlenderError, CompileStatus, CompiledModel, MappedBundle, PreparedScene,
    compile_asset, import_source,
};

use common::*;

fn mesh<'a>(scene: &'a PreparedScene, name: &str) -> &'a struction_assets::PreparedMesh {
    scene
        .meshes
        .iter()
        .find(|mesh| mesh.name == name)
        .expect(name)
}

fn node_translation(scene: &PreparedScene, name: &str) -> [f32; 3] {
    scene
        .nodes
        .iter()
        .find(|node| node.name == name)
        .expect(name)
        .translation
}

fn assert_close(actual: [f32; 3], expected: [f32; 3]) {
    for axis in 0..3 {
        assert!(
            (actual[axis] - expected[axis]).abs() < 1e-4,
            "{actual:?} != {expected:?}"
        );
    }
}

#[test]
fn blend_import_converts_to_engine_axes() {
    let Some(blender) = blender_or_skip("blend_import_converts_to_engine_axes") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let blend = make_test_blend(&blender, dir.path());
    let scene = import_source(&blend, &blender, true).unwrap();

    // Blender (x, y, z) -> engine (x, z, -y): Z-up becomes Y-up, Blender +Y is -Z.
    assert_close(node_translation(&scene, "Tower"), [1.0, 3.0, -2.0]);
    assert_close(node_translation(&scene, "Ball"), [-3.0, 1.0, 0.0]);

    // The 2 x 1 x 4 box (4 m tall along Blender Z) is 4 m tall along engine Y.
    let tower = mesh(&scene, "Tower");
    let bounds = Bounds::of(&tower.positions);
    assert_close(bounds.min, [-1.0, -2.0, -0.5]);
    assert_close(bounds.max, [1.0, 2.0, 0.5]);
    assert_eq!(tower.material.as_deref(), Some("Stone"));
    assert_eq!(tower.indices.len(), 36);
    // The tower had no UV map: Smart UV Project made one, inside the unit square.
    assert_eq!(tower.uvs.len(), tower.positions.len());
    assert!(
        tower
            .uvs
            .iter()
            .flatten()
            .all(|c| (-1e-4..=1.0 + 1e-4).contains(c))
    );
    for normal in &tower.normals {
        let length = normal.iter().map(|c| c * c).sum::<f32>().sqrt();
        assert!((length - 1.0).abs() < 1e-3);
    }

    let ball = mesh(&scene, "Ball");
    assert_eq!(ball.material.as_deref(), Some("Metal"));
    let metal = scene
        .materials
        .iter()
        .find(|material| material.name == "Metal")
        .expect("Metal material");
    for (actual, expected) in metal.base_color.iter().zip([0.8, 0.1, 0.1, 1.0]) {
        assert!((actual - expected).abs() < 1e-4, "{metal:?}");
    }
    assert!((metal.metallic - 1.0).abs() < 1e-4 && (metal.roughness - 0.25).abs() < 1e-4);
    assert_eq!(ball.indices.len() / 3, 32 * 14 * 2 + 32 * 2);
    assert!(ball.positions.iter().all(|p| {
        let radius = p.iter().map(|c| c * c).sum::<f32>().sqrt();
        (radius - 1.0).abs() < 1e-3
    }));
}

#[test]
fn blender_export_is_deterministic() {
    let Some(blender) = blender_or_skip("blender_export_is_deterministic") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let blend = make_test_blend(&blender, dir.path());
    let [a, b] = ["a.glb", "b.glb"].map(|name| {
        let path = dir.path().join(name);
        blender.export_gltf(&blend, &path, true).unwrap();
        std::fs::read(path).unwrap()
    });
    assert_eq!(a, b);
}

#[test]
fn blender_errors_include_its_output() {
    let Some(blender) = blender_or_skip("blender_errors_include_its_output") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.blend");
    std::fs::write(&broken, "not a blend file").unwrap();
    let error = import_source(&broken, &blender, true).unwrap_err();
    let AssetError::Blender { source, .. } = &error else {
        panic!("unexpected error {error}");
    };
    assert!(matches!(source, BlenderError::Failed { .. }), "{source}");
    let message = error.to_string();
    assert!(
        message.starts_with(&broken.display().to_string()),
        "{message}"
    );
    assert!(
        message.contains("Traceback") || message.contains("Error"),
        "{message}"
    );
}

#[test]
fn gltf_without_uvs_gets_them_from_blender() {
    let Some(blender) = blender_or_skip("gltf_without_uvs_gets_them_from_blender") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("bare.gltf");
    let mut bare = sphere(16, 8, 1.0);
    bare.uvs.clear();
    write_gltf(&source, &[bare], [0.0; 3]);

    let scene = import_source(&source, &blender, true).unwrap();
    assert!(
        scene
            .meshes
            .iter()
            .all(|mesh| mesh.uvs.len() == mesh.positions.len())
    );
    // Without generation the missing UVs are kept missing.
    let scene = import_source(&source, &blender, false).unwrap();
    assert!(scene.meshes[0].uvs.is_empty());
}

/// Generated .blend -> glTF -> prepared -> UVs/collision/LODs -> compiled -> loaded
/// in Bevy -> equal to the imported geometry.
#[test]
fn blend_end_to_end() {
    let Some(blender) = blender_or_skip("blend_end_to_end") else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let blend = make_test_blend(&blender, dir.path());
    let out = dir.path().join("scene.smesh");

    assert_eq!(
        compile_asset(&blend, &out).unwrap(),
        CompileStatus::Compiled
    );
    assert_eq!(
        compile_asset(&blend, &out).unwrap(),
        CompileStatus::UpToDate
    );
    let imported = import_source(&blend, &blender, true).unwrap();

    // The compiled file, read in place.
    let mapped = MappedBundle::open(&out).unwrap();
    let bundle = mapped.bundle();
    assert_eq!(bundle.meshes.len(), imported.meshes.len());
    for (compiled, source) in bundle.meshes.iter().zip(&imported.meshes) {
        assert_eq!(compiled.name.as_str(), source.name);
        let lod0 = &compiled.lods[0];
        assert_eq!(to_native(&lod0.positions), source.positions);
        assert_eq!(to_native(&lod0.normals), source.normals);
        assert_eq!(to_native(&lod0.uvs), source.uvs);
        let hull = compiled
            .collision
            .hull
            .as_ref()
            .expect("closed meshes have hulls");
        assert!(hull.points.len() >= 4);
    }
    let ball = bundle
        .meshes
        .iter()
        .find(|mesh| mesh.name == "Ball")
        .unwrap();
    assert!(ball.lods.len() >= 3, "ball has {} LODs", ball.lods.len());
    assert!(
        ball.lods
            .windows(2)
            .all(|pair| pair[1].positions.len() < pair[0].positions.len())
    );

    // The Bevy loader, which is all a shipped game needs.
    let mut app = asset_app(dir.path());
    let handle: Handle<CompiledModel> = app.world().resource::<AssetServer>().load("scene.smesh");
    assert!(wait_loaded(&mut app, &handle).is_loaded());
    let models = app.world().resource::<Assets<CompiledModel>>();
    let meshes = app.world().resource::<Assets<Mesh>>();
    let model = models.get(&handle).unwrap();
    for (loaded, source) in model.meshes.iter().zip(&imported.meshes) {
        let lod0 = meshes.get(&loaded.lods[0]).unwrap();
        assert_eq!(mesh_positions(lod0), source.positions);
        let indices: Vec<u32> = lod0.indices().unwrap().iter().map(|i| i as u32).collect();
        assert_eq!(indices, source.indices);
        assert_eq!(loaded.material, source.material);
    }
    assert_eq!(model.nodes, imported.nodes);
    assert_eq!(model.materials, imported.materials);
    assert_eq!(model.material("Metal").map(|m| m.metallic), Some(1.0));
    let tower = model
        .nodes
        .iter()
        .find(|node| node.name == "Tower")
        .unwrap();
    assert_close(tower.translation, [1.0, 3.0, -2.0]);
    assert!(Path::new(&out).is_file());
}
