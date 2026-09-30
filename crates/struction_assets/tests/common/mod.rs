#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use base64::Engine;
use bevy::asset::{AssetPlugin, LoadState};
use bevy::prelude::*;
use serde_json::json;
use struction_assets::{Blender, PreparedMesh, StructionAssetsPlugin};

/// The configured Blender, or `None` (with a message) when it is not installed.
pub fn blender_or_skip(test: &str) -> Option<Blender> {
    let blender = Blender::default();
    if blender.is_available() {
        Some(blender)
    } else {
        eprintln!(
            "skipping {test}: Blender executable {:?} not found (install Blender or set STRUCTION_BLENDER)",
            blender.executable
        );
        None
    }
}

/// Builds the known scene of `scripts/make_test_scene.py` into `dir/scene.blend`.
pub fn make_test_blend(blender: &Blender, dir: &Path) -> PathBuf {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/make_test_scene.py");
    let blend = dir.join("scene.blend");
    blender
        .run_script(&script, &[blend.clone().into()])
        .expect("test scene script runs");
    blend
}

/// A smooth UV sphere with seams (Y up), like Blender's.
pub fn sphere(segments: u32, rings: u32, radius: f32) -> PreparedMesh {
    let mut mesh = PreparedMesh {
        name: "Sphere".into(),
        material: Some("Metal".into()),
        positions: vec![],
        normals: vec![],
        uvs: vec![],
        indices: vec![],
    };
    for ring in 0..=rings {
        let v = ring as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let phi = u * std::f32::consts::TAU;
            let normal = [
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            ];
            mesh.positions.push(normal.map(|c| c * radius));
            mesh.normals.push(normal);
            mesh.uvs.push([u, v]);
        }
    }
    let row = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * row + segment;
            let b = a + row;
            mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
        }
    }
    mesh
}

/// Writes `meshes` as a `.gltf` with an embedded buffer, one root node per mesh
/// placed at `translation`.
pub fn write_gltf(path: &Path, meshes: &[PreparedMesh], translation: [f32; 3]) {
    let mut buffer: Vec<u8> = Vec::new();
    let mut views = Vec::new();
    let mut accessors = Vec::new();
    let mut add = |bytes: &[u8], accessor: serde_json::Value| {
        views.push(json!({ "buffer": 0, "byteOffset": buffer.len(), "byteLength": bytes.len() }));
        buffer.extend_from_slice(bytes);
        let mut accessor = accessor;
        accessor["bufferView"] = json!(views.len() - 1);
        accessors.push(accessor);
        accessors.len() - 1
    };
    let floats = |values: &[f32]| {
        values
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect::<Vec<u8>>()
    };

    let mut gltf_meshes = Vec::new();
    let mut materials = Vec::new();
    for mesh in meshes {
        let flat: Vec<f32> = mesh.positions.iter().flatten().copied().collect();
        let (mut min, mut max) = (mesh.positions[0], mesh.positions[0]);
        for p in &mesh.positions {
            for axis in 0..3 {
                min[axis] = min[axis].min(p[axis]);
                max[axis] = max[axis].max(p[axis]);
            }
        }
        let count = mesh.positions.len();
        let position = add(
            &floats(&flat),
            json!({ "componentType": 5126, "count": count, "type": "VEC3", "min": min, "max": max }),
        );
        let normals: Vec<f32> = mesh.normals.iter().flatten().copied().collect();
        let normal = add(
            &floats(&normals),
            json!({ "componentType": 5126, "count": count, "type": "VEC3" }),
        );
        let mut attributes = json!({ "POSITION": position, "NORMAL": normal });
        if !mesh.uvs.is_empty() {
            let uvs: Vec<f32> = mesh.uvs.iter().flatten().copied().collect();
            attributes["TEXCOORD_0"] = json!(add(
                &floats(&uvs),
                json!({ "componentType": 5126, "count": count, "type": "VEC2" })
            ));
        }
        let index_bytes: Vec<u8> = mesh.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let indices = add(
            &index_bytes,
            json!({ "componentType": 5125, "count": mesh.indices.len(), "type": "SCALAR" }),
        );
        let mut primitive = json!({ "attributes": attributes, "indices": indices });
        if let Some(material) = &mesh.material {
            materials.push(json!({ "name": material }));
            primitive["material"] = json!(materials.len() - 1);
        }
        gltf_meshes.push(json!({ "name": mesh.name, "primitives": [primitive] }));
    }
    let nodes: Vec<_> = meshes
        .iter()
        .enumerate()
        .map(
            |(index, mesh)| json!({ "name": mesh.name, "mesh": index, "translation": translation }),
        )
        .collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&buffer);
    let document = json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": (0..meshes.len()).collect::<Vec<_>>() }],
        "nodes": nodes,
        "meshes": gltf_meshes,
        "materials": materials,
        "buffers": [{ "byteLength": buffer.len(), "uri": format!("data:application/octet-stream;base64,{encoded}") }],
        "bufferViews": views,
        "accessors": accessors,
    });
    std::fs::write(path, serde_json::to_vec_pretty(&document).unwrap()).unwrap();
}

/// A headless app that loads `.smesh` files from `asset_dir`.
pub fn asset_app(asset_dir: &Path) -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        AssetPlugin {
            file_path: asset_dir.to_string_lossy().into_owned(),
            ..default()
        },
        bevy::mesh::MeshPlugin,
        StructionAssetsPlugin,
    ));
    app
}

/// Updates `app` until `handle` and its sub-assets are loaded (or loading failed).
pub fn wait_loaded<A: Asset>(app: &mut App, handle: &Handle<A>) -> LoadState {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.update();
        let server = app.world().resource::<AssetServer>();
        let state = server.load_state(handle);
        if server.is_loaded_with_dependencies(handle)
            || state.is_failed()
            || Instant::now() > deadline
        {
            return state;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Positions of a Bevy mesh.
pub fn mesh_positions(mesh: &Mesh) -> Vec<[f32; 3]> {
    mesh.attribute(Mesh::ATTRIBUTE_POSITION)
        .and_then(|values| values.as_float3())
        .expect("positions")
        .to_vec()
}
