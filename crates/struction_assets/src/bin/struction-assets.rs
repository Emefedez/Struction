//! `struction-assets build <generator.py> <out.blend|.glb>`
//! `struction-assets compile <source> [-o <out>] [--force]`
//! `struction-assets inspect <compiled>`
//! `struction-assets presets`
//! `struction-assets preview|apply <source> [--lod <preset>] [--collision <preset>] [--recipe <file>]`
//!
//! `presets`, `preview` and `apply` print JSON for tools: `preview` reports what the
//! settings would produce without writing; `apply` makes them the source's recipe and
//! writes its compiled output, through the same session the editor's utilities use.

use std::path::PathBuf;
use std::process::ExitCode;

use serde_json::json;
use struction_assets::compile::PrepareSettings;
use struction_assets::compile::default_output;
use struction_assets::format::FORMAT_VERSION;
use struction_assets::{
    Blender, CollisionPreset, CompileSettings, CompileStatus, LodPreset, MappedBundle, PrepSession,
    compile_asset_with,
};

const USAGE: &str = "usage:
  struction-assets build <generator.py> <out.blend|.glb>
  struction-assets compile <source.blend|.gltf|.glb> [-o <out.smesh>] [--force]
  struction-assets inspect <compiled.smesh>
  struction-assets presets
  struction-assets preview <source> [--lod <preset>] [--collision <preset>] [--recipe <file.json>]
  struction-assets apply <source> [--lod <preset>] [--collision <preset>] [--recipe <file.json>]

preview and apply start from the source's recipe (or the defaults); --recipe replaces it
and the presets then replace their section. Run `presets` for the names.";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("build") if args.len() == 3 => build(&args[1], &args[2]),
        Some("compile") => compile(&args[1..]),
        Some("inspect") if args.len() == 2 => inspect(&args[1]),
        Some("presets") if args.len() == 1 => presets(),
        Some("preview") => prepare(&args[1..], false),
        Some("apply") => prepare(&args[1..], true),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Runs a Blender generator script (headless) that builds and saves an asset.
fn build(script: &str, out: &str) -> Result<(), String> {
    let blender = Blender::default();
    if !blender.is_available() {
        return Err(format!(
            "Blender not found at {:?}; set STRUCTION_BLENDER to its executable",
            blender.executable
        ));
    }
    blender
        .run_generator(script.as_ref(), out.as_ref())
        .map_err(|error| error.to_string())?;
    println!("built {out}");
    Ok(())
}

fn compile(args: &[String]) -> Result<(), String> {
    let mut source = None;
    let mut out = None;
    let mut settings = CompileSettings::default();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-o" | "--out" => out = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            "--force" => settings.force = true,
            _ if source.is_none() && !arg.starts_with('-') => source = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let source = source.ok_or(USAGE)?;
    let out = out.unwrap_or_else(|| default_output(&source));
    let settings = settings
        .with_recipe_of(&source)
        .map_err(|error| error.to_string())?;
    match compile_asset_with(&source, &out, &settings).map_err(|error| error.to_string())? {
        CompileStatus::Compiled => println!("compiled {}", out.display()),
        CompileStatus::UpToDate => println!("up to date {}", out.display()),
    }
    Ok(())
}

fn inspect(path: &str) -> Result<(), String> {
    let mapped = MappedBundle::open(path.as_ref()).map_err(|error| error.to_string())?;
    let bundle = mapped.bundle();
    println!(
        "{path}: format version {FORMAT_VERSION}, {} bytes, {} meshes, {} nodes",
        mapped.bytes().len(),
        bundle.meshes.len(),
        bundle.nodes.len()
    );
    for (index, mesh) in bundle.meshes.iter().enumerate() {
        let material = mesh.material.as_ref().map_or("-", |name| name.as_str());
        let [min, max] = [&mesh.bounds.min, &mesh.bounds.max].map(|v| v.map(|c| c.to_native()));
        println!(
            "mesh {index} {:?} material {material:?} bounds {min:?}..{max:?}",
            mesh.name.as_str()
        );
        for (level, lod) in mesh.lods.iter().enumerate() {
            println!(
                "  lod {level}: {} vertices, {} triangles, uvs: {}, error {:.5} m",
                lod.positions.len(),
                lod.indices.len() / 3,
                !lod.uvs.is_empty(),
                lod.error.to_native()
            );
        }
        let collision = &mesh.collision;
        let hull = collision.hull.as_ref().map_or("none".to_owned(), |hull| {
            format!("{} points", hull.points.len())
        });
        println!(
            "  collision: hull {hull}, trimesh {} triangles, {} convex parts",
            collision.trimesh.triangles.len(),
            collision.parts.len()
        );
    }
    for material in bundle.materials.iter() {
        let color = material.base_color.map(|c| c.to_native());
        let emissive = material.emissive.map(|c| c.to_native());
        println!(
            "material {:?} color {color:?} metallic {:.2} roughness {:.2} emissive {emissive:?}",
            material.name.as_str(),
            material.metallic.to_native(),
            material.roughness.to_native()
        );
    }
    for node in bundle.nodes.iter() {
        let parent = node.parent.as_ref().map(|parent| parent.to_native());
        let translation = node.translation.map(|c| c.to_native());
        let meshes: Vec<u32> = node.meshes.iter().map(|mesh| mesh.to_native()).collect();
        println!(
            "node {:?} parent {parent:?} translation {translation:?} meshes {meshes:?}",
            node.name.as_str()
        );
    }
    Ok(())
}

fn presets() -> Result<(), String> {
    let lod: Vec<_> = LodPreset::ALL
        .into_iter()
        .map(|preset| {
            json!({ "name": preset, "label": preset.label(), "description": preset.description(),
                    "settings": preset.settings() })
        })
        .collect();
    let collision: Vec<_> = CollisionPreset::ALL
        .into_iter()
        .map(|preset| {
            json!({ "name": preset, "label": preset.label(), "description": preset.description(),
                    "settings": preset.settings() })
        })
        .collect();
    print_json(&json!({ "lod": lod, "collision": collision }));
    Ok(())
}

/// `preview` or `apply`: settings from the recipe, a recipe file and presets.
fn prepare(args: &[String], apply: bool) -> Result<(), String> {
    let mut source = None;
    let (mut lod, mut collision, mut recipe) = (None, None, None);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().cloned().ok_or(USAGE.to_owned());
        match arg.as_str() {
            "--lod" => lod = Some(preset::<LodPreset>(&value()?)?),
            "--collision" => collision = Some(preset::<CollisionPreset>(&value()?)?),
            "--recipe" => recipe = Some(PathBuf::from(value()?)),
            _ if source.is_none() && !arg.starts_with('-') => source = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.to_owned()),
        }
    }
    let source = source.ok_or(USAGE)?;
    let mut session =
        PrepSession::open(&source, Blender::default()).map_err(|error| error.to_string())?;
    let mut settings = session.applied().clone();
    if let Some(path) = recipe {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        settings = serde_json::from_str::<PrepareSettings>(&text)
            .map_err(|error| format!("{}: {error}", path.display()))?;
    }
    if let Some(preset) = lod {
        settings.lod = preset.settings();
    }
    if let Some(preset) = collision {
        settings.collision = preset.settings();
    }
    let preview = session
        .preview(&settings)
        .map_err(|error| error.to_string())?;
    if apply {
        session
            .apply(&preview, "Apply from the command line")
            .map_err(|error| error.to_string())?;
    }
    print_json(&json!({
        "source": source,
        "output": session.output(),
        "applied": apply,
        "preview": preview,
    }));
    Ok(())
}

fn preset<T: serde::de::DeserializeOwned>(name: &str) -> Result<T, String> {
    serde_json::from_value(json!(name))
        .map_err(|_| format!("unknown preset {name:?}; run `struction-assets presets`"))
}

fn print_json(value: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("JSON serializes")
    );
}
