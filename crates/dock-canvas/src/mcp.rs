//! Thin MCP adapter over the permission-checked owner-local Canvas socket.
use crate::{
    WorkspaceDocument, WorkspacePatch,
    control::{self, Command, ControlReply, ControlRequest, Outcome, Receipt},
    reconcile::{self, DesiredWorkspaceState, ReconcileAction, Reconciliation},
};
use dock_core::Catalog;
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
        Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
        Tool,
    },
    service::RequestContext,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Inspect {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ApplyDocument {
    /// Correlation identifier: 1-64 lowercase letters, digits, hyphens or underscores.
    request_id: String,
    /// Runtime revision from canvas_inspect, not the document revision.
    expected_revision: u64,
    document: WorkspaceDocument,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ApplyPatch {
    request_id: String,
    /// Runtime revision from canvas_inspect. The patch also carries its document base_revision.
    expected_revision: u64,
    patch: WorkspacePatch,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SurfaceMutation {
    request_id: String,
    expected_revision: u64,
    surface_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum NavigateTarget {
    Next,
    Previous,
    Page { index: u32 },
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NavigateSurface {
    request_id: String,
    expected_revision: u64,
    surface_id: String,
    target: NavigateTarget,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenFile {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    surface_id: String,
    path: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderMermaid {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    surface_id: String,
    source: String,
    #[serde(default)]
    title: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReconcileIntent {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    desired: DesiredWorkspaceState,
    #[serde(default)]
    dry_run: bool,
}

#[derive(Serialize)]
struct IntentResult {
    reconciliation: Reconciliation,
    #[serde(skip_serializing_if = "Option::is_none")]
    application: Option<ControlReply>,
}

fn tool<T: JsonSchema>(name: &'static str, description: &'static str) -> Tool {
    let schema = serde_json::to_value(schemars::schema_for!(T)).expect("serializable schema");
    Tool::new(
        name,
        description,
        schema.as_object().expect("object schema").clone(),
    )
}

fn tools() -> Vec<Tool> {
    vec![
        tool::<Inspect>(
            "canvas_inspect",
            "Read the live Canvas document and runtime snapshot together. Inspect before editing; resolve exact surface and project IDs. Does not start Canvas.",
        ),
        tool::<ApplyDocument>(
            "canvas_apply_document",
            "Replace the visible workspace with a validated Canvas document at the observed runtime revision. Presentation only; cannot launch processes or execute code. Prefer a patch for small edits.",
        ),
        tool::<ApplyPatch>(
            "canvas_apply_patch",
            "Atomically edit surfaces, layout, focus, scene content or data bindings using the Canvas patch protocol. Requires both the observed runtime revision and document base_revision. On conflict inspect again; never blindly retry a mutation.",
        ),
        tool::<SurfaceMutation>(
            "canvas_close_surface",
            "Close one exact Canvas surface at the observed runtime revision. Closing a surface does not terminate its underlying application or session.",
        ),
        tool::<Inspect>(
            "canvas_focus",
            "Bring the Canvas window forward. Does not change document content, launch applications, or select a surface.",
        ),
        tool::<NavigateSurface>(
            "canvas_navigate",
            "Navigate an already-open paged surface to the next, previous, or exact zero-based page at the observed runtime revision.",
        ),
        tool::<OpenFile>(
            "canvas_open_file",
            "Open a file (image, PDF, Markdown, or rendered Mermaid) as a Canvas surface. Omit expected_revision for fire-and-forget.",
        ),
        tool::<RenderMermaid>(
            "canvas_mermaid",
            "Render a Mermaid diagram via mmdc (outside the Canvas process) and open the result as a surface. Keeps Canvas thin.",
        ),
        tool::<ReconcileIntent>(
            "canvas_reconcile_intent",
            "Reach a resolved desired workspace state deterministically. Canvas inspects current state, computes the smallest validated change, and applies it through the revisioned owner-local socket. Set dry_run to explain without applying. Project and resource IDs must already be exact and authorized.",
        ),
    ]
}

fn parse(name: &str, args: Value) -> Result<ControlRequest, String> {
    if serde_json::to_vec(&args).map_err(|e| e.to_string())?.len() as u64
        > control::MAX_CONTROL_MESSAGE
    {
        return Err("Canvas MCP arguments exceed the control limit".into());
    }
    let (request_id, expected_revision, command) = match name {
        "canvas_inspect" => {
            serde_json::from_value::<Inspect>(args).map_err(|e| e.to_string())?;
            ("mcp-inspect".into(), None, Command::ReadDocument)
        }
        "canvas_apply_document" => {
            let args: ApplyDocument = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                Some(args.expected_revision),
                Command::ApplyDocument {
                    document: args.document,
                },
            )
        }
        "canvas_apply_patch" => {
            let args: ApplyPatch = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                Some(args.expected_revision),
                Command::ApplyPatch { patch: args.patch },
            )
        }
        "canvas_close_surface" => {
            let args: SurfaceMutation = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                Some(args.expected_revision),
                Command::CloseSurface {
                    surface_id: args.surface_id,
                },
            )
        }
        "canvas_focus" => {
            serde_json::from_value::<Inspect>(args).map_err(|e| e.to_string())?;
            ("mcp-focus".into(), None, Command::Focus)
        }
        "canvas_navigate" => {
            let args: NavigateSurface = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let target = match args.target {
                NavigateTarget::Next => control::Navigation::Next,
                NavigateTarget::Previous => control::Navigation::Previous,
                NavigateTarget::Page { index } => control::Navigation::Page { index },
            };
            (
                args.request_id,
                Some(args.expected_revision),
                Command::Navigate {
                    surface_id: args.surface_id,
                    target,
                },
            )
        }
        "canvas_mermaid" => {
            let args: RenderMermaid = serde_json::from_value(args).map_err(|e| e.to_string())?;
            let png_path = render_mermaid(&args.source)?;
            (
                args.request_id,
                args.expected_revision,
                Command::OpenFile {
                    surface_id: args.surface_id,
                    path: png_path,
                    title: args.title,
                },
            )
        }
        "canvas_open_file" => {
            let args: OpenFile = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                args.expected_revision,
                Command::OpenFile {
                    surface_id: args.surface_id,
                    path: args.path,
                    title: args.title,
                },
            )
        }
        _ => return Err("Unknown Canvas tool".into()),
    };
    let request = ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id,
        expected_revision,
        command,
    };
    request.validate()?;
    Ok(request)
}

fn render_mermaid(source: &str) -> Result<String, String> {
    let temp_dir = std::env::temp_dir();
    let pid = std::process::id();
    let input = temp_dir.join(format!("canvas-mmd-{}.mmd", pid));
    let output = temp_dir.join(format!("canvas-mmd-{}.png", pid));
    std::fs::write(&input, source).map_err(|e| format!("write temp file: {e}"))?;
    let status = std::process::Command::new("mmdc")
        .arg("-i")
        .arg(&input)
        .arg("-o")
        .arg(&output)
        .arg("--quiet")
        .status()
        .map_err(|e| format!("mmdc not found: {e}"))?;
    std::fs::remove_file(&input).ok();
    if !status.success() {
        return Err("mmdc failed to render Mermaid diagram".into());
    }
    Ok(output.to_string_lossy().to_string())
}

type Sender = dyn Fn(&ControlRequest) -> Result<ControlReply, String> + Send + Sync;

#[derive(Clone)]
struct Canvas {
    send: Arc<Sender>,
    slot: Arc<tokio::sync::Semaphore>,
    catalog: Arc<Catalog>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            send: Arc::new(control::send),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            catalog: Arc::new(Catalog::bundled().expect("bundled project catalog must be valid")),
        }
    }
}

impl Canvas {
    async fn reconcile_intent(&self, args: Value) -> Result<CallToolResponse, ErrorData> {
        let args: ReconcileIntent = match serde_json::from_value(args) {
            Ok(args) => args,
            Err(error) => {
                return Ok(
                    CallToolResult::error(vec![ContentBlock::text(error.to_string())]).into(),
                );
            }
        };
        let Ok(permit) = self.slot.clone().try_acquire_owned() else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "Canvas is busy; inspect before retrying a mutation",
            )])
            .into());
        };
        let send = self.send.clone();
        let catalog = self.catalog.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let inspect_request = ControlRequest {
                schema_version: control::CONTROL_VERSION,
                request_id: args.request_id.clone(),
                expected_revision: None,
                command: Command::ReadDocument,
            };
            inspect_request.validate()?;
            let inspect = send(&inspect_request)?;
            if args
                .expected_revision
                .is_some_and(|expected| expected != inspect.revision)
            {
                return Err(format!(
                    "Canvas revision conflict: expected {}, observed {}",
                    args.expected_revision.expect("checked above"),
                    inspect.revision
                ));
            }
            let Outcome::Ok {
                receipt: Receipt::DocumentSnapshot { document, .. },
            } = inspect.outcome
            else {
                return Err(serde_json::to_string(&inspect).map_err(|error| error.to_string())?);
            };
            let reconciliation = reconcile::reconcile(&document, &args.desired, &catalog)
                .map_err(|error| error.to_string())?;
            if args.dry_run {
                return Ok(IntentResult {
                    reconciliation,
                    application: None,
                });
            }
            let command = match &reconciliation.action {
                ReconcileAction::NoChange { .. } => {
                    return Ok(IntentResult {
                        reconciliation,
                        application: None,
                    });
                }
                ReconcileAction::ApplyPatch { patch } => Command::ApplyPatch {
                    patch: patch.clone(),
                },
                ReconcileAction::ApplyDocument { document } => Command::ApplyDocument {
                    document: document.clone(),
                },
            };
            let apply_request = ControlRequest {
                schema_version: control::CONTROL_VERSION,
                request_id: args.request_id,
                expected_revision: Some(inspect.revision),
                command,
            };
            apply_request.validate()?;
            let application = send(&apply_request)?;
            Ok(IntentResult {
                reconciliation,
                application: Some(application),
            })
        })
        .await
        .map_err(|_| ErrorData::internal_error("Canvas intent worker failed", None))?;

        match result {
            Ok(result) => {
                let is_error = result
                    .application
                    .as_ref()
                    .is_some_and(|reply| matches!(reply.outcome, Outcome::Error { .. }));
                let content = vec![ContentBlock::text(serde_json::to_string(&result).map_err(
                    |_| ErrorData::internal_error("Canvas intent receipt encoding failed", None),
                )?)];
                if is_error {
                    Ok(CallToolResult::error(content).into())
                } else {
                    Ok(CallToolResult::success(content).into())
                }
            }
            Err(error) => Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into()),
        }
    }
}

impl ServerHandler for Canvas {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("dock-canvas", env!("CARGO_PKG_VERSION"));
        info.instructions = Some("Canvas is presentation, not shell execution. Inspect first. Respect revision conflicts and human changes. A timeout leaves the outcome unknown: inspect before retrying. Request IDs correlate replies but are not durable idempotency keys.".into());
        info
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|t| t.name == name)
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        if request.name == "canvas_reconcile_intent" {
            return self.reconcile_intent(arguments).await;
        }
        let request = match parse(&request.name, arguments) {
            Ok(request) => request,
            Err(error) => return Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into()),
        };
        let Ok(permit) = self.slot.clone().try_acquire_owned() else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "Canvas is busy; inspect before retrying a mutation",
            )])
            .into());
        };
        let send = self.send.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Keep the slot until socket I/O finishes even if the MCP caller disconnects.
            let _permit = permit;
            send(&request)
        })
        .await
        .map_err(|_| ErrorData::internal_error("Canvas control worker failed", None))?;
        let result = match result {
            Ok(reply) => {
                let is_error = matches!(reply.outcome, Outcome::Error { .. });
                let content = vec![ContentBlock::text(serde_json::to_string(&reply).map_err(
                    |_| ErrorData::internal_error("Canvas receipt encoding failed", None),
                )?)];
                if is_error {
                    CallToolResult::error(content)
                } else {
                    CallToolResult::success(content)
                }
            }
            Err(error) => CallToolResult::error(vec![ContentBlock::text(format!(
                "Canvas control unavailable or outcome unknown: {error}"
            ))]),
        };
        Ok(result.into())
    }
}

pub fn serve() -> Result<(), String> {
    tokio::runtime::Runtime::new()
        .map_err(|e| e.to_string())?
        .block_on(async {
            Canvas::default()
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| e.to_string())?
                .waiting()
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mutations_require_revision_and_reject_unknown_arguments() {
        assert!(parse("canvas_inspect", json!({"command":"shutdown"})).is_err());
        assert!(parse("shutdown", json!({})).is_err());
        let patch = json!({"schema_version":1,"workspace_id":"workspace","base_revision":1,"revision":2,"operations":[]});
        assert!(
            parse(
                "canvas_apply_patch",
                json!({"request_id":"edit-1","patch":patch})
            )
            .is_err()
        );
        let request = parse(
            "canvas_apply_patch",
            json!({"request_id":"edit-1","expected_revision":4,"patch":patch}),
        )
        .unwrap();
        assert_eq!(request.expected_revision, Some(4));
        assert!(matches!(request.command, Command::ApplyPatch { .. }));
    }

    #[test]
    fn schemas_include_native_document_and_patch_definitions() {
        let tools = tools();
        assert_eq!(tools.len(), 9);
        assert!(
            tools[1].input_schema["$defs"]
                .get("WorkspaceDocument")
                .is_some()
        );
        assert!(
            tools[2].input_schema["$defs"]
                .get("PatchOperation")
                .is_some()
        );
        assert_eq!(tools[2].input_schema["additionalProperties"], false);
        assert!(
            tools[8].input_schema["$defs"]
                .get("DesiredWorkspaceState")
                .is_some()
        );
    }

    #[tokio::test]
    async fn intent_tool_inspects_reconciles_and_applies_through_the_socket_boundary() {
        use crate::{Layout, Surface, SurfaceContent, ThemeRef};
        use dock_core::layout::Rect;
        use std::sync::Mutex;

        let catalog = Arc::new(
            Catalog::parse(
                r#"{
                  "schema_version":1,
                  "projects":[{
                    "id":"dock",
                    "name":"Dock",
                    "theme":{"source":"coherence","mode":"dark","accent":"info"},
                    "resources":[
                      {"id":"docs","title":"Docs","host":"mac","kind":"mac_app","target":"com.apple.Safari"},
                      {"id":"invoice","title":"Invoice","host":"mac","kind":"mac_app","target":"com.apple.Preview"}
                    ]
                  }]
                }"#,
            )
            .unwrap(),
        );
        let state = Arc::new(Mutex::new(WorkspaceDocument {
            schema_version: crate::SCHEMA_VERSION,
            id: "primary".into(),
            project_id: "dock".into(),
            revision: 4,
            theme: ThemeRef::Project,
            background: crate::Background::default(),
            surfaces: vec![Surface {
                id: "docs".into(),
                title: "Docs".into(),
                content: SurfaceContent::ProjectResource {
                    resource_id: "docs".into(),
                },
                inputs: Vec::new(),
            }],
            layout: Layout::Surface {
                surface_id: "docs".into(),
            },
            bindings: Vec::new(),
            focused_surface_id: Some("docs".into()),
        }));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let send_state = state.clone();
        let send_catalog = catalog.clone();
        let send_calls = calls.clone();
        let canvas = Canvas {
            send: Arc::new(move |request| {
                let mut state = send_state.lock().unwrap();
                match &request.command {
                    Command::ReadDocument => {
                        send_calls.lock().unwrap().push("read");
                        Ok(ControlReply {
                            schema_version: control::CONTROL_VERSION,
                            request_id: request.request_id.clone(),
                            revision: 10,
                            outcome: Outcome::Ok {
                                receipt: Receipt::DocumentSnapshot {
                                    document: Box::new(state.clone()),
                                    snapshot: control::RuntimeSnapshot {
                                        workspace_id: state.id.clone(),
                                        project_id: state.project_id.clone(),
                                        revision: 10,
                                        frame: Rect {
                                            x: 0,
                                            y: 0,
                                            width: 1920,
                                            height: 1080,
                                        },
                                        focused_surface_id: state.focused_surface_id.clone(),
                                        surfaces: Vec::new(),
                                        active_indicators: Vec::new(),
                                        mesh: None,
                                        presence: None,
                                        presence_preview_active: false,
                                        screensaver: Default::default(),
                                        workspaces: Vec::new(),
                                    },
                                },
                            },
                        })
                    }
                    Command::ApplyPatch { patch } => {
                        send_calls.lock().unwrap().push("apply_patch");
                        assert_eq!(request.expected_revision, Some(10));
                        *state = state.apply_patch(patch, &send_catalog).unwrap();
                        Ok(ControlReply {
                            schema_version: control::CONTROL_VERSION,
                            request_id: request.request_id.clone(),
                            revision: 11,
                            outcome: Outcome::Ok {
                                receipt: Receipt::DocumentApplied {
                                    workspace_id: state.id.clone(),
                                },
                            },
                        })
                    }
                    _ => panic!("intent tool sent an unexpected socket command"),
                }
            }),
            catalog: catalog.clone(),
            ..Canvas::default()
        };
        let desired = reconcile::DesiredWorkspaceState {
            schema_version: crate::SCHEMA_VERSION,
            workspace_id: "primary".into(),
            project_id: "dock".into(),
            mode: reconcile::ReconcileMode::Preserve,
            background: None,
            surfaces: vec![Surface {
                id: "invoice".into(),
                title: "Invoice".into(),
                content: SurfaceContent::ProjectResource {
                    resource_id: "invoice".into(),
                },
                inputs: Vec::new(),
            }],
            bindings: Vec::new(),
            layout: None,
            focus: reconcile::DesiredFocus::Surface {
                surface_id: "invoice".into(),
            },
        };
        canvas
            .reconcile_intent(json!({
                "request_id":"intent-invoice",
                "desired": desired,
                "dry_run": false
            }))
            .await
            .unwrap();
        assert_eq!(*calls.lock().unwrap(), ["read", "apply_patch"]);
        let state = state.lock().unwrap();
        assert!(state.surfaces.iter().any(|surface| surface.id == "docs"));
        assert!(state.surfaces.iter().any(|surface| surface.id == "invoice"));
        assert_eq!(state.focused_surface_id.as_deref(), Some("invoice"));
    }

    #[tokio::test]
    async fn sdk_roundtrip_preserves_runtime_conflicts() {
        let canvas = Canvas {
            send: Arc::new(|request| {
                Ok(ControlReply {
                    schema_version: 1,
                    request_id: request.request_id.clone(),
                    revision: 9,
                    outcome: Outcome::Error {
                        code: control::ErrorCode::Conflict,
                        message: "stale revision".into(),
                    },
                })
            }),
            ..Canvas::default()
        };
        let (client_io, server_io) = tokio::io::duplex(65536);
        let server = tokio::spawn(async move {
            canvas
                .serve(server_io)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap()
        });
        let client = ().serve(client_io).await.unwrap();
        assert_eq!(client.list_all_tools().await.unwrap().len(), 9);
        let result = client
            .call_tool(CallToolRequestParams::new("canvas_inspect"))
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let reply: ControlReply =
            serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
        assert_eq!(reply.revision, 9);
        client.cancel().await.unwrap();
        server.await.unwrap();
    }
}
