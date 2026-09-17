//! Durable, owner-local project workspace documents.
//!
//! A workspace is a full Canvas desktop. The store keeps one exact document
//! per project; renderers retain decoded/GPU state separately and rebuild it
//! from these documents when a project becomes active again.

use crate::{
    Background, Error, Layout, SCHEMA_VERSION, SurfaceChrome, ThemeRef, WorkspaceDocument,
    control::{MAX_PROJECT_WORKSPACES, OverlaySettings, ProjectWorkspaceSpec},
    display::DisplayTarget,
};
use canvas_core::{Catalog, Project};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA_VERSION: u32 = 2;
const MAX_STORE_BYTES: u64 = 4 * 1024 * 1024;

/// How many projects back "go back" can reach.
///
/// Bounded because the trail would otherwise grow with every switch for the life
/// of the install, inside a store already capped at [`MAX_STORE_BYTES`]. Sixteen
/// is well past the point where someone still remembers which one they meant.
const MAX_RECENT_PROJECTS: usize = 16;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceStore {
    pub schema_version: u32,
    #[serde(default)]
    pub definitions: BTreeMap<String, ProjectWorkspaceSpec>,
    pub projects: BTreeMap<String, WorkspaceDocument>,
    #[serde(default)]
    pub active_project_id: Option<String>,
    /// Projects departed, most recent first, never including the active one.
    ///
    /// `active_project_id` answers "where am I" and nothing else: the instant a
    /// switch lands it is overwritten by the destination, so the place you came
    /// from is gone and "go back" has nothing to name. This is that missing
    /// half, and it is persisted rather than held in the GUI because the point
    /// is to survive the restart that loses the human's own memory of the order.
    #[serde(default)]
    pub recent_project_ids: Vec<String>,
    /// Saved display and overlay policy, restored across restarts.
    ///
    /// Absent in v1 stores; defaults are applied when None.
    #[serde(default)]
    pub display_policy: Option<DisplayPolicy>,
}

/// A durable, validated display policy. Canvas restores this after restart
/// so the wall does not need to be reconfigured every morning.
///
/// `DisplayTarget::Exact` carries display IDs resolved from live topology
/// at load time — a monitor plugged into a different port still resolves to
/// the same name, and an unplugged one is rejected. Role targets
/// (`All`, `Ordinals`) are topology-relative and resolve on each start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayPolicy {
    #[serde(default)]
    pub target: DisplayTarget,
    #[serde(default)]
    pub overlays: OverlaySettings,
}

impl Default for DisplayPolicy {
    fn default() -> Self {
        Self {
            target: DisplayTarget::All,
            overlays: OverlaySettings::default(),
        }
    }
}

impl DisplayPolicy {
    /// Validate against live display inventory, resolving role targets and
    /// rejecting Exact targets that name absent displays.
    pub fn validate(&self, display_ids: &[String]) -> Result<(), Error> {
        match &self.target {
            DisplayTarget::Exact { display_ids: ids } => {
                for id in ids {
                    if !display_ids.contains(id) {
                        return Err(Error(format!(
                            "Display policy references unknown display: {id}"
                        )));
                    }
                }
            }
            DisplayTarget::All => {}
            DisplayTarget::Ordinals { ordinals } => {
                for ordinal in ordinals {
                    if *ordinal > display_ids.len() {
                        return Err(Error(format!(
                            "Display policy ordinals {ordinal} exceed available displays"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

impl Default for WorkspaceStore {
    fn default() -> Self {
        Self {
            schema_version: STORE_SCHEMA_VERSION,
            definitions: BTreeMap::new(),
            projects: BTreeMap::new(),
            active_project_id: None,
            recent_project_ids: Vec::new(),
            display_policy: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncReport {
    pub created: usize,
    pub updated: usize,
    pub preserved: usize,
    pub total: usize,
}

impl WorkspaceStore {
    /// Make `project_id` active, remembering where we came from.
    ///
    /// Most-recently-used order, not a browser-style stack: someone toggling
    /// between two projects who says "go back" twice means to end up where they
    /// started. A stack would instead walk them a third project away on the
    /// second "back", which is the opposite of what the words promise.
    ///
    /// Switching to the project already active is not navigation and records
    /// nothing; otherwise a repeated no-op switch would push the same id until
    /// it displaced everything else worth remembering.
    pub fn activate(&mut self, project_id: &str) {
        if self.active_project_id.as_deref() == Some(project_id) {
            return;
        }
        if let Some(departed) = self.active_project_id.replace(project_id.to_string()) {
            self.recent_project_ids.insert(0, departed);
        }
        // Where you are is never also somewhere to go back to, and a project
        // appears once however many times it has been visited.
        let mut seen = BTreeSet::new();
        self.recent_project_ids
            .retain(|id| id != project_id && seen.insert(id.clone()));
        self.recent_project_ids.truncate(MAX_RECENT_PROJECTS);
    }

    /// Where "go back" goes, or `None` when there is nowhere to go.
    ///
    /// Skips projects whose workspace has since been removed rather than
    /// failing: a project deleted months ago must not make "go back" an error,
    /// and it must not silently become a switch to something that cannot load.
    pub fn previous_project_id(&self) -> Option<&str> {
        self.recent_project_ids
            .iter()
            .map(String::as_str)
            .find(|id| self.projects.contains_key(*id))
    }

    pub fn validate(&self, catalog: &Catalog) -> Result<(), Error> {
        if self.schema_version != STORE_SCHEMA_VERSION {
            return Err(Error(
                "Unsupported Canvas workspace store schema_version".into(),
            ));
        }
        if self.projects.len() > MAX_PROJECT_WORKSPACES
            || self.definitions.len() > MAX_PROJECT_WORKSPACES
        {
            return Err(Error(format!(
                "Canvas workspace store exceeds {MAX_PROJECT_WORKSPACES} projects"
            )));
        }
        for (project_id, definition) in &self.definitions {
            if project_id != &definition.project_id {
                return Err(Error(format!(
                    "Project definition key {project_id:?} does not match project_id {:?}",
                    definition.project_id
                )));
            }
            definition.validate().map_err(Error)?;
        }
        let effective_catalog = self.effective_catalog(catalog)?;
        for (project_id, document) in &self.projects {
            if project_id != &document.project_id {
                return Err(Error(format!(
                    "Workspace key {project_id:?} does not match project_id {:?}",
                    document.project_id
                )));
            }
            document.validate(&effective_catalog)?;
        }
        if self
            .active_project_id
            .as_ref()
            .is_some_and(|id| !self.projects.contains_key(id))
        {
            return Err(Error(
                "Active Canvas project must have a saved workspace".into(),
            ));
        }
        // Bounded on the way in as well as the way out: a store handed to us
        // with a million-entry trail is the same denial of service as one with
        // a million workspaces, and `activate` is not the only writer of a file
        // a person can edit. Stale ids are not an error here — they are skipped
        // by `previous_project_id`, because a deleted project should cost you
        // one step of history, not the whole store.
        if self.recent_project_ids.len() > MAX_RECENT_PROJECTS {
            return Err(Error(
                "Canvas workspace history exceeds the retained project count".into(),
            ));
        }
        Ok(())
    }

    pub fn effective_catalog(&self, base: &Catalog) -> Result<Catalog, Error> {
        let mut projects = base.projects.clone();
        for definition in self.definitions.values() {
            if let Some(project) = projects
                .iter_mut()
                .find(|project| project.id == definition.project_id)
            {
                project.name.clone_from(&definition.name);
                project.theme = definition.theme;
            } else {
                projects.push(Project {
                    id: definition.project_id.clone(),
                    name: definition.name.clone(),
                    theme: definition.theme,
                    resources: Vec::new(),
                });
            }
        }
        let catalog = Catalog {
            schema_version: base.schema_version,
            projects,
        };
        catalog
            .validate()
            .map_err(|error| Error(error.to_string()))?;
        Ok(catalog)
    }

    /// Upsert public project definitions and create only missing desktops.
    /// Existing documents remain the source of truth for layout and appearance.
    pub fn sync(
        &mut self,
        projects: Vec<ProjectWorkspaceSpec>,
        base: &Catalog,
    ) -> Result<SyncReport, Error> {
        if projects.is_empty() || projects.len() > MAX_PROJECT_WORKSPACES {
            return Err(Error(format!(
                "Canvas workspace sync must contain 1-{MAX_PROJECT_WORKSPACES} projects"
            )));
        }
        let mut ids = BTreeSet::new();
        for definition in &projects {
            definition.validate().map_err(Error)?;
            if !ids.insert(definition.project_id.as_str()) {
                return Err(Error(
                    "Canvas workspace sync contains duplicate project ids".into(),
                ));
            }
        }
        let mut next = self.clone();
        let mut created = 0;
        let mut updated = 0;
        let mut preserved = 0;
        for definition in projects {
            if next.definitions.get(&definition.project_id) != Some(&definition) {
                updated += 1;
            }
            next.definitions
                .insert(definition.project_id.clone(), definition.clone());
            let inherited_empty =
                next.projects
                    .get(&definition.project_id)
                    .is_some_and(|document| {
                        document.id == format!("{}-workspace", definition.project_id)
                            && document.revision == 1
                            && document.surfaces.is_empty()
                            && document.bindings.is_empty()
                            && matches!(document.layout, Layout::Empty)
                            && document.theme == ThemeRef::Project
                            && document.background == Background::Theme
                            && document.chrome == SurfaceChrome::default()
                            && document.focused_surface_id.is_none()
                    });
            if next.projects.contains_key(&definition.project_id) && !inherited_empty {
                preserved += 1;
            } else {
                next.projects.insert(
                    definition.project_id.clone(),
                    default_alfalfa_workspace(&definition.project_id)?,
                );
                created += 1;
            }
        }
        next.validate(base)?;
        let report = SyncReport {
            created,
            updated,
            preserved,
            total: next.projects.len(),
        };
        *self = next;
        Ok(report)
    }

    pub fn upsert(&mut self, document: WorkspaceDocument) {
        self.projects.insert(document.project_id.clone(), document);
    }

    pub fn get(&self, project_id: &str) -> Option<&WorkspaceDocument> {
        self.projects.get(project_id)
    }

    /// Replace one project-specific override with the current inherited
    /// Alfalfa workspace. Other projects and navigation state are untouched.
    pub fn reset_to_default(
        &mut self,
        project_id: &str,
        base: &Catalog,
    ) -> Result<WorkspaceDocument, Error> {
        if !self.definitions.contains_key(project_id) {
            return Err(Error(format!("Unknown Alfalfa workspace: {project_id}")));
        }
        let document = default_alfalfa_workspace(project_id)?;
        let mut next = self.clone();
        next.projects
            .insert(project_id.to_string(), document.clone());
        next.validate(base)?;
        *self = next;
        Ok(document)
    }

    pub fn project_ids(&self) -> impl Iterator<Item = &str> {
        self.projects.keys().map(String::as_str)
    }

    /// The saved display policy, or the default if none was persisted.
    pub fn display_policy(&self) -> DisplayPolicy {
        self.display_policy.clone().unwrap_or_default()
    }

    /// Set and persist the display policy.
    pub fn set_display_policy(&mut self, policy: DisplayPolicy) {
        self.display_policy = Some(policy);
    }
}

fn default_alfalfa_workspace(project_id: &str) -> Result<WorkspaceDocument, Error> {
    let stream = canvas_alfalfa::dashboard_stream(project_id);
    serde_json::from_value(serde_json::json!({
        "schema_version": SCHEMA_VERSION,
        "id": format!("{project_id}-workspace"),
        "project_id": project_id,
        "revision": 1,
        "theme": {"source": "project"},
        "background": {"kind": "theme"},
        "chrome": {"title": "hidden", "canvas_padding": 16, "surface_gap": 12},
        "surfaces": [
            {"id": "project-html", "title": "Project dashboard", "content": {"kind": "live_image", "source_id": format!("{stream}-html")}},
            widget_surface("project-goals", "Goals", "stat", serde_json::json!({"label": "Tracked goals", "value": 0.0, "history": []})),
            widget_surface("project-work", "Open marbles", "stat", serde_json::json!({"label": "Open marbles", "value": 0.0, "history": []})),
            widget_surface("project-agents", "Agents", "stat", serde_json::json!({"label": "Running agents", "value": 0.0, "history": []})),
            widget_surface("project-reviewers", "Reviewers", "stat", serde_json::json!({"label": "Reviewers", "value": 0.0, "history": []})),
        ],
        "layout": {"kind": "canvas", "placements": [
            {"surface_id": "project-html", "bounds": {"x": 0.0, "y": 0.0, "width": 0.55, "height": 1.0}},
            {"surface_id": "project-goals", "bounds": {"x": 0.55, "y": 0.0, "width": 0.225, "height": 0.22}},
            {"surface_id": "project-work", "bounds": {"x": 0.775, "y": 0.0, "width": 0.225, "height": 0.22}},
            {"surface_id": "project-agents", "bounds": {"x": 0.55, "y": 0.22, "width": 0.225, "height": 0.22}},
            {"surface_id": "project-reviewers", "bounds": {"x": 0.775, "y": 0.22, "width": 0.225, "height": 0.22}},
        ]},
        "bindings": [
            dashboard_binding("project-goals-bind", &stream, "project-goals", "alfalfa-project-goals-v1"),
            dashboard_binding("project-work-bind", &stream, "project-work", "alfalfa-project-work-v1"),
            dashboard_binding("project-agents-bind", &stream, "project-agents", "alfalfa-project-agents-v1"),
            dashboard_binding("project-reviewers-bind", &stream, "project-reviewers", "alfalfa-project-reviewers-v1"),
        ],
    }))
    .map_err(|error| Error(format!("Build default Alfalfa workspace: {error}")))
}

fn widget_surface(id: &str, title: &str, kind: &str, body: serde_json::Value) -> serde_json::Value {
    let mut spec = body;
    spec["schema_version"] = serde_json::json!(2);
    spec["id"] = serde_json::json!(id);
    spec["kind"] = serde_json::json!(kind);
    spec["data"] = serde_json::json!({"state": "loading", "source": "alfalfa"});
    serde_json::json!({
        "id": id,
        "title": title,
        "content": {"kind": "widget", "spec": spec},
        "inputs": [{"id": "widget", "contract": "canvas.widget.snapshot.v1"}],
    })
}

fn dashboard_binding(id: &str, stream: &str, surface_id: &str, adapter: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "source": {"owner": "alfalfa", "stream": stream, "contract": "canvas.widget.snapshot.v1"},
        "target": {"surface_id": surface_id, "input_id": "widget"},
        "adapter": adapter,
        "delivery": {"mode": "latest"},
    })
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

/// Read the store back, salvaging around individually invalid workspaces.
/// The returned notes name every workspace dropped, so a boot can report
/// what it refused instead of quietly rewriting history.
pub fn load(path: &Path, catalog: &Catalog) -> Result<(WorkspaceStore, Vec<String>), Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((WorkspaceStore::default(), Vec::new()));
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
    let mut store: WorkspaceStore = serde_json::from_slice(&bytes)
        .map_err(|error| Error(format!("Parse Canvas workspace store: {error}")))?;
    // Salvage, don't annihilate: one workspace for an Alfalfa-discovered
    // project (or one legacy document that no longer validates) must not
    // discard every other arrangement — that silent reset destroyed users'
    // walls (dock-8gr). Each document gets the lenient structural pass now;
    // strict catalog membership resumes at switch/apply/save, when the live
    // catalog is complete. Rejections are returned as notes for the caller
    // to report — loud at boot, never swallowed.
    let effective = store.effective_catalog(catalog)?;
    let mut rejected = Vec::new();
    let mut notes = Vec::new();
    for (project_id, document) in &store.projects {
        if let Err(error) = document.validate_with(&effective, false) {
            notes.push(format!("workspace {project_id} rejected: {error}"));
            rejected.push(project_id.clone());
        }
    }
    for project_id in rejected {
        store.projects.remove(&project_id);
    }
    Ok((store, notes))
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
    use canvas_core::{Accent, ThemeMode, ThemeSelection};

    pub(super) fn definition(project_id: &str, name: &str) -> ProjectWorkspaceSpec {
        ProjectWorkspaceSpec {
            project_id: project_id.into(),
            name: name.into(),
            status: "active".into(),
            theme: ThemeSelection::Coherence {
                mode: ThemeMode::Dark,
                accent: Accent::Success,
            },
        }
    }

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
            "canvas-workspace-store-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("round-trip")
        ));
        let path = root.join("workspaces.json");
        let _ = fs::remove_dir_all(&root);
        let mut store = WorkspaceStore::default();
        store.upsert(document(&project_id));
        save(&path, &store, &catalog).unwrap();
        let loaded = load(&path, &catalog).unwrap().0;
        assert_eq!(loaded, store);
        assert_eq!(loaded.project_ids().collect::<Vec<_>>(), [project_id]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn load_salvages_unknown_projects_and_reports_rejects() {
        let catalog = Catalog::bundled().unwrap();
        let known = catalog.projects[0].id.clone();
        let root = std::env::temp_dir().join(format!(
            "canvas-workspace-salvage-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("salvage")
        ));
        let path = root.join("workspaces.json");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let mut store = WorkspaceStore::default();
        store.upsert(document(&known));
        store.projects.insert("ghost".into(), document("ghost"));
        let mut broken = document("broken");
        broken.id = "Bad Id!".into();
        store.projects.insert("broken".into(), broken);
        // Written raw, because strict save() refuses the ghost — that is
        // precisely the real shape: the store was saved against a complete
        // (Alfalfa-merged) catalog and meets a bare bundled one at boot.
        fs::write(&path, serde_json::to_vec_pretty(&store).unwrap()).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let (loaded, notes) = load(&path, &catalog).unwrap();
        assert!(loaded.get(&known).is_some(), "valid workspace survives");
        assert!(
            loaded.get("ghost").is_some(),
            "an unknown project's workspace must survive boot, not annihilate the store"
        );
        assert!(
            loaded.get("broken").is_none(),
            "structural garbage is dropped"
        );
        assert!(
            notes.iter().any(|note| note.contains("broken")),
            "every drop is reported: {notes:?}"
        );
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

    #[test]
    fn catalog_sync_creates_missing_desktops_and_preserves_saved_appearance() {
        let base = Catalog::bundled().unwrap();
        let mut store = WorkspaceStore::default();
        let mut dock = document("dock");
        dock.background = Background::Solid {
            color: [10, 20, 30, 255],
        };
        dock.chrome.canvas_padding = 17;
        store.upsert(dock.clone());

        let report = store
            .sync(
                vec![
                    definition("dock", "Dock renamed upstream"),
                    definition("gh-1288853134", "ohmOS"),
                ],
                &base,
            )
            .unwrap();

        assert_eq!(report.created, 1);
        assert_eq!(report.preserved, 1);
        assert_eq!(store.get("dock"), Some(&dock));
        let ohm = store.get("gh-1288853134").unwrap();
        assert_eq!(ohm.background, Background::Theme);
        assert_eq!(ohm.theme, ThemeRef::Project);
        assert_eq!(ohm.surfaces.len(), 5);
        assert_eq!(ohm.bindings.len(), 4);
        assert!(
            ohm.bindings
                .iter()
                .all(|binding| binding.source.stream == "gh-1288853134-dashboard")
        );
        let catalog = store.effective_catalog(&base).unwrap();
        assert_eq!(catalog.project("dock").unwrap().resources.len(), 2);
        assert_eq!(
            catalog.project("dock").unwrap().name,
            "Dock renamed upstream"
        );
        assert_eq!(catalog.project("gh-1288853134").unwrap().name, "ohmOS");
    }

    #[test]
    fn catalog_sync_upgrades_generated_empty_desktops_to_current_default() {
        let base = Catalog::bundled().unwrap();
        let mut store = WorkspaceStore::default();
        store.upsert(document("ohm"));

        let report = store.sync(vec![definition("ohm", "Ohm")], &base).unwrap();

        assert_eq!(report.created, 1);
        assert_eq!(report.preserved, 0);
        assert_eq!(store.get("ohm").unwrap().surfaces.len(), 5);
    }

    #[test]
    fn dynamic_catalog_and_active_workspace_survive_restart() {
        let base = Catalog::bundled().unwrap();
        let root = std::env::temp_dir().join(format!(
            "canvas-dynamic-workspace-store-test-{}",
            std::process::id()
        ));
        let path = root.join("workspaces.json");
        let _ = fs::remove_dir_all(&root);
        let mut store = WorkspaceStore::default();
        store
            .sync(vec![definition("gh-1288853134", "ohmOS")], &base)
            .unwrap();
        store.active_project_id = Some("gh-1288853134".into());
        save(&path, &store, &base).unwrap();

        let loaded = load(&path, &base).unwrap().0;
        assert_eq!(loaded, store);
        assert_eq!(loaded.active_project_id.as_deref(), Some("gh-1288853134"));
        assert!(
            loaded
                .effective_catalog(&base)
                .unwrap()
                .project("gh-1288853134")
                .is_ok()
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejected_sync_does_not_partially_mutate_the_store() {
        let base = Catalog::bundled().unwrap();
        let mut store = WorkspaceStore::default();
        let before = store.clone();
        let project = definition("gh-1288853134", "ohmOS");
        assert!(store.sync(vec![project.clone(), project], &base).is_err());
        assert_eq!(store, before);
    }

    #[test]
    fn project_overrides_survive_sync_and_reset_is_scoped() {
        let base = Catalog::bundled().unwrap();
        let mut store = WorkspaceStore::default();
        store
            .sync(
                vec![definition("ohm", "Ohm"), definition("dock", "Dock")],
                &base,
            )
            .unwrap();
        store.projects.get_mut("ohm").unwrap().background = Background::Solid {
            color: [1, 2, 3, 255],
        };
        store.projects.get_mut("dock").unwrap().revision = 7;

        store
            .sync(
                vec![definition("ohm", "Ohm"), definition("dock", "Dock")],
                &base,
            )
            .unwrap();
        assert!(matches!(
            store.get("ohm").unwrap().background,
            Background::Solid { .. }
        ));

        let reset = store.reset_to_default("ohm", &base).unwrap();
        assert_eq!(reset.background, Background::Theme);
        assert_eq!(reset.surfaces.len(), 5);
        assert_eq!(store.get("dock").unwrap().revision, 7);
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    /// Build a store the way the application does, through `sync`, so the
    /// projects are registered in the effective catalog. Inserting documents
    /// directly leaves them unknown to the catalog and `validate` rightly
    /// rejects them, which has nothing to do with what these tests assert.
    fn store_with(projects: &[&str]) -> WorkspaceStore {
        let base = Catalog::bundled().expect("bundled catalog");
        let mut store = WorkspaceStore::default();
        store
            .sync(
                projects
                    .iter()
                    .map(|id| super::tests::definition(id, id))
                    .collect(),
                &base,
            )
            .expect("sync test projects");
        store
    }

    #[test]
    fn going_back_twice_returns_where_you_started() {
        // The whole promise of the words. A stack would answer `alfalfa` here,
        // walking the human a third project away on the second "back".
        let mut store = store_with(&["dock", "alfalfa", "bifrost"]);
        store.activate("alfalfa");
        store.activate("dock");
        store.activate("bifrost");

        let back = store
            .previous_project_id()
            .expect("somewhere to go")
            .to_string();
        assert_eq!(back, "dock");
        store.activate(&back);
        assert_eq!(store.previous_project_id(), Some("bifrost"));
    }

    #[test]
    fn a_project_is_remembered_once_however_often_it_is_visited() {
        let mut store = store_with(&["dock", "alfalfa"]);
        for _ in 0..5 {
            store.activate("dock");
            store.activate("alfalfa");
        }
        assert_eq!(store.recent_project_ids, vec!["dock".to_string()]);
        assert_eq!(store.active_project_id.as_deref(), Some("alfalfa"));
    }

    #[test]
    fn switching_to_where_you_already_are_is_not_navigation() {
        let mut store = store_with(&["dock", "alfalfa"]);
        store.activate("dock");
        store.activate("alfalfa");
        store.activate("alfalfa");
        // Otherwise a repeated no-op switch pushes the same id until it
        // displaces everything actually worth remembering.
        assert_eq!(store.recent_project_ids, vec!["dock".to_string()]);
    }

    #[test]
    fn a_deleted_project_costs_one_step_of_history_not_the_store() {
        let mut store = store_with(&["dock", "alfalfa", "bifrost"]);
        store.activate("dock");
        store.activate("alfalfa");
        store.activate("bifrost");
        store.projects.remove("alfalfa");

        assert_eq!(store.previous_project_id(), Some("dock"));
        let catalog = Catalog::bundled().expect("bundled catalog");
        assert!(
            store.validate(&catalog).is_ok(),
            "a stale id must not invalidate the store"
        );
    }

    #[test]
    fn the_trail_is_bounded_in_both_directions() {
        let ids: Vec<String> = (0..MAX_RECENT_PROJECTS + 8)
            .map(|n| format!("p{n}"))
            .collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        let mut store = store_with(&refs);
        for id in &ids {
            store.activate(id);
        }
        assert_eq!(store.recent_project_ids.len(), MAX_RECENT_PROJECTS);
        // Newest first: the project left most recently is the first one back.
        assert_eq!(store.recent_project_ids[0], ids[ids.len() - 2]);

        // And a file someone hand-edited cannot smuggle an unbounded trail in.
        store.recent_project_ids = ids.clone();
        let catalog = Catalog::bundled().expect("bundled catalog");
        assert!(store.validate(&catalog).is_err());
    }

    #[test]
    fn a_store_written_before_history_existed_still_loads() {
        // `recent_project_ids` is `serde(default)`, so the field's arrival does
        // not strand a store from the previous build.
        let json = serde_json::json!({
            "schema_version": STORE_SCHEMA_VERSION,
            "projects": {},
            "active_project_id": null,
        });
        let store: WorkspaceStore = serde_json::from_value(json).expect("older store");
        assert!(store.recent_project_ids.is_empty());
        assert_eq!(store.previous_project_id(), None);
    }

    #[test]
    fn a_v1_store_without_display_policy_loads_with_defaults() {
        // `display_policy` is serde(default), so a v1 store without the field
        // loads as None, and display_policy() returns the default.
        let json = serde_json::json!({
            "schema_version": 1,
            "projects": {},
            "active_project_id": null,
        });
        let store: WorkspaceStore = serde_json::from_value(json).expect("v1 store");
        assert_eq!(store.display_policy(), DisplayPolicy::default());
    }

    #[test]
    fn display_policy_rejects_unknown_display_ids() {
        let policy = DisplayPolicy {
            target: DisplayTarget::Exact {
                display_ids: vec!["missing-display".into()],
            },
            ..Default::default()
        };
        let available = vec!["display-1".to_string()];
        assert!(policy.validate(&available).is_err());
    }

    #[test]
    fn display_policy_accepts_known_display_ids() {
        let policy = DisplayPolicy {
            target: DisplayTarget::Exact {
                display_ids: vec!["display-1".to_string()],
            },
            ..Default::default()
        };
        let available = vec!["display-1".to_string()];
        assert!(policy.validate(&available).is_ok());
    }

    #[test]
    fn display_policy_round_trips_through_json() {
        let policy = DisplayPolicy {
            target: DisplayTarget::Ordinals { ordinals: vec![1] },
            overlays: OverlaySettings {
                presence_rail: crate::control::OverlayTarget::Rightmost,
                ..Default::default()
            },
        };
        let json = serde_json::to_value(&policy).expect("serialize");
        let restored: DisplayPolicy = serde_json::from_value(json).expect("deserialize");
        assert_eq!(restored, policy);
    }
}
