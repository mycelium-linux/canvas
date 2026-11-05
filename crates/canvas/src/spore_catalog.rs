//! Installed Spore packages exposed to the Canvas app switcher.
//!
//! The Canvas shell owns the launcher UI and workspace switching. This module
//! only reads package manifests from host-approved roots; it never executes a
//! package or grants a guest access to the filesystem.

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SporeApp {
    pub id: String,
    pub name: String,
    pub version: String,
    pub summary: String,
    pub workspace: String,
    pub default_surface: String,
}

#[derive(Deserialize)]
struct Manifest {
    spore: Meta,
    launch: Launch,
}

#[derive(Deserialize)]
struct Meta {
    id: String,
    name: String,
    version: String,
    summary: String,
}

#[derive(Deserialize)]
struct Launch {
    default: String,
}

pub fn roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = std::env::var_os("CANVAS_SPORE_ROOT") {
        roots.push(PathBuf::from(root));
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        roots.push(data.join("spores"));
    }
    if let Some(home) = std::env::var_os("HOME") {
        let home = PathBuf::from(home);
        // Where spores are installed, in the order a person would look: the
        // runtime directory a package manager writes, then the XDG data dir, then
        // the source trees a developer runs from. A launcher that only knows about
        // installed packages cannot show the one being worked on, and one that
        // only knows about source trees hides everything installed.
        roots.push(home.join("Library/Application Support/Dock/canvas/spores"));
        roots.push(home.join(".local/share/spores"));
        roots.push(home.join("src/fungOS/spores/spores"));
        roots.push(home.join("src/fungOS/spores-unibus/spores"));
    }
    roots.push(PathBuf::from("spores"));
    roots
}

pub fn scan() -> Vec<SporeApp> {
    let mut apps = Vec::new();
    for root in roots() {
        scan_root(&root, &mut apps);
    }
    apps.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
    apps.dedup_by(|left, right| left.id == right.id);
    apps
}

fn workspace_for(name: &str) -> String {
    match name {
        "unibus" => "spore-dev".into(),
        _ => format!("{name}-dev"),
    }
}

fn scan_root(root: &Path, apps: &mut Vec<SporeApp>) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let package = entry.path();
        if !package.is_dir() {
            continue;
        }
        let path = package.join("spore.toml");
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(manifest) = toml::from_str::<Manifest>(&source) else {
            continue;
        };
        if !manifest.spore.id.contains('.')
            || manifest.spore.name.is_empty()
            || manifest.launch.default.is_empty()
        {
            continue;
        }
        let display_name = match manifest.spore.name.as_str() {
            "unibus" => "Inbox",
            "contacts" => "Contacts",
            "launcher" => "Launcher",
            "terminal" => "Terminal",
            "markdown" => "Markdown",
            _ => manifest.spore.name.as_str(),
        }
        .to_string();
        apps.push(SporeApp {
            id: manifest.spore.id,
            name: display_name,
            version: manifest.spore.version,
            summary: manifest.spore.summary,
            workspace: workspace_for(&manifest.spore.name),
            default_surface: manifest.launch.default,
        });
    }
}
