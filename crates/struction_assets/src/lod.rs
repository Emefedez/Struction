//! LOD generation with meshoptimizer.
//!
//! `meshopt` (the maintained C++ meshoptimizer behind a thin buffer-level FFI:
//! slices in, owned `Vec`s out, no callbacks or unwinding across the boundary) was
//! chosen over the pure-Rust `meshopt-rs`, an older partial port that trails
//! upstream (no error metrics for every mode, no newer simplifier options).

use meshopt::{SimplifyOptions, VertexDataAdapter};
use serde::{Deserialize, Serialize};

use crate::format::MeshLod;
use crate::import::PreparedMesh;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct LodSettings {
    /// Levels generated after LOD 0.
    pub levels: u32,
    /// Target triangle count of each level relative to the previous one.
    pub reduction: f32,
    /// Largest allowed deviation, relative to the mesh extent. The simplifier stops
    /// short of the target triangle count rather than exceed it.
    pub max_error: f32,
}

impl Default for LodSettings {
    fn default() -> Self {
        Self {
            levels: 3,
            reduction: 0.5,
            max_error: 0.05,
        }
    }
}

/// LOD 0 (the mesh itself) followed by up to `levels` simplified levels. Stops
/// early once a level would not remove at least a tenth of the triangles.
pub fn generate_lods(mesh: &PreparedMesh, settings: &LodSettings) -> Vec<MeshLod> {
    let mut lods = vec![MeshLod {
        positions: mesh.positions.clone(),
        normals: mesh.normals.clone(),
        uvs: mesh.uvs.clone(),
        indices: mesh.indices.clone(),
        error: 0.0,
    }];
    if mesh.indices.is_empty() {
        return lods;
    }
    let adapter = position_adapter(&mesh.positions);
    let scale = meshopt::simplify_scale(&adapter);
    let mut previous = mesh.indices.len();
    for _ in 0..settings.levels {
        let target = ((previous as f32 * settings.reduction) as usize / 3 * 3).max(3);
        let mut relative_error = 0.0;
        // Always simplify from LOD 0 so errors do not compound between levels.
        let indices = meshopt::simplify(
            &mesh.indices,
            &adapter,
            target,
            settings.max_error,
            SimplifyOptions::None,
            Some(&mut relative_error),
        );
        if indices.is_empty() || indices.len() as f32 > previous as f32 * 0.9 {
            break;
        }
        previous = indices.len();
        lods.push(compact(mesh, &indices, relative_error * scale));
    }
    lods
}

pub(crate) fn position_adapter(positions: &[[f32; 3]]) -> VertexDataAdapter<'_> {
    let bytes = meshopt::typed_to_bytes(positions);
    VertexDataAdapter::new(bytes, size_of::<[f32; 3]>(), 0).expect("tightly packed positions")
}

/// Keeps only the vertices `indices` uses, in first-use order.
fn compact(mesh: &PreparedMesh, indices: &[u32], error: f32) -> MeshLod {
    let mut remap = vec![u32::MAX; mesh.positions.len()];
    let mut lod = MeshLod {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::with_capacity(indices.len()),
        error,
    };
    for &index in indices {
        let old = index as usize;
        if remap[old] == u32::MAX {
            remap[old] = lod.positions.len() as u32;
            lod.positions.push(mesh.positions[old]);
            lod.normals.push(mesh.normals[old]);
            if !mesh.uvs.is_empty() {
                lod.uvs.push(mesh.uvs[old]);
            }
        }
        lod.indices.push(remap[old]);
    }
    lod
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::format::Bounds;

    /// A smooth UV sphere with seams, like Blender's.
    pub(crate) fn sphere(segments: u32, rings: u32, radius: f32) -> PreparedMesh {
        let mut mesh = PreparedMesh {
            name: "sphere".into(),
            material: None,
            positions: vec![],
            normals: vec![],
            uvs: vec![],
            indices: vec![],
        };
        for ring in 0..=rings {
            let v = ring as f32 / rings as f32;
            let theta = v * std::f32::consts::PI;
            for segment in 0..=segments {
                let u = segment as f32 / segments as f32;
                let phi = u * std::f32::consts::TAU;
                let normal = [
                    theta.sin() * phi.cos(),
                    theta.cos(),
                    theta.sin() * phi.sin(),
                ];
                mesh.positions.push(normal.map(|c| c * radius));
                mesh.normals.push(normal);
                mesh.uvs.push([u, v]);
            }
        }
        let row = segments + 1;
        for ring in 0..rings {
            for segment in 0..segments {
                let a = ring * row + segment;
                let b = a + row;
                mesh.indices.extend([a, b, a + 1, a + 1, b, b + 1]);
            }
        }
        mesh
    }

    #[test]
    fn lods_shrink_and_keep_bounds() {
        let mesh = sphere(48, 24, 2.0);
        let lods = generate_lods(&mesh, &LodSettings::default());
        assert_eq!(lods.len(), 4);
        assert_eq!(lods[0].positions, mesh.positions);
        assert_eq!(lods[0].indices, mesh.indices);

        let original = Bounds::of(&mesh.positions);
        for pair in lods.windows(2) {
            assert!(pair[1].indices.len() < pair[0].indices.len());
            assert!(pair[1].positions.len() < pair[0].positions.len());
            assert!(pair[1].error >= pair[0].error);
        }
        for lod in &lods {
            assert!(
                lod.error <= 0.05 * 4.0 + 1e-4,
                "error {} over limit",
                lod.error
            );
            assert_eq!(lod.normals.len(), lod.positions.len());
            assert_eq!(lod.uvs.len(), lod.positions.len());
            assert!(
                lod.indices
                    .iter()
                    .all(|&i| (i as usize) < lod.positions.len())
            );
            let bounds = Bounds::of(&lod.positions);
            for axis in 0..3 {
                let tolerance = 2.0 * lod.error + 0.02 * 4.0;
                assert!((bounds.min[axis] - original.min[axis]).abs() <= tolerance);
                assert!((bounds.max[axis] - original.max[axis]).abs() <= tolerance);
            }
        }
    }

    #[test]
    fn error_limit_stops_simplification() {
        let mesh = sphere(48, 24, 2.0);
        let strict = LodSettings {
            max_error: 1e-5,
            ..LodSettings::default()
        };
        // A curved surface cannot lose half its triangles within that error.
        assert_eq!(generate_lods(&mesh, &strict).len(), 1);
    }
}
