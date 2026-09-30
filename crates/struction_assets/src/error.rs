use std::path::PathBuf;
use std::process::ExitStatus;
use std::time::Duration;

use thiserror::Error;

/// Any failure of the asset pipeline. Messages start with the offending path.
#[derive(Debug, Error)]
pub enum AssetError {
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: unsupported source type (expected .blend, .gltf or .glb)", path.display())]
    UnsupportedSource { path: PathBuf },
    #[error("{}: invalid glTF: {message}", path.display())]
    Gltf { path: PathBuf, message: String },
    #[error(
        "{}: mesh {mesh:?} has no UVs and UV generation is disabled or needs Blender",
        path.display()
    )]
    MissingUvs { path: PathBuf, mesh: String },
    #[error("{}: {source}", path.display())]
    Blender {
        path: PathBuf,
        #[source]
        source: BlenderError,
    },
    #[error("{}: {source}", path.display())]
    Format {
        path: PathBuf,
        #[source]
        source: FormatError,
    },
}

impl AssetError {
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

/// Running headless Blender failed. Carries Blender's own output so the cause is visible.
#[derive(Debug, Error)]
pub enum BlenderError {
    #[error("Blender executable {executable:?} could not be started: {source}")]
    Spawn {
        executable: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("Blender failed ({status}):\n{output}")]
    Failed { status: ExitStatus, output: String },
    #[error("Blender did not finish within {timeout:?} and was killed:\n{output}")]
    Timeout { timeout: Duration, output: String },
    #[error("Blender finished but wrote no output file:\n{output}")]
    NoOutput { output: String },
    #[error("could not move the generated file into place at {path:?}: {source}")]
    Move {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// A compiled file that cannot be read by this build.
#[derive(Debug, Error, PartialEq)]
pub enum FormatError {
    #[error("file is too short to be a compiled mesh bundle ({len} bytes)")]
    TooShort { len: usize },
    #[error("not a compiled mesh bundle (bad magic bytes)")]
    BadMagic,
    #[error(
        "compiled with format version {found}, this build reads version {expected}; recompile the source"
    )]
    UnsupportedVersion { found: u32, expected: u32 },
    #[error("compiled data is corrupt: {0}")]
    Corrupt(String),
}
