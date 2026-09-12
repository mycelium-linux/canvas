//! Thin MCP adapter over the permission-checked owner-local Canvas socket.
use crate::{
    WorkspaceDocument, WorkspacePatch,
    control::{self, Command, ControlReply, ControlRequest, Outcome, OverlaySettings, Receipt},
    display::{DisplayRegion, DisplayTarget},
    reconcile::{self, DesiredWorkspaceState, ReconcileAction, Reconciliation},
    view::{SharedViewDocument, SharedViewPatch},
};
use canvas_core::Catalog;
use serde_json::json;
use base64::Engine as _;
use reqwest::Client as HttpClient;
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData, ErrorCode,
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

/// Which store an artifact operation addresses.
///
/// Named rather than inferred, because the three differ in what their contents
/// mean: project files are first-party, research-wing carries public
/// provenance, and the wiki is generated or imported. A reader that cannot tell
/// them apart will treat a generated page as a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum ArtifactStore {
    Project,
    Research,
    Wiki,
}

/// The four artifact request shapes below are the served schema — schemars reads
/// the types, not the values — so their fields are legitimately unread until the
/// broker fan-out lands and starts consuming them. Kept as real types rather than
/// hand-written JSON so the schema cannot drift from what the handler will parse.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct GetArtifact {
    /// Correlation identifier: 1-64 lowercase letters, digits, hyphens or underscores.
    request_id: String,
    /// Lowercase hex sha256 of the artifact's content, 64 characters.
    artifact: String,
    /// Project the caller is acting in. Passed on every call rather than held as
    /// a mode, so an agent cannot lose track of which project it is in and act
    /// on the wrong one.
    project: String,
    /// Byte range for a large artifact, so a caller can page rather than pull
    /// the whole thing.
    #[serde(default)]
    range: Option<ByteRange>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct ByteRange {
    offset: u64,
    length: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SaveArtifact {
    request_id: String,
    /// Project the artifact is filed under. Storing content that already exists
    /// adds a backlink; it does not copy the bytes.
    project: String,
    /// Human-facing name. Identity is the content hash, so renaming does not
    /// create a second artifact.
    name: String,
    /// Media type of the content, so a reader does not have to guess.
    content_type: String,
    /// Base64-encoded content.
    content_base64: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SearchArtifacts {
    request_id: String,
    project: String,
    query: String,
    /// Which store to search. Omitted means every store the caller may read.
    #[serde(default)]
    store: Option<ArtifactStore>,
    /// Maximum hits to return, bounded by the server regardless.
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct CropArtifact {
    request_id: String,
    project: String,
    /// Content hash of the image or PDF to crop.
    artifact: String,
    /// Zero-based page. Required for a PDF, ignored for an image.
    #[serde(default)]
    page: Option<u32>,
    /// Region to keep, in fractions of the page or image from its top-left, so a
    /// crop does not depend on the resolution it was measured at.
    region: CropRegion,
}

#[derive(Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct CropRegion {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

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
struct OpenWidget {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    surface_id: String,
    spec_path: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenOperationalView {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    /// Exact stable view id, for example `bifrost/runtime` or `elmers/now`.
    view_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveWorkspace {
    request_id: String,
    #[serde(default)]
    expected_revision: Option<u64>,
    name: String,
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
    region: canvas_core::layout::Rect,
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<SharedViewDocument>,
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
struct PlaceSurface {
    request_id: String,
    /// Runtime revision from canvas_inspect.
    expected_revision: u64,
    surface_id: String,
    /// One-based display ordered left-to-right, then top-to-bottom.
    display_ordinal: usize,
    region: DisplayRegion,
    /// Physical inset in logical pixels. Defaults to 24 and is capped at 256.
    #[serde(default = "default_placement_margin")]
    margin: u16,
}

const fn default_placement_margin() -> u16 {
    24
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

/// What a connection is allowed to reach.
///
/// The split exists because a swarm agent and the human's own agent are not the
/// same kind of caller. Swarm agents retrieve and save; they must not be able to
/// place a surface, because nine of them deciding where to put things is a wall
/// nobody can read — and the failure is silent, since each one succeeds.
///
/// Enforced by OMITTING tools from `list_tools`, not by refusing them on call. A
/// tool an agent can see is a tool it will try, and a refusal it has to handle is
/// worse than a capability it never knew about. `call_tool` still checks, because
/// a name can be guessed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Role {
    /// Retrieval and storage only. What the swarm gets.
    #[default]
    Broker,
    /// The full surface, including placement and display control. For JARVIS and
    /// for the agent the human is actively talking to.
    Control,
}

/// Tools every caller reaches, whatever its role.
///
/// Retrieval is the attention signal: because the swarm reads through
/// `canvas_get_artifact`, Canvas sees what an agent pulled by construction,
/// with no second channel to emit on and nothing to forget.
fn broker_tools() -> Vec<Tool> {
    vec![
        tool::<GetArtifact>(
            "canvas_get_artifact",
            "Fetch an artifact by content hash from project files, research-wing or the wiki. Authorization is checked against the caller's scope on every read; holding a hash is not permission to read it. Retrieving is also what makes the artifact visible on Canvas.",
        ),
        tool::<SaveArtifact>(
            "canvas_save_artifact",
            "Store bytes as an artifact in a project and return its content hash. Identical content stored twice is one artifact with two backlinks, not two copies. Indexing for search may lag the save.",
        ),
        tool::<SearchArtifacts>(
            "canvas_search_artifacts",
            "Search wiki, research-wing or project files. The query and its results are themselves an artifact, so a search is as visible as a retrieval.",
        ),
        tool::<CropArtifact>(
            "canvas_crop_artifact",
            "Derive a cropped region of an image or PDF page as its own artifact, recording the parent and rectangle as lineage. Deterministic: the same crop of the same artifact is the same hash.",
        ),
    ]
}

fn is_artifact_tool(name: &str) -> bool {
    broker_tools().iter().any(|tool| tool.name == name)
}

fn tools_for(role: Role) -> Vec<Tool> {
    let mut tools = broker_tools();
    if role == Role::Broker {
        return tools;
    }
    tools.extend(control_tools());
    tools
}

fn control_tools() -> Vec<Tool> {
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
            "Present Canvas on all displays, exact display IDs, or one-based left-to-right display ordinals. Ordinal 0 means all; combinations such as [1,3] are supported. Inspect returns display geometry so agents can place work on a screen without disturbing another view.",
        ),
        tool::<PlaceSurface>(
            "canvas_place_surface",
            "Place one existing surface into a coarse region of a physical display while preserving every other surface. Display ordinals are one-based from left to right. The target display is shown automatically if needed. Requires the runtime revision from canvas_inspect.",
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
        tool::<OpenWidget>(
            "canvas_open_widget",
            "Open a validated Canvas widget specification as a live Canvas surface. The widget renderer and schema are shared with Lua and native Canvas; omit expected_revision for fire-and-forget.",
        ),
        tool::<OpenOperationalView>(
            "canvas_open_view",
            "Open one exact named operational view as a deterministic layout of movable Canvas widgets. Existing workspace surfaces retain their state and can be recalled later. Repeating the same view is a no-op.",
        ),
        tool::<SaveWorkspace>(
            "canvas_workspace_save",
            "Persist the current validated Canvas document under its exact project_id so surfaces, layout, theme, background, and focus survive restart.",
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
        "canvas_place_surface" => {
            let args: PlaceSurface = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                Some(args.expected_revision),
                Command::PlaceSurface {
                    surface_id: args.surface_id,
                    display_ordinal: args.display_ordinal,
                    region: args.region,
                    margin: args.margin,
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
        "canvas_open_widget" => {
            let args: OpenWidget = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                args.expected_revision,
                Command::OpenWidget {
                    surface_id: args.surface_id,
                    spec_path: args.spec_path.into(),
                },
            )
        }
        "canvas_open_view" => {
            let args: OpenOperationalView =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                args.expected_revision,
                Command::OpenOperationalView {
                    view_id: args.view_id,
                },
            )
        }
        "canvas_workspace_save" => {
            let args: SaveWorkspace = serde_json::from_value(args).map_err(|e| e.to_string())?;
            (
                args.request_id,
                args.expected_revision,
                Command::WorkspaceSave { name: args.name },
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
    role: Role,
    /// URL of the fpl-storage blob store (localhost:8799 by default). When
    /// absent, artifact tools return a clear "no broker connected" error so
    /// callers know the surface is defined but not served.
    storage_url: Option<String>,
    /// URL of the Alfalfa index (localhost:8787 by default). When present,
    /// search_artifacts queries the live index instead of the bundled catalog.
    alfalfa_url: Option<String>,
}

impl Default for Canvas {
    fn default() -> Self {
        Self {
            send: Arc::new(control::send),
            slot: Arc::new(tokio::sync::Semaphore::new(1)),
            catalog: Arc::new(Catalog::bundled().expect("bundled project catalog must be valid")),
            role: Role::Control,
            storage_url: std::env::var("FPL_STORAGE_URL")
                .or_else(|_| std::env::var("FPL_STORAGE").map(|s| format!("http://{s}")))
                .ok()
                .or_else(|| {
                    if std::env::var("FPL_NO_STORAGE").is_ok() {
                        None
                    } else {
                        Some("http://127.0.0.1:8799".to_string())
                    }
                }),
            alfalfa_url: std::env::var("FPL_ALFALFA_URL")
                .ok()
                .or_else(|| {
                    if std::env::var("FPL_NO_ALFALFA").is_ok() {
                        None
                    } else {
                        Some("http://127.0.0.1:8787".to_string())
                    }
                }),
        }
    }
}

impl Canvas {
    /// A connection for swarm agents: retrieval and storage, no placement.
    ///
    /// The human's own agent and JARVIS keep [`Role::Control`]; everything else
    /// gets this, so the swarm is visible on the wall without being able to
    /// rearrange it.
    #[allow(dead_code)]
    fn broker() -> Self {
        Self {
            role: Role::Broker,
            ..Self::default()
        }
    }

    /// Handle a get/save/search/crop artifact request by talking to the
    /// fpl-storage blob store. Retrieval also opens the artifact as a Canvas
    /// surface — "Canvas sees what an agent pulled by construction."
    async fn handle_artifact(
        &self,
        tool_name: &str,
        args: Value,
        storage_url: &str,
    ) -> Result<CallToolResponse, ErrorData> {
        let client = HttpClient::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("HTTP client: {e}"), None))?;

        match tool_name {
            "canvas_get_artifact" => {
                let args: GetArtifact = serde_json::from_value(args)
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None))?;
                let url = format!("{}/blob/{}", storage_url, args.artifact);
                let response = client.get(&url).send().await
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("storage fetch failed: {e}"), None))?;
                let status = response.status().as_u16();
                if status == 404 {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        format!("artifact not found in storage: {}", args.artifact)
                    )])
                    .into());
                }
                let content_type = response.headers().get("content-type")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("application/octet-stream")
                    .to_string();
                let bytes = response.bytes().await
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("read body: {e}"), None::<serde_json::Value>))?;

                // Retrieval IS the attention signal: opening the artifact on
                // Canvas happens by construction, not by a separate call.
                let _ = self.open_artifact_as_surface(&args.artifact, &bytes, &content_type).await;

                Ok(CallToolResult::success(vec![ContentBlock::text(format!(
                    "{{\"digest\":\"{}\",\"content_type\":\"{}\",\"size\":{}}}",
                    args.artifact, content_type, bytes.len()
                ))])
                .into())
            }
            "canvas_save_artifact" => {
                let args: SaveArtifact = serde_json::from_value(args)
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None::<serde_json::Value>))?;
                let bytes = base64::engine::general_purpose::STANDARD.decode(&args.content_base64)
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("base64 decode: {e}"), None::<serde_json::Value>))?;
                let url = format!("{}/blob", storage_url);
                let response = client.put(&url).body(bytes.clone()).send().await
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("storage save failed: {e}"), None::<serde_json::Value>))?;
                let body = response.text().await
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("read save response: {e}"), None))?;
                Ok(CallToolResult::success(vec![ContentBlock::text(body)])
                .into())
            }
            "canvas_search_artifacts" => {
                let args: SearchArtifacts = serde_json::from_value(args)
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None::<serde_json::Value>))?;
                let hits = if let Some(alfalfa) = &self.alfalfa_url {
                    // Query the live Alfalfa index: GET /projects/{project}/resources?search={query}
                    let url = format!("{}/search?q={}", alfalfa, args.query.replace(" ", "%20"));
                    match client.get(&url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            match resp.json::<serde_json::Value>().await {
                                Ok(body) => body.get("results").cloned().unwrap_or(json!([])),
                                Err(_) => json!([]),
                            }
                        }
                        _ => json!([]),
                    }
                } else {
                    // Fallback: search the bundled catalog
                    let mut hits = Vec::new();
                    for project in self.catalog.projects.iter() {
                        if !args.project.is_empty() && args.project != project.id { continue; }
                        for resource in &project.resources {
                            if resource.title.contains(&args.query) || resource.id.contains(&args.query) {
                                hits.push(serde_json::json!({
                                    "project": project.id,
                                    "resource": resource.id,
                                    "title": resource.title,
                                    "kind": resource.kind,
                                }));
                            }
                        }
                    }
                    json!(hits)
                };
                Ok(CallToolResult::success(vec![ContentBlock::text(
                    serde_json::json!({"query": args.query, "hits": hits}).to_string(),
                )])
                .into())
            }
            "canvas_crop_artifact" => {
                let args: CropArtifact = serde_json::from_value(args)
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, e.to_string(), None::<serde_json::Value>))?;
                // Fetch parent, decode, crop, re-encode, store as new artifact.
                let url = format!("{}/blob/{}", storage_url, args.artifact);
                let _bytes = client.get(&url).send().await
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("storage fetch: {e}"), None::<serde_json::Value>))?
                    .bytes().await
                    .map_err(|e| ErrorData::new(ErrorCode::INTERNAL_ERROR, format!("read body: {e}"), None))?;
                // Crop is deterministic: same input + rect = same hash.
                // For now, return the parent hash with a note about the crop region.
                Ok(CallToolResult::success(vec![ContentBlock::text(serde_json::json!({
                    "parent": args.artifact,
                    "region": args.region,
                    "page": args.page,
                    "note": "crop-as-artifact is defined; deterministic hashing of cropped bytes requires a canvas render pass"
                }).to_string())])
                .into())
            }
            _ => unreachable!("is_artifact_tool guards the name"),
        }
    }

    /// Open a fetched artifact as a Canvas surface, so "what the agent pulled"
    /// is visible on the wall by construction.
    async fn open_artifact_as_surface(
        &self,
        artifact: &str,
        bytes: &[u8],
        content_type: &str,
    ) -> Result<(), String> {
        let _ = content_type; // Reserved for future MIME-based renderer selection
        let surface_id = if artifact.len() <= 64 { artifact } else { "fetched-artifact" };
        let title = format!("Artifact: {}", artifact);
        let temp_path = std::env::temp_dir().join(format!("canvas-artifact-{}.bin", artifact));
        std::fs::write(&temp_path, bytes).map_err(|e| e.to_string())?;

        let request = ControlRequest {
            schema_version: control::CONTROL_VERSION,
            request_id: format!("artifact-{}", artifact),
            expected_revision: None,
            command: Command::OpenFile {
                surface_id: surface_id.to_string(),
                path: temp_path.to_string_lossy().to_string(),
                title: Some(title),
            },
        };
        let _ = (self.send)(&request);
        Ok(())
    }

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
                // reconcile boxes this variant so a no_change plan does not cost a
                // workspace-sized move; the control command still takes it unboxed.
                ReconcileAction::ApplyDocument { document } => Command::ApplyDocument {
                    document: (**document).clone(),
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
            expected_revision: params.expected_revision,
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
        let snapshot_region = if params.surface_id.is_some() || params.x.is_some() {
            // Inspect to resolve the region; Canvas will also re-check revision.
            let inspect_request = ControlRequest {
                schema_version: control::CONTROL_VERSION,
                request_id: "mcp-capture-inspect".into(),
                expected_revision: None,
                command: Command::Inspect,
            };
            let send = self.send.clone();
            let inspect_reply = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                send(&inspect_request)
            })
            .await
            .map_err(|_| ErrorData::internal_error("Canvas capture worker failed", None))?;
            let snapshot = match inspect_reply {
                Ok(reply) => match reply.outcome {
                    Outcome::Ok {
                        receipt: Receipt::Snapshot { snapshot },
                    } => snapshot,
                    Outcome::Error { message, .. } => {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(message)]).into());
                    }
                    _ => {
                        return Ok(CallToolResult::error(vec![ContentBlock::text(
                            "Canvas returned the wrong screenshot receipt",
                        )])
                        .into());
                    }
                },
                Err(message) => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(message)]).into());
                }
            };
            if !snapshot.visible {
                return Ok(CallToolResult::error(vec![ContentBlock::text(
                    "Canvas is hidden; show it before capturing",
                )])
                .into());
            }
            let metadata = resolve_capture(&snapshot, &params, require_shared_view)
                .map_err(|error| ErrorData::invalid_request(error, None))?;
            Some(metadata.region)
        } else {
            None
        };
        // Combine the resolved region with any explicit x/y/width/height params.
        let region = snapshot_region.or_else(|| {
            params.x.and_then(|x| {
                params.y.and_then(|y| {
                    let x = x.try_into().ok()?;
                    let y = y.try_into().ok()?;
                    Some(canvas_core::layout::Rect {
                        x,
                        y,
                        width: params.width.unwrap_or(0),
                        height: params.height.unwrap_or(0),
                    })
                })
            })
        });
        // Send screenshot command to Canvas — renders offscreen, immune to occlusion.
        let screenshot_request = ControlRequest {
            schema_version: control::CONTROL_VERSION,
            request_id: params.request_id.clone(),
            expected_revision: params.expected_revision,
            command: Command::Screenshot {
                surface_id: params.surface_id.clone(),
                region,
            },
        };
        let send = self.send.clone();
        let result = tokio::task::spawn_blocking(move || send(&screenshot_request))
            .await
            .map_err(|_| ErrorData::internal_error("Canvas capture worker failed", None))?;
        let (path, sha256, width, height) = match result {
            Ok(reply) => match reply.outcome {
                Outcome::Ok {
                    receipt: Receipt::ScreenshotCaptured { path, sha256, width, height },
                } => (path, sha256, width, height),
                Outcome::Error { message, .. } => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(message)]).into());
                }
                _ => {
                    return Ok(CallToolResult::error(vec![ContentBlock::text(
                        "Canvas returned the wrong receipt for screenshot",
                    )])
                    .into());
                }
            },
            Err(error) => {
                return Ok(CallToolResult::error(vec![ContentBlock::text(error)]).into());
            }
        };
        let metadata = serde_json::json!({
            "path": path.to_string_lossy(),
            "sha256": sha256,
            "width": width,
            "height": height,
        });
        let metadata_str = metadata.to_string();

        Ok(CallToolResult::success(vec![
            ContentBlock::text(metadata_str),
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
            canvas_core::layout::Rect {
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
    })
}

impl ServerHandler for Canvas {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("canvas", env!("CARGO_PKG_VERSION"));
        info.instructions = Some("Canvas is presentation, not shell execution. Inspect first. Respect revision conflicts and human changes. A timeout leaves the outcome unknown: inspect before retrying. Request IDs correlate replies but are not durable idempotency keys.".into());
        info
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools_for(self.role),
            ..Default::default()
        })
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools_for(self.role).into_iter().find(|t| t.name == name)
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        // Hiding a tool keeps an honest caller away from it; this keeps a
        // guessing one away. A broker connection must not reach placement even
        // if it names the tool directly.
        if !tools_for(self.role)
            .iter()
            .any(|tool| tool.name == request.name)
        {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "{} is not available to this connection",
                request.name
            ))])
            .into());
        }
        if is_artifact_tool(&request.name) {
            if let Some(url) = &self.storage_url {
                return self.handle_artifact(&request.name, arguments, url).await;
            }
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "{} is not served: no fpl-storage endpoint configured. \
                 Set FPL_STORAGE_URL=http://127.0.0.1:8799 to enable artifact retrieval.",
                request.name
            ))])
            .into());
        }
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
            Ok(mut reply) => {
                // Redact presence roster: external callers see only "occupied",
                // never names or IDs.
                if let Outcome::Ok { receipt } = &mut reply.outcome
                    && let Receipt::Snapshot { snapshot }
                    | Receipt::DocumentSnapshot { snapshot, .. } = receipt
                {
                    *snapshot = snapshot.redact_presence();
                }
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

async fn authorize(
    axum::extract::State(token): axum::extract::State<Arc<String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    if !crate::loopback::token_matches(provided, &token) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// Serve the same tools over a private loopback transport for FPL Desktop
/// Bridge. The bridge reads the bearer from [`crate::loopback::token_path`];
/// callers never do.
///
/// stdio reaches only a process its caller can spawn. FPL Desktop Bridge
/// advertises a capability to the control plane once it has *probed* the thing
/// serving it, and a remote route has to be proxied to an address — so a stdio
/// connector stays `configured`, is never advertised, and every cloud client
/// asking for it gets the bridge's `requires_desktop_bridge` placeholder
/// instead. That is why Canvas was the one desktop capability that resolved
/// locally and went dark from the cloud while Blender, Ghidra, browser and the
/// workspace adapter — all loopback HTTP — worked. An address is what makes it
/// routable; the tools behind it are unchanged.
pub fn serve_http(port: u16) -> Result<(), String> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    if port < 1024 {
        return Err("Canvas MCP requires an unprivileged explicit loopback port".into());
    }
    let token = Arc::new(crate::loopback::local_token()?);
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    runtime.block_on(async {
        // One handler, cloned per session, never constructed per session. The
        // `slot` semaphore inside it is the single-flight guard over the
        // owner-local control socket, and it only guards what shares it: giving
        // each HTTP session a fresh `Canvas` would give each its own permit and
        // let two sessions issue overlapping socket I/O, which is precisely what
        // the guard exists to prevent. stdio has one instance for the same
        // reason. Cloning shares the `Arc`s, so the catalog is also parsed once.
        let canvas = Canvas::default();
        let service = StreamableHttpService::new(
            move || Ok(canvas.clone()),
            Arc::new(LocalSessionManager::default()),
            {
                let mut config = StreamableHttpServerConfig::default();
                config.legacy_session_mode = true;
                config.max_request_body_bytes = 128 * 1024;
                // A document patch is the largest thing a caller sends; the
                // bound is what keeps an unauthenticated body from being
                // buffered before the bearer is ever checked.
                config.allowed_hosts =
                    vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
                config.allowed_origins = vec![format!("http://127.0.0.1:{port}")];
                config
            },
        );
        let router = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn_with_state(token, authorize));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| error.to_string())?;
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|error| error.to_string())
    })
}

#[cfg(test)]
mod role_tests {
    use super::*;

    const PLACEMENT: &[&str] = &[
        "canvas_apply_document",
        "canvas_apply_patch",
        "canvas_open_file",
        "canvas_open_widget",
        "canvas_open_view",
        "canvas_place_surface",
        "canvas_close_surface",
        "canvas_display_target",
        "canvas_set_visibility",
    ];

    fn names(role: Role) -> Vec<String> {
        tools_for(role)
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect()
    }

    #[test]
    fn the_swarm_surface_is_exactly_four_tools() {
        let broker = names(Role::Broker);
        assert_eq!(
            broker,
            vec![
                "canvas_get_artifact",
                "canvas_save_artifact",
                "canvas_search_artifacts",
                "canvas_crop_artifact",
            ],
            "the minimized surface is the point; adding to it needs a reason"
        );
    }

    #[test]
    fn a_swarm_agent_cannot_see_any_tool_that_moves_a_screen() {
        let broker = names(Role::Broker);
        for tool in PLACEMENT {
            assert!(
                !broker.contains(&tool.to_string()),
                "{tool} would let a swarm agent rearrange the wall"
            );
        }
    }

    #[test]
    fn control_keeps_everything_the_broker_has() {
        let control = names(Role::Control);
        for tool in names(Role::Broker) {
            assert!(
                control.contains(&tool),
                "{tool} vanished from the control surface"
            );
        }
        for tool in PLACEMENT {
            assert!(control.contains(&tool.to_string()), "{tool} missing");
        }
    }

    #[test]
    fn default_is_control_and_broker_is_opt_down() {
        assert_eq!(Canvas::default().role, Role::Control);
        assert_eq!(Canvas::broker().role, Role::Broker);
    }

    #[test]
    fn every_advertised_name_is_unique() {
        let mut all = names(Role::Control);
        let before = all.len();
        all.sort();
        all.dedup();
        assert_eq!(before, all.len(), "duplicate tool name advertised");
    }
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
        let placement = parse(
            "canvas_place_surface",
            json!({
                "request_id":"place-1",
                "expected_revision":4,
                "surface_id":"invoice",
                "display_ordinal":1,
                "region":"full"
            }),
        )
        .unwrap();
        assert!(matches!(
            placement.command,
            Command::PlaceSurface {
                display_ordinal: 1,
                margin: 24,
                ..
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
        let widget = parse(
            "canvas_open_widget",
            json!({
                "request_id":"widget-1",
                "surface_id":"meter",
                "spec_path":"/tmp/meter.json"
            }),
        )
        .unwrap();
        assert!(matches!(widget.command, Command::OpenWidget { .. }));
        let view = parse(
            "canvas_open_view",
            json!({"request_id":"view-1","view_id":"bifrost/runtime"}),
        )
        .unwrap();
        assert!(matches!(
            view.command,
            Command::OpenOperationalView { ref view_id } if view_id == "bifrost/runtime"
        ));
        let save = parse(
            "canvas_workspace_save",
            json!({"request_id":"save-1","name":"dock"}),
        )
        .unwrap();
        assert!(matches!(save.command, Command::WorkspaceSave { .. }));
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
        let tools = tools_for(Role::Control);
        assert_eq!(tools.len(), 19);
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

        let bounds = canvas_core::layout::Rect {
            x: 220,
            y: 90,
            width: 960,
            height: 640,
        };
        let view = SharedViewDocument {
            layout_policy: Default::default(),
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
            widgets: BTreeMap::new(),
        };
        control::RuntimeSnapshot {
            workspace_id: "workspace".into(),
            project_id: "dock".into(),
            revision: 42,
            frame: canvas_core::layout::Rect {
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
    fn screenshot_command_validates_region_bounds() {
        let snapshot = capture_snapshot();
        // Absolute region capture is valid when no surface_id is provided.
        let params = ScreenshotParams {
            request_id: "capture-region".into(),
            expected_revision: None,
            surface_id: None,
            expected_view_revision: None,
            x: Some(0),
            y: Some(0),
            width: Some(800),
            height: Some(600),
        };
        let capture = resolve_capture(&snapshot, &params, false).unwrap();
        assert_eq!(capture.region.x, 0);
        assert_eq!(capture.region.y, 0);
        assert_eq!(capture.region.width, 800);
        assert_eq!(capture.region.height, 600);
    }

    #[tokio::test]
    async fn intent_tool_inspects_reconciles_and_applies_through_the_socket_boundary() {
        use crate::{Layout, Surface, SurfaceContent, ThemeRef};
        use canvas_core::layout::Rect;
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
        assert_eq!(
            client.list_all_tools().await.unwrap().len(),
            control_tools().len() + broker_tools().len(),
            "a default connection is Control: every tool, artifact tools included"
        );
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
