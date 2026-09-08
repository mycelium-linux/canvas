//! Durable, owner-local project workspace documents.
//!
//! A workspace is a full Canvas desktop. The store keeps one exact document
//! per project; renderers retain decoded/GPU state separately and rebuild it
//! from these documents when a project becomes active again.

use crate::{Error, WorkspaceDocument};
use dock_core::Catalog;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA_VERSION: u32 = 1;
pub const MAX_PROJECT_WORKSPACES: usize = 128;
const MAX_STORE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStore {
    pub schema_version: u32,
    pub projects: BTreeMap<String, WorkspaceDocument>,
}

impl Default for WorkspaceStore {
    fn default() -> Self {
        Self {
            schema_version: STORE_SCHEMA_VERSION,
            projects: BTreeMap::new(),
        }
    }
}

impl WorkspaceStore {
    pub fn validate(&self, catalog: &Catalog) -> Result<(), Error> {
        if self.schema_version != STORE_SCHEMA_VERSION {
            return Err(Error(
                "Unsupported Canvas workspace store schema_version".into(),
            ));
        }
        if self.projects.len() > MAX_PROJECT_WORKSPACES {
            return Err(Error(format!(
                "Canvas workspace store exceeds {MAX_PROJECT_WORKSPACES} projects"
            )));
        }
        for (project_id, document) in &self.projects {
            if project_id != &document.project_id {
                return Err(Error(format!(
                    "Workspace key {project_id:?} does not match project_id {:?}",
                    document.project_id
                )));
            }
            document.validate(catalog)?;
        }
        Ok(())
    }

    pub fn upsert(&mut self, document: WorkspaceDocument) {
        self.projects.insert(document.project_id.clone(), document);
    }

    pub fn get(&self, project_id: &str) -> Option<&WorkspaceDocument> {
        self.projects.get(project_id)
    }

    pub fn project_ids(&self) -> impl Iterator<Item = &str> {
        self.projects.keys().map(String::as_str)
    }
}

pub fn default_path() -> Result<PathBuf, Error> {
    #[cfg(target_os = "macos")]
    let path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| Error("HOME is unavailable".into()))?
        .join("Library/Application Support/Dock/canvas/workspaces.json");
    #[cfg(not(target_os = "macos"))]
    let path = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".local/state"))
        })
        .ok_or_else(|| Error("XDG_STATE_HOME and HOME are unavailable".into()))?
        .join("dock/canvas/workspaces.json");
    Ok(path)
}

pub fn load(path: &Path, catalog: &Catalog) -> Result<WorkspaceStore, Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(WorkspaceStore::default());
        }
        Err(error) => return Err(Error(format!("Inspect Canvas workspace store: {error}"))),
    };
    if !metadata.file_type().is_file() || metadata.len() > MAX_STORE_BYTES {
        return Err(Error("Unsafe or oversized Canvas workspace store".into()));
    }
    #[cfg(unix)]
    if metadata.uid() != unsafe { libc::geteuid() } || metadata.permissions().mode() & 0o077 != 0 {
        return Err(Error(
            "Canvas workspace store must be owner-only and owner-owned".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    File::open(path)
        .and_then(|file| file.take(MAX_STORE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| Error(format!("Read Canvas workspace store: {error}")))?;
    if bytes.len() as u64 > MAX_STORE_BYTES {
        return Err(Error(
            "Canvas workspace store exceeds its byte limit".into(),
        ));
    }
    let store: WorkspaceStore = serde_json::from_slice(&bytes)
        .map_err(|error| Error(format!("Parse Canvas workspace store: {error}")))?;
    store.validate(catalog)?;
    Ok(store)
}

pub fn save(path: &Path, store: &WorkspaceStore, catalog: &Catalog) -> Result<(), Error> {
    store.validate(catalog)?;
    let parent = path
        .parent()
        .ok_or_else(|| Error("Canvas workspace store has no parent directory".into()))?;
    create_private_dir(parent)?;
    let temp = parent.join(format!(
        ".workspaces-{}-{}.tmp",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| Error(error.to_string()))?
            .as_nanos()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let file = options
            .open(&temp)
            .map_err(|error| Error(format!("Create Canvas workspace store: {error}")))?;
        let mut writer = BufWriter::new(file);
        serde_json::to_writer_pretty(&mut writer, store)
            .map_err(|error| Error(format!("Encode Canvas workspace store: {error}")))?;
        writer
            .write_all(b"\n")
            .and_then(|_| writer.flush())
            .map_err(|error| Error(format!("Write Canvas workspace store: {error}")))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| Error(format!("Sync Canvas workspace store: {error}")))?;
        fs::rename(&temp, path)
            .map_err(|error| Error(format!("Install Canvas workspace store: {error}")))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn create_private_dir(path: &Path) -> Result<(), Error> {
    if !path.exists() {
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        builder.mode(0o700);
        builder
            .create(path)
            .map_err(|error| Error(format!("Create Canvas state directory: {error}")))?;
    }
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| Error(format!("Inspect Canvas state directory: {error}")))?;
        if !metadata.file_type().is_dir()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
        {
            return Err(Error(
                "Canvas state directory must be owner-only and owner-owned".into(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Background, Layout, SCHEMA_VERSION, SurfaceChrome, ThemeRef};

    fn document(project_id: &str) -> WorkspaceDocument {
        WorkspaceDocument {
            schema_version: SCHEMA_VERSION,
            id: format!("{project_id}-workspace"),
            project_id: project_id.into(),
            revision: 1,
            theme: ThemeRef::Project,
            background: Background::default(),
            chrome: SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: Layout::Empty,
            bindings: Vec::new(),
            focused_surface_id: None,
        }
    }

    #[test]
    fn project_workspaces_round_trip_atomically_and_keep_exact_labels() {
        let catalog = Catalog::bundled().unwrap();
        let project_id = catalog.projects[0].id.clone();
        let root = std::env::temp_dir().join(format!(
            "dock-canvas-workspace-store-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("round-trip")
        ));
        let path = root.join("workspaces.json");
        let _ = fs::remove_dir_all(&root);
        let mut store = WorkspaceStore::default();
        store.upsert(document(&project_id));
        save(&path, &store, &catalog).unwrap();
        let loaded = load(&path, &catalog).unwrap();
        assert_eq!(loaded, store);
        assert_eq!(loaded.project_ids().collect::<Vec<_>>(), [project_id]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn mismatched_project_keys_fail_loudly() {
        let catalog = Catalog::bundled().unwrap();
        let project_id = catalog.projects[0].id.clone();
        let mut store = WorkspaceStore::default();
        store
            .projects
            .insert("wrong-project".into(), document(&project_id));
        assert!(
            store
                .validate(&catalog)
                .unwrap_err()
                .0
                .contains("does not match")
        );
    }
}
