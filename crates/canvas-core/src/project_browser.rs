//! Pure project-file browser over an Alfalfa manifest projection.
//!
//! Alfalfa owns revisions, names, folders and CIDs. Canvas owns only ephemeral
//! navigation and selection. Keeping this module pure makes the same browser
//! usable by native Canvas, remote edges and tests without teaching it how an
//! IPFS gateway or cache works.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::{appearance::Motion, manifest_query};

const MAX_ENTRIES: usize = 20_000;
const MAX_TEXT: usize = 512;
const MAX_PATH: usize = 2_048;
pub const SEARCH_CONTRACT: &str = "canvas.artifact.search.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectManifest {
    pub schema_version: u32,
    pub project_id: String,
    pub revision: String,
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestEntry {
    /// Stable manifest identity. This is not shown to a person.
    pub id: String,
    /// Slash-separated path relative to the project root.
    pub path: String,
    /// Human name from Alfalfa. This is what Canvas shows instead of a CID.
    pub display_name: String,
    /// Immutable content identity passed to the resolver unchanged.
    pub cid: String,
    pub media_type: String,
    pub bytes: u64,
    #[serde(default)]
    pub modified_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserState {
    /// Empty means project root.
    pub folder: String,
    pub query: String,
    #[serde(default)]
    pub selected_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserAction {
    Query(String),
    OpenFolder(String),
    Select(String),
    /// Open this exact file from an icon/list activation. Unlike
    /// `OpenSelected`, callers do not need a separate selection round-trip.
    Open(String),
    SelectNext,
    SelectPrevious,
    OpenParent,
    OpenSelected,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserModel {
    pub revision: String,
    pub breadcrumbs: Vec<Breadcrumb>,
    pub items: Vec<BrowserItem>,
    pub matched: usize,
    pub more: usize,
    pub selected: Option<ManifestEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breadcrumb {
    pub label: String,
    pub folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserItem {
    Folder {
        name: String,
        path: String,
        descendants: usize,
    },
    File {
        entry: ManifestEntry,
        preview: PreviewKind,
    },
}

impl BrowserItem {
    pub fn label(&self) -> &str {
        match self {
            Self::Folder { name, .. } => name,
            Self::File { entry, .. } => &entry.display_name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewKind {
    /// Decode a bounded image thumbnail through the media cache.
    Image,
    /// Rasterize page one through the PDF cache.
    Pdf,
    /// Ask the model renderer for its deterministic poster frame.
    Model,
    /// Render a bounded text excerpt.
    Markdown,
    Text,
    /// Show a themed type tile; never guess at bytes Canvas cannot render.
    Generic,
}

/// Exact query JARVIS (or another authorized caller) asks Canvas to present.
/// Relative language such as "last three months" is resolved to an absolute
/// cutoff by the caller, keeping execution reproducible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GalleryQuery {
    pub project_id: String,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub terms: Vec<String>,
    #[serde(default)]
    pub modified_after_ms: Option<u64>,
    #[serde(default)]
    pub modified_before_ms: Option<u64>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GalleryPlan {
    pub project_id: String,
    pub manifest_revision: String,
    pub query: GalleryQuery,
    pub slots: Vec<GallerySlot>,
    pub matched: usize,
    pub more: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GallerySlot {
    pub entry: ManifestEntry,
    pub preview: PreviewKind,
    /// Stable slots render immediately; content fades/settles in after this
    /// bounded delay as each cached thumbnail becomes available.
    pub reveal_after_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchResults {
    pub project_id: String,
    pub revision: String,
    pub query: String,
    pub results: Vec<ManifestEntry>,
    pub matched: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl ProjectManifest {
    pub const SCHEMA_VERSION: u32 = 1;

    pub fn validate(&self) -> Result<(), Error> {
        bounded("project id", &self.project_id, MAX_TEXT)?;
        bounded("revision", &self.revision, MAX_TEXT)?;
        if self.schema_version != Self::SCHEMA_VERSION {
            return Err(Error(format!(
                "unsupported project manifest schema {}",
                self.schema_version
            )));
        }
        if self.entries.len() > MAX_ENTRIES {
            return Err(Error(format!(
                "project manifest exceeds {MAX_ENTRIES} entries"
            )));
        }
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for entry in &self.entries {
            bounded("entry id", &entry.id, MAX_TEXT)?;
            bounded("display name", &entry.display_name, MAX_TEXT)?;
            bounded("CID", &entry.cid, MAX_TEXT)?;
            bounded("media type", &entry.media_type, MAX_TEXT)?;
            validate_path(&entry.path)?;
            if !ids.insert(entry.id.as_str()) {
                return Err(Error(format!("duplicate manifest entry id: {}", entry.id)));
            }
            if !paths.insert(entry.path.as_str()) {
                return Err(Error(format!("duplicate manifest path: {}", entry.path)));
            }
        }
        Ok(())
    }

    /// Preserve a useful location across manifest revisions without retaining
    /// an entry Alfalfa removed or renamed.
    pub fn reconcile(&self, state: &BrowserState) -> BrowserState {
        let folder = if state.folder.is_empty()
            || self
                .entries
                .iter()
                .any(|entry| parent_or_self(&entry.path, &state.folder))
        {
            state.folder.clone()
        } else {
            String::new()
        };
        let selected_id = state.selected_id.as_ref().and_then(|selected| {
            self.entries
                .iter()
                .any(|entry| entry.id == *selected)
                .then(|| selected.clone())
        });
        BrowserState {
            folder,
            query: state.query.clone(),
            selected_id,
        }
    }

    pub fn browse(&self, state: &BrowserState, limit: usize) -> Result<BrowserModel, Error> {
        self.validate()?;
        validate_folder(&state.folder)?;
        let query = state.query.trim().to_lowercase();
        let mut items = if query.is_empty() {
            self.folder_items(&state.folder)
        } else {
            self.search_items(&query)
        };
        items.sort_by(item_order);
        let matched = items.len();
        items.truncate(limit);
        let selected = state
            .selected_id
            .as_ref()
            .and_then(|id| self.entries.iter().find(|entry| entry.id == *id).cloned());
        Ok(BrowserModel {
            revision: self.revision.clone(),
            breadcrumbs: breadcrumbs(&self.project_id, &state.folder),
            more: matched.saturating_sub(items.len()),
            matched,
            items,
            selected,
        })
    }

    /// Apply one UI action. Opening returns the exact manifest entry for the
    /// authorized resolver; every other action changes only ephemeral state.
    pub fn apply(
        &self,
        state: &mut BrowserState,
        action: BrowserAction,
    ) -> Result<Option<ManifestEntry>, Error> {
        self.validate()?;
        match action {
            BrowserAction::Query(query) => {
                if query.len() > MAX_TEXT || query.chars().any(char::is_control) {
                    return Err(Error("invalid project browser query".into()));
                }
                state.query = query;
                state.selected_id = None;
            }
            BrowserAction::OpenFolder(folder) => {
                validate_folder(&folder)?;
                if !folder.is_empty()
                    && !self
                        .entries
                        .iter()
                        .any(|entry| parent_or_self(&entry.path, &folder))
                {
                    return Err(Error(format!("unknown project folder: {folder}")));
                }
                state.folder = folder;
                state.query.clear();
                state.selected_id = None;
            }
            BrowserAction::Select(id) => {
                if !self.entries.iter().any(|entry| entry.id == id) {
                    return Err(Error(format!("unknown project entry: {id}")));
                }
                state.selected_id = Some(id);
            }
            BrowserAction::Open(id) => {
                let entry = self
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .ok_or_else(|| Error(format!("unknown project entry: {id}")))?;
                state.selected_id = Some(id);
                return Ok(Some(entry.clone()));
            }
            BrowserAction::SelectNext | BrowserAction::SelectPrevious => {
                let model = self.browse(state, MAX_ENTRIES)?;
                let files = model
                    .items
                    .into_iter()
                    .filter_map(|item| match item {
                        BrowserItem::File { entry, .. } => Some(entry.id),
                        BrowserItem::Folder { .. } => None,
                    })
                    .collect::<Vec<_>>();
                if !files.is_empty() {
                    let current = state
                        .selected_id
                        .as_ref()
                        .and_then(|id| files.iter().position(|candidate| candidate == id));
                    let next = match action {
                        BrowserAction::SelectNext => {
                            current.map_or(0, |index| (index + 1) % files.len())
                        }
                        BrowserAction::SelectPrevious => current.map_or(files.len() - 1, |index| {
                            (index + files.len() - 1) % files.len()
                        }),
                        _ => unreachable!(),
                    };
                    state.selected_id = Some(files[next].clone());
                }
            }
            BrowserAction::OpenParent => {
                state.folder = state
                    .folder
                    .rsplit_once('/')
                    .map_or_else(String::new, |(parent, _)| parent.to_owned());
                state.query.clear();
                state.selected_id = None;
            }
            BrowserAction::OpenSelected => {
                let id = state
                    .selected_id
                    .as_deref()
                    .ok_or_else(|| Error("no project entry is selected".into()))?;
                let entry = self
                    .entries
                    .iter()
                    .find(|entry| entry.id == id)
                    .ok_or_else(|| Error(format!("selected project entry is stale: {id}")))?;
                return Ok(Some(entry.clone()));
            }
        }
        Ok(None)
    }

    pub fn gallery(&self, query: &GalleryQuery) -> Result<GalleryPlan, Error> {
        self.gallery_with_motion(query, Motion::default())
    }

    /// Run the shared manifest query language and return only owner-provided
    /// artifact metadata. Presentation (folders, labels, icons and widgets)
    /// remains a caller concern, normally a Lua adapter.
    pub fn search(&self, query: &str, limit: usize) -> Result<SearchResults, Error> {
        self.validate()?;
        if query.len() > MAX_TEXT || query.chars().any(char::is_control) {
            return Err(Error("invalid project manifest query".into()));
        }
        if limit == 0 || limit > 256 {
            return Err(Error("manifest search limit must be 1-256".into()));
        }
        let searchable = self.searchable_entries();
        let hits = manifest_query::run(&manifest_query::parse(query), &searchable, MAX_ENTRIES);
        let matched = hits.matched;
        let by_id: BTreeMap<_, _> = self
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect();
        let results = hits
            .hits
            .into_iter()
            .take(limit)
            .filter_map(|hit| by_id.get(hit.entry.id.as_str()).copied())
            .cloned()
            .collect();
        Ok(SearchResults {
            project_id: self.project_id.clone(),
            revision: self.revision.clone(),
            query: query.to_string(),
            results,
            matched,
        })
    }

    /// Build a stable reveal plan using the active Canvas motion language.
    /// Keeping the profile explicit makes remote and native renderers agree.
    pub fn gallery_with_motion(
        &self,
        query: &GalleryQuery,
        motion: Motion,
    ) -> Result<GalleryPlan, Error> {
        self.validate()?;
        query.validate()?;
        if query.project_id != self.project_id {
            return Err(Error(format!(
                "gallery query project {} does not match manifest {}",
                query.project_id, self.project_id
            )));
        }
        let searchable = self.searchable_entries();
        let mut parsed = manifest_query::Query::default();
        parsed.formats.clone_from(&query.formats);
        parsed.terms = query.terms.iter().map(|term| term.to_lowercase()).collect();
        let hits = manifest_query::run(&parsed, &searchable, MAX_ENTRIES);
        let by_id: BTreeMap<_, _> = self
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect();
        let mut entries: Vec<_> = hits
            .hits
            .into_iter()
            .filter_map(|hit| by_id.get(hit.entry.id.as_str()).copied())
            .filter(|entry| {
                query
                    .modified_after_ms
                    .is_none_or(|cutoff| entry.modified_at_ms.is_some_and(|at| at >= cutoff))
                    && query
                        .modified_before_ms
                        .is_none_or(|cutoff| entry.modified_at_ms.is_some_and(|at| at < cutoff))
            })
            .cloned()
            .collect();
        entries.sort_by(|left, right| {
            right
                .modified_at_ms
                .cmp(&left.modified_at_ms)
                .then_with(|| {
                    left.display_name
                        .to_lowercase()
                        .cmp(&right.display_name.to_lowercase())
                })
                .then_with(|| left.id.cmp(&right.id))
        });
        let matched = entries.len();
        entries.truncate(query.limit);
        let count = entries.len();
        let slots = entries
            .into_iter()
            .enumerate()
            .map(|(index, entry)| GallerySlot {
                reveal_after_ms: motion.reveal_delay_ms(index, count),
                preview: preview_kind(&entry),
                entry,
            })
            .collect();
        Ok(GalleryPlan {
            project_id: self.project_id.clone(),
            manifest_revision: self.revision.clone(),
            query: query.clone(),
            more: matched.saturating_sub(query.limit),
            matched,
            slots,
        })
    }

    fn searchable_entries(&self) -> Vec<manifest_query::Entry> {
        self.entries
            .iter()
            .map(|entry| manifest_query::Entry {
                id: entry.id.clone(),
                name: entry.display_name.clone(),
                path: Some(entry.path.clone()),
                content_type: entry.media_type.clone(),
                bytes: entry.bytes,
                store: manifest_query::Store::Project,
                projects: vec![self.project_id.clone()],
                created_at_ms: entry.modified_at_ms.unwrap_or(0),
                indexed_at_ms: entry.modified_at_ms,
            })
            .collect()
    }

    fn folder_items(&self, folder: &str) -> Vec<BrowserItem> {
        let mut folders: BTreeMap<String, usize> = BTreeMap::new();
        let mut files = Vec::new();
        for entry in &self.entries {
            let Some(relative) = strip_folder(&entry.path, folder) else {
                continue;
            };
            if let Some((child, _)) = relative.split_once('/') {
                *folders.entry(child.to_string()).or_default() += 1;
            } else {
                files.push(BrowserItem::File {
                    entry: entry.clone(),
                    preview: preview_kind(entry),
                });
            }
        }
        let mut items: Vec<_> = folders
            .into_iter()
            .map(|(name, descendants)| BrowserItem::Folder {
                path: join(folder, &name),
                name,
                descendants,
            })
            .collect();
        items.extend(files);
        items
    }

    fn search_items(&self, query: &str) -> Vec<BrowserItem> {
        let searchable = self.searchable_entries();
        let hits = manifest_query::run(&manifest_query::parse(query), &searchable, MAX_ENTRIES);
        let by_id: BTreeMap<_, _> = self
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect();
        hits.hits
            .into_iter()
            .filter_map(|hit| by_id.get(hit.entry.id.as_str()).copied())
            .cloned()
            .map(|entry| BrowserItem::File {
                preview: preview_kind(&entry),
                entry,
            })
            .collect()
    }
}

impl GalleryQuery {
    fn validate(&self) -> Result<(), Error> {
        bounded("gallery project id", &self.project_id, MAX_TEXT)?;
        if self.limit == 0 || self.limit > 256 {
            return Err(Error("gallery limit must be 1-256".into()));
        }
        if self.formats.len() > 16 || self.terms.len() > 16 {
            return Err(Error("gallery query has too many filters".into()));
        }
        for format in &self.formats {
            bounded("gallery format", format, 64)?;
        }
        for term in &self.terms {
            bounded("gallery term", term, MAX_TEXT)?;
        }
        if self
            .modified_after_ms
            .zip(self.modified_before_ms)
            .is_some_and(|(after, before)| after >= before)
        {
            return Err(Error("gallery time range is empty or reversed".into()));
        }
        Ok(())
    }
}

pub fn preview_kind(entry: &ManifestEntry) -> PreviewKind {
    let media = entry.media_type.to_lowercase();
    let extension = entry
        .path
        .rsplit_once('.')
        .map(|(_, value)| value.to_lowercase());
    if media.starts_with("image/") {
        PreviewKind::Image
    } else if media == "application/pdf" || extension.as_deref() == Some("pdf") {
        PreviewKind::Pdf
    } else if matches!(
        extension.as_deref(),
        Some("stl" | "obj" | "step" | "stp" | "gltf" | "glb")
    ) {
        PreviewKind::Model
    } else if media == "text/markdown" || matches!(extension.as_deref(), Some("md" | "markdown")) {
        PreviewKind::Markdown
    } else if media.starts_with("text/")
        || matches!(
            extension.as_deref(),
            Some("json" | "toml" | "yaml" | "yml" | "csv")
        )
    {
        PreviewKind::Text
    } else {
        PreviewKind::Generic
    }
}

fn bounded(label: &str, value: &str, max: usize) -> Result<(), Error> {
    if value.is_empty() || value.len() > max || value.chars().any(char::is_control) {
        return Err(Error(format!("invalid {label}")));
    }
    Ok(())
}

fn validate_path(path: &str) -> Result<(), Error> {
    if path.is_empty() || path.len() > MAX_PATH || path.starts_with('/') || path.ends_with('/') {
        return Err(Error(format!("invalid project path: {path}")));
    }
    if path.split('/').any(|part| {
        part.is_empty() || part == "." || part == ".." || part.chars().any(char::is_control)
    }) {
        return Err(Error(format!("invalid project path: {path}")));
    }
    Ok(())
}

fn validate_folder(folder: &str) -> Result<(), Error> {
    if folder.is_empty() {
        return Ok(());
    }
    validate_path(folder)
}

fn strip_folder<'a>(path: &'a str, folder: &str) -> Option<&'a str> {
    if folder.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(folder)?.strip_prefix('/')
    }
}

fn parent_or_self(path: &str, folder: &str) -> bool {
    path == folder
        || path
            .strip_prefix(folder)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn join(folder: &str, name: &str) -> String {
    if folder.is_empty() {
        name.to_string()
    } else {
        format!("{folder}/{name}")
    }
}

fn breadcrumbs(project: &str, folder: &str) -> Vec<Breadcrumb> {
    let mut result = vec![Breadcrumb {
        label: project.to_string(),
        folder: String::new(),
    }];
    let mut path = String::new();
    for component in folder.split('/').filter(|value| !value.is_empty()) {
        path = join(&path, component);
        result.push(Breadcrumb {
            label: component.to_string(),
            folder: path.clone(),
        });
    }
    result
}

fn item_order(left: &BrowserItem, right: &BrowserItem) -> std::cmp::Ordering {
    let rank = |item: &BrowserItem| matches!(item, BrowserItem::File { .. });
    rank(left)
        .cmp(&rank(right))
        .then_with(|| {
            left.label()
                .to_lowercase()
                .cmp(&right.label().to_lowercase())
        })
        .then_with(|| left.label().cmp(right.label()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, path: &str, name: &str, media: &str) -> ManifestEntry {
        ManifestEntry {
            id: id.into(),
            path: path.into(),
            display_name: name.into(),
            cid: format!("bafy{id}"),
            media_type: media.into(),
            bytes: 42,
            modified_at_ms: Some(7),
        }
    }

    fn manifest() -> ProjectManifest {
        ProjectManifest {
            schema_version: 1,
            project_id: "transmog".into(),
            revision: "r3".into(),
            entries: vec![
                entry("a", "models/enclosure.stl", "Enclosure", "model/stl"),
                entry("b", "docs/report.pdf", "Thermal report", "application/pdf"),
                entry("c", "docs/notes.md", "Build notes", "text/markdown"),
                entry("d", "renders/front.png", "Front", "image/png"),
            ],
        }
    }

    #[test]
    fn generic_search_returns_manifest_metadata_for_lua_presentation() {
        let results = manifest().search("pdf", 20).unwrap();
        assert_eq!(results.project_id, "transmog");
        assert_eq!(results.revision, "r3");
        assert_eq!(results.matched, 1);
        assert_eq!(results.results[0].media_type, "application/pdf");
    }

    #[test]
    fn root_is_folders_first_and_deterministic() {
        let model = manifest().browse(&BrowserState::default(), 20).unwrap();
        assert_eq!(
            model
                .items
                .iter()
                .map(BrowserItem::label)
                .collect::<Vec<_>>(),
            ["docs", "models", "renders"]
        );
    }

    #[test]
    fn folder_navigation_has_breadcrumbs_and_direct_children() {
        let state = BrowserState {
            folder: "docs".into(),
            ..Default::default()
        };
        let model = manifest().browse(&state, 20).unwrap();
        assert_eq!(
            model
                .breadcrumbs
                .iter()
                .map(|crumb| crumb.label.as_str())
                .collect::<Vec<_>>(),
            ["transmog", "docs"]
        );
        assert_eq!(
            model
                .items
                .iter()
                .map(BrowserItem::label)
                .collect::<Vec<_>>(),
            ["Build notes", "Thermal report"]
        );
    }

    #[test]
    fn search_names_and_paths_across_folders() {
        let state = BrowserState {
            query: "docs thermal".into(),
            ..Default::default()
        };
        let model = manifest().browse(&state, 20).unwrap();
        assert_eq!(
            model
                .items
                .iter()
                .map(BrowserItem::label)
                .collect::<Vec<_>>(),
            ["Thermal report"]
        );
    }

    #[test]
    fn friendly_duplicate_names_are_allowed_but_paths_and_ids_are_not() {
        let mut valid = manifest();
        valid.entries.push(entry(
            "e",
            "archive/notes.md",
            "Build notes",
            "text/markdown",
        ));
        valid.validate().unwrap();
        valid
            .entries
            .push(entry("e", "other.md", "Other", "text/markdown"));
        assert!(
            valid
                .validate()
                .unwrap_err()
                .0
                .contains("duplicate manifest entry id")
        );
    }

    #[test]
    fn traversal_and_absolute_paths_are_rejected() {
        for path in ["../secret", "docs/../secret", "/etc/passwd", "docs/"] {
            let mut value = manifest();
            value.entries[0].path = path.into();
            assert!(value.validate().is_err(), "{path}");
        }
    }

    #[test]
    fn revision_refresh_keeps_valid_location_and_drops_removed_selection() {
        let old = BrowserState {
            folder: "docs".into(),
            query: "report".into(),
            selected_id: Some("b".into()),
        };
        let mut next = manifest();
        next.revision = "r4".into();
        next.entries.retain(|entry| entry.id != "b");
        let state = next.reconcile(&old);
        assert_eq!(state.folder, "docs");
        assert_eq!(state.query, "report");
        assert_eq!(state.selected_id, None);
    }

    #[test]
    fn previews_use_existing_specialized_viewers() {
        let kinds: Vec<_> = manifest().entries.iter().map(preview_kind).collect();
        assert_eq!(
            kinds,
            [
                PreviewKind::Model,
                PreviewKind::Pdf,
                PreviewKind::Markdown,
                PreviewKind::Image
            ]
        );
    }

    #[test]
    fn result_limit_discloses_hidden_matches() {
        let model = manifest().browse(&BrowserState::default(), 2).unwrap();
        assert_eq!(model.items.len(), 2);
        assert_eq!(model.matched, 3);
        assert_eq!(model.more, 1);
    }

    #[test]
    fn actions_return_an_identity_only_when_opening() {
        let manifest = manifest();
        let mut state = BrowserState::default();
        assert_eq!(
            manifest
                .apply(&mut state, BrowserAction::OpenFolder("docs".into()))
                .unwrap(),
            None
        );
        manifest
            .apply(&mut state, BrowserAction::Select("b".into()))
            .unwrap();
        let opened = manifest
            .apply(&mut state, BrowserAction::OpenSelected)
            .unwrap()
            .unwrap();
        assert_eq!(opened.cid, "bafyb");
        assert_eq!(opened.path, "docs/report.pdf");

        let direct = manifest
            .apply(&mut state, BrowserAction::Open("a".into()))
            .unwrap()
            .unwrap();
        assert_eq!(direct.path, "models/enclosure.stl");
        assert_eq!(state.selected_id.as_deref(), Some("a"));
    }

    #[test]
    fn keyboard_navigation_moves_between_files_and_back_to_the_parent() {
        let manifest = manifest();
        let mut state = BrowserState {
            folder: "docs".into(),
            ..BrowserState::default()
        };
        manifest
            .apply(&mut state, BrowserAction::SelectNext)
            .unwrap();
        let first = state.selected_id.clone().expect("a file is selected");
        manifest
            .apply(&mut state, BrowserAction::SelectNext)
            .unwrap();
        assert_ne!(state.selected_id.as_deref(), Some(first.as_str()));
        manifest
            .apply(&mut state, BrowserAction::SelectPrevious)
            .unwrap();
        assert_eq!(state.selected_id.as_deref(), Some(first.as_str()));
        manifest
            .apply(&mut state, BrowserAction::OpenParent)
            .unwrap();
        assert!(state.folder.is_empty());
        assert!(state.selected_id.is_none());
    }

    #[test]
    fn structured_gallery_filters_format_project_and_absolute_time() {
        let mut manifest = manifest();
        manifest.entries[0].modified_at_ms = Some(100);
        manifest.entries[1].modified_at_ms = Some(300);
        manifest.entries.push(ManifestEntry {
            modified_at_ms: Some(200),
            ..entry("e", "docs/older.pdf", "Older report", "application/pdf")
        });
        let plan = manifest
            .gallery(&GalleryQuery {
                project_id: "transmog".into(),
                formats: vec!["pdf".into()],
                terms: vec![],
                modified_after_ms: Some(250),
                modified_before_ms: None,
                limit: 20,
            })
            .unwrap();
        assert_eq!(plan.slots.len(), 1);
        assert_eq!(plan.slots[0].entry.display_name, "Thermal report");
    }

    #[test]
    fn gallery_slots_are_newest_first_with_a_bounded_reveal() {
        let mut manifest = manifest();
        for (index, entry) in manifest.entries.iter_mut().enumerate() {
            entry.media_type = "application/pdf".into();
            entry.modified_at_ms = Some(index as u64);
        }
        let plan = manifest
            .gallery(&GalleryQuery {
                project_id: "transmog".into(),
                formats: vec!["pdf".into()],
                terms: vec![],
                modified_after_ms: None,
                modified_before_ms: None,
                limit: 256,
            })
            .unwrap();
        assert_eq!(plan.slots[0].entry.id, "d");
        assert_eq!(plan.slots[0].reveal_after_ms, 0);
        assert!(
            plan.slots
                .windows(2)
                .all(|pair| pair[0].reveal_after_ms < pair[1].reveal_after_ms)
        );
        assert!(plan.slots.last().unwrap().reveal_after_ms <= 2_000);
    }

    #[test]
    fn gallery_refuses_relative_or_ambiguous_execution_inputs() {
        let manifest = manifest();
        let error = manifest
            .gallery(&GalleryQuery {
                project_id: "other".into(),
                formats: vec!["pdf".into()],
                terms: vec![],
                modified_after_ms: None,
                modified_before_ms: None,
                limit: 20,
            })
            .unwrap_err();
        assert!(error.0.contains("does not match"));
    }
}
