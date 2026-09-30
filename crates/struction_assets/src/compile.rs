//! `compile_asset`: source -> import -> UVs/collision/LODs -> `.smesh`, skipped when
//! neither the source bytes nor the settings changed since the last compile.

use std::collections::BTreeMap;
use std::hash::Hasher;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use twox_hash::XxHash3_64;

use crate::blender::Blender;
use crate::collision::{CollisionSettings, generate_collision};
use crate::error::AssetError;
use crate::format::{self, Bounds, BundleMesh, MeshBundle};
use crate::import::{PreparedScene, SourceKind, gltf_dependencies, import_source};
use crate::lod::{LodSettings, generate_lods};
use crate::recipe::read_recipe;

/// Extension of compiled mesh bundles.
pub const COMPILED_EXTENSION: &str = "smesh";
/// Per-directory record of what each compiled file was built from.
pub const MANIFEST_NAME: &str = "struction-assets.manifest.json";

/// Settings that change the compiled output (hashed into the manifest). A source's
/// recipe (`<source>.recipe.json`, see [`crate::recipe`]) stores them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PrepareSettings {
    /// Unwrap meshes that have no UVs (needs Blender).
    pub generate_uvs: bool,
    pub lod: LodSettings,
    pub collision: CollisionSettings,
}

impl Default for PrepareSettings {
    fn default() -> Self {
        Self {
            generate_uvs: true,
            lod: LodSettings::default(),
            collision: CollisionSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompileSettings {
    pub prepare: PrepareSettings,
    pub blender: Blender,
    /// Recompile even when the manifest says the output is up to date.
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompileStatus {
    Compiled,
    UpToDate,
}

impl CompileSettings {
    /// Default settings, prepared as the source's recipe says when it has one.
    pub fn for_source(source: &Path) -> Result<Self, AssetError> {
        Self::default().with_recipe_of(source)
    }

    /// Replaces the preparation settings with the source's recipe, if any.
    pub fn with_recipe_of(mut self, source: &Path) -> Result<Self, AssetError> {
        if let Some(prepare) = read_recipe(source)? {
            self.prepare = prepare;
        }
        Ok(self)
    }
}

/// Compiles `source` (`.blend`, `.gltf`, `.glb`) into `out` with default settings,
/// or its recipe's.
pub fn compile_asset(source: &Path, out: &Path) -> Result<CompileStatus, AssetError> {
    compile_asset_with(source, out, &CompileSettings::for_source(source)?)
}

pub fn compile_asset_with(
    source: &Path,
    out: &Path,
    settings: &CompileSettings,
) -> Result<CompileStatus, AssetError> {
    let entry = manifest_entry(source, &settings.prepare)?;
    let (manifest, key) = (Manifest::read(&manifest_path(out)), manifest_key(out));
    if !settings.force && out.is_file() && manifest.entries.get(&key) == Some(&entry) {
        return Ok(CompileStatus::UpToDate);
    }

    let scene = import_source(source, &settings.blender, settings.prepare.generate_uvs)?;
    let bundle = prepare_scene(&scene, &settings.prepare);
    write_output(out, &bundle, entry)?;
    Ok(CompileStatus::Compiled)
}

/// Writes a bundle prepared from `source` with `prepare`, and records it so later
/// compiles of the same source and settings are up to date.
pub(crate) fn write_compiled(
    source: &Path,
    out: &Path,
    prepare: &PrepareSettings,
    bundle: &MeshBundle,
) -> Result<(), AssetError> {
    write_output(out, bundle, manifest_entry(source, prepare)?)
}

fn write_output(out: &Path, bundle: &MeshBundle, entry: ManifestEntry) -> Result<(), AssetError> {
    write_atomically(out, &format::encode(bundle))?;
    let manifest_path = manifest_path(out);
    let mut manifest = Manifest::read(&manifest_path);
    manifest.entries.insert(manifest_key(out), entry);
    let json = serde_json::to_vec_pretty(&manifest).expect("manifest serializes");
    write_atomically(&manifest_path, &json)
}

fn manifest_entry(source: &Path, prepare: &PrepareSettings) -> Result<ManifestEntry, AssetError> {
    let kind = SourceKind::of(source).ok_or_else(|| AssetError::UnsupportedSource {
        path: source.to_owned(),
    })?;
    Ok(ManifestEntry {
        source: source.to_string_lossy().into_owned(),
        source_hash: format!("{:016x}", source_hash(source, kind)?),
        settings_hash: format!("{:016x}", settings_hash(prepare)),
        format_version: format::FORMAT_VERSION,
    })
}

fn manifest_key(out: &Path) -> String {
    out.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Mesh preparation: LODs and collision for every imported mesh.
pub fn prepare_scene(scene: &PreparedScene, settings: &PrepareSettings) -> MeshBundle {
    MeshBundle {
        meshes: scene
            .meshes
            .iter()
            .map(|mesh| BundleMesh {
                name: mesh.name.clone(),
                material: mesh.material.clone(),
                bounds: Bounds::of(&mesh.positions),
                lods: generate_lods(mesh, &settings.lod),
                collision: generate_collision(&mesh.positions, &mesh.indices, &settings.collision),
            })
            .collect(),
        nodes: scene.nodes.clone(),
        materials: scene.materials.clone(),
    }
}

/// Default output path: the source with the compiled extension.
pub fn default_output(source: &Path) -> PathBuf {
    source.with_extension(COMPILED_EXTENSION)
}

fn manifest_path(out: &Path) -> PathBuf {
    out.parent().unwrap_or(Path::new("")).join(MANIFEST_NAME)
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Manifest {
    /// Keyed by compiled file name within the manifest's directory.
    entries: BTreeMap<String, ManifestEntry>,
}

impl Manifest {
    /// A missing or unreadable manifest only means everything gets recompiled.
    fn read(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestEntry {
    source: String,
    source_hash: String,
    settings_hash: String,
    format_version: u32,
}

/// Hash of the source bytes plus, for `.gltf`, its external buffers.
fn source_hash(source: &Path, kind: SourceKind) -> Result<u64, AssetError> {
    let mut hasher = XxHash3_64::new();
    hash_file(&mut hasher, source)?;
    if kind == SourceKind::Gltf {
        for dependency in gltf_dependencies(source)? {
            hash_file(&mut hasher, &dependency)?;
        }
    }
    Ok(hasher.finish())
}

fn hash_file(hasher: &mut XxHash3_64, path: &Path) -> Result<(), AssetError> {
    let mut file = std::fs::File::open(path).map_err(|error| AssetError::io(path, error))?;
    let mut chunk = vec![0; 1 << 16];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => return Ok(()),
            Ok(read) => hasher.write(&chunk[..read]),
            Err(error) => return Err(AssetError::io(path, error)),
        }
    }
}

fn settings_hash(settings: &PrepareSettings) -> u64 {
    let json = serde_json::to_vec(settings).expect("settings serialize");
    XxHash3_64::oneshot(&json)
}

/// Writes through a temporary sibling and a rename, so readers (the asset loader,
/// memory maps) never see a partial file.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), AssetError> {
    let dir = path.parent().filter(|dir| !dir.as_os_str().is_empty());
    let dir = dir.unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|error| AssetError::io(dir, error))?;
    let mut temp =
        tempfile::NamedTempFile::new_in(dir).map_err(|error| AssetError::io(dir, error))?;
    std::io::Write::write_all(&mut temp, bytes).map_err(|error| AssetError::io(path, error))?;
    temp.persist(path)
        .map_err(|error| AssetError::io(path, error.error))?;
    Ok(())
}
