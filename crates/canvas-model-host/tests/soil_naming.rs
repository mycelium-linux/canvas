//! Naming gate for the slice-2 namespace rename: the old prefix -> `soil:`.
//!
//! The WIT packages were renamed in a913c40 and the capability identifiers in
//! `src/caps.rs` follow them. This test is the drift guard: a package id or
//! capability identifier that still carries the retired prefix fails the build
//! instead of surviving as a half-renamed surface that no manifest can reach.
//!
//! It scans the crate's `src/`, `wit/`, and `tests/` trees (plus the crate-level
//! Markdown docs) for `*.rs`, `*.wit`, `*.json`, and `*.md`, and fails on any
//! occurrence of the retired prefix. Prose that narrates the old namespace is
//! allowed only through [`ALLOWED_PROSE`], and only outside `src/` and `wit/`:
//! historical text may mention the old name, executable surface may not.

use std::path::{Path, PathBuf};

/// The retired namespace prefix, assembled at runtime so this file does not
/// match (and therefore does not need to allowlist) its own source text.
const RETIRED_PREFIX: &str = concat!("fleet", ":");

/// File types that can carry a package id or capability identifier.
const SCANNED_EXTENSIONS: &[&str] = &["rs", "wit", "json", "md"];

/// Historical-prose exemptions, relative to the crate root.
///
/// Empty by default: every identifier is expected to be renamed. Add an entry
/// only for a document that deliberately records the old name as history, and
/// never for a path under `src/` or `wit/` (see [`is_exempt`]).
const ALLOWED_PROSE: &[&str] = &[];

/// Trees scanned recursively from the crate root.
const SCANNED_DIRS: &[&str] = &["src", "wit", "tests"];

fn is_scanned_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| SCANNED_EXTENSIONS.contains(&ext))
}

fn is_exempt(root: &Path, path: &Path) -> bool {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let relative = relative.to_string_lossy().replace('\\', "/");

    // Executable surface is never exempt, even if a future entry is added.
    if relative.starts_with("src/") || relative.starts_with("wit/") {
        return false;
    }

    ALLOWED_PROSE.contains(&relative.as_str())
}

fn scan(root: &Path, dir: &Path, hits: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => panic!("cannot read {}: {error}", dir.display()),
    };

    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            scan(root, &path, hits);
            continue;
        }
        if !is_scanned_extension(&path) || is_exempt(root, &path) {
            continue;
        }

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) => panic!("cannot read {}: {error}", path.display()),
        };
        for (index, line) in text.lines().enumerate() {
            if line.contains(RETIRED_PREFIX) {
                hits.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
            }
        }
    }
}

fn collect_hits() -> (Vec<PathBuf>, Vec<String>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    let mut hits = Vec::new();

    for dir in SCANNED_DIRS {
        scan(root, &root.join(dir), &mut hits);
    }
    for extension in SCANNED_EXTENSIONS {
        // Crate-level docs (README.md) are part of the published surface too.
        for entry in std::fs::read_dir(root).expect("crate root") {
            let path = entry.expect("directory entry").path();
            if path.is_file()
                && path.extension().and_then(|ext| ext.to_str()) == Some(extension)
                && !is_exempt(root, &path)
            {
                files.push(path);
            }
        }
    }
    for path in &files {
        let text = std::fs::read_to_string(path).expect("crate-level doc");
        for (index, line) in text.lines().enumerate() {
            if line.contains(RETIRED_PREFIX) {
                hits.push(format!("{}:{}: {}", path.display(), index + 1, line.trim()));
            }
        }
    }

    (files, hits)
}

/// The gate itself: no retired namespace identifier anywhere in the crate.
#[test]
fn no_retired_fleet_namespace_identifiers() {
    let (_, hits) = collect_hits();
    assert!(
        hits.is_empty(),
        "retired `{RETIRED_PREFIX}` identifier(s) still present; rename them to `soil:`:\n{}",
        hits.join("\n")
    );
}

/// Guard against the gate passing because it scanned nothing.
#[test]
fn gate_is_not_vacuous() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut scanned = 0usize;

    for dir in SCANNED_DIRS {
        let mut stack = vec![root.join(dir)];
        while let Some(path) = stack.pop() {
            for entry in std::fs::read_dir(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()))
            {
                let path = entry.expect("directory entry").path();
                if path.is_dir() {
                    stack.push(path);
                } else if is_scanned_extension(&path) && !is_exempt(root, &path) {
                    scanned += 1;
                }
            }
        }
    }

    assert!(
        scanned >= 10,
        "expected the naming gate to cover the crate's Rust, WIT, JSON and Markdown files, \
         but it found only {scanned}"
    );
    assert!(
        root.join("src/caps.rs").is_file(),
        "caps.rs must be scanned"
    );
    assert!(
        root.join("wit/soil-gpu.wit").is_file(),
        "the renamed WIT package must be scanned"
    );
}
