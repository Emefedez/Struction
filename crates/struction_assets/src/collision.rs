//! Collision shapes as plain data: convex hull, simplified trimesh and an optional
//! convex decomposition. Geometry comes from parry (Avian's backend), but nothing
//! here depends on Avian; the runtime builds `Collider`s from these arrays.

use meshopt::SimplifyOptions;
use parry3d::math::Vector;
use parry3d::transformation::try_convex_hull;
use parry3d::transformation::vhacd::{VHACD, VHACDParameters};
use serde::{Deserialize, Serialize};

use crate::format::{CollisionShapes, ConvexHull, TriMesh};
use crate::lod::position_adapter;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct CollisionSettings {
    /// Target triangle count of the collision trimesh relative to the render mesh.
    pub trimesh_ratio: f32,
    /// Largest deviation of the trimesh, relative to the mesh extent.
    pub trimesh_max_error: f32,
    /// Also compute a convex decomposition (V-HACD); slower, needed for concave
    /// dynamic bodies.
    pub convex_decomposition: bool,
    /// Most convex parts the decomposition may produce.
    pub max_parts: u32,
    /// Concavity each decomposed part may keep, relative to the mesh size; lower
    /// values split more finely.
    pub concavity: f32,
}

impl Default for CollisionSettings {
    fn default() -> Self {
        Self {
            trimesh_ratio: 0.25,
            trimesh_max_error: 0.02,
            convex_decomposition: false,
            max_parts: 16,
            concavity: 0.01,
        }
    }
}

pub fn generate_collision(
    positions: &[[f32; 3]],
    indices: &[u32],
    settings: &CollisionSettings,
) -> CollisionShapes {
    let trimesh = simplified_trimesh(positions, indices, settings);
    let parts = if settings.convex_decomposition && !trimesh.triangles.is_empty() {
        convex_decomposition(&trimesh, settings)
    } else {
        Vec::new()
    };
    CollisionShapes {
        hull: convex_hull(positions),
        trimesh,
        parts,
    }
}

/// `None` when the points are too few or coplanar to enclose a volume.
pub fn convex_hull(points: &[[f32; 3]]) -> Option<ConvexHull> {
    let points: Vec<Vector> = points.iter().map(|&p| Vector::from_array(p)).collect();
    let (points, faces) = try_convex_hull(&points).ok()?;
    // Coplanar input yields a flat, two-sided "hull" with no volume.
    let volume: f32 = faces
        .iter()
        .map(|face| {
            let [a, b, c] = face.map(|i| points[i as usize]);
            a.dot(b.cross(c)) / 6.0
        })
        .sum();
    let extent = points
        .iter()
        .fold(Vector::ZERO, |max, p| max.max(p.abs()))
        .max_element();
    if faces.len() < 4 || volume <= 1e-6 * extent.powi(3) {
        return None;
    }
    Some(ConvexHull {
        points: points.iter().map(|p| p.to_array()).collect(),
        faces,
    })
}

/// Welds vertices split for UV/normal seams, then simplifies positions only.
fn simplified_trimesh(
    positions: &[[f32; 3]],
    indices: &[u32],
    settings: &CollisionSettings,
) -> TriMesh {
    let (count, remap) = meshopt::generate_vertex_remap(positions, Some(indices));
    let welded_positions = meshopt::remap_vertex_buffer(positions, count, &remap);
    let welded_indices = meshopt::remap_index_buffer(Some(indices), count, &remap);
    let target = ((welded_indices.len() as f32 * settings.trimesh_ratio) as usize / 3 * 3).max(3);
    let simplified = meshopt::simplify(
        &welded_indices,
        &position_adapter(&welded_positions),
        target,
        settings.trimesh_max_error,
        SimplifyOptions::None,
        None,
    );

    let mut used = vec![u32::MAX; welded_positions.len()];
    let mut mesh = TriMesh::default();
    for triangle in simplified.as_chunks::<3>().0 {
        let corners = [0, 1, 2].map(|corner| {
            let old = triangle[corner] as usize;
            if used[old] == u32::MAX {
                used[old] = mesh.vertices.len() as u32;
                mesh.vertices.push(welded_positions[old]);
            }
            used[old]
        });
        mesh.triangles.push(corners);
    }
    mesh
}

fn convex_decomposition(mesh: &TriMesh, settings: &CollisionSettings) -> Vec<Vec<[f32; 3]>> {
    let points: Vec<Vector> = mesh
        .vertices
        .iter()
        .map(|&p| Vector::from_array(p))
        .collect();
    let parameters = VHACDParameters {
        max_convex_hulls: settings.max_parts.max(1),
        concavity: settings.concavity,
        ..VHACDParameters::default()
    };
    let decomposition = VHACD::decompose(&parameters, &points, &mesh.triangles, true);
    decomposition
        .compute_exact_convex_hulls(&points, &mesh.triangles)
        .into_iter()
        .filter(|(_, faces)| !faces.is_empty())
        .map(|(points, _)| points.iter().map(|p| p.to_array()).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lod::tests::sphere;

    fn is_inside(hull: &ConvexHull, point: [f32; 3], tolerance: f32) -> bool {
        hull.faces.iter().all(|face| {
            let [a, b, c] = face.map(|i| Vector::from_array(hull.points[i as usize]));
            let normal = (b - a).cross(c - a).normalize();
            normal.dot(Vector::from_array(point) - a) <= tolerance
        })
    }

    #[test]
    fn hull_contains_all_points() {
        let mesh = sphere(24, 12, 1.5);
        let shapes = generate_collision(&mesh.positions, &mesh.indices, &Default::default());
        let hull = shapes.hull.expect("a sphere has a volume");
        assert!(hull.points.len() < mesh.positions.len());
        for point in &mesh.positions {
            assert!(is_inside(&hull, *point, 1e-4), "{point:?} outside the hull");
        }
        // Faces point outward: the center is behind every face.
        assert!(is_inside(&hull, [0.0; 3], 0.0));
    }

    #[test]
    fn trimesh_is_welded_and_simplified() {
        let mesh = sphere(32, 16, 1.0);
        let shapes = generate_collision(&mesh.positions, &mesh.indices, &Default::default());
        let trimesh = shapes.trimesh;
        assert!(!trimesh.triangles.is_empty());
        assert!(trimesh.triangles.len() * 3 < mesh.indices.len() / 2);
        assert!(trimesh.vertices.len() < mesh.positions.len() / 2);
        let bounds = crate::format::Bounds::of(&trimesh.vertices);
        for axis in 0..3 {
            assert!((bounds.max[axis] - 1.0).abs() < 0.1);
            assert!((bounds.min[axis] + 1.0).abs() < 0.1);
        }
        assert!(shapes.parts.is_empty());
    }

    #[test]
    fn flat_mesh_has_no_hull() {
        let positions = [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ];
        let shapes = generate_collision(&positions, &[0, 2, 1, 0, 3, 2], &Default::default());
        assert!(shapes.hull.is_none());
        assert_eq!(shapes.trimesh.triangles.len(), 2);
    }

    #[test]
    fn decomposition_splits_concave_shapes() {
        // An L made of two boxes sharing a face.
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for (min, max) in [
            ([0.0, 0.0, 0.0], [3.0, 1.0, 1.0]),
            ([0.0, 1.0, 0.0], [1.0, 3.0, 1.0]),
        ] {
            let base = positions.len() as u32;
            for corner in 0..8 {
                positions.push([0, 1, 2].map(|axis| {
                    if corner >> axis & 1 == 1 {
                        max[axis]
                    } else {
                        min[axis]
                    }
                }));
            }
            for quad in [
                [0, 2, 3, 1],
                [4, 5, 7, 6],
                [0, 1, 5, 4],
                [2, 6, 7, 3],
                [0, 4, 6, 2],
                [1, 3, 7, 5],
            ] {
                let [a, b, c, d] = quad.map(|i: u32| base + i);
                indices.extend([a, b, c, a, c, d]);
            }
        }
        let settings = CollisionSettings {
            convex_decomposition: true,
            trimesh_ratio: 1.0,
            ..Default::default()
        };
        let shapes = generate_collision(&positions, &indices, &settings);
        assert!(shapes.parts.len() >= 2, "got {} parts", shapes.parts.len());
    }
}
