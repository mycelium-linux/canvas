//! Shared in-memory Canvas project and document session.
//!
//! Catalogs must come from the explicitly selected owner. This module has no
//! discovery, network credentials, filesystem, or desktop renderer dependency.
use canvas_core::{Action, Actor, Catalog, WorkspaceState};

#[derive(Clone)]
pub struct Workspaces {
    catalog: Catalog,
    state: WorkspaceState,
    recent: Vec<String>,
    documents: std::collections::BTreeMap<String, crate::WorkspaceDocument>,
}

impl Workspaces {
    pub fn from_catalog(input: &str, selected: &str) -> Result<Self, String> {
        if input.len() > 64 * 1024 {
            return Err("Project projection exceeds 64 KiB".into());
        }
        let catalog = Catalog::parse(input).map_err(|error| error.to_string())?;
        if catalog.projects.len() > 64 {
            return Err("Project projection exceeds 64 projects".into());
        }
        let mut state = WorkspaceState::new(&catalog).map_err(|error| error.to_string())?;
        state
            .apply(Actor::Human, Action::SelectProject(selected.into()))
            .map_err(|error| error.to_string())?;
        state
            .validate(&catalog)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            catalog,
            state,
            recent: Vec::new(),
            documents: std::collections::BTreeMap::new(),
        })
    }

    /// Admit a complete owner-provided document; no renderer or I/O runs here.
    /// Older snapshots cannot overwrite a newer generation.
    pub fn install_document(&mut self, input: &str) -> Result<(), String> {
        if input.len() > 4 * 1024 * 1024 {
            return Err("Workspace document exceeds 4 MiB".into());
        }
        let document = crate::WorkspaceDocument::parse(input, &self.catalog)
            .map_err(|error| error.to_string())?;
        if self.documents.values().any(|existing| {
            existing.id == document.id && existing.project_id != document.project_id
        }) {
            return Err("Workspace identity belongs to another project".into());
        }
        if let Some(current) = self.documents.get(&document.project_id) {
            if current.id != document.id {
                return Err(
                    "Workspace identity changed; replace the owner session explicitly".into(),
                );
            }
            if document.revision < current.revision
                || (document.revision == current.revision && document != *current)
            {
                return Err("Stale or conflicting workspace generation".into());
            }
        }
        self.documents.insert(document.project_id.clone(), document);
        Ok(())
    }

    pub fn document(&self) -> Option<&crate::WorkspaceDocument> {
        self.documents.get(self.selected())
    }

    /// Import one complete generation without replacing the owner's catalog.
    /// Validation and generation conflicts leave selection and all documents intact.
    pub fn import_document(
        &mut self,
        document: &crate::WorkspaceDocument,
        activate: bool,
    ) -> Result<(), String> {
        let mut next = self.clone();
        if next.catalog.project(&document.project_id).is_err() {
            next.catalog.projects.push(canvas_core::Project {
                id: document.project_id.clone(),
                name: document.project_id.replace('-', " "),
                theme: canvas_core::ThemeSelection::Coherence {
                    mode: canvas_core::ThemeMode::Dark,
                    accent: canvas_core::Accent::Info,
                },
                // A portable scene is presentation data, not host execution authority.
                resources: Vec::new(),
            });
            if next.catalog.projects.len() > 64 {
                return Err("Project projection exceeds 64 projects".into());
            }
            let encoded = serde_json::to_string(&next.catalog).map_err(|e| e.to_string())?;
            if encoded.len() > 64 * 1024 {
                return Err("Project projection exceeds 64 KiB".into());
            }
            next.catalog = Catalog::parse(&encoded).map_err(|e| e.to_string())?;
            let mut state = WorkspaceState::new(&next.catalog).map_err(|e| e.to_string())?;
            // Preserve owner-local lifecycle/control state for existing projects.
            for (id, previous) in &next.state.projects {
                state.projects.insert(id.clone(), previous.clone());
            }
            state.selected_project = next.state.selected_project.clone();
            if let Some(added) = state.projects.get_mut(&document.project_id) {
                added.lifecycle = canvas_core::Lifecycle::Idle;
            }
            state.validate(&next.catalog).map_err(|e| e.to_string())?;
            next.state = state;
        }
        next.install_document(&serde_json::to_string(document).map_err(|e| e.to_string())?)?;
        if activate {
            next.select(Actor::Human, &document.project_id)?;
        }
        *self = next;
        Ok(())
    }

    pub fn document_for(&self, project: &str) -> Option<&crate::WorkspaceDocument> {
        self.documents.get(project)
    }

    pub fn document_by_id(&self, id: &str) -> Option<&crate::WorkspaceDocument> {
        self.documents.values().find(|document| document.id == id)
    }

    /// Replace only the selected workspace's named views, preserving all surfaces.
    pub fn set_pages(&mut self, pages: Vec<crate::Page>) -> Result<(), String> {
        let mut next = self.document().ok_or("No selected document")?.clone();
        next.pages = pages;
        next.active_page = 0;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("Document revision exhausted")?;
        next.validate(&self.catalog).map_err(|e| e.to_string())?;
        self.documents.insert(next.project_id.clone(), next);
        Ok(())
    }

    /// Select a declared view without dropping transient instrument frames.
    pub fn select_page(&mut self, index: usize) -> Result<(), String> {
        let mut next = self.document().ok_or("No selected document")?.clone();
        if index >= next.page_count() {
            return Err("Page index out of range".into());
        }
        next.active_page = index;
        next.revision = next
            .revision
            .checked_add(1)
            .ok_or("Document revision exhausted")?;
        self.documents.insert(next.project_id.clone(), next);
        Ok(())
    }

    pub fn palette(&self) -> Result<Option<canvas_core::ThemePalette>, String> {
        self.document()
            .map(|document| {
                document
                    .resolved_theme(&self.catalog)
                    .map(|theme| theme.palette())
                    .map_err(|error| error.to_string())
            })
            .transpose()
    }

    pub fn retained_projection(&self) -> serde_json::Value {
        serde_json::json!({"catalog":self.catalog,
            "documents":self.documents.values().collect::<Vec<_>>(), "selected":self.selected()})
    }

    /// Revision-bound patches are transactional: validation failure leaves the
    /// last complete document available to the platform renderer.
    pub fn apply_document_patch(&mut self, patch: &crate::WorkspacePatch) -> Result<(), String> {
        let current = self
            .document_by_id(&patch.workspace_id)
            .ok_or("Patch workspace is not declared")?;
        let next = current
            .apply_patch(patch, &self.catalog)
            .map_err(|error| error.to_string())?;
        self.documents.insert(next.project_id.clone(), next);
        Ok(())
    }

    pub fn select(&mut self, actor: Actor, id: &str) -> Result<(), String> {
        self.catalog
            .project(id)
            .map_err(|error| error.to_string())?;
        let departed = self.state.selected_project.clone();
        self.state
            .apply(actor, Action::SelectProject(id.into()))
            .map_err(|error| error.to_string())?;
        canvas_core::workspace_navigation::record_switch(&mut self.recent, Some(&departed), id);
        self.state
            .validate(&self.catalog)
            .map_err(|error| error.to_string())
    }

    pub fn selected(&self) -> &str {
        &self.state.selected_project
    }

    pub fn previous(&self) -> Option<&str> {
        canvas_core::workspace_navigation::previous(&self.recent, |id| {
            self.catalog.project(id).is_ok()
        })
    }

    /// Only identifiers and labels reach the native selector. Resources and
    /// host actions stay at the owner; catalog parsing isn't permission to run.
    pub fn selector_projection(&self) -> serde_json::Value {
        serde_json::json!({
            "selected_project": self.selected(),
            "projects": self.catalog.projects.iter().map(|project| {
                serde_json::json!({"id": project.id, "name": project.name})
            }).collect::<Vec<_>>()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const CATALOG: &str = r#"{"schema_version":1,"projects":[
        {"id":"television","name":"Television","theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]},
        {"id":"studio","name":"Studio","theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]}
    ]}"#;

    #[test]
    fn selection_uses_shared_core_and_rejects_unknown_ids() {
        let mut workspaces = Workspaces::from_catalog(CATALOG, "television").unwrap();
        workspaces.select(Actor::Human, "studio").unwrap();
        assert_eq!(workspaces.selected(), "studio");
        assert_eq!(workspaces.previous(), Some("television"));
        assert!(workspaces.select(Actor::Human, "missing").is_err());
        assert_eq!(workspaces.selected(), "studio");
        assert_eq!(workspaces.previous(), Some("television"));
    }

    #[test]
    fn agent_cannot_bypass_shared_human_control_policy() {
        let mut workspaces = Workspaces::from_catalog(CATALOG, "television").unwrap();
        assert!(workspaces.select(Actor::Agent, "studio").is_err());
        assert_eq!(workspaces.selected(), "television");
    }

    #[test]
    fn projection_is_bounded_and_contains_no_resource_targets() {
        assert!(Workspaces::from_catalog(&" ".repeat(65 * 1024), "television").is_err());
        assert!(Workspaces::from_catalog(CATALOG, "missing").is_err());
        let workspaces = Workspaces::from_catalog(CATALOG, "television").unwrap();
        assert_eq!(
            workspaces.selector_projection()["projects"][0]["id"],
            "television"
        );
        assert!(
            !workspaces
                .selector_projection()
                .to_string()
                .contains("resources")
        );
    }

    fn document(project: &str, revision: u64) -> String {
        serde_json::json!({
            "schema_version":1,"id":format!("workspace-{project}"),
            "project_id":project,"revision":revision,"theme":{"source":"project"},
            "surfaces":[{"id":"video","title":"Video",
                "content":{"kind":"stream","url":"http://192.168.2.67:18089/komo4.m3u8"}}],
            "layout":{"kind":"surface","surface_id":"video"}
        })
        .to_string()
    }

    #[test]
    fn documents_are_retained_per_project_and_round_trip() {
        let mut session = Workspaces::from_catalog(CATALOG, "television").unwrap();
        let raw = document("television", 1);
        session.install_document(&raw).unwrap();
        let original = session.document().unwrap().clone();
        let encoded = serde_json::to_string(&original).unwrap();
        assert_eq!(
            crate::WorkspaceDocument::parse(&encoded, &session.catalog).unwrap(),
            original
        );
        session.select(Actor::Human, "studio").unwrap();
        assert!(session.document().is_none());
        session.install_document(&document("studio", 1)).unwrap();
        session.select(Actor::Human, "television").unwrap();
        assert_eq!(session.document(), Some(&original));
    }

    #[test]
    fn single_import_retains_other_projects_and_selection_transactionally() {
        let mut session = Workspaces::from_catalog(CATALOG, "television").unwrap();
        session
            .install_document(&document("television", 1))
            .unwrap();
        let original = session.document().unwrap().clone();
        let incoming: crate::WorkspaceDocument =
            serde_json::from_str(&document("live-work", 2)).unwrap();
        session.import_document(&incoming, false).unwrap();
        assert_eq!(session.selected(), "television");
        assert_eq!(session.document(), Some(&original));
        assert_eq!(session.document_for("live-work"), Some(&incoming));
        assert!(
            session
                .catalog
                .project("live-work")
                .unwrap()
                .resources
                .is_empty()
        );
        session.import_document(&incoming, false).unwrap();
        let before = session.retained_projection();
        let mut conflict = incoming.clone();
        conflict.surfaces[0].title = "Conflict".into();
        assert!(session.import_document(&conflict, true).is_err());
        assert_eq!(session.retained_projection(), before);
        let mut alias = incoming.clone();
        alias.project_id = "another-new-project".into();
        assert!(session.import_document(&alias, false).is_err());
        assert_eq!(session.retained_projection(), before);
        session.import_document(&incoming, true).unwrap();
        assert_eq!(session.selected(), "live-work");
        assert_eq!(session.document_for("television"), Some(&original));
    }

    #[test]
    fn stale_conflicting_and_invalid_documents_preserve_last_good_generation() {
        let mut session = Workspaces::from_catalog(CATALOG, "television").unwrap();
        session
            .install_document(&document("television", 2))
            .unwrap();
        let previous = session.document().unwrap().clone();
        assert!(
            session
                .install_document(&document("television", 1))
                .is_err()
        );
        let mut conflict: serde_json::Value =
            serde_json::from_str(&document("television", 2)).unwrap();
        conflict["surfaces"][0]["title"] = serde_json::json!("Changed");
        assert!(session.install_document(&conflict.to_string()).is_err());
        assert!(session.install_document(&document("unknown", 3)).is_err());
        assert!(
            session
                .install_document(&" ".repeat(4 * 1024 * 1024 + 1))
                .is_err()
        );
        assert_eq!(session.document(), Some(&previous));
        session
            .install_document(&document("television", 2))
            .unwrap();
    }

    #[test]
    fn patches_are_revision_bound_and_transactional() {
        let mut session = Workspaces::from_catalog(CATALOG, "television").unwrap();
        session
            .install_document(&document("television", 1))
            .unwrap();
        let previous = session.document().unwrap().clone();
        let mut patch = crate::WorkspacePatch {
            schema_version: 1,
            workspace_id: previous.id.clone(),
            base_revision: 1,
            revision: 2,
            operations: vec![crate::PatchOperation::SetFocus {
                surface_id: Some("missing".into()),
            }],
        };
        assert!(session.apply_document_patch(&patch).is_err());
        assert_eq!(session.document(), Some(&previous));
        patch.operations = vec![crate::PatchOperation::SetFocus {
            surface_id: Some("video".into()),
        }];
        session.apply_document_patch(&patch).unwrap();
        assert_eq!(session.document().unwrap().revision, 2);
        assert_eq!(
            session.document().unwrap().focused_surface_id.as_deref(),
            Some("video")
        );
        assert!(session.apply_document_patch(&patch).is_err());
    }

    #[test]
    fn workspace_identity_cannot_alias_another_project() {
        let mut session = Workspaces::from_catalog(CATALOG, "television").unwrap();
        session
            .install_document(&document("television", 1))
            .unwrap();
        let saved = session.retained_projection();
        let mut alias: serde_json::Value =
            serde_json::from_str(&document("television", 1)).unwrap();
        alias["project_id"] = serde_json::json!("studio");
        assert!(session.install_document(&alias.to_string()).is_err());
        assert_eq!(session.retained_projection(), saved);
    }
}
