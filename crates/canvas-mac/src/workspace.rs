//! Persistent, project-scoped macOS window bindings.
//!
//! The GUI, CLI, and MCP server all call this module so project switching has
//! one persistence format and one activation path.
use crate::{activate_project_windows, work_area, Rect, Window};
use canvas_core::desktop::LayoutReceipt;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Bindings(pub BTreeMap<String, Vec<Window>>);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Activation {
    pub project: String,
    pub windows: Vec<Window>,
    pub layout: LayoutReceipt,
}

impl Bindings {
    pub fn windows(&self, project: &str) -> &[Window] {
        self.0.get(project).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn bind(&mut self, project: &str, window: Window) -> bool {
        let windows = self.0.entry(project.into()).or_default();
        if windows.iter().any(|bound| bound.id == window.id) {
            return false;
        }
        windows.push(window);
        true
    }

    pub fn unbind_id(&mut self, project: &str, window_id: &str) -> bool {
        let Some(windows) = self.0.get_mut(project) else {
            return false;
        };
        let before = windows.len();
        windows.retain(|window| window.id != window_id);
        windows.len() != before
    }

    pub fn unbind_index(&mut self, project: &str, index: usize) -> bool {
        let Some(windows) = self.0.get_mut(project) else {
            return false;
        };
        if index >= windows.len() {
            return false;
        }
        windows.remove(index);
        true
    }
}

pub fn path() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
            .join("Library/Application Support/Dock/window-bindings.json"),
    )
}

pub fn load() -> Result<Bindings, String> {
    load_from(&path()?)
}

fn load_from(path: &Path) -> Result<Bindings, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("Invalid window bindings: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Bindings::default()),
        Err(error) => Err(format!("Cannot read window bindings: {error}")),
    }
}

pub fn save(bindings: &Bindings) -> Result<(), String> {
    save_to(&path()?, bindings)
}

fn save_to(path: &Path, bindings: &Bindings) -> Result<(), String> {
    let parent = path.parent().ok_or("Window bindings path has no parent")?;
    let mut directory = fs::DirBuilder::new();
    directory.recursive(true);
    #[cfg(unix)]
    directory.mode(0o700);
    directory
        .create(parent)
        .map_err(|error| format!("Cannot create window bindings directory: {error}"))?;

    let bytes = serde_json::to_vec_pretty(bindings)
        .map_err(|error| format!("Cannot encode window bindings: {error}"))?;
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary)
        .map_err(|error| format!("Cannot create window bindings: {error}"))?;
    use std::io::Write;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Cannot write window bindings: {error}"))?;
    fs::rename(&temporary, path).map_err(|error| format!("Cannot install window bindings: {error}"))
}

pub fn activate(
    bindings: &mut Bindings,
    project: &str,
    area: Option<Rect>,
) -> Result<Activation, String> {
    let windows = bindings
        .0
        .get(project)
        .filter(|windows| !windows.is_empty())
        .ok_or("Project has no local window bindings")?
        .clone();
    let (resolved, layout) =
        activate_project_windows(area.map(Ok).unwrap_or_else(work_area)?, &windows)?;
    bindings.0.insert(project.into(), resolved.clone());
    Ok(Activation {
        project: project.into(),
        windows: resolved,
        layout,
    })
}

/// Activate a saved project only when it has local bindings. An unbound
/// project remains a valid remote-only workspace recall.
pub fn activate_saved_if_bound(project: &str) -> Result<Option<Activation>, String> {
    activate_saved_if_bound_at(project, None)
}

pub fn activate_saved_if_bound_at(
    project: &str,
    area: Option<Rect>,
) -> Result<Option<Activation>, String> {
    let mut bindings = load()?;
    if bindings.windows(project).is_empty() {
        return Ok(None);
    }
    let activation = activate(&mut bindings, project, area)?;
    save(&bindings)?;
    Ok(Some(activation))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(id: &str) -> Window {
        Window {
            id: id.into(),
            pid: 7,
            app: "Safari".into(),
            title: "Project".into(),
        }
    }

    #[test]
    fn bindings_deduplicate_and_unbind_by_stable_id() {
        let mut bindings = Bindings::default();
        assert!(bindings.bind("dock", window("one")));
        assert!(!bindings.bind("dock", window("one")));
        assert_eq!(bindings.windows("dock"), &[window("one")]);
        assert!(bindings.unbind_id("dock", "one"));
        assert!(!bindings.unbind_id("dock", "one"));
    }

    #[test]
    fn persistence_round_trips_without_changing_schema() {
        let root = std::env::temp_dir().join(format!("dock-workspace-test-{}", std::process::id()));
        let file = root.join("bindings.json");
        let mut bindings = Bindings::default();
        bindings.bind("dock", window("one"));
        save_to(&file, &bindings).unwrap();
        assert_eq!(load_from(&file).unwrap(), bindings);
        fs::remove_dir_all(root).unwrap();
    }
}
