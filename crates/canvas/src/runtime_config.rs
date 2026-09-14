//! Reloadable operator-authored Canvas configuration.

use crate::{Error, Result};
use std::path::{Component, Path, PathBuf};

pub fn read(relative: &Path, max_bytes: u64) -> Result<Option<Vec<u8>>> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error("invalid Canvas runtime configuration path".into()));
    }
    let Some(root) = root() else {
        return Ok(None);
    };
    let path = root.join(relative);
    let metadata = match std::fs::metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error(format!("cannot inspect {}: {error}", path.display()))),
    };
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > max_bytes {
        return Err(Error(format!(
            "Canvas runtime configuration {} must contain 1-{max_bytes} bytes",
            path.display()
        )));
    }
    std::fs::read(&path)
        .map(Some)
        .map_err(|error| Error(format!("cannot read {}: {error}", path.display())))
}

/// Write operator-authored configuration back to the runtime root.
///
/// Atomic by construction: the bytes land in a sibling temporary file and are
/// renamed over the target, so a crash mid-write leaves the previous document
/// intact rather than a truncated one. That matters more here than it looks —
/// [`read`] validates what it loads and refuses a malformed document, so a
/// half-written view would not render wrong, it would refuse to render at all.
///
/// Fails loudly when there is no runtime root. Silently discarding a save
/// because `HOME` is unset would lose the caller's work and report success.
pub fn write(relative: &Path, bytes: &[u8], max_bytes: u64) -> Result<PathBuf> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(Error("invalid Canvas runtime configuration path".into()));
    }
    let length = bytes.len() as u64;
    if length == 0 || length > max_bytes {
        return Err(Error(format!(
            "Canvas runtime configuration must contain 1-{max_bytes} bytes, not {length}"
        )));
    }
    let Some(root) = root() else {
        return Err(Error(
            "cannot save Canvas runtime configuration: no runtime root".into(),
        ));
    };
    write_under(&root, relative, bytes)
}

/// The filesystem half of [`write`], with the root passed in.
///
/// Split out so the atomicity and replacement behaviour can be tested against a
/// real directory without a test having to mutate the process-wide environment
/// that [`root`] reads — which would race every other test in the binary.
fn write_under(root: &Path, relative: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| Error(format!("cannot create {}: {error}", parent.display())))?;
    }
    let temporary = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&temporary, bytes)
        .map_err(|error| Error(format!("cannot write {}: {error}", temporary.display())))?;
    match std::fs::rename(&temporary, &path) {
        Ok(()) => Ok(path),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            Err(Error(format!("cannot replace {}: {error}", path.display())))
        }
    }
}

/// Load `appearance.json` from the runtime root and install it.
///
/// Absent means defaults, which is not an error: the file exists to override,
/// and requiring one would make a fresh machine fail to start.
///
/// A malformed one IS an error and is reported, because the alternative -
/// falling back silently - is indistinguishable from a setting that had no
/// effect, and the reason these values moved out of the code was to be able to
/// tell those apart.
pub fn load_appearance() -> Result<canvas_core::appearance::Appearance> {
    let relative = Path::new("appearance.json");
    let Some(bytes) = read(relative, 64 * 1024)? else {
        return Ok(canvas_core::appearance::current());
    };
    let text = std::str::from_utf8(&bytes)
        .map_err(|error| Error(format!("appearance.json is not UTF-8: {error}")))?;
    let parsed = canvas_core::appearance::Appearance::parse(text).map_err(Error)?;
    Ok(canvas_core::appearance::install(parsed))
}

pub fn root() -> Option<PathBuf> {
    canvas_core::runtime_root()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU32, Ordering};

    fn scratch(label: &str) -> PathBuf {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let unique = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "canvas-runtime-config-{}-{label}-{unique}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    #[test]
    fn rejects_parent_paths() {
        assert!(read(Path::new("../secret"), 100).is_err());
        assert!(read(Path::new("views/../../secret"), 100).is_err());
    }

    #[test]
    fn write_rejects_the_same_paths_read_does() {
        assert!(write(Path::new("../secret"), b"x", 100).is_err());
        assert!(write(Path::new("views/../../secret"), b"x", 100).is_err());
        assert!(write(Path::new("/etc/passwd"), b"x", 100).is_err());
    }

    #[test]
    fn write_refuses_what_read_would_later_reject() {
        // read() treats empty and oversize as errors, so writing either would
        // create a file that can never be loaded back.
        assert!(write(Path::new("views/a.json"), b"", 100).is_err());
        assert!(write(Path::new("views/a.json"), &[b'x'; 101], 100).is_err());
    }

    #[test]
    fn a_written_document_reads_back_byte_for_byte() {
        let root = scratch("roundtrip");
        let relative = Path::new("views/fleet-runtime.json");
        let body = br#"{"id":"fleet-runtime"}"#;
        let written = write_under(&root, relative, body).expect("write");
        assert_eq!(written, root.join(relative));
        assert_eq!(std::fs::read(&written).expect("read back"), body);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn writing_creates_missing_directories() {
        let root = scratch("mkdir");
        let written =
            write_under(&root, Path::new("views/nested/deep.json"), b"{}").expect("write");
        assert!(written.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_second_write_replaces_the_first_and_leaves_no_temporary_behind() {
        let root = scratch("replace");
        let relative = Path::new("views/v.json");
        write_under(&root, relative, b"first").expect("first write");
        write_under(&root, relative, b"second").expect("second write");
        assert_eq!(
            std::fs::read(root.join(relative)).expect("read back"),
            b"second"
        );
        let leftovers: Vec<_> = std::fs::read_dir(root.join("views"))
            .expect("list")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.contains("tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "atomic replace must not leave temporaries: {leftovers:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
