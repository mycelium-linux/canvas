//! Thin MCP adapter over the permission-checked owner-local Canvas socket.
use crate::{
    WorkspaceDocument, WorkspacePatch,
    control::{self, Command, ControlReply, ControlRequest, Outcome},
};
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
use serde::Deserialize;
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
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            send: Arc::new(control::send),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
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
        let request = match parse(
            &request.name,
            Value::Object(request.arguments.unwrap_or_default()),
        ) {
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
        assert_eq!(tools.len(), 8);
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
        assert_eq!(client.list_all_tools().await.unwrap().len(), 8);
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
