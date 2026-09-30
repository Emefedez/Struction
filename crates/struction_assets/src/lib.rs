//! Asset pipeline: Blender/glTF import, mesh preparation, compiled binary format.
//!
//! Editor side: [`compile_asset`] turns a `.blend`/`.gltf`/`.glb` source into a
//! `.smesh` bundle (import through headless Blender or directly, UV generation,
//! collision shapes, LODs), and [`SourceWatcherPlugin`] recompiles sources
//! edited through "Open in…". Runtime side: [`StructionAssetsPlugin`] loads
//! `.smesh` bundles without Blender.
//!
//! Coordinates: Blender (x, y, z) becomes engine (x, z, -y) (Y up, right-handed,
//! meters) through the glTF exporter's +Y up conversion, so Blender's +Y is the
//! engine's -Z forward.

pub mod blender;
pub mod collision;
pub mod compile;
pub mod error;
pub mod format;
pub mod import;
pub mod loader;
pub mod lod;
pub mod watch;

use bevy::prelude::*;

pub use blender::Blender;
pub use collision::CollisionSettings;
pub use compile::{
    COMPILED_EXTENSION, CompileSettings, CompileStatus, PrepareSettings, compile_asset,
    compile_asset_with, prepare_scene,
};
pub use error::{AssetError, BlenderError, FormatError};
pub use format::{MappedBundle, MeshBundle};
pub use import::{PreparedMesh, PreparedScene, import_source, read_gltf};
pub use loader::{CompiledMesh, CompiledModel, CompiledModelLoader, lod_label};
pub use lod::LodSettings;
pub use watch::{OpenIn, SourceRecompiled, SourceWatcher, SourceWatcherPlugin};

/// Registers the `.smesh` loader. Needs `AssetPlugin` and a `Mesh` asset
/// (`MeshPlugin`, part of the default render plugins).
pub struct StructionAssetsPlugin;

impl Plugin for StructionAssetsPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<CompiledModel>()
            .init_asset_loader::<CompiledModelLoader>();
    }
}
