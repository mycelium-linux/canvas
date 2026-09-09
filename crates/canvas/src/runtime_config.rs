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

pub fn root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CANVAS_CONFIG_DIR") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    #[cfg(target_os = "macos")]
    return Some(home.join("Library/Application Support/Dock/canvas"));
    #[cfg(not(target_os = "macos"))]
    Some(
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("dock/canvas"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_parent_paths() {
        assert!(read(Path::new("../secret"), 100).is_err());
        assert!(read(Path::new("views/../../secret"), 100).is_err());
    }
}
