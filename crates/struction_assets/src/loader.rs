//! Bevy loader for compiled `.smesh` bundles.

use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages};
use bevy::mesh::{Indices, Mesh, PrimitiveTopology};
use bevy::prelude::*;

use crate::error::FormatError;
use crate::format::{
    self, ArchivedMeshLod, Bounds, BundleMaterial, CollisionShapes, SceneNode, to_native,
};

/// A loaded bundle. Every LOD is a labeled [`Mesh`] sub-asset
/// (`model.smesh#Mesh0/Lod1`); collision and hierarchy are plain data.
#[derive(Asset, TypePath, Debug)]
pub struct CompiledModel {
    pub meshes: Vec<CompiledMesh>,
    pub nodes: Vec<SceneNode>,
    pub materials: Vec<BundleMaterial>,
}

impl CompiledModel {
    pub fn material(&self, name: &str) -> Option<&BundleMaterial> {
        self.materials.iter().find(|material| material.name == name)
    }
}

#[derive(Debug)]
pub struct CompiledMesh {
    pub name: String,
    pub material: Option<String>,
    pub bounds: Bounds,
    /// LOD 0 first.
    pub lods: Vec<Handle<Mesh>>,
    /// Deviation of each LOD from LOD 0, in meters.
    pub lod_errors: Vec<f32>,
    pub collision: CollisionShapes,
}

/// Label of a LOD sub-asset.
pub fn lod_label(mesh: usize, lod: usize) -> String {
    format!("Mesh{mesh}/Lod{lod}")
}

#[derive(Default, TypePath)]
pub struct CompiledModelLoader;

impl AssetLoader for CompiledModelLoader {
    type Asset = CompiledModel;
    type Settings = ();
    type Error = FormatError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<CompiledModel, FormatError> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|error| FormatError::Corrupt(format!("read failed: {error}")))?;
        let aligned;
        let bytes = if (bytes.as_ptr() as usize).is_multiple_of(format::ARCHIVE_ALIGN) {
            &bytes[..]
        } else {
            aligned = format::aligned_copy(&bytes);
            &aligned[..]
        };
        let bundle = format::access(bytes)?;

        let mut meshes = Vec::with_capacity(bundle.meshes.len());
        for (mesh_index, mesh) in bundle.meshes.iter().enumerate() {
            let lods = mesh
                .lods
                .iter()
                .enumerate()
                .map(|(lod_index, lod)| {
                    load_context.add_labeled_asset(lod_label(mesh_index, lod_index), to_mesh(lod))
                })
                .collect();
            meshes.push(CompiledMesh {
                name: mesh.name.to_string(),
                material: mesh.material.as_ref().map(|name| name.to_string()),
                bounds: deserialize(&mesh.bounds)?,
                lods,
                lod_errors: mesh.lods.iter().map(|lod| lod.error.to_native()).collect(),
                collision: deserialize(&mesh.collision)?,
            });
        }
        Ok(CompiledModel {
            meshes,
            nodes: deserialize(&bundle.nodes)?,
            materials: deserialize(&bundle.materials)?,
        })
    }

    fn extensions(&self) -> &[&str] {
        &[crate::compile::COMPILED_EXTENSION]
    }
}

fn deserialize<T: rkyv::Archive>(archived: &T::Archived) -> Result<T, FormatError>
where
    T::Archived: rkyv::Deserialize<T, rkyv::api::high::HighDeserializer<rkyv::rancor::Error>>,
{
    rkyv::deserialize::<T, rkyv::rancor::Error>(archived)
        .map_err(|error| FormatError::Corrupt(error.to_string()))
}

/// Builds a render mesh; the vertex arrays are copied once, into the mesh.
pub fn to_mesh(lod: &ArchivedMeshLod) -> Mesh {
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, to_native(&lod.positions))
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, to_native(&lod.normals))
    .with_inserted_indices(Indices::U32(
        lod.indices.iter().map(|index| index.to_native()).collect(),
    ));
    if !lod.uvs.is_empty() {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, to_native(&lod.uvs));
    }
    mesh
}
