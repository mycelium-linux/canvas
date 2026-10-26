//! Compile the committed `.wat` scene fixtures into the `.wasm` files the CLI
//! and tests load. Run from the repository root:
//!
//! ```text
//! cargo run -p canvas-model-host --example build-wasm-fixtures
//! ```
//!
//! Scene `.wat` files are the source of truth; the `.wasm` is a build artifact
//! kept in the tree so `cargo test` and the documented CLI invocations need no
//! external `wat2wasm` binary. The BLE fixture is different: it is generated
//! from WIT through a guest crate (`tests/fixtures/ble/guest`) and
//! `wasm-tools component new`. Component fixtures are generated from WIT,
//! never hand-written.

use std::{fs, path::PathBuf};

fn main() -> anyhow::Result<()> {
    let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("scenes");
    for name in ["field-orbit", "follow-latest"] {
        compile(&fixtures, name)?;
    }
    build_ble_fixture()?;
    Ok(())
}

/// The BLE fixture is generated from the WIT contract through a tiny guest
/// crate, never authored as component WAT. That is what gives the imported
/// instance the type identity the component model requires.
fn build_ble_fixture() -> anyhow::Result<()> {
    let ble = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("ble");
    let guest = ble.join("guest");
    let status = std::process::Command::new("cargo")
        .args([
            "build",
            "--release",
            "--target",
            "wasm32-unknown-unknown",
            "--manifest-path",
        ])
        .arg(guest.join("Cargo.toml"))
        .status()?;
    anyhow::ensure!(status.success(), "build soil:ble guest");
    let core = guest.join("target/wasm32-unknown-unknown/release/soil_ble_guest.wasm");
    let wasm = ble.join("soil-ble.wasm");
    let status = std::process::Command::new("wasm-tools")
        .args(["component", "new"])
        .arg(&core)
        .arg("-o")
        .arg(&wasm)
        .status()?;
    anyhow::ensure!(status.success(), "componentize soil:ble guest");
    println!("wrote {}", wasm.display());
    Ok(())
}

fn compile(dir: &std::path::Path, name: &str) -> anyhow::Result<()> {
    let source_path = dir.join(format!("{name}.wat"));
    let wasm_path = dir.join(format!("{name}.wasm"));
    let source = fs::read_to_string(&source_path)?;
    let wasm = wat::parse_str(&source)?;
    fs::write(&wasm_path, wasm)?;
    println!("wrote {}", wasm_path.display());
    Ok(())
}
