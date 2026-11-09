//! Hand a shell region over to an installed Spore package.
//!
//! A `spore.toml` claims a slice of the shell in `[chrome]`: which regions the
//! package draws, and which composed Lua file draws each one. The host keeps its
//! own implementation of every region (see [`crate::shell_presentation`]) and
//! hands one over only after the claim resolves *and* the candidate presents,
//! because a Spore that fails must never leave a hole in the desktop. That rule
//! is the whole design, so refusal is not an error path here: the built-in
//! artifact is the answer, and the reason travels with it so the shell can say
//! what happened.
//!
//! ```text
//! [ui]
//! entry = "ui/divebar.lua"
//!
//! [chrome]
//! regions = ["top-bar"]
//!
//! [chrome.sources]
//! top-bar = "dist/top-bar.lua"   # optional; defaults to [ui] entry
//! ```
use crate::{
    Error, Result,
    lua_module::{
        InputDeclaration, MODULE_SCHEMA_VERSION, ModuleCapabilities, ModuleInputSnapshot,
        ModuleManifest, ModuleThemeTokens,
    },
    lua_scene::LuaSceneLimits,
    shell_presentation::{Artifact, Presentation},
};
use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

/// Chrome regions a Spore may claim.
///
/// Mirrors `soil_spore::CHROME_REGIONS` and the region names `chrome-host`
/// resolves against. An unlisted name is a typo or a claim this host cannot
/// honour, and either way the package is not trusted to draw anything.
pub const REGIONS: [&str; 7] = [
    TOP_BAR,
    "dock",
    "launcher",
    "switcher",
    "control-center",
    "notifications",
    "attention",
];

/// The region the desktop bar draws.
pub const TOP_BAR: &str = "top-bar";

/// First line `chrome-host compose` stamps on a composed artifact.
///
/// Composed output still *mentions* `require`, but only as the parameter of the
/// wrapper it installs, so scanning for the word would reject exactly the
/// artifacts that are loadable. The marker is instead the composer's own claim
/// that imports were inlined. Without it the file is an authored view, and
/// Canvas's scene VM deletes the `require` global
/// ([`crate::lua_scene`]), so loading it would raise once per frame instead of
/// once. Refusing an unmarked source here is what keeps that failure out of the
/// per-frame path.
const COMPOSED: &str = "-- chrome-composed: 1";

/// A `spore.toml` is metadata, not a scene, so this is a generous ceiling.
const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

/// A refusal reason, not a host error: everything that can go wrong while
/// resolving a package ends in the same place — the built-in implementation —
/// so resolution reports *why* it declined instead of propagating a failure that
/// would stop the shell drawing.
type Failure<T> = std::result::Result<T, String>;

/// A directory of installed Spore packages: one subdirectory per package, each
/// holding a `spore.toml`.
///
/// Resolution never fails. A missing directory, an unreadable package, or a
/// package that claims nothing all mean the same thing to the shell: its own
/// implementation stays up.
#[derive(Debug, Clone)]
pub struct Loader {
    packages: Option<PathBuf>,
}

impl Loader {
    /// Scan an explicit directory of installed packages.
    pub fn dir(packages: impl Into<PathBuf>) -> Self {
        Self {
            packages: Some(packages.into()),
        }
    }

    /// The operator's installed packages: `<runtime root>/spores`.
    ///
    /// The runtime root is the one tree Canvas already reads its operator
    /// configuration from (and `CANVAS_CONFIG_DIR` already overrides it for
    /// tests and isolated daemons), so installed Spores live beside that
    /// configuration instead of inventing a second location.
    pub fn installed() -> Self {
        Self {
            packages: crate::runtime_config::root().map(|root| root.join("spores")),
        }
    }

    /// The Lua an installed package draws `region` with, or `Ok(None)` when no
    /// package claims it.
    ///
    /// `Err` is a refusal reason for the host's status line, not a failure that
    /// stops drawing: [`present_files`] turns it into the built-in implementation.
    fn find(&self, region: &str) -> Failure<Option<Candidate>> {
        let Some(packages) = &self.packages else {
            return Ok(None);
        };
        let Ok(entries) = std::fs::read_dir(packages) else {
            return Ok(None);
        };
        let mut dirs: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|dir| dir.join("spore.toml").is_file())
            .collect();
        dirs.sort();
        // One region has one owner: the first package in directory order that
        // claims it decides, and a later package cannot paper over its refusal.
        let mut unreadable = None;
        for dir in dirs {
            let claim = match read_manifest(&dir.join("spore.toml")) {
                Ok(manifest) => manifest,
                Err(why) => {
                    // A manifest that cannot be read cannot be known not to
                    // claim this region, so it is reported rather than ignored.
                    unreadable.get_or_insert(why);
                    continue;
                }
            };
            let claims = claim
                .chrome
                .as_ref()
                .is_some_and(|chrome| chrome.regions.iter().any(|claimed| claimed == region));
            if !claims {
                continue;
            }
            return resolve(&dir, &claim, region).map(Some);
        }
        match unreadable {
            Some(why) => Err(why),
            None => Ok(None),
        }
    }
}

/// The outcome of [`present_files`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presented {
    /// Whether the retained scene changed.
    pub changed: bool,
    /// The built-in implementation stayed up because of this problem. `None`
    /// when a package presented, or when none claimed the region at all, so the
    /// host does not report the ordinary case.
    pub refused: Option<String>,
}

/// Present `region` from an installed Spore package, or from `builtin`.
///
/// This is the polling counterpart of [`Presentation::refresh_files`], and the
/// built-in branch is exactly that call, so an operator's own
/// `modules/desktop-bar-v1.*` files keep working untouched. The region is
/// handed over only when the claim resolves *and* the candidate presents. Every
/// other outcome is the built-in artifact, and [`Presentation::refresh`] keeps
/// its last good scene when a candidate fails, so neither a broken package nor a
/// refused scene can leave the desktop empty.
pub fn present_files(
    presentation: &mut Presentation,
    loader: &Loader,
    region: &str,
    builtin: Artifact,
    snapshot: ModuleInputSnapshot,
    theme: ModuleThemeTokens,
) -> Result<Presented> {
    let mut refused = None;
    match loader.find(region) {
        Ok(Some(candidate)) => {
            let attempt = candidate.manifest(region, &snapshot).and_then(|manifest| {
                presentation.refresh(&manifest, &candidate.source, snapshot.clone(), theme)
            });
            match attempt {
                Ok(changed) => {
                    return Ok(Presented {
                        changed,
                        refused: None,
                    });
                }
                Err(error) => refused = Some(format!("Spore {}: {error}", candidate.package)),
            }
        }
        Ok(None) => {}
        Err(why) => refused = Some(why),
    }
    let changed = presentation.refresh_files(builtin, snapshot, theme)?;
    Ok(Presented { changed, refused })
}

/// A claiming package whose region Lua was found and read.
struct Candidate {
    package: String,
    source: String,
}

impl Candidate {
    /// The module manifest a Spore-derived region needs.
    ///
    /// A package declares no module manifest of its own: `[chrome]` names a
    /// region and a Lua file and says nothing about inputs. This synthesizes
    /// the smallest manifest that can present one — no actions, because native
    /// chrome owns every control; scene writing and theme reading, so the
    /// region gets the semantic widget vocabulary; and one optional input per
    /// value the caller is supplying. Deriving the inputs from the snapshot
    /// rather than a static list is what keeps the manifest from drifting from
    /// the shell's own state: a region may read exactly the state it is handed.
    fn manifest(&self, region: &str, snapshot: &ModuleInputSnapshot) -> Result<String> {
        let inputs: Vec<InputDeclaration> = snapshot
            .values
            .iter()
            .map(|(id, value)| InputDeclaration {
                id: id.clone(),
                kind: value.kind(),
                required: false,
            })
            .collect();
        let manifest = ModuleManifest {
            schema_version: MODULE_SCHEMA_VERSION,
            id: module_id(&self.package),
            title: format!("{} ({region})", self.package),
            capabilities: ModuleCapabilities {
                write_scene: true,
                read_inputs: inputs.iter().map(|input| input.id.clone()).collect(),
                read_theme: true,
                present_resources: Vec::new(),
            },
            inputs,
            resources: Vec::new(),
            actions: Vec::new(),
        };
        manifest.validate()?;
        serde_json::to_string(&manifest).map_err(|error| Error(error.to_string()))
    }
}

/// The parts of a `spore.toml` a region needs.
///
/// Everything else the manifest carries (engine, launch surfaces, grants,
/// provenance) is Soil's business, not the shell's, so unknown fields are
/// ignored rather than rejected.
#[derive(serde::Deserialize)]
struct Manifest {
    spore: Spore,
    #[serde(default)]
    chrome: Option<Chrome>,
    #[serde(default)]
    ui: Option<Ui>,
}

#[derive(serde::Deserialize)]
struct Spore {
    id: String,
}

#[derive(serde::Deserialize)]
struct Chrome {
    #[serde(default)]
    regions: Vec<String>,
    #[serde(default)]
    sources: BTreeMap<String, String>,
}

#[derive(serde::Deserialize)]
struct Ui {
    entry: String,
}

fn read_manifest(path: &Path) -> Failure<Manifest> {
    let text = read_bounded(path, MAX_MANIFEST_BYTES)?;
    toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))
}

/// Resolve the region's Lua the way the reference host does: `[chrome.sources]`
/// first, then `[ui] entry`, both relative to the package directory.
///
/// Only the requested region is read. The reference host validates every
/// source a package ships at once; here a package whose `dock` file is missing
/// still gets to draw `top-bar`, because one region's fallback must not take
/// another region's owner away.
fn resolve(dir: &Path, manifest: &Manifest, region: &str) -> Failure<Candidate> {
    let chrome = manifest
        .chrome
        .as_ref()
        .ok_or_else(|| format!("{} declares no [chrome]", manifest.spore.id))?;
    for claimed in &chrome.regions {
        if !REGIONS.contains(&claimed.as_str()) {
            return Err(format!(
                "{} claims unknown chrome region {claimed:?}; known regions are {REGIONS:?}",
                manifest.spore.id
            ));
        }
    }
    let relative = chrome
        .sources
        .get(region)
        .or_else(|| manifest.ui.as_ref().map(|ui| &ui.entry))
        .ok_or_else(|| {
            format!(
                "{} claims {region} with no Lua: add [chrome.sources] or [ui] entry",
                manifest.spore.id
            )
        })?;
    let relative = safe_lua(relative)
        .map_err(|why| format!("{} claims {region}: {why}", manifest.spore.id))?;
    let source = read_bounded(
        &dir.join(&relative),
        LuaSceneLimits::default().max_source_bytes as u64,
    )
    .map_err(|why| format!("{} claims {region}: {why}", manifest.spore.id))?;
    if !composed(&source) {
        return Err(format!(
            "{} claims {region} with {}, which is not a composed artifact ({COMPOSED:?} \
             missing); run `chrome-host compose`",
            manifest.spore.id,
            relative.display()
        ));
    }
    Ok(Candidate {
        package: manifest.spore.id.clone(),
        source,
    })
}

/// A manifest path is attacker-adjacent input: the package directory is the
/// root, and a region may only name a `.lua` file inside it. Mirrors
/// `soil_spore::safe_relative`, which the installer already enforces, so a
/// hand-placed package is held to the same rule.
fn safe_lua(relative: &str) -> Failure<PathBuf> {
    if relative.is_empty()
        || relative.len() > 255
        || relative.starts_with('/')
        || relative.contains('\\')
        || relative.chars().any(char::is_control)
        || !relative.ends_with(".lua")
    {
        return Err(format!("{relative:?} is not a relative .lua artifact"));
    }
    let mut path = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            _ => return Err(format!("{relative:?} is not a relative .lua artifact")),
        }
    }
    if path.as_os_str().is_empty() {
        return Err(format!("{relative:?} is not a relative .lua artifact"));
    }
    Ok(path)
}

/// Whether `source` is a composed artifact; see [`COMPOSED`].
fn composed(source: &str) -> bool {
    source
        .lines()
        .next()
        .is_some_and(|line| line.trim_end() == COMPOSED)
}

/// A Spore id (`dev.fungos.divebar`) is not a Canvas module id (lowercase
/// identifier characters only), so everything else becomes `-`.
fn module_id(package: &str) -> String {
    let mut slug: String = package
        .chars()
        .map(|character| match character {
            'a'..='z' | '0'..='9' | '-' => character,
            'A'..='Z' => character.to_ascii_lowercase(),
            _ => '-',
        })
        .take(64)
        .collect();
    if !slug.starts_with(|character: char| character.is_ascii_lowercase()) {
        slug.insert_str(0, "spore-");
        slug.truncate(64);
    }
    slug
}

/// Read a regular file that must be `1..=max` bytes.
fn read_bounded(path: &Path, max: u64) -> Failure<String> {
    let metadata =
        std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    if metadata.len() > max {
        return Err(format!(
            "{} is {} bytes, over the {max}-byte budget",
            path.display(),
            metadata.len()
        ));
    }
    std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lua_module::ModuleInputValue as Value;
    use canvas_core::{Accent, ProjectTheme, ThemeMode};

    /// Exactly the state `refresh_desktop_bar` supplies, because the built-in
    /// bar's manifest requires every one of these inputs: a smaller snapshot
    /// would fail validation and make a fallback test prove nothing.
    fn snapshot() -> ModuleInputSnapshot {
        ModuleInputSnapshot {
            revision: 1,
            now_ms: 1_768_000_000_000,
            values: BTreeMap::from([
                ("width".into(), Value::Number(1408.0)),
                ("height".into(), Value::Number(44.0)),
                ("padding".into(), Value::Number(8.0)),
                ("labelsize".into(), Value::Number(12.0)),
                ("workspace".into(), Value::Text("HOME".into())),
                ("page".into(), Value::Text("DESKTOP".into())),
                ("clock".into(), Value::Text("12:34:56".into())),
                ("sources".into(), Value::Number(3.0)),
                ("back".into(), Value::Boolean(false)),
                ("forward".into(), Value::Boolean(true)),
                ("create".into(), Value::Boolean(false)),
            ]),
        }
    }

    fn theme() -> ModuleThemeTokens {
        ProjectTheme {
            mode: ThemeMode::Dark,
            accent: Accent::Info,
        }
        .into()
    }

    /// A composed region: the header `chrome-host compose` stamps, an import
    /// bound to a local parameter exactly as the composer emits it, and two
    /// rects. Nothing here is authored by Canvas.
    const REGION_LUA: &str = r#"-- chrome-composed: 1
-- Composed by chrome-host (stand-in). The `require` below is a parameter, so
-- the scene VM's deleted global is never consulted.
local ui = (function(require) return require("fungos.ui") end)(function()
  return { panel = function() end }
end)
canvas.rect {
  id = "spore-panel", ink = "surface", opacity = 1, radius = 0,
  translation = { -4, 4, 0 }, scale = { 8, 8, 1 },
}
canvas.rect {
  id = "spore-tile", ink = "primary", opacity = 1, radius = 0.5,
  translation = { -3, 4, 0.2 }, scale = { 2, 1, 1 },
}
"#;

    fn scratch(label: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "canvas-chrome-regions-{}-{label}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("scratch directory");
        path
    }

    /// Install one package: `<root>/pkg/spore.toml` plus any files it names.
    fn package(root: &Path, manifest: &str, files: &[(&str, &str)]) -> Loader {
        let dir = root.join("pkg");
        std::fs::create_dir_all(&dir).expect("package directory");
        std::fs::write(dir.join("spore.toml"), manifest).expect("spore.toml");
        for (relative, body) in files {
            let path = dir.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).expect("parent directory");
            }
            std::fs::write(path, body).expect("package file");
        }
        Loader::dir(root)
    }

    const CLAIMS_TOP_BAR: &str = r#"
[spore]
id = "dev.fungos.divebar"
name = "divebar"
version = "0.1.0"

[ui]
entry = "ui/divebar.lua"

[chrome]
regions = ["top-bar"]

[chrome.sources]
top-bar = "dist/top-bar.lua"
"#;

    /// The built-in bar with test-only paths, so `refresh_files` cannot pick up
    /// an operator's `modules/desktop-bar-v1.*` override off the machine running
    /// the tests and change what "the built-in bar" means.
    const BUILTIN: Artifact = Artifact {
        manifest_path: "test/modules/desktop-bar-v1.json",
        manifest_fallback: crate::shell_presentation::DESKTOP_BAR.manifest_fallback,
        source_path: "test/modules/desktop-bar-v1.lua",
        source_fallback: crate::shell_presentation::DESKTOP_BAR.source_fallback,
    };

    fn presented(loader: &Loader) -> (Presented, Vec<String>) {
        let mut presentation = Presentation::default();
        let presented = present_files(
            &mut presentation,
            loader,
            TOP_BAR,
            BUILTIN,
            snapshot(),
            theme(),
        )
        .expect("a refused region must not surface an error");
        let ids = presentation
            .retained()
            .expect("something is always presented")
            .scene
            .nodes
            .iter()
            .map(|node| node.id.clone())
            .collect();
        (presented, ids)
    }

    #[test]
    fn a_composed_region_presents_instead_of_the_built_in_bar() {
        let root = scratch("presents");
        let loader = package(&root, CLAIMS_TOP_BAR, &[("dist/top-bar.lua", REGION_LUA)]);
        let (presented, ids) = presented(&loader);
        assert!(presented.changed);
        assert_eq!(presented.refused, None, "a valid package must not report");
        assert!(ids.iter().any(|id| id == "spore-panel"), "{ids:?}");
        assert!(ids.iter().any(|id| id == "spore-tile"), "{ids:?}");
        assert!(
            !ids.iter().any(|id| id == "ground"),
            "the built-in bar must not also draw: {ids:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_region_source_keeps_the_built_in_bar() {
        let root = scratch("missing");
        let loader = package(&root, CLAIMS_TOP_BAR, &[]);
        let (presented, ids) = presented(&loader);
        assert!(presented.changed);
        assert!(
            presented
                .refused
                .as_deref()
                .is_some_and(|why| why.contains("dist/top-bar.lua")),
            "{presented:?}"
        );
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        assert!(!ids.iter().any(|id| id == "spore-panel"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unknown_region_claim_keeps_the_built_in_bar() {
        let root = scratch("unknown-region");
        let manifest = CLAIMS_TOP_BAR.replace(
            "regions = [\"top-bar\"]",
            "regions = [\"top-bar\", \"gutter\"]",
        );
        let loader = package(&root, &manifest, &[("dist/top-bar.lua", REGION_LUA)]);
        let (presented, ids) = presented(&loader);
        assert!(
            presented
                .refused
                .as_deref()
                .is_some_and(|why| why.contains("gutter")),
            "{presented:?}"
        );
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_malformed_manifest_keeps_the_built_in_bar() {
        let root = scratch("malformed");
        let loader = package(&root, "this is not [[[ toml", &[]);
        let (presented, ids) = presented(&loader);
        assert!(
            presented
                .refused
                .as_deref()
                .is_some_and(|why| why.contains("spore.toml")),
            "{presented:?}"
        );
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_authored_view_that_still_imports_is_refused_before_it_loads() {
        let root = scratch("authored");
        let authored = "local ui = require(\"fungos.ui\")\ncanvas.rect { id = \"spore-panel\" }\n";
        let loader = package(&root, CLAIMS_TOP_BAR, &[("dist/top-bar.lua", authored)]);
        let (presented, ids) = presented(&loader);
        assert!(
            presented
                .refused
                .as_deref()
                .is_some_and(|why| why.contains("chrome-composed")),
            "{presented:?}"
        );
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn no_installed_package_keeps_the_built_in_bar_silently() {
        let root = scratch("empty");
        let (presented, ids) = presented(&Loader::dir(&root));
        assert!(presented.changed);
        assert_eq!(presented.refused, None, "the ordinary case must be quiet");
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_package_that_claims_another_region_leaves_this_one_alone() {
        let root = scratch("other-region");
        let manifest = CLAIMS_TOP_BAR.replace("regions = [\"top-bar\"]", "regions = [\"dock\"]");
        let loader = package(&root, &manifest, &[("dist/top-bar.lua", REGION_LUA)]);
        let (presented, ids) = presented(&loader);
        assert_eq!(presented.refused, None);
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_region_source_outranks_the_ui_entry() {
        let root = scratch("resolution-order");
        let authored = "local ui = require(\"fungos.ui\")\n";
        let loader = package(
            &root,
            CLAIMS_TOP_BAR,
            &[
                ("ui/divebar.lua", authored),
                ("dist/top-bar.lua", REGION_LUA),
            ],
        );
        let (presented, ids) = presented(&loader);
        assert_eq!(presented.refused, None);
        assert!(ids.iter().any(|id| id == "spore-panel"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_region_with_no_lua_at_all_keeps_the_built_in_bar() {
        let root = scratch("no-lua");
        let manifest =
            "[spore]\nid = \"dev.fungos.divebar\"\n\n[chrome]\nregions = [\"top-bar\"]\n";
        let loader = package(&root, manifest, &[]);
        let (presented, ids) = presented(&loader);
        assert!(
            presented
                .refused
                .as_deref()
                .is_some_and(|why| why.contains("[ui] entry")),
            "{presented:?}"
        );
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_composed_region_that_raises_falls_back_after_the_attempt() {
        let root = scratch("broken-lua");
        let broken = "-- chrome-composed: 1\ncanvas.nope({ id = \"spore-panel\" })\n";
        let loader = package(&root, CLAIMS_TOP_BAR, &[("dist/top-bar.lua", broken)]);
        let (presented, ids) = presented(&loader);
        assert!(
            presented
                .refused
                .as_deref()
                .is_some_and(|why| why.contains("dev.fungos.divebar")),
            "{presented:?}"
        );
        assert!(ids.iter().any(|id| id == "ground"), "{ids:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn spore_ids_become_canvas_module_ids() {
        assert_eq!(module_id("dev.fungos.divebar"), "dev-fungos-divebar");
        assert_eq!(module_id("Studio_Bar"), "studio-bar");
        assert_eq!(module_id("9lives"), "spore-9lives");
        assert!(module_id(&"x".repeat(200)).len() <= 64);
    }
}
