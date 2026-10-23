//! Compile the committed `.wat` scene fixtures into the `.wasm` files the CLI
//! and tests load. Run from the repository root:
//!
//! ```text
//! cargo run -p canvas-model-host --example build-wasm-fixtures
//! ```
//!
//! The `.wat` is the source of truth; the `.wasm` is a build artifact kept in
//! the tree so `cargo test` and the documented CLI invocations need no
//! external `wat2wasm` binary.

use std::{fs, path::PathBuf};

fn main() -> anyhow::Result<()> {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scenes");
    for name in ["field-orbit", "follow-latest"] {
        let source_path = fixtures.join(format!("{name}.wat"));
        let wasm_path = fixtures.join(format!("{name}.wasm"));
        let source = fs::read_to_string(&source_path)?;
        let wasm = wat::parse_str(&source)?;
        fs::write(&wasm_path, wasm)?;
        println!("wrote {}", wasm_path.display());
    }
    Ok(())
}
