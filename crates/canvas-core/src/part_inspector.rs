//! Pure part-and-assembly projection over an Alfalfa project manifest.
//!
//! Alfalfa remains the only owner of artifact identity, paths, revisions and
//! CIDs. This module merely composes [`ProjectManifest`] entries into the shape
//! an inspector needs; it performs no resolution, caching or rendering.

use std::collections::BTreeMap;

use crate::project_browser::{Error, ManifestEntry, PreviewKind, ProjectManifest, preview_kind};
use serde::{Deserialize, Serialize};

const MAX_ITERATION_PROMPT: usize = 8_192;
const MAX_ITERATION_ARTIFACTS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PartKind {
    Part,
    Assembly,
    Folder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ArtifactRole {
    Model,
    Drawing,
    Metadata,
    Supporting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartArtifact {
    pub entry: ManifestEntry,
    pub preview: PreviewKind,
    pub role: ArtifactRole,
    /// Revision named by a `revision/<name>` or `revisions/<name>` folder.
    /// The manifest revision still identifies the complete project snapshot.
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectedPart {
    /// Stable within the manifest path language (`parts/bracket`, for example).
    pub id: String,
    pub name: String,
    pub kind: PartKind,
    pub artifacts: Vec<PartArtifact>,
}

impl InspectedPart {
    pub fn models(&self) -> impl Iterator<Item = &PartArtifact> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.role == ArtifactRole::Model)
    }

    pub fn drawings(&self) -> impl Iterator<Item = &PartArtifact> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.role == ArtifactRole::Drawing)
    }

    pub fn metadata(&self) -> impl Iterator<Item = &PartArtifact> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.role == ArtifactRole::Metadata)
    }

    pub fn revisions(&self) -> impl Iterator<Item = &str> {
        let mut revisions = self
            .artifacts
            .iter()
            .map(|artifact| artifact.revision.as_str())
            .collect::<Vec<_>>();
        revisions.sort_unstable();
        revisions.dedup();
        revisions.into_iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartProject {
    pub project_id: String,
    pub manifest_revision: String,
    pub parts: Vec<InspectedPart>,
    /// Entries without a part identity remain visible at project scope. The
    /// projection never invents a part merely to avoid an unassigned artifact.
    pub project_artifacts: Vec<PartArtifact>,
}

/// Narrow Canvas -> Transmog handoff. It names immutable manifest artifacts;
/// transport and process lifecycle deliberately live elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationRequest {
    pub project_id: String,
    pub manifest_revision: String,
    pub part_id: String,
    pub prompt: String,
    pub artifacts: Vec<IterationArtifact>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IterationArtifact {
    pub id: String,
    pub cid: String,
}

impl IterationRequest {
    pub fn validate(&self) -> Result<(), Error> {
        bounded_request("project id", &self.project_id, 512, false)?;
        bounded_request("manifest revision", &self.manifest_revision, 512, false)?;
        bounded_request("part id", &self.part_id, 2_048, false)?;
        bounded_request("iteration prompt", &self.prompt, MAX_ITERATION_PROMPT, true)?;
        if self.artifacts.is_empty() || self.artifacts.len() > MAX_ITERATION_ARTIFACTS {
            return Err(Error(format!(
                "iteration request must select 1-{MAX_ITERATION_ARTIFACTS} artifacts"
            )));
        }
        let mut ids = std::collections::BTreeSet::new();
        for artifact in &self.artifacts {
            bounded_request("artifact id", &artifact.id, 512, false)?;
            bounded_request("artifact CID", &artifact.cid, 512, false)?;
            if !ids.insert(artifact.id.as_str()) {
                return Err(Error(format!(
                    "duplicate iteration artifact id: {}",
                    artifact.id
                )));
            }
        }
        Ok(())
    }
}

impl ProjectManifest {
    /// Compose this exact manifest revision into inspectable parts.
    ///
    /// Explicit `parts/<name>/...` and `assemblies/<name>/...` folders win.
    /// All other entries stay at project scope rather than being guessed into
    /// a part from filenames or media categories.
    pub fn inspect_parts(&self) -> Result<PartProject, Error> {
        self.validate()?;
        let mut parts: BTreeMap<String, InspectedPart> = BTreeMap::new();
        let mut project_artifacts = Vec::new();

        for entry in &self.entries {
            let artifact = classify_artifact(entry, &self.revision);
            let Some(identity) = part_identity(entry) else {
                project_artifacts.push(artifact);
                continue;
            };
            parts
                .entry(identity.id.clone())
                .or_insert_with(|| InspectedPart {
                    id: identity.id,
                    name: identity.name,
                    kind: identity.kind,
                    artifacts: Vec::new(),
                })
                .artifacts
                .push(artifact);
        }

        for part in parts.values_mut() {
            if !part
                .artifacts
                .iter()
                .any(|artifact| matches!(artifact.preview, PreviewKind::Model | PreviewKind::Pdf))
            {
                return Err(Error(format!(
                    "inspector folder has no model or PDF: {}",
                    part.id
                )));
            }
            part.artifacts.sort_by(artifact_order);
        }
        project_artifacts.sort_by(artifact_order);

        Ok(PartProject {
            project_id: self.project_id.clone(),
            manifest_revision: self.revision.clone(),
            parts: parts.into_values().collect(),
            project_artifacts,
        })
    }

    /// Inspect the explicit folder containing a selected manifest entry.
    /// `parts/<name>` and `assemblies/<name>` are stable scope boundaries;
    /// otherwise the entry's immediate parent folder is used.
    pub fn inspect_entry(&self, entry_id: &str) -> Result<InspectedPart, Error> {
        self.validate()?;
        let selected = self
            .entries
            .iter()
            .find(|entry| entry.id == entry_id)
            .ok_or_else(|| Error(format!("unknown project entry: {entry_id}")))?;
        let scope = selected
            .path
            .rsplit_once('/')
            .map_or(String::new(), |(parent, _)| parent.to_string());
        self.inspect_folder(&scope)
    }

    /// Compose one exact manifest folder into an inspector model.
    pub fn inspect_folder(&self, folder: &str) -> Result<InspectedPart, Error> {
        self.validate()?;
        let identity = folder_identity(folder);
        let mut artifacts = self
            .entries
            .iter()
            .filter(|entry| {
                folder.is_empty()
                    || entry
                        .path
                        .strip_prefix(folder)
                        .is_some_and(|rest| rest.starts_with('/'))
            })
            .map(|entry| classify_artifact(entry, &self.revision))
            .collect::<Vec<_>>();
        if artifacts.is_empty() {
            return Err(Error(format!(
                "inspector folder has no artifacts: {folder}"
            )));
        }
        if !artifacts
            .iter()
            .any(|artifact| matches!(artifact.preview, PreviewKind::Model | PreviewKind::Pdf))
        {
            return Err(Error(format!(
                "inspector folder has no model or PDF: {folder}"
            )));
        }
        artifacts.sort_by(artifact_order);
        Ok(InspectedPart {
            id: identity.id,
            name: identity.name,
            kind: identity.kind,
            artifacts,
        })
    }

    pub fn iteration_request(
        &self,
        part: &InspectedPart,
        prompt: impl Into<String>,
        selected_artifact_ids: &[String],
    ) -> Result<IterationRequest, Error> {
        self.validate()?;
        let part_entries = part
            .artifacts
            .iter()
            .map(|artifact| (artifact.entry.id.as_str(), &artifact.entry))
            .collect::<BTreeMap<_, _>>();
        let manifest_entries = self
            .entries
            .iter()
            .map(|entry| (entry.id.as_str(), entry))
            .collect::<BTreeMap<_, _>>();
        let artifacts = selected_artifact_ids
            .iter()
            .map(|id| {
                let entry = part_entries
                    .get(id.as_str())
                    .ok_or_else(|| Error(format!("artifact is outside inspector scope: {id}")))?;
                let manifest_entry = manifest_entries
                    .get(id.as_str())
                    .filter(|candidate| candidate.cid == entry.cid)
                    .ok_or_else(|| Error(format!("artifact is not in this manifest: {id}")))?;
                Ok(IterationArtifact {
                    id: manifest_entry.id.clone(),
                    cid: manifest_entry.cid.clone(),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let request = IterationRequest {
            project_id: self.project_id.clone(),
            manifest_revision: self.revision.clone(),
            part_id: part.id.clone(),
            prompt: prompt.into(),
            artifacts,
        };
        request.validate()?;
        Ok(request)
    }
}

struct PartIdentity {
    id: String,
    name: String,
    kind: PartKind,
}

fn part_identity(entry: &ManifestEntry) -> Option<PartIdentity> {
    explicit_scope(&entry.path).map(|scope| folder_identity(&scope))
}

fn explicit_scope(path: &str) -> Option<String> {
    let components = path.split('/').collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        match component.to_ascii_lowercase().as_str() {
            "parts" | "part" | "assemblies" | "assembly" => {}
            _ => continue,
        }
        components.get(index + 1)?;
        return Some(components[..=index + 1].join("/"));
    }
    None
}

fn folder_identity(folder: &str) -> PartIdentity {
    if folder.is_empty() {
        return PartIdentity {
            id: String::new(),
            name: "Project".into(),
            kind: PartKind::Folder,
        };
    }
    let components = folder.split('/').collect::<Vec<_>>();
    let name = components.last().copied().unwrap_or("project");
    let kind = components
        .iter()
        .rev()
        .nth(1)
        .map(|parent| match parent.to_ascii_lowercase().as_str() {
            "parts" | "part" => PartKind::Part,
            "assemblies" | "assembly" => PartKind::Assembly,
            _ => PartKind::Folder,
        })
        .unwrap_or(PartKind::Folder);
    PartIdentity {
        id: folder.to_string(),
        name: humanize(name),
        kind,
    }
}

fn bounded_request(label: &str, value: &str, max: usize, multiline: bool) -> Result<(), Error> {
    let invalid_control = value.chars().any(|character| {
        character.is_control() && (!multiline || !matches!(character, '\n' | '\t'))
    });
    if value.trim().is_empty() || value.len() > max || invalid_control {
        return Err(Error(format!("invalid {label}")));
    }
    Ok(())
}

fn classify_artifact(entry: &ManifestEntry, manifest_revision: &str) -> PartArtifact {
    let preview = preview_kind(entry);
    let lower_path = entry.path.to_ascii_lowercase();
    let components = lower_path.split('/').collect::<Vec<_>>();
    let role = if preview == PreviewKind::Model {
        ArtifactRole::Model
    } else if preview == PreviewKind::Pdf
        && (components
            .iter()
            .any(|part| matches!(*part, "drawing" | "drawings"))
            || lower_path.contains("drawing"))
    {
        ArtifactRole::Drawing
    } else if components.contains(&"metadata")
        || lower_path.ends_with("metadata.json")
        || lower_path.ends_with("metadata.toml")
        || lower_path.ends_with("metadata.yaml")
        || lower_path.ends_with("metadata.yml")
    {
        ArtifactRole::Metadata
    } else {
        ArtifactRole::Supporting
    };
    let revision = components
        .windows(2)
        .find(|pair| matches!(pair[0], "revision" | "revisions"))
        .map(|pair| pair[1].to_string())
        .unwrap_or_else(|| manifest_revision.to_string());
    PartArtifact {
        entry: entry.clone(),
        preview,
        role,
        revision,
    }
}

fn humanize(value: &str) -> String {
    value
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().chain(chars).collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn artifact_order(left: &PartArtifact, right: &PartArtifact) -> std::cmp::Ordering {
    left.role
        .cmp(&right.role)
        .then_with(|| left.revision.cmp(&right.revision))
        .then_with(|| left.entry.path.cmp(&right.entry.path))
        .then_with(|| left.entry.id.cmp(&right.entry.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, path: &str, media_type: &str) -> ManifestEntry {
        ManifestEntry {
            id: id.into(),
            path: path.into(),
            display_name: path.rsplit('/').next().unwrap().into(),
            cid: format!("bafy{id}"),
            media_type: media_type.into(),
            bytes: 42,
            modified_at_ms: Some(7),
        }
    }

    fn manifest(entries: Vec<ManifestEntry>) -> ProjectManifest {
        ProjectManifest {
            schema_version: ProjectManifest::SCHEMA_VERSION,
            project_id: "transmog".into(),
            revision: "r7".into(),
            entries,
        }
    }

    #[test]
    fn explicit_part_collects_models_drawings_metadata_and_revisions() {
        let project = manifest(vec![
            entry("a", "parts/pi-case/revisions/r1/case.stl", "model/stl"),
            entry(
                "b",
                "parts/pi-case/revisions/r1/drawing.pdf",
                "application/pdf",
            ),
            entry("c", "parts/pi-case/metadata.json", "application/json"),
        ])
        .inspect_parts()
        .unwrap();

        assert_eq!(project.manifest_revision, "r7");
        assert_eq!(project.parts.len(), 1);
        let part = &project.parts[0];
        assert_eq!(
            (part.id.as_str(), part.name.as_str()),
            ("parts/pi-case", "Pi Case")
        );
        assert_eq!(part.models().count(), 1);
        assert_eq!(part.drawings().count(), 1);
        assert_eq!(part.metadata().count(), 1);
        assert_eq!(part.revisions().collect::<Vec<_>>(), ["r1", "r7"]);
    }

    #[test]
    fn assembly_identity_does_not_collide_with_a_part_of_the_same_name() {
        let project = manifest(vec![
            entry("a", "parts/frame/frame.stl", "model/stl"),
            entry("b", "assemblies/frame/frame.step", "model/step"),
        ])
        .inspect_parts()
        .unwrap();
        assert_eq!(project.parts.len(), 2);
        assert_eq!(project.parts[0].kind, PartKind::Assembly);
        assert_eq!(project.parts[1].kind, PartKind::Part);
    }

    #[test]
    fn flat_category_artifacts_remain_at_project_scope() {
        let project = manifest(vec![
            entry("a", "models/bracket.stl", "model/stl"),
            entry("b", "drawings/bracket-drawing.pdf", "application/pdf"),
            entry("c", "metadata/bracket_metadata.json", "application/json"),
        ])
        .inspect_parts()
        .unwrap();
        assert!(project.parts.is_empty());
        assert_eq!(project.project_artifacts.len(), 3);
    }

    #[test]
    fn unrelated_project_files_are_preserved_without_inventing_a_part() {
        let project = manifest(vec![entry("a", "reports/thermal.pdf", "application/pdf")])
            .inspect_parts()
            .unwrap();
        assert!(project.parts.is_empty());
        assert_eq!(project.project_artifacts[0].entry.id, "a");
        assert_eq!(project.project_artifacts[0].role, ArtifactRole::Supporting);
    }

    #[test]
    fn invalid_manifest_fails_before_it_is_projected() {
        let mut invalid = manifest(vec![entry("a", "parts/a/a.stl", "model/stl")]);
        invalid.schema_version = 99;
        assert!(invalid.inspect_parts().is_err());
    }

    #[test]
    fn selected_artifact_uses_its_parent_folder_as_scope() {
        let manifest = manifest(vec![
            entry("a", "parts/frame/frame.stl", "model/stl"),
            entry("b", "parts/frame/drawing.pdf", "application/pdf"),
            entry("c", "parts/other/other.stl", "model/stl"),
        ]);
        let part = manifest.inspect_entry("a").unwrap();
        assert_eq!(part.id, "parts/frame");
        assert_eq!(part.artifacts.len(), 2);
    }

    #[test]
    fn inspector_fails_loudly_without_a_model_or_pdf() {
        let manifest = manifest(vec![entry(
            "a",
            "parts/frame/metadata.json",
            "application/json",
        )]);
        let error = manifest.inspect_entry("a").unwrap_err();
        assert!(error.0.contains("no model or PDF"));
        assert!(
            manifest
                .inspect_parts()
                .unwrap_err()
                .0
                .contains("no model or PDF")
        );
    }

    #[test]
    fn iteration_request_resolves_selected_ids_to_immutable_cids() {
        let manifest = manifest(vec![
            entry("model", "parts/frame/frame.stl", "model/stl"),
            entry("drawing", "parts/frame/drawing.pdf", "application/pdf"),
        ]);
        let part = manifest.inspect_entry("model").unwrap();
        let request = manifest
            .iteration_request(&part, "Add two mounting holes", &["model".into()])
            .unwrap();
        assert_eq!(request.project_id, "transmog");
        assert_eq!(request.manifest_revision, "r7");
        assert_eq!(request.part_id, "parts/frame");
        assert_eq!(request.artifacts[0].cid, "bafymodel");
        request.validate().unwrap();
        let roundtrip: IterationRequest =
            serde_json::from_slice(&serde_json::to_vec(&request).unwrap()).unwrap();
        assert_eq!(roundtrip, request);
    }

    #[test]
    fn iteration_request_rejects_out_of_scope_empty_and_duplicate_selections() {
        let manifest = manifest(vec![
            entry("model", "parts/frame/frame.stl", "model/stl"),
            entry("other", "parts/other/other.stl", "model/stl"),
        ]);
        let part = manifest.inspect_entry("model").unwrap();
        assert!(
            manifest
                .iteration_request(&part, "Change it", &["other".into()])
                .is_err()
        );
        assert!(manifest.iteration_request(&part, "Change it", &[]).is_err());
        assert!(
            manifest
                .iteration_request(&part, "Change it", &["model".into(), "model".into()])
                .is_err()
        );
    }
}
