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
    /// The composed Lua UI that launches this app on Canvas, when the package
    /// ships one. Without it the app needs its workspace installed first.
    pub canvas_module: Option<String>,
    /// Two characters for the launcher row, and the app's own colour.
    pub badge: Option<String>,
    pub tint: Option<[u8; 3]>,
}

#[derive(Deserialize)]
struct Manifest {
    spore: Meta,
    launch: Launch,
    #[serde(default)]
    presentation: Presentation,
    #[serde(default)]
    chrome: Chrome,
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
    #[serde(default)]
    canvas_module: Option<String>,
}

#[derive(Deserialize, Default)]
struct Chrome {
    #[serde(default)]
    regions: Vec<String>,
    #[serde(default)]
    sources: std::collections::BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
struct Presentation {
    #[serde(default)]
    badge: Option<String>,
    #[serde(default)]
    color: Option<String>,
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
        // The unibus tree first: it is the one carrying contacts, the launcher and
        // the inbox, so a duplicate package id here should resolve to the tree that
        // actually declares those apps.
        roots.push(home.join("src/fungOS/spores-unibus/spores"));
        roots.push(home.join("src/fungOS/spores/spores"));
    }
    roots.push(PathBuf::from("spores"));
    roots
}

/// A shell region a Spore draws, with the composed Lua that draws it.
///
/// Chrome is a region, not a window: the host owns the extent, input routing and
/// the fallback. A chrome Spore that fails to render must never leave a hole in
/// the desktop, so the built-in module takes over per region.
pub struct ChromeRegion {
    pub region: String,
    pub spore_id: String,
    pub source: String,
}

/// Resolve one region from the installed chrome Spores, first match wins.
///
/// Roots are ordered by trust: a runtime-installed package beats a source tree,
/// so a shipped chrome can be replaced without rebuilding the shell.
pub fn chrome_source(region: &str) -> Option<ChromeRegion> {
    for root in roots() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut packages: Vec<_> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
        packages.sort();
        for package in packages {
            if !package.is_dir() {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(package.join("spore.toml")) else {
                continue;
            };
            let Ok(manifest) = toml::from_str::<Manifest>(&source) else {
                continue;
            };
            if !manifest.chrome.regions.iter().any(|name| name == region) {
                continue;
            }
            let Some(relative) = manifest.chrome.sources.get(region) else {
                continue;
            };
            if relative.starts_with('/') || relative.contains("..") {
                continue;
            }
            let Ok(lua) = std::fs::read_to_string(package.join(relative)) else {
                continue;
            };
            if lua.len() > 256 * 1024 {
                continue;
            }
            return Some(ChromeRegion {
                region: region.to_owned(),
                spore_id: manifest.spore.id,
                source: lua,
            });
        }
    }
    None
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

/// `#rrggbb`, the notation a manifest author would write.
fn rgb(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if hex.len() != 6 {
        return None;
    }
    Some([
        u8::from_str_radix(&hex[0..2], 16).ok()?,
        u8::from_str_radix(&hex[2..4], 16).ok()?,
        u8::from_str_radix(&hex[4..6], 16).ok()?,
    ])
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
            canvas_module: manifest.launch.canvas_module.clone(),
            badge: manifest.presentation.badge.clone(),
            tint: manifest.presentation.color.as_deref().and_then(rgb),
        });
    }
}
