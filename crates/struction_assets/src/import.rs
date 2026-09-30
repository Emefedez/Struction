//! Source import: `.blend` through headless Blender, glTF read directly, into a
//! [`PreparedScene`] that does not depend on Bevy render types.

use std::path::{Path, PathBuf};

use base64::Engine;
use gltf::Gltf;
use gltf::mesh::Mode;

use crate::blender::Blender;
use crate::error::AssetError;
use crate::format::{BundleMaterial, SceneNode};

/// Imported geometry in engine coordinates (Y up, right-handed, meters).
#[derive(Debug, Clone, PartialEq)]
pub struct PreparedScene {
    /// One entry per glTF primitive.
    pub meshes: Vec<PreparedMesh>,
    /// Nodes of the default scene; parents come before their children.
    pub nodes: Vec<SceneNode>,
    /// Named materials; meshes refer to them by name.
    pub materials: Vec<BundleMaterial>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PreparedMesh {
    pub name: String,
    pub material: Option<String>,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Empty when the source has no UVs.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Source kinds the pipeline accepts, chosen by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Blend,
    Gltf,
}

impl SourceKind {
    pub fn of(path: &Path) -> Option<Self> {
        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
        match extension.as_str() {
            "blend" => Some(Self::Blend),
            "gltf" | "glb" => Some(Self::Gltf),
            _ => None,
        }
    }
}

/// Imports `source`, running Blender for `.blend` files and for glTF files whose
/// meshes lack UVs when `generate_uvs` is set (Blender's Smart UV Project).
pub fn import_source(
    source: &Path,
    blender: &Blender,
    generate_uvs: bool,
) -> Result<PreparedScene, AssetError> {
    let kind = SourceKind::of(source).ok_or_else(|| AssetError::UnsupportedSource {
        path: source.to_owned(),
    })?;
    let scene = match kind {
        SourceKind::Blend => export_with_blender(source, blender, generate_uvs)?,
        SourceKind::Gltf => {
            let scene = read_gltf(source)?;
            if generate_uvs && scene.meshes.iter().any(|mesh| mesh.uvs.is_empty()) {
                if !blender.is_available() {
                    return Err(missing_uvs(source, &scene));
                }
                export_with_blender(source, blender, true)?
            } else {
                scene
            }
        }
    };
    if generate_uvs && scene.meshes.iter().any(|mesh| mesh.uvs.is_empty()) {
        return Err(missing_uvs(source, &scene));
    }
    Ok(scene)
}

fn missing_uvs(source: &Path, scene: &PreparedScene) -> AssetError {
    let mesh = scene.meshes.iter().find(|mesh| mesh.uvs.is_empty());
    AssetError::MissingUvs {
        path: source.to_owned(),
        mesh: mesh.map(|mesh| mesh.name.clone()).unwrap_or_default(),
    }
}

fn export_with_blender(
    source: &Path,
    blender: &Blender,
    generate_uvs: bool,
) -> Result<PreparedScene, AssetError> {
    let dir = tempfile::tempdir().map_err(|error| AssetError::io(std::env::temp_dir(), error))?;
    let glb = dir.path().join("export.glb");
    blender
        .export_gltf(source, &glb, generate_uvs)
        .map_err(|error| AssetError::Blender {
            path: source.to_owned(),
            source: error,
        })?;
    let bytes = std::fs::read(&glb).map_err(|error| AssetError::io(&glb, error))?;
    parse_gltf(&bytes, None, source)
}

/// Reads a `.gltf` (with external or embedded buffers) or `.glb` file.
pub fn read_gltf(path: &Path) -> Result<PreparedScene, AssetError> {
    let bytes = std::fs::read(path).map_err(|error| AssetError::io(path, error))?;
    parse_gltf(&bytes, path.parent(), path)
}

/// Files a glTF source reads besides itself (external buffers), for change detection.
pub fn gltf_dependencies(path: &Path) -> Result<Vec<PathBuf>, AssetError> {
    let bytes = std::fs::read(path).map_err(|error| AssetError::io(path, error))?;
    let gltf = Gltf::from_slice(&bytes).map_err(|error| gltf_error(path, error))?;
    let base = path.parent().unwrap_or(Path::new(""));
    Ok(gltf
        .buffers()
        .filter_map(|buffer| match buffer.source() {
            gltf::buffer::Source::Uri(uri) if !uri.starts_with("data:") => Some(base.join(uri)),
            _ => None,
        })
        .collect())
}

fn gltf_error(path: &Path, error: impl ToString) -> AssetError {
    AssetError::Gltf {
        path: path.to_owned(),
        message: error.to_string(),
    }
}

fn parse_gltf(
    bytes: &[u8],
    base: Option<&Path>,
    source: &Path,
) -> Result<PreparedScene, AssetError> {
    let gltf = Gltf::from_slice(bytes).map_err(|error| gltf_error(source, error))?;
    let buffers = load_buffers(&gltf, base, source)?;

    // Primitive (mesh index, primitive index) -> prepared mesh index.
    let mut meshes = Vec::new();
    let mut primitive_meshes = Vec::new();
    for mesh in gltf.meshes() {
        let mesh_name = mesh
            .name()
            .map_or_else(|| format!("mesh{}", mesh.index()), str::to_owned);
        let primitive_count = mesh.primitives().len();
        let mut indices = Vec::new();
        for primitive in mesh.primitives() {
            let name = if primitive_count == 1 {
                mesh_name.clone()
            } else {
                format!("{mesh_name}/{}", primitive.index())
            };
            indices.push(meshes.len() as u32);
            meshes.push(read_primitive(&primitive, &buffers, name, source)?);
        }
        primitive_meshes.push(indices);
    }

    let roots: Vec<gltf::Node> = match gltf.default_scene().or_else(|| gltf.scenes().next()) {
        Some(scene) => scene.nodes().collect(),
        None => Vec::new(),
    };
    let mut nodes = Vec::new();
    let mut stack: Vec<(gltf::Node, Option<u32>)> =
        roots.into_iter().rev().map(|node| (node, None)).collect();
    while let Some((node, parent)) = stack.pop() {
        let (translation, rotation, scale) = node.transform().decomposed();
        let index = nodes.len() as u32;
        nodes.push(SceneNode {
            name: node
                .name()
                .map_or_else(|| format!("node{}", node.index()), str::to_owned),
            parent,
            translation,
            rotation,
            scale,
            meshes: node
                .mesh()
                .map(|mesh| primitive_meshes[mesh.index()].clone())
                .unwrap_or_default(),
        });
        let children: Vec<_> = node.children().collect();
        stack.extend(children.into_iter().rev().map(|child| (child, Some(index))));
    }

    let materials = gltf
        .materials()
        .filter_map(|material| {
            let pbr = material.pbr_metallic_roughness();
            let strength = material.emissive_strength().unwrap_or(1.0);
            Some(BundleMaterial {
                name: material.name()?.to_owned(),
                base_color: pbr.base_color_factor(),
                metallic: pbr.metallic_factor(),
                roughness: pbr.roughness_factor(),
                emissive: material.emissive_factor().map(|c| c * strength),
            })
        })
        .collect();

    Ok(PreparedScene {
        meshes,
        nodes,
        materials,
    })
}

fn load_buffers(
    gltf: &Gltf,
    base: Option<&Path>,
    source: &Path,
) -> Result<Vec<Vec<u8>>, AssetError> {
    gltf.buffers()
        .map(|buffer| {
            let data = match buffer.source() {
                gltf::buffer::Source::Bin => gltf
                    .blob
                    .clone()
                    .ok_or_else(|| gltf_error(source, "missing binary chunk"))?,
                gltf::buffer::Source::Uri(uri) => {
                    if let Some(data) = uri.strip_prefix("data:") {
                        let (_, encoded) = data
                            .split_once(";base64,")
                            .ok_or_else(|| gltf_error(source, "unsupported data URI"))?;
                        base64::engine::general_purpose::STANDARD
                            .decode(encoded)
                            .map_err(|error| gltf_error(source, error))?
                    } else {
                        let path = base.unwrap_or(Path::new("")).join(uri);
                        std::fs::read(&path).map_err(|error| AssetError::io(path, error))?
                    }
                }
            };
            if data.len() < buffer.length() {
                return Err(gltf_error(
                    source,
                    format!("buffer {} is shorter than declared", buffer.index()),
                ));
            }
            Ok(data)
        })
        .collect()
}

fn read_primitive(
    primitive: &gltf::Primitive,
    buffers: &[Vec<u8>],
    name: String,
    source: &Path,
) -> Result<PreparedMesh, AssetError> {
    if primitive.mode() != Mode::Triangles {
        return Err(gltf_error(
            source,
            format!(
                "mesh {name:?} uses {:?}; only triangle lists are supported",
                primitive.mode()
            ),
        ));
    }
    let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(Vec::as_slice));
    let positions: Vec<[f32; 3]> = reader
        .read_positions()
        .ok_or_else(|| gltf_error(source, format!("mesh {name:?} has no positions")))?
        .collect();
    let indices: Vec<u32> = match reader.read_indices() {
        Some(indices) => indices.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    if let Some(index) = indices
        .iter()
        .find(|&&index| index as usize >= positions.len())
    {
        return Err(gltf_error(
            source,
            format!("mesh {name:?} has out-of-range index {index}"),
        ));
    }
    let normals = match reader.read_normals() {
        Some(normals) => normals.collect(),
        None => smooth_normals(&positions, &indices),
    };
    let uvs = reader
        .read_tex_coords(0)
        .map(|uvs| uvs.into_f32().collect())
        .unwrap_or_default();
    Ok(PreparedMesh {
        name,
        material: primitive.material().name().map(str::to_owned),
        positions,
        normals,
        uvs,
        indices,
    })
}

/// Area-weighted vertex normals, for sources that omit them.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    use bevy::math::Vec3;
    let mut normals = vec![Vec3::ZERO; positions.len()];
    for triangle in indices.as_chunks::<3>().0 {
        let [a, b, c] = [0, 1, 2].map(|corner| Vec3::from(positions[triangle[corner] as usize]));
        let weighted = (b - a).cross(c - a);
        for &index in triangle {
            normals[index as usize] += weighted;
        }
    }
    normals
        .into_iter()
        .map(|normal| normal.normalize_or(Vec3::Y).to_array())
        .collect()
}
