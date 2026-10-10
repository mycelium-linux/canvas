//! Transport-independent presentation endpoint. Transport authenticates callers;
//! this state machine owns admission, revisions and the target's own catalog.
use crate::{WorkspaceDocument, WorkspacePatch, session::Workspaces};
use canvas_core::{Actor, Catalog};
use serde::{Deserialize, Serialize};

pub const MAX_FRAME: usize = 4 * 1024 * 1024;
/// Optional declared status-list surface occupying the workspace's header slot.
/// Uses the same authenticated transient frame ingress and freshness as tiles.
pub const HEADER_SURFACE_ID: &str = "workspace-header";

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
    /// Display-only task focus/expansion; never provider execution authority.
    SurfaceNavigate {
        surface_id: String,
        action: fpl_gfx::panels::TaskNavigation,
    },
    /// Transient display data, not a document edit or a persisted checkpoint.
    SurfaceSnapshot {
        workspace_id: String,
        surface_id: String,
        frame: serde_json::Value,
        data: canvas_widgets::WidgetData,
    },
    Inspect,
    ReadDocument,
    /// Read current transient presentation; not a persisted checkpoint.
    ReadPresentation,
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
    /// Retain a single portable scene, preserving every other project and selection.
    WorkspaceImport {
        document: Box<WorkspaceDocument>,
        #[serde(default)]
        activate: bool,
    },
    WorkspacePages {
        pages: Vec<crate::Page>,
    },
    WorkspacePage {
        index: usize,
    },
    ApplyPatch {
        patch: WorkspacePatch,
    },
}

#[derive(Clone)]
pub struct Endpoint {
    task_selection: std::collections::BTreeMap<String, (Option<String>, bool)>,
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
            task_selection: Default::default(),
            owner: owner.into(),
            revision: 1,
            session: None,
            frames: Default::default(),
        }
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let mut snapshot = serde_json::json!({
            "owner":self.owner, "revision":self.revision,
            "capabilities":["workspace_read", "workspace_import"],
            "catalog":self.session.as_ref().map(Workspaces::selector_projection),
            "palette":self.session.as_ref().and_then(|session| session.palette().ok().flatten()),
            "document":self.session.as_ref().and_then(Workspaces::document)
        });
        snapshot["header"] = self
            .surface_snapshot(HEADER_SURFACE_ID)
            .unwrap_or(serde_json::Value::Null);
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
        let (mut frame, mut data) = self.frames.get(surface_id).map_or_else(
            || (declared, spec.data.clone()),
            |(frame, data, received)| {
                let mut data = data.clone();
                // Allow three declared refresh periods plus transport jitter.
                // Fast legacy instruments retain their original three-second floor.
                if received.elapsed() > surface_frame_expiry(spec.refresh_ms) {
                    data.state = canvas_widgets::WidgetDataState::Stale;
                }
                (frame.clone(), data)
            },
        );
        if !self.frames.contains_key(surface_id) {
            data.state = canvas_widgets::WidgetDataState::NoData;
        }
        if frame["view"] == "task_deck" {
            if let Some((selected, expanded)) = self.task_selection.get(surface_id) {
                let present = selected.as_ref().is_some_and(|id| {
                    frame["panel"]["items"]
                        .as_array()
                        .is_some_and(|items| items.iter().any(|i| i["id"].as_str() == Some(id)))
                });
                if present {
                    frame["panel"]["selected"] = serde_json::json!(selected);
                    frame["panel"]["expanded"] = serde_json::json!(expanded);
                }
            }
        }
        if matches!(
            data.state,
            canvas_widgets::WidgetDataState::Stale | canvas_widgets::WidgetDataState::NoData
        ) && matches!(frame["view"].as_str(), Some("attention" | "task_deck"))
        {
            frame["panel"]["fresh"] = serde_json::json!(false);
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
            Command::Inspect
                | Command::ReadDocument
                | Command::ReadPresentation
                | Command::WorkspaceRead { .. }
                | Command::SurfaceSnapshot { .. }
                | Command::SurfaceNavigate { .. }
        );
        if writes && request.expected_revision != Some(self.revision) {
            return Err("Stale or missing endpoint revision; inspect this target first".into());
        }
        let next_revision = self
            .revision
            .checked_add(1)
            .ok_or("Endpoint revision exhausted")?;
        match request.command {
            Command::SurfaceNavigate { surface_id, action } => {
                let snapshot = self
                    .surface_snapshot(&surface_id)
                    .ok_or("Surface is not active")?;
                let fpl_gfx::signals::SignalFrame::TaskDeck { mut panel } =
                    serde_json::from_value(snapshot["frame"].clone())
                        .map_err(|_| "Invalid task deck")?
                else {
                    return Err("Surface does not support task navigation".into());
                };
                let handled = panel
                    .navigate(action)
                    .map_err(|_| "Invalid task navigation")?;
                self.task_selection
                    .insert(surface_id, (panel.selected.clone(), panel.expanded));
                return Ok(
                    serde_json::json!({"handled":handled,"selected":panel.selected,"expanded":panel.expanded}),
                );
            }
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
                if let Some((Some(selected), _)) = self.task_selection.get(&surface_id) {
                    if !frame["panel"]["items"].as_array().is_some_and(|items| {
                        items.iter().any(|i| i["id"].as_str() == Some(selected))
                    }) {
                        self.task_selection.remove(&surface_id);
                    }
                }
                self.frames
                    .insert(surface_id, (frame, data, std::time::Instant::now()));
                return Ok(serde_json::json!({"accepted":true}));
            }
            Command::Inspect => {}
            Command::ReadDocument => {
                return serde_json::to_value(self.session.as_ref().and_then(Workspaces::document))
                    .map_err(|error| error.to_string());
            }
            Command::ReadPresentation => {
                let document = self.session.as_ref().and_then(Workspaces::document);
                let mut surfaces = std::collections::BTreeMap::new();
                if let Some(document) = document {
                    for surface in &document.surfaces {
                        if let Some(snapshot) = self.surface_snapshot(&surface.id) {
                            surfaces.insert(surface.id.clone(), snapshot);
                        }
                    }
                }
                return Ok(
                    serde_json::json!({"schema_version":1,"owner":self.owner,"revision":self.revision,"document":document,"surfaces":surfaces}),
                );
            }
            Command::WorkspaceRead { name } => {
                let session = self.session.as_ref().ok_or("Endpoint has no catalog")?;
                return serde_json::to_value(session.document_for(&name))
                    .map_err(|error| error.to_string());
            }
            Command::WorkspaceImport { document, activate } => {
                let session = self.session.as_mut().ok_or("Endpoint has no catalog")?;
                let changed = session.document_for(&document.project_id) != Some(document.as_ref());
                session.import_document(&document, activate)?;
                if changed {
                    self.frames.retain(|(workspace, _), _| workspace != &document.id);
                    self.task_selection.retain(|(workspace, _), _| workspace != &document.id);
                }
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
                self.task_selection.clear();
            }
            Command::WorkspaceSwitch { name } => {
                let session = self.session.as_mut().ok_or("Endpoint has no catalog")?;
                if session.document_for(&name).is_none() {
                    return Err("Project has no retained document".into());
                }
                session.select(Actor::Human, &name)?;
                self.frames.clear();
                self.task_selection.clear();
            }
            Command::WorkspacePages { pages } => {
                self.session
                    .as_mut()
                    .ok_or("Endpoint has no catalog")?
                    .set_pages(pages)?;
            }
            Command::WorkspacePage { index } => {
                self.session
                    .as_mut()
                    .ok_or("Endpoint has no catalog")?
                    .select_page(index)?;
            }
            Command::ApplyPatch { patch } => {
                let previous = self
                    .session
                    .as_ref()
                    .and_then(Workspaces::document)
                    .cloned();
                self.session
                    .as_mut()
                    .ok_or("Endpoint has no catalog")?
                    .apply_document_patch(&patch)?;
                let current = self.session.as_ref().and_then(Workspaces::document);
                let compatible = |id: &str| {
                    let old = previous
                        .as_ref()
                        .and_then(|d| d.surfaces.iter().find(|s| s.id == id));
                    let new = current
                        .as_ref()
                        .and_then(|d| d.surfaces.iter().find(|s| s.id == id));
                    let (
                        Some(crate::Surface {
                            content: crate::SurfaceContent::Widget { spec: old },
                            ..
                        }),
                        Some(crate::Surface {
                            content: crate::SurfaceContent::Widget { spec: new },
                            ..
                        }),
                    ) = (old, new)
                    else {
                        return false;
                    };
                    if old.data.source != new.data.source {
                        return false;
                    }
                    match (&old.kind, &new.kind) {
                        (
                            canvas_widgets::WidgetKind::SignalView { frame: a, .. },
                            canvas_widgets::WidgetKind::SignalView { frame: b, .. },
                        ) => a["view"] == b["view"],
                        (
                            canvas_widgets::WidgetKind::StatusList { .. },
                            canvas_widgets::WidgetKind::StatusList { .. },
                        ) => true,
                        _ => false,
                    }
                };
                self.frames.retain(|id, _| compatible(id));
                self.task_selection.retain(|id, _| compatible(id));
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

fn surface_frame_expiry(refresh_ms: Option<u64>) -> std::time::Duration {
    std::time::Duration::from_millis(refresh_ms.map_or(3000, |ms| {
        ms.saturating_mul(3)
            .saturating_add(1000)
            .clamp(3000, 300000)
    }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn task_navigation_survives_refresh_but_not_removed_identity_or_restart() {
        let mut endpoint = Endpoint::new("firetv-omni");
        let mut request: Request = serde_json::from_str(include_str!(
            "../../../config/deploy/firetv/workspaces.json"
        ))
        .unwrap();
        let Command::Install { documents, .. } = &mut request.command else {
            panic!()
        };
        documents[0].surfaces[1].content=crate::SurfaceContent::Widget { spec: canvas_widgets::WidgetSpec::from_json(br#"{"schema_version":2,"id":"tasks","kind":"signal_view","title":"Tasks","refresh_ms":2000,"frame":{"view":"task_deck","panel":{"items":[],"fresh":false}}}"#).unwrap() };
        endpoint.execute(request).unwrap();
        let before = endpoint.checkpoint();
        let a = serde_json::json!({"id":"a","project":"Canvas","title":"Task A","meta":"neo","state":"active"});
        let b = serde_json::json!({"id":"b","project":"Bus","title":"Task B","meta":"neo","state":"input"});
        let snapshot = |items: Vec<serde_json::Value>| Request {
            schema_version: 1,
            request_id: "tasks".into(),
            expected_revision: None,
            command: Command::SurfaceSnapshot {
                workspace_id: "tv-television".into(),
                surface_id: "clock".into(),
                frame: serde_json::json!({"view":"task_deck","panel":{"items":items,"fresh":true}}),
                data: Default::default(),
            },
        };
        endpoint
            .execute(snapshot(vec![a.clone(), b.clone()]))
            .unwrap();
        let navigate = |action| Request {
            schema_version: 1,
            request_id: "navigation".into(),
            expected_revision: None,
            command: Command::SurfaceNavigate {
                surface_id: "clock".into(),
                action,
            },
        };
        endpoint
            .execute(navigate(fpl_gfx::panels::TaskNavigation::Open))
            .unwrap();
        endpoint.execute(snapshot(vec![b, a.clone()])).unwrap();
        assert_eq!(
            endpoint.surface_snapshot("clock").unwrap()["frame"]["panel"]["selected"],
            "a"
        );
        assert_eq!(endpoint.checkpoint(), before);
        let presentation = endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "mirror-read".into(),
                expected_revision: None,
                command: Command::ReadPresentation,
            })
            .unwrap();
        assert_eq!(presentation["owner"], "firetv-omni");
        assert_eq!(
            presentation["surfaces"]["clock"]["frame"]["panel"]["selected"],
            "a"
        );
        assert_eq!(
            presentation["surfaces"]["clock"]["frame"]["panel"]["expanded"],
            true
        );
        assert_eq!(endpoint.checkpoint(), before);
        let doc = endpoint.session.as_ref().unwrap().document().unwrap();
        let patch = WorkspacePatch {
            schema_version: 1,
            workspace_id: doc.id.clone(),
            base_revision: doc.revision,
            revision: doc.revision + 1,
            operations: vec![crate::PatchOperation::SetFocus {
                surface_id: Some("clock".into()),
            }],
        };
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "layout-only".into(),
                expected_revision: Some(endpoint.revision),
                command: Command::ApplyPatch { patch },
            })
            .unwrap();
        assert_eq!(
            endpoint.surface_snapshot("clock").unwrap()["frame"]["panel"]["selected"],
            "a"
        );
        assert!(endpoint.frames.contains_key("clock"));
        endpoint.frames.get_mut("clock").unwrap().2 =
            std::time::Instant::now() - std::time::Duration::from_secs(4);
        assert_ne!(
            endpoint.surface_snapshot("clock").unwrap()["data"]["state"],
            "stale"
        );
        endpoint.frames.get_mut("clock").unwrap().2 =
            std::time::Instant::now() - std::time::Duration::from_secs(8);
        assert_eq!(
            endpoint.surface_snapshot("clock").unwrap()["frame"]["panel"]["fresh"],
            false
        );
        endpoint.execute(snapshot(vec![])).unwrap();
        endpoint.execute(snapshot(vec![a])).unwrap();
        assert!(
            endpoint.surface_snapshot("clock").unwrap()["frame"]["panel"]["selected"].is_null()
        );
        assert!(
            Endpoint::restore("firetv-omni", &before.to_string())
                .unwrap()
                .task_selection
                .is_empty()
        );
        let doc = endpoint.session.as_ref().unwrap().document().unwrap();
        let mut surface = doc.surfaces[1].clone();
        if let crate::SurfaceContent::Widget { spec } = &mut surface.content {
            spec.data.source = Some("unibus:other-owner".into());
        }
        let patch = WorkspacePatch {
            schema_version: 1,
            workspace_id: doc.id.clone(),
            base_revision: doc.revision,
            revision: doc.revision + 1,
            operations: vec![crate::PatchOperation::UpsertSurface { surface }],
        };
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "new-source".into(),
                expected_revision: Some(endpoint.revision),
                command: Command::ApplyPatch { patch },
            })
            .unwrap();
        assert!(!endpoint.frames.contains_key("clock"));
    }
    #[test]
    fn frame_expiry_tracks_refresh_and_bounds_jitter_budget() {
        assert_eq!(super::surface_frame_expiry(None).as_millis(), 3000);
        assert_eq!(super::surface_frame_expiry(Some(33)).as_millis(), 3000);
        assert_eq!(super::surface_frame_expiry(Some(2000)).as_millis(), 7000);
        assert_eq!(super::surface_frame_expiry(Some(30000)).as_millis(), 91000);
        assert_eq!(
            super::surface_frame_expiry(Some(u64::MAX)).as_millis(),
            300000
        );
    }
    use super::*;
    #[test]
    fn named_views_are_revision_bound_and_invalid_selection_is_atomic() {
        let mut endpoint = Endpoint::new("firetv-omni");
        let request: Request = serde_json::from_str(include_str!(
            "../../../config/deploy/firetv/workspaces.json"
        ))
        .unwrap();
        endpoint.execute(request).unwrap();
        let layout = endpoint
            .session
            .as_ref()
            .unwrap()
            .document()
            .unwrap()
            .layout
            .clone();
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "views".into(),
                expected_revision: Some(endpoint.revision),
                command: Command::WorkspacePages {
                    pages: vec![crate::Page {
                        name: "Routing".into(),
                        layout,
                    }],
                },
            })
            .unwrap();
        let revision = endpoint.revision;
        assert!(
            endpoint
                .execute(Request {
                    schema_version: 1,
                    request_id: "stale".into(),
                    expected_revision: Some(revision - 1),
                    command: Command::WorkspacePage { index: 1 }
                })
                .is_err()
        );
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "select".into(),
                expected_revision: Some(revision),
                command: Command::WorkspacePage { index: 1 },
            })
            .unwrap();
        assert_eq!(
            endpoint
                .session
                .as_ref()
                .unwrap()
                .document()
                .unwrap()
                .active_page_name(),
            "Routing"
        );
        let before = endpoint.snapshot();
        assert!(
            endpoint
                .execute(Request {
                    schema_version: 1,
                    request_id: "invalid".into(),
                    expected_revision: Some(endpoint.revision),
                    command: Command::WorkspacePage { index: 99 }
                })
                .is_err()
        );
        assert_eq!(endpoint.snapshot(), before);
    }
    #[test]
    fn header_slot_is_declared_transient_and_has_owner_freshness() {
        let mut endpoint = Endpoint::new("firetv-omni");
        let mut request: Request = serde_json::from_str(include_str!(
            "../../../config/deploy/firetv/workspaces.json"
        ))
        .unwrap();
        let Command::Install { documents, .. } = &mut request.command else {
            panic!()
        };
        let mut header = documents[0].surfaces[1].clone();
        header.id = HEADER_SURFACE_ID.into();
        header.content = crate::SurfaceContent::Widget { spec: canvas_widgets::WidgetSpec::from_json(br#"{"schema_version":2,"id":"workspace-header","kind":"status_list","title":"Waiting","nodes":[]}"#).unwrap() };
        documents[0].surfaces.push(header);
        endpoint.execute(request).unwrap();
        assert_eq!(endpoint.snapshot()["header"]["data"]["state"], "no_data");
        let saved = endpoint.checkpoint();
        endpoint
            .execute(Request {
                schema_version: 1,
                request_id: "header".into(),
                expected_revision: None,
                command: Command::SurfaceSnapshot {
                    workspace_id: "tv-television".into(),
                    surface_id: HEADER_SURFACE_ID.into(),
                    frame: serde_json::json!({"title":"Album · Song","nodes":[]}),
                    data: canvas_widgets::WidgetData {
                        state: canvas_widgets::WidgetDataState::Ready,
                        ..Default::default()
                    },
                },
            })
            .unwrap();
        assert_eq!(
            endpoint.snapshot()["header"]["frame"]["title"],
            "Album · Song"
        );
        assert_eq!(endpoint.checkpoint(), saved);
        endpoint.frames.get_mut(HEADER_SURFACE_ID).unwrap().2 =
            std::time::Instant::now() - std::time::Duration::from_secs(4);
        assert_eq!(endpoint.snapshot()["header"]["data"]["state"], "stale");
        let restored = Endpoint::restore("firetv-omni", &saved.to_string()).unwrap();
        assert_eq!(restored.snapshot()["header"]["data"]["state"], "no_data");
    }
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
                // Transient frames stay valid across navigation revisions;
                // selected workspace and declared surface still gate ingress.
                expected_revision: Some(1),
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
