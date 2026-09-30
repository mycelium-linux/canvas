//! Merge already-positioned STL components into one renderable assembly mesh.

use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::PathBuf,
};

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let output = args
        .next()
        .context("usage: canvas-stl-merge <output.stl> <component.stl>...")?;
    let inputs = args.collect::<Vec<_>>();
    ensure!(!inputs.is_empty(), "at least one component STL is required");

    let mut triangles = Vec::new();
    for input in &inputs {
        let file = File::open(input).with_context(|| format!("open {}", input.display()))?;
        let reader = BufReader::new(file);
        let mesh = stl_io::read_stl(&mut reader.take(256 * 1024 * 1024))
            .with_context(|| format!("parse {}", input.display()))?;
        ensure!(
            triangles.len().saturating_add(mesh.faces.len()) <= 10_000_000,
            "merged assembly exceeds 10M triangles"
        );
        triangles.extend(mesh.faces.into_iter().map(|face| stl_io::Triangle {
            normal: face.normal,
            vertices: face.vertices.map(|index| mesh.vertices[index]),
        }));
    }

    let temporary = output.with_extension("stl.tmp");
    let mut file =
        File::create(&temporary).with_context(|| format!("create {}", temporary.display()))?;
    stl_io::write_stl(&mut file, triangles.iter())
        .with_context(|| format!("write {}", temporary.display()))?;
    std::fs::rename(&temporary, &output)
        .with_context(|| format!("publish {}", output.display()))?;
    println!(
        "merged {} component(s), {} triangles",
        inputs.len(),
        triangles.len()
    );
    Ok(())
}
