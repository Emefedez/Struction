use std::{io, path::PathBuf};
use struction_editor::AuthoringProject;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: struction-language <project-directory>")?;
    let mut project = AuthoringProject::open(root, struction_scene::authoring_app)?;
    struction_language::serve(&mut project, io::stdin().lock(), io::stdout().lock())?;
    Ok(())
}
