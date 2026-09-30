//! `struction-assets compile <source> [-o <out>] [--force]`
//! `struction-assets inspect <compiled>`

use std::path::PathBuf;
use std::process::ExitCode;

use struction_assets::compile::default_output;
use struction_assets::format::FORMAT_VERSION;
use struction_assets::{CompileSettings, CompileStatus, MappedBundle, compile_asset_with};

const USAGE: &str = "usage:
  struction-assets compile <source.blend|.gltf|.glb> [-o <out.smesh>] [--force]
  struction-assets inspect <compiled.smesh>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("compile") => compile(&args[1..]),
        Some("inspect") if args.len() == 2 => inspect(&args[1]),
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
