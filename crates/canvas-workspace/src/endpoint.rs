//! Transport-independent presentation endpoint. Transport authenticates callers;
//! this state machine owns admission, revisions and the target's own catalog.
use crate::{WorkspaceDocument, WorkspacePatch, session::Workspaces};
use canvas_core::{Actor, Catalog};
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 4 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub schema_version: u32,
    pub request_id: String,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    pub command: Command,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Transient display data, not a document edit or a persisted checkpoint.
    SurfaceSnapshot {
        workspace_id: String,
        surface_id: String,
        frame: serde_json::Value,
        data: canvas_widgets::WidgetData,
    },
    Inspect,
    ReadDocument,
    Install {
        catalog: Catalog,
        documents: Vec<WorkspaceDocument>,
        selected: String,
    },
    WorkspaceSwitch {
        name: String,
    },
    WorkspaceRead {
        name: String,
    },
    ApplyPatch {
        patch: WorkspacePatch,
    },
}

#[derive(Clone)]
pub struct Endpoint {
    owner: String,
    revision: u64,
    session: Option<Workspaces>,
    frames: std::collections::BTreeMap<
        String,
        (
            serde_json::Value,
            canvas_widgets::WidgetData,
            std::time::Instant,
        ),
    >,
}

impl Endpoint {
    pub fn new(owner: &str) -> Self {
        Self {
            owner: owner.into(),
            revision: 1,
            session: None,
            frames: Default::default(),
        }
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let mut snapshot = serde_json::json!({
            "owner":self.owner, "revision":self.revision,
            "catalog":self.session.as_ref().map(Workspaces::selector_projection),
            "palette":self.session.as_ref().and_then(|session| session.palette().ok().flatten()),
            "document":self.session.as_ref().and_then(Workspaces::document)
        });
        if let Some(surfaces) = snapshot["document"]["surfaces"].as_array_mut() {
            for surface in surfaces {
                if surface["content"]["spec"]["kind"] != "status_list" {
                    continue;
                }
                if let Some(value) = surface["id"]
                    .as_str()
                    .and_then(|id| self.surface_snapshot(id))
                {
                    surface["content"]["spec"]["title"] = value["frame"]["title"].clone();
                    surface["content"]["spec"]["nodes"] = value["frame"]["nodes"].clone();
                    surface["content"]["spec"]["data"] = value["data"].clone();
                }
            }
        }
        snapshot
    }

    pub fn checkpoint(&self) -> serde_json::Value {
        serde_json::json!({"schema_version":1,"owner":self.owner,"revision":self.revision,
            "projection":self.session.as_ref().map(Workspaces::retained_projection)})
    }

    /// Latest transient frame for a declared signal or status surface. The
    /// receiver's monotonic clock owns staleness; frames never survive restart.
    pub fn surface_snapshot(&self, surface_id: &str) -> Option<serde_json::Value> {
        let document = self.session.as_ref()?.document()?;
        let surface = document.surfaces.iter().find(|s| s.id == surface_id)?;
        let crate::SurfaceContent::Widget { spec } = &surface.content else {
            return None;
        };
        let declared = match &spec.kind {
            canvas_widgets::WidgetKind::SignalView { frame, .. } => frame.clone(),
            canvas_widgets::WidgetKind::StatusList { title, nodes } => {
                serde_json::json!({"title":title,"nodes":nodes})
            }
            _ => return None,
        };
        let (frame, mut data) = self.frames.get(surface_id).map_or_else(
            || (declared, spec.data.clone()),
            |(frame, data, received)| {
                let mut data = data.clone();
                if received.elapsed() > std::time::Duration::from_secs(3) {
                    data.state = canvas_widgets::WidgetDataState::Stale;
                }
                (frame.clone(), data)
            },
        );
        if !self.frames.contains_key(surface_id) {
            data.state = canvas_widgets::WidgetDataState::NoData;
        }
        Some(serde_json::json!({"frame":frame, "data":data}))
    }

    pub fn restore(owner: &str, raw: &str) -> Result<Self, String> {
        if raw.len() > MAX_FRAME {
            return Err("Checkpoint exceeds limit".into());
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Projection {
            catalog: Catalog,
            documents: Vec<WorkspaceDocument>,
            selected: String,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Checkpoint {
            schema_version: u32,
            owner: String,
            revision: u64,
            projection: Option<Projection>,
        }
        let checkpoint: Checkpoint = serde_json::from_str(raw).map_err(|_| "Invalid checkpoint")?;
        if checkpoint.schema_version != 1 || checkpoint.owner != owner || checkpoint.revision == 0 {
            return Err("Invalid checkpoint provenance/version".into());
        }
        let mut endpoint = Self::new(owner);
        if let Some(projection) = checkpoint.projection {
            if checkpoint.revision < 2 {
                return Err("Invalid checkpoint revision".into());
            }
            endpoint.execute(Request {
                schema_version: 1,
                request_id: "restore".into(),
                expected_revision: Some(1),
                command: Command::Install {
                    catalog: projection.catalog,
                    documents: projection.documents,
                    selected: projection.selected,
                },
            })?;
        }
        endpoint.revision = checkpoint.revision;
        Ok(endpoint)
    }

    pub fn execute(&mut self, request: Request) -> Result<serde_json::Value, String> {
        if request.schema_version != 1
            || request.request_id.is_empty()
            || request.request_id.len() > 64
        {
            return Err("Invalid presentation request".into());
        }
        let writes = !matches!(
            request.command,
            Command::Inspect | Command::ReadDocument | Command::WorkspaceRead { .. }
        );
        if writes && request.expected_revision != Some(self.revision) {
            return Err("Stale or missing endpoint revision; inspect this target first".into());
        }
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or("Endpoint revision exhausted")?;
        match request.command {
            Command::SurfaceSnapshot {
                workspace_id,
                surface_id,
                frame,
                data,
            } => {
                let document = self
                    .session
                    .as_ref()
                    .and_then(Workspaces::document)
                    .ok_or("No active workspace")?;
                if document.id != workspace_id {
                    return Err("Snapshot target is not active".into());
                }
                let surface = document
                    .surfaces
                    .iter()
                    .find(|s| s.id == surface_id)
                    .ok_or("Snapshot surface is not declared")?;
                let crate::SurfaceContent::Widget { spec } = &surface.content else {
                    return Err("Snapshot surface is not a widget".into());
                };
                let mut next = (*spec).clone();
                next.kind = match &spec.kind {
                    canvas_widgets::WidgetKind::SignalView { title, .. } => {
                        canvas_widgets::WidgetKind::SignalView {
                            title: title.clone(),
                            frame: frame.clone(),
                        }
                    }
                    canvas_widgets::WidgetKind::StatusList { .. } => {
                        #[derive(Deserialize)]
                        #[serde(deny_unknown_fields)]
                        struct StatusFrame {
                            title: String,
                            nodes: Vec<canvas_widgets::NodeSummaryItem>,
                        }
                        let status: StatusFrame = serde_json::from_value(frame.clone())
                            .map_err(|_| "Invalid status snapshot")?;
                        canvas_widgets::WidgetKind::StatusList {
                            title: status.title,
                            nodes: status.nodes,
                        }
                    }
                    _ => return Err("Snapshot surface is not a supported live view".into()),
                };
                next.data = data.clone();
                next.validate().map_err(|e| e.to_string())?;
                self.frames
                    .insert(surface_id, (frame, data, std::time::Instant::now()));
                return Ok(serde_json::json!({"accepted":true}));
            }
            Command::Inspect => {}
            Command::ReadDocument => {
                return serde_json::to_value(self.session.as_ref().and_then(Workspaces::document))
                    .map_err(|error| error.to_string());
            }
            Command::WorkspaceRead { name } => {
                let session = self.session.as_ref().ok_or("Endpoint has no catalog")?;
                return serde_json::to_value(session.document_for(&name))
                    .map_err(|error| error.to_string());
            }
            Command::Install {
                catalog,
                documents,
                selected,
            } => {
                if documents.len() > 64 {
                    return Err("Too many documents".into());
                }
                let mut next = Workspaces::from_catalog(
                    &serde_json::to_string(&catalog).map_err(|error| error.to_string())?,
                    &selected,
                )?;
                let mut ids = std::collections::BTreeSet::new();
                for document in documents {
                    if !ids.insert(document.project_id.clone()) {
                        return Err("Duplicate project document".into());
                    }
                    next.install_document(
                        &serde_json::to_string(&document).map_err(|error| error.to_string())?,
                    )?;
                }
                if next.document().is_none() {
                    return Err("Selected project has no document".into());
                }
                self.session = Some(next);
                self.frames.clear();
            }
            Command::WorkspaceSwitch { name } => {
                let session = self.session.as_mut().ok_or("Endpoint has no catalog")?;
                if session.document_for(&name).is_none() {
                    return Err("Project has no retained document".into());
                }
                session.select(Actor::Human, &name)?;
                self.frames.clear();
            }
            Command::ApplyPatch { patch } => {
                self.session
                    .as_mut()
                    .ok_or("Endpoint has no catalog")?
                    .apply_document_patch(&patch)?;
                self.frames.clear();
            }
        }
        if writes {
            self.revision = next_revision;
        }
        Ok(self.snapshot())
    }

    pub fn dispatch(&mut self, raw: &str) -> String {
        let result = if raw.len() > MAX_FRAME {
            Err("Presentation request exceeds limit".into())
        } else {
            serde_json::from_str::<Request>(raw)
                .map_err(|_| "Invalid presentation command".into())
                .and_then(|request| self.execute(request))
        };
        match result {
            Ok(data) => {
                serde_json::json!({"ok":true,"revision":self.revision,"data":data}).to_string()
            }
            Err(error) => {
                serde_json::json!({"ok":false,"revision":self.revision,"error":error}).to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn status_snapshots_are_live_not_saved_and_expire() {
        let mut endpoint = Endpoint::new("firetv-omni");
        let mut request: Request = serde_json::from_str(include_str!(
            "../../../config/deploy/firetv/workspaces.json"
        ))
        .unwrap();
        let Command::Install { documents, .. } = &mut request.command else {
            panic!()
        };
        documents[0].surfaces[1].content = crate::SurfaceContent::Widget {
            spec: canvas_widgets::WidgetSpec::from_json(br#"{"schema_version":2,"id":"status","kind":"status_list","title":"Studio","nodes":[]}"#).unwrap()
        };
        endpoint.execute(request).unwrap();
        let saved = endpoint.checkpoint();
        let frame = serde_json::json!({"title":"Output","nodes":[{"label":"Main","state":"good","detail":"Connected"}]});
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "status".into(),
                expected_revision: Some(2),
                command: Command::SurfaceSnapshot {
                    workspace_id: "tv-television".into(),
                    surface_id: "clock".into(),
                    frame: frame.clone(),
                    data: Default::default(),
                },
            })
            .unwrap();
        assert_eq!(endpoint.checkpoint(), saved);
        assert_eq!(
            endpoint.snapshot()["document"]["surfaces"][1]["content"]["spec"]["nodes"],
            frame["nodes"]
        );
        endpoint.frames.get_mut("clock").unwrap().2 =
            std::time::Instant::now() - std::time::Duration::from_secs(4);
        assert_eq!(
            endpoint.snapshot()["document"]["surfaces"][1]["content"]["spec"]["data"]["state"],
            "stale"
        );
        let restored = Endpoint::restore("firetv-omni", &saved.to_string()).unwrap();
        assert_eq!(
            restored.surface_snapshot("clock").unwrap()["data"]["state"],
            "no_data"
        );
        assert!(
            restored.surface_snapshot("clock").unwrap()["frame"]["nodes"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }
    #[test]
    fn transient_frames_do_not_rewrite_documents_or_survive_restart() {
        let mut endpoint = Endpoint::new("firetv-omni");
        let mut request: Request = serde_json::from_str(include_str!(
            "../../../config/deploy/firetv/workspaces.json"
        ))
        .unwrap();
        let Command::Install { documents, .. } = &mut request.command else {
            panic!()
        };
        let surface = &mut documents[0].surfaces[1];
        surface.content = crate::SurfaceContent::Widget { spec: canvas_widgets::WidgetSpec::from_json(
            br#"{"schema_version":2,"id":"signal","kind":"signal_view","title":"Scope","frame":{"view":"scope","range":{"minimum":-1,"maximum":1},"samples":[]}}"#).unwrap() };
        endpoint.execute(request).unwrap();
        let before = endpoint.checkpoint();
        let frame = serde_json::json!({"view":"scope","range":{"minimum":-1,"maximum":1},"samples":[0,1,0]});
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "frame".into(),
                expected_revision: Some(2),
                command: Command::SurfaceSnapshot {
                    workspace_id: "tv-television".into(),
                    surface_id: "clock".into(),
                    frame: frame.clone(),
                    data: Default::default(),
                },
            })
            .unwrap();
        assert_eq!(endpoint.checkpoint(), before);
        assert_eq!(endpoint.surface_snapshot("clock").unwrap()["frame"], frame);
        endpoint.frames.get_mut("clock").unwrap().2 =
            std::time::Instant::now() - std::time::Duration::from_secs(4);
        assert_eq!(
            endpoint.surface_snapshot("clock").unwrap()["data"]["state"],
            "stale"
        );
        let restored = Endpoint::restore("firetv-omni", &before.to_string()).unwrap();
        assert_eq!(
            restored.surface_snapshot("clock").unwrap()["data"]["state"],
            "no_data"
        );
        assert!(
            endpoint
                .execute(Request {
                    schema_version: 1,
                    request_id: "wrong".into(),
                    expected_revision: Some(2),
                    command: Command::SurfaceSnapshot {
                        workspace_id: "other".into(),
                        surface_id: "clock".into(),
                        frame,
                        data: Default::default()
                    }
                })
                .is_err()
        );
        assert_eq!(endpoint.checkpoint(), before);
    }
    #[test]
    fn real_tv_install_switch_patch_and_restart_use_one_session() {
        let mut endpoint = Endpoint::new("firetv-omni");
        let raw = include_str!("../../../config/deploy/firetv/workspaces.json");
        let request: Request = serde_json::from_str(raw).unwrap();
        endpoint.execute(request).unwrap();
        assert_eq!(
            endpoint.snapshot()["catalog"]["selected_project"],
            "television"
        );
        let before = endpoint.checkpoint();
        let stale = Request {
            schema_version: 1,
            request_id: "stale".into(),
            expected_revision: Some(1),
            command: Command::WorkspaceSwitch {
                name: "clock".into(),
            },
        };
        assert!(endpoint.execute(stale).is_err());
        assert_eq!(endpoint.checkpoint(), before);
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "switch".into(),
                expected_revision: Some(2),
                command: Command::WorkspaceSwitch {
                    name: "clock".into(),
                },
            })
            .unwrap();
        let saved = endpoint.checkpoint().to_string();
        let restored = Endpoint::restore("firetv-omni", &saved).unwrap();
        assert_eq!(restored.snapshot(), endpoint.snapshot());
        assert_eq!(restored.snapshot()["catalog"]["selected_project"], "clock");
        assert!(Endpoint::restore("neo", &saved).is_err());
        assert!(Endpoint::restore("firetv-omni", "{}").is_err());
    }
    #[test]
    fn stale_mutations_and_unsupported_commands_leave_target_unchanged() {
        let mut endpoint = Endpoint::new("tv");
        assert!(
            endpoint
                .execute(Request {
                    schema_version: 1,
                    request_id: "test".into(),
                    expected_revision: None,
                    command: Command::WorkspaceSwitch {
                        name: "studio".into()
                    }
                })
                .is_err()
        );
        assert_eq!(endpoint.snapshot()["revision"], 1);
        assert!(
            !serde_json::from_str::<serde_json::Value>(&endpoint.dispatch(
                r#"{"schema_version":1,"request_id":"test","command":{"command":"shutdown"}}"#
            ))
            .unwrap()["ok"]
                .as_bool()
                .unwrap()
        );
        assert_eq!(endpoint.snapshot()["owner"], "tv");
    }
}
