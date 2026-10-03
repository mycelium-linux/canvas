//! CPU-side mesh baking performed away from the render loop.

use anyhow::{Context, Result, ensure};
use bytemuck::{Pod, Zeroable};
use glam::{EulerRot, Mat3, Mat4, Vec3};
use serde::Deserialize;
use std::{
    collections::BTreeSet,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    thread,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Field value at this vertex, already normalised to 0..=1.
    ///
    /// Normalised on the CPU rather than in the shader so the GPU never needs
    /// the field's unit or range, and so a solved field and a closed form look
    /// identical by the time they reach it.
    pub field: f32,
}

#[derive(Debug, Clone)]
pub struct PreparedMesh {
    pub asset: PathBuf,
    pub vertices: Vec<Vertex>,
    pub triangle_indices: Vec<u32>,
    pub edge_indices: Vec<u32>,
    pub grid_indices: Vec<u32>,
    pub source_center: [f32; 3],
    pub source_extent: f32,
}

pub fn bake_stl(path: &Path) -> Result<PreparedMesh> {
    bake_stl_with_field(path, None)
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SceneManifest {
    schema: String,
    parts: Vec<ScenePart>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScenePart {
    file: String,
    #[serde(default)]
    translation_mm: [f32; 3],
    #[serde(default)]
    rotation_xyz_deg: [f32; 3],
}

pub fn bake_asset(path: &Path, field: Option<&crate::Field>) -> Result<PreparedMesh> {
    match path.extension().and_then(|value| value.to_str()) {
        Some("stl") => bake_stl_with_field(path, field),
        Some("glb") => bake_glb_scene(path, &[(path.to_owned(), Mat4::IDENTITY)], field),
        Some("json") if path.to_string_lossy().ends_with(".scene.json") => {
            let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
            let manifest: SceneManifest = serde_json::from_slice(&bytes)
                .with_context(|| format!("parse {}", path.display()))?;
            ensure!(
                manifest.schema == "canvas.model.scene-assets.v1",
                "unsupported scene manifest"
            );
            ensure!(
                !manifest.parts.is_empty() && manifest.parts.len() <= 1024,
                "scene must contain 1–1024 parts"
            );
            let root = path.parent().context("scene manifest has no parent")?;
            let mut parts = Vec::with_capacity(manifest.parts.len());
            for part in manifest.parts {
                ensure!(
                    super::safe_asset(&part.file) && part.file.ends_with(".glb"),
                    "unsafe scene part"
                );
                let r = part.rotation_xyz_deg.map(f32::to_radians);
                let transform = Mat4::from_translation(Vec3::from_array(part.translation_mm))
                    * Mat4::from_euler(EulerRot::XYZ, r[0], r[1], r[2]);
                parts.push((root.join(part.file), transform));
            }
            bake_glb_scene(path, &parts, field)
        }
        _ => anyhow::bail!("unsupported model asset {}", path.display()),
    }
}

fn bake_glb_scene(
    asset: &Path,
    parts: &[(PathBuf, Mat4)],
    field: Option<&crate::Field>,
) -> Result<PreparedMesh> {
    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut triangle_indices = Vec::new();
    for (path, outer) in parts {
        let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
        let gltf =
            gltf::Gltf::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        let blob = gltf
            .blob
            .as_deref()
            .context("GLB has no embedded binary buffer")?;
        for scene in gltf.scenes() {
            for node in scene.nodes() {
                append_node(
                    node,
                    blob,
                    *outer,
                    &mut positions,
                    &mut normals,
                    &mut triangle_indices,
                )?;
            }
        }
    }
    ensure!(
        !positions.is_empty() && !triangle_indices.is_empty(),
        "GLB scene is empty"
    );
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for point in &positions {
        min = min.min(*point);
        max = max.max(*point);
    }
    let center = (min + max) * 0.5;
    let extent = (max - min).max_element();
    ensure!(
        extent.is_finite() && extent > f32::EPSILON,
        "scene bounds are degenerate"
    );
    let mut vertices = positions
        .iter()
        .zip(normals)
        .map(|(position, normal)| Vertex {
            field: field
                .map(|spec| spec.normalize(spec.sample(position.to_array())))
                .unwrap_or(0.0),
            position: ((*position - center) / extent * 2.0).to_array(),
            normal: normal.normalize_or_zero().to_array(),
        })
        .collect();
    let mut edges = BTreeSet::new();
    for face in triangle_indices.chunks_exact(3) {
        for (a, b) in [(face[0], face[1]), (face[1], face[2]), (face[2], face[0])] {
            edges.insert(if a < b { (a, b) } else { (b, a) });
        }
    }
    let grid_indices = engineering_grid(&mut vertices);
    Ok(PreparedMesh {
        asset: asset.to_owned(),
        vertices,
        triangle_indices,
        edge_indices: edges.into_iter().flat_map(|(a, b)| [a, b]).collect(),
        grid_indices,
        source_center: center.to_array(),
        source_extent: extent,
    })
}

fn append_node(
    node: gltf::Node<'_>,
    blob: &[u8],
    parent: Mat4,
    positions: &mut Vec<Vec3>,
    normals: &mut Vec<Vec3>,
    indices: &mut Vec<u32>,
) -> Result<()> {
    let local = Mat4::from_cols_array_2d(&node.transform().matrix());
    let world = parent * local;
    if let Some(mesh) = node.mesh() {
        for primitive in mesh.primitives() {
            ensure!(
                primitive.mode() == gltf::mesh::Mode::Triangles,
                "only triangle GLB primitives are supported"
            );
            let reader = primitive.reader(|buffer| match buffer.source() {
                gltf::buffer::Source::Bin => Some(blob),
                _ => None,
            });
            let source_positions: Vec<_> = reader
                .read_positions()
                .context("GLB primitive has no positions")?
                .collect();
            let source_normals: Vec<_> = reader
                .read_normals()
                .map(Iterator::collect)
                .unwrap_or_else(|| vec![[0.0, 0.0, 0.0]; source_positions.len()]);
            let base = positions.len() as u32;
            let normal_matrix = Mat3::from_mat4(world).inverse().transpose();
            positions.extend(
                source_positions
                    .into_iter()
                    .map(|p| world.transform_point3(Vec3::from_array(p))),
            );
            normals.extend(
                source_normals
                    .into_iter()
                    .map(|n| normal_matrix * Vec3::from_array(n)),
            );
            if let Some(source) = reader.read_indices() {
                indices.extend(source.into_u32().map(|i| base + i));
            } else {
                indices.extend((0..positions.len() as u32 - base).map(|i| base + i));
            }
        }
    }
    for child in node.children() {
        append_node(child, blob, world, positions, normals, indices)?;
    }
    Ok(())
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
        mesh.vertices.len() <= 10_000_000,
        "mesh exceeds 10M vertices"
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
    let mut vertices = positions
        .iter()
        .zip(normals)
        .map(|(position, normal)| Vertex {
            // Sampled in the part's own coordinates, before the mesh is
            // recentred for display: a heat source sits where the designer put
            // it, not where the viewport happens to place the model.
            field: field
                .map(|spec| spec.normalize(spec.sample(position.to_array())))
                .unwrap_or(0.0),
            position: ((*position - center) / extent * 2.0).to_array(),
            normal: normal.normalize_or_zero().to_array(),
        })
        .collect();
    let edge_indices = edges
        .into_iter()
        .flat_map(|(a, b)| [a as u32, b as u32])
        .collect();
    let grid_indices = engineering_grid(&mut vertices);
    Ok(PreparedMesh {
        asset: path.to_owned(),
        vertices,
        triangle_indices,
        edge_indices,
        grid_indices,
        source_center: center.to_array(),
        source_extent: extent,
    })
}

fn engineering_grid(vertices: &mut Vec<Vertex>) -> Vec<u32> {
    let mut indices = Vec::with_capacity(100);
    for step in 0..=24 {
        let offset = -1.5 + step as f32 * 3.0 / 24.0;
        for (a, b) in [
            ([-1.5, -1.04, offset], [1.5, -1.04, offset]),
            ([offset, -1.04, -1.5], [offset, -1.04, 1.5]),
        ] {
            let base = vertices.len() as u32;
            vertices.push(Vertex {
                position: a,
                normal: [0.0, 1.0, 0.0],
                field: -1.0,
            });
            vertices.push(Vertex {
                position: b,
                normal: [0.0, 1.0, 0.0],
                field: -1.0,
            });
            indices.extend([base, base + 1]);
        }
    }
    indices
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
                        let result = bake_asset(&request.asset, request.field.as_ref());
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
