//! Android presentation adapter for the existing Canvas project policy.
//!
//! Catalogs must come from the explicitly selected owner. This module has no
//! discovery, network credentials, filesystem, or desktop renderer dependency.
use canvas_core::{Action, Actor, Catalog, WorkspaceState};

pub struct Workspaces {
    catalog: Catalog,
    state: WorkspaceState,
    recent: Vec<String>,
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
        })
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
}
