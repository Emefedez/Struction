//! Collision and LOD preparation as a tool session: import a source once, preview
//! settings against it (presets or hand-tuned), then apply them as the source's
//! recipe and compiled output, with undo. The editor's utilities and the CLI both
//! drive this; neither writes recipes or compiled files another way.
//!
//! Outside changes are detected, not overwritten: applying or undoing after the
//! recipe changed elsewhere fails and drops the session's history for it, and a
//! changed source must be refreshed (imported again) first. A failed reimport
//! keeps the last good import, so previews keep working while the source is fixed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;

use crate::blender::Blender;
use crate::compile::{PrepareSettings, default_output, prepare_scene, write_compiled};
use crate::error::AssetError;
use crate::format::{BundleMesh, MeshBundle};
use crate::import::{PreparedScene, import_source};
use crate::recipe::{check_settings, parse_recipe, read_recipe_text, recipe_json, recipe_path};
use crate::watch::FileStamp;

pub struct PrepSession {
    source: PathBuf,
    output: PathBuf,
    blender: Blender,
    scene: Arc<PreparedScene>,
    /// UV generation the scene was imported with.
    scene_uvs: bool,
    /// Source stamp when the scene was imported.
    scene_stamp: Option<FileStamp>,
    /// Bumped on every import, so previews of an older import are refused.
    generation: u64,
    applied: PrepareSettings,
    /// Recipe text as last read or written; `None` when there is no recipe file.
    recipe: Option<String>,
    undo: Vec<RecipeStep>,
    redo: Vec<RecipeStep>,
}

/// Exact recipe texts around one apply; `None` is "no recipe file".
#[derive(Debug, Clone)]
struct RecipeStep {
    label: String,
    before: Option<String>,
    after: Option<String>,
}

/// What an apply, undo or redo left in place.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PrepApplied {
    pub label: String,
    pub settings: PrepareSettings,
}

/// What [`PrepSession::refresh`] picked up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Refreshed {
    /// The recipe changed outside the session; its undo history was dropped.
    pub recipe_changed: bool,
    /// The source was imported again.
    pub reimported: bool,
}

impl PrepSession {
    /// Imports `source` (running Blender for `.blend`) with its recipe's settings.
    /// Applying writes the default output beside it (`<source>.smesh`).
    pub fn open(source: impl Into<PathBuf>, blender: Blender) -> Result<Self, AssetError> {
        let source = source.into();
        let output = default_output(&source);
        Self::open_with_output(source, output, blender)
    }

    pub fn open_with_output(
        source: PathBuf,
        output: PathBuf,
        blender: Blender,
    ) -> Result<Self, AssetError> {
        let recipe = read_recipe_text(&source)?;
        let applied = match &recipe {
            Some(text) => parse_recipe(&source, text)?,
            None => PrepareSettings::default(),
        };
        let scene_stamp = FileStamp::read(&source);
        let scene = import_source(&source, &blender, applied.generate_uvs)?;
        Ok(Self {
            source,
            output,
            blender,
            scene: Arc::new(scene),
            scene_uvs: applied.generate_uvs,
            scene_stamp,
            generation: 0,
            applied,
            recipe,
            undo: Vec::new(),
            redo: Vec::new(),
        })
    }

    pub fn source(&self) -> &Path {
        &self.source
    }

    pub fn output(&self) -> &Path {
        &self.output
    }

    /// The settings in effect: the recipe's, or the defaults without one.
    pub fn applied(&self) -> &PrepareSettings {
        &self.applied
    }

    pub fn has_recipe(&self) -> bool {
        self.recipe.is_some()
    }

    pub fn scene(&self) -> &PreparedScene {
        &self.scene
    }

    /// The source changed since it was imported; [`Self::refresh`] imports it again.
    pub fn is_source_stale(&self) -> bool {
        FileStamp::read(&self.source) != self.scene_stamp
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|step| step.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|step| step.label.as_str())
    }

    /// A preview of `settings` that can run on another thread.
    pub fn preview_job(&self, settings: PrepareSettings) -> PreviewJob {
        PreviewJob {
            source: self.source.clone(),
            scene: self.scene.clone(),
            generation: self.generation,
            settings,
        }
    }

    pub fn preview(&self, settings: &PrepareSettings) -> Result<PrepPreview, AssetError> {
        self.preview_job(settings.clone()).run()
    }

    /// Makes a preview's settings the source's recipe and writes its compiled output.
    /// Applying the settings already in effect records nothing.
    pub fn apply(&mut self, preview: &PrepPreview, label: &str) -> Result<PrepApplied, AssetError> {
        self.check_recipe()?;
        self.check_source()?;
        if preview.generation != self.generation {
            return Err(AssetError::Changed {
                path: self.source.clone(),
                message: "it was imported again after this preview; preview again".into(),
            });
        }
        let after = recipe_json(&preview.settings);
        let applied = PrepApplied {
            label: label.to_owned(),
            settings: preview.settings.clone(),
        };
        if self.recipe.as_deref() == Some(after.as_str()) && self.output.is_file() {
            return Ok(applied);
        }
        let prepared;
        let bundle = if preview.settings.generate_uvs == self.scene_uvs {
            &preview.bundle
        } else {
            self.import(preview.settings.generate_uvs)?;
            prepared = prepare_scene(&self.scene, &preview.settings);
            &prepared
        };
        let before = self.recipe.clone();
        self.write(Some(&after), preview.settings.clone(), bundle)?;
        self.undo.push(RecipeStep {
            label: label.to_owned(),
            before,
            after: Some(after),
        });
        self.redo.clear();
        Ok(applied)
    }

    /// Previews and applies `settings` in one step.
    pub fn apply_settings(
        &mut self,
        settings: &PrepareSettings,
        label: &str,
    ) -> Result<PrepApplied, AssetError> {
        let preview = self.preview(settings)?;
        self.apply(&preview, label)
    }

    /// Restores the recipe (and output) from before the last apply.
    pub fn undo(&mut self) -> Result<Option<PrepApplied>, AssetError> {
        self.check_recipe()?;
        let Some(step) = self.undo.last().cloned() else {
            return Ok(None);
        };
        self.restore(step.before.as_deref())?;
        self.undo.pop();
        self.redo.push(step.clone());
        Ok(Some(PrepApplied {
            label: step.label,
            settings: self.applied.clone(),
        }))
    }

    pub fn redo(&mut self) -> Result<Option<PrepApplied>, AssetError> {
        self.check_recipe()?;
        let Some(step) = self.redo.last().cloned() else {
            return Ok(None);
        };
        self.restore(step.after.as_deref())?;
        self.redo.pop();
        self.undo.push(step.clone());
        Ok(Some(PrepApplied {
            label: step.label,
            settings: self.applied.clone(),
        }))
    }

    /// Picks up outside changes: a changed recipe becomes the applied settings (and
    /// drops the history), a changed source is imported again. On a failed import
    /// the last good one stays and the error is returned.
    pub fn refresh(&mut self) -> Result<Refreshed, AssetError> {
        let mut refreshed = Refreshed::default();
        let current = read_recipe_text(&self.source)?;
        if current != self.recipe {
            self.undo.clear();
            self.redo.clear();
            self.applied = match &current {
                Some(text) => parse_recipe(&self.source, text)?,
                None => PrepareSettings::default(),
            };
            self.recipe = current;
            refreshed.recipe_changed = true;
        }
        if self.is_source_stale() || self.applied.generate_uvs != self.scene_uvs {
            self.import(self.applied.generate_uvs)?;
            refreshed.reimported = true;
        }
        Ok(refreshed)
    }

    fn import(&mut self, generate_uvs: bool) -> Result<(), AssetError> {
        let stamp = FileStamp::read(&self.source);
        let scene = import_source(&self.source, &self.blender, generate_uvs)?;
        self.scene = Arc::new(scene);
        self.scene_uvs = generate_uvs;
        self.scene_stamp = stamp;
        self.generation += 1;
        Ok(())
    }

    fn restore(&mut self, recipe: Option<&str>) -> Result<(), AssetError> {
        self.check_source()?;
        let settings = match recipe {
            Some(text) => parse_recipe(&self.source, text)?,
            None => PrepareSettings::default(),
        };
        if settings.generate_uvs != self.scene_uvs {
            self.import(settings.generate_uvs)?;
        }
        let bundle = prepare_scene(&self.scene, &settings);
        self.write(recipe, settings, &bundle)
    }

    /// Output first: if the recipe write then fails, the manifest no longer matches
    /// and the next compile rebuilds from the old recipe, so nothing stays mixed.
    fn write(
        &mut self,
        recipe: Option<&str>,
        settings: PrepareSettings,
        bundle: &MeshBundle,
    ) -> Result<(), AssetError> {
        write_compiled(&self.source, &self.output, &settings, bundle)?;
        let path = recipe_path(&self.source);
        match recipe {
            Some(text) => crate::compile::write_atomically(&path, text.as_bytes())?,
            None => match std::fs::remove_file(&path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    return Err(AssetError::io(path, error));
                }
                _ => {}
            },
        }
        self.recipe = recipe.map(str::to_owned);
        self.applied = settings;
        Ok(())
    }

    fn check_recipe(&mut self) -> Result<(), AssetError> {
        if read_recipe_text(&self.source)? != self.recipe {
            self.undo.clear();
            self.redo.clear();
            return Err(AssetError::Changed {
                path: recipe_path(&self.source),
                message: "refresh to use its settings (its undo history was cleared)".into(),
            });
        }
        Ok(())
    }

    fn check_source(&self) -> Result<(), AssetError> {
        if self.is_source_stale() {
            return Err(AssetError::Changed {
                path: self.source.clone(),
                message: "refresh to import it again".into(),
            });
        }
        Ok(())
    }
}

/// A preview detached from its session, for a worker thread.
pub struct PreviewJob {
    source: PathBuf,
    scene: Arc<PreparedScene>,
    generation: u64,
    settings: PrepareSettings,
}

impl PreviewJob {
    pub fn settings(&self) -> &PrepareSettings {
        &self.settings
    }

    pub fn run(self) -> Result<PrepPreview, AssetError> {
        let problems = check_settings(&self.settings);
        if !problems.is_empty() {
            return Err(AssetError::Recipe {
                path: recipe_path(&self.source),
                message: problems.join("; "),
            });
        }
        let bundle = prepare_scene(&self.scene, &self.settings);
        let meshes = bundle
            .meshes
            .iter()
            .map(|mesh| report(mesh, &self.settings))
            .collect();
        Ok(PrepPreview {
            settings: self.settings,
            meshes,
            bundle,
            generation: self.generation,
        })
    }
}

/// Prepared result of some settings: the full bundle to draw, and a summary.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrepPreview {
    pub settings: PrepareSettings,
    pub meshes: Vec<MeshReport>,
    #[serde(skip)]
    pub bundle: MeshBundle,
    #[serde(skip)]
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshReport {
    pub name: String,
    /// Bounds size in meters.
    pub size: [f32; 3],
    /// LOD 0 first.
    pub lods: Vec<LodReport>,
    pub hull: Option<HullReport>,
    pub trimesh: TrimeshReport,
    /// Point count of each convex part.
    pub parts: Vec<usize>,
    /// Plain-language feedback on the result.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LodReport {
    pub vertices: usize,
    pub triangles: usize,
    /// Triangles relative to LOD 0.
    pub share: f32,
    /// Approximate deviation from LOD 0, in meters.
    pub error: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct HullReport {
    pub points: usize,
    pub faces: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrimeshReport {
    pub vertices: usize,
    pub triangles: usize,
    /// Triangles relative to LOD 0.
    pub share: f32,
}

/// Trimeshes above this many triangles get a cost note.
const LARGE_TRIMESH: usize = 20_000;

fn report(mesh: &BundleMesh, settings: &PrepareSettings) -> MeshReport {
    let base = mesh.lods.first().map_or(0, |lod| lod.indices.len() / 3);
    let share = |triangles: usize| {
        if base == 0 {
            0.0
        } else {
            triangles as f32 / base as f32
        }
    };
    let lods: Vec<LodReport> = mesh
        .lods
        .iter()
        .map(|lod| LodReport {
            vertices: lod.positions.len(),
            triangles: lod.indices.len() / 3,
            share: share(lod.indices.len() / 3),
            error: lod.error,
        })
        .collect();
    let collision = &mesh.collision;
    let trimesh = TrimeshReport {
        vertices: collision.trimesh.vertices.len(),
        triangles: collision.trimesh.triangles.len(),
        share: share(collision.trimesh.triangles.len()),
    };

    let mut notes = Vec::new();
    let produced = lods.len().saturating_sub(1);
    let wanted = settings.lod.levels as usize;
    if base > 0 && produced < wanted {
        notes.push(format!(
            "Stopped at {produced} of {wanted} LOD levels: the next would exceed the error \
             limit or remove too little. Raise Max error to go further."
        ));
    }
    if collision.hull.is_none() {
        notes.push("Flat or degenerate: no convex hull, so only a static trimesh fits.".into());
    }
    let (ratio, collision_settings) = (settings.collision.trimesh_ratio, &settings.collision);
    if base > 0 && ratio < 1.0 && trimesh.triangles >= base {
        notes.push(
            "The trimesh kept every triangle: its error limit allows no simplification.".into(),
        );
    }
    if trimesh.triangles > LARGE_TRIMESH {
        notes.push(format!(
            "Large trimesh ({} triangles): fine for static scenery, costly for moving bodies.",
            trimesh.triangles
        ));
    }
    if collision_settings.convex_decomposition {
        if collision.parts.is_empty() {
            notes.push("The decomposition found no convex parts.".into());
        } else if collision.parts.len() >= collision_settings.max_parts as usize {
            notes.push(format!(
                "Used all {} parts; raise Max parts for a closer fit.",
                collision_settings.max_parts
            ));
        }
    }

    let size = [0, 1, 2].map(|axis| mesh.bounds.max[axis] - mesh.bounds.min[axis]);
    MeshReport {
        name: mesh.name.clone(),
        size,
        lods,
        hull: collision.hull.as_ref().map(|hull| HullReport {
            points: hull.points.len(),
            faces: hull.faces.len(),
        }),
        trimesh,
        parts: collision.parts.iter().map(Vec::len).collect(),
        notes,
    }
}
