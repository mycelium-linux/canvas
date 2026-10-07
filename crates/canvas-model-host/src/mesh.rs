//! CPU-side mesh baking performed away from the render loop.

use anyhow::{Context, Result, ensure};
pub use fpl_gfx::mesh::Vertex;
use glam::Vec3;
use std::{
    collections::BTreeSet,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    thread,
};

#[derive(Debug, Clone)]
pub struct PreparedMesh {
    pub asset: PathBuf,
    pub vertices: Vec<Vertex>,
    pub triangle_indices: Vec<u32>,
    pub edge_indices: Vec<u32>,
    pub source_center: [f32; 3],
    pub source_extent: f32,
}

pub fn bake_stl(path: &Path) -> Result<PreparedMesh> {
    bake_stl_with_field(path, None)
}

pub fn bake_stl_with_field(path: &Path, field: Option<&crate::Field>) -> Result<PreparedMesh> {
    let file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mesh = stl_io::read_stl(&mut BufReader::new(file))
        .with_context(|| format!("parse {}", path.display()))?;
    ensure!(
        !mesh.vertices.is_empty() && !mesh.faces.is_empty(),
        "mesh is empty"
    );
    ensure!(
        mesh.vertices.len() <= fpl_gfx::analysis::MAX_FIELD_VERTICES,
        "mesh exceeds {} vertices",
        fpl_gfx::analysis::MAX_FIELD_VERTICES
    );

    let positions = mesh
        .vertices
        .iter()
        .map(|p| Vec3::new(p[0], p[1], p[2]))
        .collect::<Vec<_>>();
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for point in &positions {
        ensure!(point.is_finite(), "mesh contains non-finite coordinates");
        min = min.min(*point);
        max = max.max(*point);
    }
    let source_positions: Vec<_> = positions.iter().map(|p| p.to_array()).collect();
    let scalar_values = field
        .map(|f| f.normalized_values(&source_positions))
        .transpose()?;
    let center = (min + max) * 0.5;
    let extent = (max - min).max_element();
    ensure!(
        extent.is_finite() && extent > f32::EPSILON,
        "mesh bounds are degenerate"
    );

    let mut normals = vec![Vec3::ZERO; positions.len()];
    let mut triangle_indices = Vec::with_capacity(mesh.faces.len() * 3);
    let mut edges = BTreeSet::new();
    for face in &mesh.faces {
        let [a, b, c] = face.vertices;
        ensure!(
            a < positions.len() && b < positions.len() && c < positions.len(),
            "face index is out of bounds"
        );
        let face_normal = (positions[b] - positions[a]).cross(positions[c] - positions[a]);
        if face_normal.length_squared() > f32::EPSILON {
            let normal = face_normal.normalize();
            normals[a] += normal;
            normals[b] += normal;
            normals[c] += normal;
        }
        triangle_indices.extend([a as u32, b as u32, c as u32]);
        for (left, right) in [(a, b), (b, c), (c, a)] {
            edges.insert(if left < right {
                (left, right)
            } else {
                (right, left)
            });
        }
    }
    let vertices = positions
        .iter()
        .zip(normals)
        .enumerate()
        .map(|(index, (position, normal))| Vertex {
            // Sampled in the part's own coordinates, before the mesh is
            // recentred for display: a heat source sits where the designer put
            // it, not where the viewport happens to place the model.
            field: scalar_values.as_ref().map_or(0.0, |values| values[index]),
            position: ((*position - center) / extent * 2.0).to_array(),
            normal: normal.normalize_or_zero().to_array(),
        })
        .collect();
    let edge_indices = edges
        .into_iter()
        .flat_map(|(a, b)| [a as u32, b as u32])
        .collect();
    Ok(PreparedMesh {
        asset: path.to_owned(),
        vertices,
        triangle_indices,
        edge_indices,
        source_center: center.to_array(),
        source_extent: extent,
    })
}

#[derive(Debug)]
pub struct BakeRequest {
    pub generation: u64,
    pub asset: PathBuf,
    /// Painted while the mesh is baked, on the worker thread. Changing the
    /// field therefore costs a rebake, which is the right trade: it keeps every
    /// frame free of it.
    pub field: Option<crate::Field>,
}

#[derive(Debug)]
pub struct BakeResult {
    pub generation: u64,
    pub result: Result<PreparedMesh>,
}

/// Bounded background pool. Queue pressure is explicit rather than spawning a
/// thread per model switch; callers can discard stale generations on receipt.
pub struct BakePool {
    requests: SyncSender<BakeRequest>,
    pub results: Receiver<BakeResult>,
}

impl BakePool {
    pub fn new(worker_count: usize) -> Self {
        let workers = worker_count.clamp(1, 8);
        let (request_tx, request_rx) = sync_channel::<BakeRequest>(workers * 2);
        let (result_tx, result_rx) = sync_channel::<BakeResult>(workers * 2);
        let shared = std::sync::Arc::new(std::sync::Mutex::new(request_rx));
        for index in 0..workers {
            let requests = std::sync::Arc::clone(&shared);
            let results = result_tx.clone();
            thread::Builder::new()
                .name(format!("canvas-model-bake-{index}"))
                .spawn(move || {
                    loop {
                        let request = match requests.lock().expect("bake queue poisoned").recv() {
                            Ok(request) => request,
                            Err(_) => break,
                        };
                        let result = bake_stl_with_field(&request.asset, request.field.as_ref());
                        if results
                            .send(BakeResult {
                                generation: request.generation,
                                result,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                })
                .expect("spawn model bake worker");
        }
        Self {
            requests: request_tx,
            results: result_rx,
        }
    }

    pub fn submit(&self, request: BakeRequest) -> Result<()> {
        self.requests
            .try_send(request)
            .context("model bake queue is full")
    }
}
