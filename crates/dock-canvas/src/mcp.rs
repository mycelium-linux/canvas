//! Thin MCP adapter over the permission-checked owner-local Canvas socket.
use crate::{
    WorkspaceDocument, WorkspacePatch,
    control::{self, Command, ControlReply, ControlRequest, Outcome, OverlaySettings, Receipt},
    display::DisplayTarget,
    reconcile::{self, DesiredWorkspaceState, ReconcileAction, Reconciliation},
    view::{SharedViewDocument, SharedViewPatch},
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PatchSharedView {
    request_id: String,
    /// Runtime revision from canvas_inspect.
    expected_revision: u64,
    surface_id: String,
    patch: SharedViewPatch,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ScreenshotParams {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    #[serde(default)]
    surface_id: Option<String>,
    /// When supplied, reject the capture unless this exact shared view is visible.
    #[serde(default)]
    expected_view_revision: Option<u64>,
    #[serde(default)]
    x: Option<i64>,
    #[serde(default)]
    y: Option<i64>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CaptureSharedView {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    surface_id: String,
    view_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct CaptureMetadata {
    canvas_revision: u64,
    surface_id: Option<String>,
    region: dock_core::layout::Rect,
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<SharedViewDocument>,
    #[serde(skip)]
    capture_chrome: Option<control::CaptureChromeSnapshot>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetDisplayTarget {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    target: DisplayTarget,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetOverlays {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    overlays: OverlaySettings,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetVisibility {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    visible: bool,
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
        tool::<SetDisplayTarget>(
            "canvas_display_target",
            "Present the shared Canvas desktop on all connected displays or an exact list of display IDs from canvas_inspect. The workspace document and surface state remain shared across the selected display windows.",
        ),
        tool::<SetOverlays>(
            "canvas_set_overlays",
            "Set deterministic host-local placement for the presence rail, face preview, compact orb, system HUD, and FPL mark. Each overlay may be hidden, shown on all displays, or anchored to the leftmost or rightmost selected display.",
        ),
        tool::<SetVisibility>(
            "canvas_set_visibility",
            "Show or hide every native window in the shared Canvas presentation while preserving the workspace and selected displays.",
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
        tool::<PatchSharedView>(
            "canvas_view_patch",
            "Atomically change the viewport, visible attention, source position, or parameters of a mounted shared view. Use canvas_navigate before changing a PDF page. The physical Canvas and agent capture use the same resulting view revision.",
        ),
        tool::<ScreenshotParams>(
            "canvas_screenshot",
            "Capture a PNG screenshot of a Canvas surface or screen region. Provide surface_id to capture a specific surface, or x/y/width/height for an absolute screen region. Returns base64-encoded PNG data. Essential for LLM vision workflows: focus a region, screenshot it, inspect details, then zoom further.",
        ),
        tool::<CaptureSharedView>(
            "canvas_view_capture",
            "Capture the exact visibly mounted shared view revision for agent vision. Fails if the surface is hidden, moved, replaced, or has advanced since inspection. Returns capture metadata followed by PNG image content.",
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
        "canvas_display_target" => {
            let args: SetDisplayTarget = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                args.expected_revision,
                Command::SetDisplayTarget {
                    target: args.target,
                },
            )
        }
        "canvas_set_overlays" => {
            let args: SetOverlays = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                args.expected_revision,
                Command::SetOverlays {
                    overlays: args.overlays,
                },
            )
        }
        "canvas_set_visibility" => {
            let args: SetVisibility = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                args.expected_revision,
                Command::SetVisibility {
                    visible: args.visible,
                },
            )
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
        "canvas_view_patch" => {
            let args: PatchSharedView = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                Some(args.expected_revision),
                Command::PatchView {
                    surface_id: args.surface_id,
                    patch: args.patch,
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

    async fn screenshot(
        &self,
        args: Value,
        require_shared_view: bool,
    ) -> Result<CallToolResponse, ErrorData> {
        let params = if require_shared_view {
            let args: CaptureSharedView = serde_json::from_value(args)
                .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?;
            ScreenshotParams {
                request_id: args.request_id,
                expected_revision: args.expected_revision,
                surface_id: Some(args.surface_id),
                expected_view_revision: Some(args.view_revision),
                x: None,
                y: None,
                width: None,
                height: None,
            }
        } else {
            serde_json::from_value(args)
                .map_err(|error| ErrorData::invalid_params(error.to_string(), None))?
        };
        let validation = ControlRequest {
            schema_version: control::CONTROL_VERSION,
            request_id: params.request_id.clone(),
            expected_revision: None,
            command: Command::Inspect,
        };
        if let Err(error) = validation.validate() {
            return Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into());
        }
        let Ok(permit) = self.slot.clone().try_acquire_owned() else {
            return Ok(CallToolResult::error(vec![ContentBlock::text(
                "Canvas is busy; inspect before retrying capture",
            )])
            .into());
        };
        let request = ControlRequest {
            schema_version: control::CONTROL_VERSION,
            request_id: "mcp-capture-inspect".into(),
            expected_revision: None,
            command: Command::Inspect,
        };
        let send = self.send.clone();
        let result = tokio::task::spawn_blocking(move || {
            // Serialize inspection and capture so this adapter cannot change the
            // displayed view between the revision check and compositor readback.
            let _permit = permit;
            let inspect_reply = send(&request)?;
            let snapshot = match inspect_reply.outcome {
                Outcome::Ok {
                    receipt: Receipt::Snapshot { snapshot },
                } => snapshot,
                Outcome::Error { message, .. } => return Err(message),
                _ => return Err("Canvas returned the wrong screenshot receipt".into()),
            };
            let metadata = resolve_capture(&snapshot, &params, require_shared_view)?;
            let png_bytes = capture_region(metadata.region, &params.request_id)?;
            if metadata.surface_id.is_some() {
                let chrome = metadata
                    .capture_chrome
                    .ok_or("Canvas runtime did not provide capture verification colors")?;
                verify_surface_chrome(&png_bytes, metadata.region, chrome)?;
            }
            let verify_request = ControlRequest {
                schema_version: control::CONTROL_VERSION,
                request_id: "mcp-capture-verify".into(),
                expected_revision: None,
                command: Command::Inspect,
            };
            let verify_reply = send(&verify_request)?;
            let verify_snapshot = match verify_reply.outcome {
                Outcome::Ok {
                    receipt: Receipt::Snapshot { snapshot },
                } => snapshot,
                Outcome::Error { message, .. } => return Err(message),
                _ => return Err("Canvas returned the wrong screenshot verification receipt".into()),
            };
            let verified = resolve_capture(&verify_snapshot, &params, require_shared_view)?;
            if verified.surface_id != metadata.surface_id
                || verified.region != metadata.region
                || verified.view != metadata.view
            {
                return Err("Canvas changed while the screen capture was being read".into());
            }
            Ok::<_, String>((metadata, png_bytes))
        })
        .await
        .map_err(|_| ErrorData::internal_error("Canvas capture worker failed", None))?;
        let (metadata, png_bytes) = match result {
            Ok(result) => result,
            Err(error) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into());
            }
        };
        let encoded = {
            use base64::Engine as _;
            base64::engine::general_purpose::STANDARD.encode(&png_bytes)
        };
        let metadata = serde_json::to_string(&metadata)
            .map_err(|_| ErrorData::internal_error("Capture metadata encoding failed", None))?;

        Ok(CallToolResult::success(vec![
            ContentBlock::text(metadata),
            ContentBlock::image(encoded, "image/png"),
        ])
        .into())
    }
}

fn resolve_capture(
    snapshot: &control::RuntimeSnapshot,
    params: &ScreenshotParams,
    require_shared_view: bool,
) -> Result<CaptureMetadata, String> {
    if !snapshot.visible {
        return Err("Canvas is hidden; show it before capturing the displayed view".into());
    }
    if let Some(expected) = params.expected_revision
        && expected != snapshot.revision
    {
        return Err(format!(
            "Canvas revision conflict: expected {expected}, current {}",
            snapshot.revision
        ));
    }

    let (region, view) = if let Some(surface_id) = &params.surface_id {
        if params.x.is_some()
            || params.y.is_some()
            || params.width.is_some()
            || params.height.is_some()
        {
            return Err("Choose surface_id or an absolute region, not both".into());
        }
        let region = snapshot
            .surface_bounds
            .get(surface_id)
            .copied()
            .ok_or_else(|| format!("Canvas surface is not visibly mounted: {surface_id}"))?;
        let mounted = snapshot
            .views
            .iter()
            .find(|mounted| mounted.surface_id == *surface_id);
        if let Some(mounted) = mounted
            && mounted.bounds != region
        {
            return Err(format!(
                "Canvas snapshot has inconsistent bounds for shared view: {surface_id}"
            ));
        }
        if require_shared_view && mounted.is_none() {
            return Err(format!(
                "Canvas surface has no mounted shared view: {surface_id}"
            ));
        }
        let view = mounted.map(|mounted| mounted.view.clone());
        if let Some(expected) = params.expected_view_revision {
            let current = view.as_ref().ok_or_else(|| {
                format!("Canvas surface has no mounted shared view: {surface_id}")
            })?;
            if expected != current.revision {
                return Err(format!(
                    "Shared view revision conflict for {surface_id}: expected {expected}, current {}",
                    current.revision
                ));
            }
        }
        (region, view)
    } else {
        if require_shared_view || params.expected_view_revision.is_some() {
            return Err("A shared view capture requires surface_id".into());
        }
        let x = params.x.unwrap_or(i64::from(snapshot.frame.x));
        let y = params.y.unwrap_or(i64::from(snapshot.frame.y));
        (
            dock_core::layout::Rect {
                x: i32::try_from(x).map_err(|_| "Capture x coordinate is out of range")?,
                y: i32::try_from(y).map_err(|_| "Capture y coordinate is out of range")?,
                width: params.width.unwrap_or(snapshot.frame.width),
                height: params.height.unwrap_or(snapshot.frame.height),
            },
            None,
        )
    };
    if region.width == 0 || region.height == 0 || region.width > 16_384 || region.height > 16_384 {
        return Err("Capture dimensions must be between 1 and 16384 pixels".into());
    }
    Ok(CaptureMetadata {
        canvas_revision: snapshot.revision,
        surface_id: params.surface_id.clone(),
        region,
        view,
        capture_chrome: snapshot.capture_chrome,
    })
}

fn verify_surface_chrome(
    png_bytes: &[u8],
    region: dock_core::layout::Rect,
    chrome: control::CaptureChromeSnapshot,
) -> Result<(), String> {
    let image = image::load_from_memory_with_format(png_bytes, image::ImageFormat::Png)
        .map_err(|error| format!("Captured PNG is invalid: {error}"))?
        .to_rgba8();
    let scale_x = image.width() as f32 / region.width.max(1) as f32;
    let scale_y = image.height() as f32 / region.height.max(1) as f32;
    let sample = |logical_x: f32, logical_y: f32| -> [u8; 3] {
        let center_x = (logical_x * scale_x).round() as i32;
        let center_y = (logical_y * scale_y).round() as i32;
        let radius = ((2.0 * scale_x.min(scale_y)).round() as i32).max(1);
        let mut total = [0_u32; 3];
        let mut count = 0_u32;
        for y in center_y - radius..=center_y + radius {
            for x in center_x - radius..=center_x + radius {
                if x >= 0 && y >= 0 && x < image.width() as i32 && y < image.height() as i32 {
                    let pixel = image.get_pixel(x as u32, y as u32).0;
                    for channel in 0..3 {
                        total[channel] += u32::from(pixel[channel]);
                    }
                    count += 1;
                }
            }
        }
        if count == 0 {
            return [0; 3];
        }
        [
            (total[0] / count) as u8,
            (total[1] / count) as u8,
            (total[2] / count) as u8,
        ]
    };
    let observed = [sample(12.5, 14.0), sample(30.5, 14.0), sample(48.5, 14.0)];
    let expected = [chrome.danger, chrome.warning, chrome.success];
    let matches = observed
        .into_iter()
        .zip(expected)
        .all(|(actual, expected)| {
            actual
                .into_iter()
                .zip(expected)
                .all(|(actual, expected)| actual.abs_diff(expected) <= 56)
        });
    if !matches {
        return Err(
            "Captured pixels do not contain the requested Canvas surface chrome; the surface may be occluded or on another desktop"
                .into(),
        );
    }
    Ok(())
}

fn capture_region(region: dock_core::layout::Rect, request_id: &str) -> Result<Vec<u8>, String> {
    let temp_path = std::env::temp_dir().join(format!(
        "canvas-screenshot-{}-{request_id}.png",
        std::process::id()
    ));
    #[cfg(target_os = "macos")]
    let output = {
        let geometry = format!(
            "{},{},{},{}",
            region.x, region.y, region.width, region.height
        );
        std::process::Command::new("screencapture")
            .args(["-x", "-R", &geometry, "-t", "png"])
            .arg(&temp_path)
            .output()
            .map_err(|error| format!("Could not start macOS screen capture: {error}"))?
    };

    #[cfg(not(target_os = "macos"))]
    let output = {
        let geometry = format!(
            "{},{} {}x{}",
            region.x, region.y, region.width, region.height
        );
        std::process::Command::new("grim")
            .args(["-g", &geometry])
            .arg(&temp_path)
            .output()
            .map_err(|error| format!("Could not start Wayland screen capture: {error}"))?
    };

    if !output.status.success() {
        let _ = std::fs::remove_file(&temp_path);
        return Err(format!(
            "Screen capture failed with status {}",
            output.status
        ));
    }
    let png_bytes =
        std::fs::read(&temp_path).map_err(|error| format!("Could not read captured PNG: {error}"));
    let _ = std::fs::remove_file(&temp_path);
    let png_bytes = png_bytes?;
    if png_bytes.is_empty() {
        return Err("Screen capture returned an empty PNG".into());
    }
    Ok(png_bytes)
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
        if request.name == "canvas_screenshot" {
            return self.screenshot(arguments, false).await;
        }
        if request.name == "canvas_view_capture" {
            return self.screenshot(arguments, true).await;
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
        let target = parse(
            "canvas_display_target",
            json!({
                "request_id":"span-1",
                "expected_revision":4,
                "target":{"kind":"exact","display_ids":["left","right"]}
            }),
        )
        .unwrap();
        assert_eq!(target.expected_revision, Some(4));
        assert!(matches!(
            target.command,
            Command::SetDisplayTarget {
                target: DisplayTarget::Exact { .. }
            }
        ));
        let overlays = parse(
            "canvas_set_overlays",
            json!({
                "request_id":"overlays-1",
                "overlays":{
                    "presence_rail":"leftmost",
                    "presence_preview":"leftmost",
                    "presence_orb":"hidden",
                    "system_hud":"rightmost",
                    "fpl_mark":"leftmost",
                    "shortcut_help":"center"
                }
            }),
        )
        .unwrap();
        assert!(matches!(overlays.command, Command::SetOverlays { .. }));
        let visibility = parse(
            "canvas_set_visibility",
            json!({"request_id":"visibility-1","visible":false}),
        )
        .unwrap();
        assert_eq!(
            visibility.command,
            Command::SetVisibility { visible: false }
        );
    }

    #[test]
    fn schemas_include_native_document_and_patch_definitions() {
        let tools = tools();
        assert_eq!(tools.len(), 15);
        let tool = |name: &str| tools.iter().find(|tool| tool.name == name).unwrap();
        assert!(
            tool("canvas_apply_document").input_schema["$defs"]
                .get("WorkspaceDocument")
                .is_some()
        );
        assert!(
            tool("canvas_apply_patch").input_schema["$defs"]
                .get("PatchOperation")
                .is_some()
        );
        assert_eq!(
            tool("canvas_apply_patch").input_schema["additionalProperties"],
            false
        );
        assert!(
            tool("canvas_reconcile_intent").input_schema["$defs"]
                .get("DesiredWorkspaceState")
                .is_some()
        );
        assert!(
            tool("canvas_view_patch").input_schema["$defs"]
                .get("SharedViewPatch")
                .is_some()
        );
        assert!(
            tool("canvas_view_capture").input_schema["properties"]["view_revision"].is_object()
        );
    }

    fn capture_snapshot() -> control::RuntimeSnapshot {
        use crate::view::{
            SourcePosition, ViewRect, ViewSource, VisualSourceKind, VisualSourceRef,
        };
        use std::collections::BTreeMap;

        let bounds = dock_core::layout::Rect {
            x: 220,
            y: 90,
            width: 960,
            height: 640,
        };
        let view = SharedViewDocument {
            schema_version: crate::SCHEMA_VERSION,
            id: "document-view".into(),
            revision: 7,
            sources: BTreeMap::from([(
                "content".into(),
                ViewSource {
                    source: VisualSourceRef {
                        owner: "dock".into(),
                        id: format!("asset-{}", "ab".repeat(32)),
                        contract: "document.pdf.v1".into(),
                        kind: VisualSourceKind::Artifact,
                    },
                    position: SourcePosition::Revision { revision: 1 },
                },
            )]),
            viewport: ViewRect {
                x: 0.2,
                y: 0.1,
                width: 0.5,
                height: 0.6,
            },
            attention: None,
            navigation: crate::view::ViewNavigation::Page { index: 3 },
            parameters: BTreeMap::new(),
        };
        control::RuntimeSnapshot {
            workspace_id: "workspace".into(),
            project_id: "dock".into(),
            revision: 42,
            frame: dock_core::layout::Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            },
            focused_surface_id: Some("document-view".into()),
            surfaces: Vec::new(),
            surface_bounds: BTreeMap::from([("document-view".into(), bounds)]),
            views: vec![control::MountedViewSnapshot {
                surface_id: "document-view".into(),
                bounds,
                view,
            }],
            capture_chrome: Some(control::CaptureChromeSnapshot {
                danger: [240, 80, 90],
                warning: [240, 190, 70],
                success: [70, 210, 120],
            }),
            active_indicators: Vec::new(),
            mesh: None,
            presence: None,
            presence_preview_active: false,
            screensaver: Default::default(),
            workspaces: Vec::new(),
            workspace_summaries: Vec::new(),
            minimized_surface_ids: Vec::new(),
            maximized_surface_ids: Vec::new(),
            events: Vec::new(),
            display_frames: Vec::new(),
            displays: Vec::new(),
            selected_display_ids: Vec::new(),
            overlays: Default::default(),
            visible: true,
        }
    }

    #[test]
    fn shared_view_capture_resolves_the_exact_visible_surface_and_revision() {
        let snapshot = capture_snapshot();
        let params = ScreenshotParams {
            request_id: "capture-document".into(),
            expected_revision: Some(42),
            surface_id: Some("document-view".into()),
            expected_view_revision: Some(7),
            x: None,
            y: None,
            width: None,
            height: None,
        };

        let capture = resolve_capture(&snapshot, &params, true).unwrap();
        assert_eq!(capture.region, snapshot.surface_bounds["document-view"]);
        let view = capture.view.unwrap();
        assert_eq!(view.revision, 7);
        assert_eq!(
            view.navigation,
            crate::view::ViewNavigation::Page { index: 3 }
        );
        assert!(matches!(
            view.sources["content"].position,
            crate::view::SourcePosition::Revision { revision: 1 }
        ));
    }

    #[test]
    fn shared_view_capture_rejects_stale_hidden_and_unmounted_state() {
        let mut snapshot = capture_snapshot();
        let params = ScreenshotParams {
            request_id: "capture-document".into(),
            expected_revision: Some(42),
            surface_id: Some("document-view".into()),
            expected_view_revision: Some(6),
            x: None,
            y: None,
            width: None,
            height: None,
        };
        assert!(
            resolve_capture(&snapshot, &params, true)
                .unwrap_err()
                .contains("expected 6")
        );

        snapshot.visible = false;
        assert!(
            resolve_capture(&snapshot, &params, true)
                .unwrap_err()
                .contains("hidden")
        );
        snapshot.visible = true;
        snapshot.surface_bounds.clear();
        assert!(
            resolve_capture(&snapshot, &params, true)
                .unwrap_err()
                .contains("not visibly mounted")
        );
    }

    #[test]
    fn surface_screenshot_uses_surface_bounds_instead_of_the_canvas_frame() {
        let snapshot = capture_snapshot();
        let params = ScreenshotParams {
            request_id: "capture-surface".into(),
            expected_revision: None,
            surface_id: Some("document-view".into()),
            expected_view_revision: None,
            x: None,
            y: None,
            width: None,
            height: None,
        };
        let capture = resolve_capture(&snapshot, &params, false).unwrap();
        assert_eq!(capture.region.x, 220);
        assert_eq!(capture.region.y, 90);
        assert_eq!(capture.region.width, 960);
        assert_eq!(capture.region.height, 640);
    }

    #[test]
    fn surface_capture_verifies_existing_chrome_pixels() {
        use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
        use std::io::Cursor;

        let chrome = control::CaptureChromeSnapshot {
            danger: [240, 80, 90],
            warning: [240, 190, 70],
            success: [70, 210, 120],
        };
        let mut image = RgbaImage::from_pixel(200, 100, Rgba([20, 24, 30, 255]));
        for (center, color) in [
            (13, chrome.danger),
            (31, chrome.warning),
            (49, chrome.success),
        ] {
            for y in 10..=18 {
                for x in center - 4..=center + 4 {
                    image.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
                }
            }
        }
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        verify_surface_chrome(
            png.get_ref(),
            dock_core::layout::Rect {
                x: 0,
                y: 0,
                width: 200,
                height: 100,
            },
            chrome,
        )
        .unwrap();

        let wallpaper = RgbaImage::from_pixel(200, 100, Rgba([70, 80, 60, 255]));
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(wallpaper)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();
        assert!(
            verify_surface_chrome(
                png.get_ref(),
                dock_core::layout::Rect {
                    x: 0,
                    y: 0,
                    width: 200,
                    height: 100,
                },
                chrome,
            )
            .unwrap_err()
            .contains("surface chrome")
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
            chrome: crate::SurfaceChrome::default(),
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
                                        surface_bounds: Default::default(),
                                        views: Vec::new(),
                                        capture_chrome: None,
                                        active_indicators: Vec::new(),
                                        mesh: None,
                                        presence: None,
                                        presence_preview_active: false,
                                        screensaver: Default::default(),
                                        workspaces: Vec::new(),
                                        workspace_summaries: Vec::new(),
                                        minimized_surface_ids: Vec::new(),
                                        maximized_surface_ids: Vec::new(),
                                        events: Vec::new(),
                                        display_frames: Vec::new(),
                                        displays: Vec::new(),
                                        selected_display_ids: Vec::new(),
                                        overlays: Default::default(),
                                        visible: true,
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
        assert_eq!(client.list_all_tools().await.unwrap().len(), 15);
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
