//! Compiled mesh bundle (`.smesh`): a 16-byte header followed by an rkyv archive.
//!
//! rkyv already gives what the README asks of the compiled binary: fixed-layout
//! archived structs read in place, with variable-sized data (vertex and index
//! arrays, names) stored in payloads referenced by relative offsets. Validation
//! (bytecheck) runs once on load, so corrupt files fail with an error instead of
//! undefined behavior. Layout changes bump [`FORMAT_VERSION`]; older files are
//! rejected and recompiled from source.
//!
//! Header: `MAGIC` (8 bytes), format version (u32 LE), reserved (u32, zero).
//! The archive starts at offset 16 so an aligned file buffer stays aligned.

use std::fs::File;
use std::path::Path;

use bevy::math::Vec3;
use memmap2::Mmap;
use rkyv::rancor;
use rkyv::util::AlignedVec;
use serde::{Deserialize, Serialize};

use crate::error::{AssetError, FormatError};

pub const MAGIC: [u8; 8] = *b"STRMESH\0";
pub const FORMAT_VERSION: u32 = 2;
pub const HEADER_LEN: usize = 16;
/// Alignment the archive needs in memory (largest archived field alignment is 4).
pub const ARCHIVE_ALIGN: usize = 16;

/// Everything compiled from one source file.
#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct MeshBundle {
    pub meshes: Vec<BundleMesh>,
    /// Scene hierarchy; parents come before their children.
    pub nodes: Vec<SceneNode>,
    /// Named materials that meshes refer to by name.
    pub materials: Vec<BundleMaterial>,
}

/// Surface appearance of a named source material (glTF metallic-roughness). Only
/// constant factors: the pipeline does not export textures.
#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct BundleMaterial {
    pub name: String,
    /// Linear RGBA.
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    /// Linear RGB, already multiplied by the emission strength.
    pub emissive: [f32; 3],
}

/// One glTF primitive: a single material, its LOD chain and collision shapes.
#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct BundleMesh {
    pub name: String,
    pub material: Option<String>,
    pub bounds: Bounds,
    /// LOD 0 is the imported geometry; each further level has fewer triangles.
    pub lods: Vec<MeshLod>,
    pub collision: CollisionShapes,
}

#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct MeshLod {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// Empty when the source had no UVs and none were generated.
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Approximate maximum deviation from LOD 0, in meters.
    pub error: f32,
}

/// Plain collision data in mesh space, ready for Avian's `Collider` constructors.
#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct CollisionShapes {
    /// `None` for flat or degenerate meshes.
    pub hull: Option<ConvexHull>,
    /// Simplified, welded copy of the render mesh (static colliders).
    pub trimesh: TriMesh,
    /// Convex decomposition (dynamic concave bodies); empty unless enabled.
    pub parts: Vec<Vec<[f32; 3]>>,
}

#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct ConvexHull {
    pub points: Vec<[f32; 3]>,
    /// Outward-facing (counter-clockwise) triangles over `points`.
    pub faces: Vec<[u32; 3]>,
}

#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Default,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct TriMesh {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    Copy,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    /// Bounds of `points`; all zero when empty.
    pub fn of(points: &[[f32; 3]]) -> Self {
        let Some(&first) = points.first() else {
            return Self {
                min: [0.0; 3],
                max: [0.0; 3],
            };
        };
        let (min, max) = points.iter().fold(
            (Vec3::from(first), Vec3::from(first)),
            |(min, max), &point| (min.min(point.into()), max.max(point.into())),
        );
        Self {
            min: min.to_array(),
            max: max.to_array(),
        }
    }
}

/// A node of the imported scene, in engine coordinates (Y up, meters).
#[derive(
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
)]
#[rkyv(derive(Debug))]
pub struct SceneNode {
    pub name: String,
    pub parent: Option<u32>,
    pub translation: [f32; 3],
    /// Quaternion `[x, y, z, w]`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// Indices into the bundle's meshes.
    pub meshes: Vec<u32>,
}

/// Serializes `bundle` into the bytes of a `.smesh` file.
pub fn encode(bundle: &MeshBundle) -> Vec<u8> {
    let archive =
        rkyv::to_bytes::<rancor::Error>(bundle).expect("serializing plain data cannot fail");
    let mut bytes = Vec::with_capacity(HEADER_LEN + archive.len());
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&archive);
    bytes
}

/// Validates a `.smesh` file in place and returns its archived root without copying.
///
/// `bytes` must start at an address aligned to [`ARCHIVE_ALIGN`] (a memory map or
/// an [`AlignedVec`]); use [`aligned_copy`] otherwise.
pub fn access(bytes: &[u8]) -> Result<&ArchivedMeshBundle, FormatError> {
    let archive = check_header(bytes)?;
    if !(archive.as_ptr() as usize).is_multiple_of(ARCHIVE_ALIGN) {
        return Err(FormatError::Corrupt(
            "archive is not aligned in memory".into(),
        ));
    }
    rkyv::access::<ArchivedMeshBundle, rancor::Error>(archive)
        .map_err(|error| FormatError::Corrupt(error.to_string()))
}

fn check_header(bytes: &[u8]) -> Result<&[u8], FormatError> {
    if bytes.len() < HEADER_LEN {
        return Err(FormatError::TooShort { len: bytes.len() });
    }
    if bytes[..8] != MAGIC {
        return Err(FormatError::BadMagic);
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes"));
    if version != FORMAT_VERSION {
        return Err(FormatError::UnsupportedVersion {
            found: version,
            expected: FORMAT_VERSION,
        });
    }
    Ok(&bytes[HEADER_LEN..])
}

/// Copies `bytes` into a buffer suitably aligned for [`access`].
pub fn aligned_copy(bytes: &[u8]) -> AlignedVec<ARCHIVE_ALIGN> {
    let mut aligned = AlignedVec::with_capacity(bytes.len());
    aligned.extend_from_slice(bytes);
    aligned
}

/// A memory-mapped `.smesh` file, validated once on open and then read in place.
pub struct MappedBundle {
    map: Mmap,
}

impl MappedBundle {
    pub fn open(path: &Path) -> Result<Self, AssetError> {
        let file = File::open(path).map_err(|source| AssetError::io(path, source))?;
        // SAFETY: the pipeline replaces compiled files by renaming a new file over
        // them, so a mapped file is never modified in place.
        let map = unsafe { Mmap::map(&file) }.map_err(|source| AssetError::io(path, source))?;
        access(&map).map_err(|source| AssetError::Format {
            path: path.to_owned(),
            source,
        })?;
        Ok(Self { map })
    }

    pub fn bundle(&self) -> &ArchivedMeshBundle {
        // SAFETY: validated by `access` in `open`, and the mapping is immutable.
        unsafe { rkyv::access_unchecked::<ArchivedMeshBundle>(&self.map[HEADER_LEN..]) }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.map
    }
}

/// Converts archived `[f32; N]` arrays back to native ones (a copy, for APIs that own data).
pub fn to_native<const N: usize>(values: &[[rkyv::rend::f32_le; N]]) -> Vec<[f32; N]> {
    values
        .iter()
        .map(|value| value.map(|component| component.to_native()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> MeshBundle {
        MeshBundle {
            meshes: vec![BundleMesh {
                name: "quad".into(),
                material: Some("Stone".into()),
                bounds: Bounds::of(&[[0.0; 3], [1.0, 1.0, 0.0]]),
                lods: vec![MeshLod {
                    positions: vec![
                        [0.0, 0.0, 0.0],
                        [1.0, 0.0, 0.0],
                        [1.0, 1.0, 0.0],
                        [0.0, 1.0, 0.0],
                    ],
                    normals: vec![[0.0, 0.0, 1.0]; 4],
                    uvs: vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
                    indices: vec![0, 1, 2, 0, 2, 3],
                    error: 0.0,
                }],
                collision: CollisionShapes {
                    hull: None,
                    trimesh: TriMesh::default(),
                    parts: vec![],
                },
            }],
            nodes: vec![SceneNode {
                name: "root".into(),
                parent: None,
                translation: [1.0, 2.0, 3.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                scale: [1.0; 3],
                meshes: vec![0],
            }],
            materials: vec![BundleMaterial {
                name: "Stone".into(),
                base_color: [0.5, 0.5, 0.5, 1.0],
                metallic: 0.0,
                roughness: 0.9,
                emissive: [0.0; 3],
            }],
        }
    }

    #[test]
    fn round_trips_and_reads_in_place() {
        let bundle = sample();
        let bytes = aligned_copy(&encode(&bundle));
        let archived = access(&bytes).unwrap();

        let positions = archived.meshes[0].lods[0].positions.as_slice();
        let range = bytes.as_ptr_range();
        assert!(
            range.contains(&positions.as_ptr().cast()),
            "positions are read in place"
        );
        assert_eq!(to_native(positions), bundle.meshes[0].lods[0].positions);

        let deserialized = rkyv::deserialize::<MeshBundle, rancor::Error>(archived).unwrap();
        assert_eq!(deserialized, bundle);
    }

    #[test]
    fn rejects_bad_headers_and_corruption() {
        let bytes = encode(&sample());

        assert_eq!(
            access(&bytes[..4]).unwrap_err(),
            FormatError::TooShort { len: 4 }
        );

        let mut bad_magic = bytes.clone();
        bad_magic[0] = b'X';
        assert_eq!(
            access(&aligned_copy(&bad_magic)).unwrap_err(),
            FormatError::BadMagic
        );

        let mut old = bytes.clone();
        old[8..12].copy_from_slice(&0u32.to_le_bytes());
        let error = access(&aligned_copy(&old)).unwrap_err();
        assert_eq!(
            error,
            FormatError::UnsupportedVersion {
                found: 0,
                expected: FORMAT_VERSION
            }
        );
        assert!(error.to_string().contains("recompile"));

        // Point the root's mesh vector far outside the buffer.
        let mut corrupt = bytes.clone();
        let len = corrupt.len();
        corrupt[len - 16..len - 12].copy_from_slice(&i32::MIN.to_le_bytes());
        assert!(matches!(
            access(&aligned_copy(&corrupt)).unwrap_err(),
            FormatError::Corrupt(_)
        ));

        let truncated = &bytes[..bytes.len() / 2];
        assert!(matches!(
            access(&aligned_copy(truncated)).unwrap_err(),
            FormatError::Corrupt(_)
        ));
    }
}
