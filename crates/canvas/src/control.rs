//! Bounded control protocol for a long-lived Canvas runtime.

use crate::{
    Background, BackgroundShader, NormalizedRect, ThemeRef, WorkspaceDocument, WorkspacePatch,
    display::{DisplayInfo, DisplayRegion, DisplayTarget},
    view::{SharedViewDocument, SharedViewPatch},
};
use canvas_core::{
    ThemeSelection,
    desktop::{Wallpaper, WallpaperReceipt, WallpaperRequest},
    layout::Rect,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

pub const CONTROL_VERSION: u32 = 1;
pub const MAX_CONTROL_MESSAGE: u64 = 256 * 1024;
pub const MAX_ACTIVE_INDICATORS: usize = 16;
pub const MAX_MESH_NODES: usize = 8;
pub const MAX_PRESENCE_OCCUPANTS: usize = 8;
pub const MAX_PRESENCE_FACES: usize = 8;
pub const MAX_PROJECT_WORKSPACES: usize = 512;
pub const DEFAULT_SCREENSAVER_FPS: u16 = 30;
pub const MAX_SCREENSAVER_FPS: u16 = 60;
/// Ceiling on a document background shader's animation speed, read off the
/// picker's own ladder so the protocol cannot express a speed no human could
/// choose at the keyboard.
pub const MAX_BACKGROUND_SHADER_SPEED: f32 =
    crate::background_picker::SPEEDS[crate::background_picker::SPEEDS.len() - 1];
/// Speed a background shader runs at when the caller does not name one: its own
/// tempo, unscaled. Zero is a legal speed but freezes the shader, which reads as
/// a broken command rather than a still background when nobody asked for it.
pub const DEFAULT_BACKGROUND_SHADER_SPEED: f32 = 1.0;
pub const MAX_SURFACE_PLACEMENT_MARGIN: u16 = 256;
const MAX_INDICATOR_TITLE_BYTES: usize = 128;
const MAX_INDICATOR_CAPTION_BYTES: usize = 512;
const MAX_INDICATOR_DETAIL_BYTES: usize = 512;
const MAX_INDICATOR_DIAGNOSTIC_BYTES: usize = 512;
const MAX_MESH_NODE_LABEL_BYTES: usize = 64;
const MAX_MESH_NODE_DETAIL_BYTES: usize = 128;
const MAX_PRESENCE_TEXT_BYTES: usize = 64;
const MAX_PRESENCE_PREVIEW_BASE64_BYTES: usize = 192 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlRequest {
    pub schema_version: u32,
    pub request_id: String,
    #[serde(default)]
    pub expected_revision: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<ActorContext>,
    pub command: Command,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActorContext {
    /// Stable agent identifier, e.g. "opencode:canvas" or "alfalfa:sync".
    pub id: String,
    /// Milliseconds since the UNIX epoch at signing time (replay window).
    pub timestamp_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    Inspect,
    ReadDocument,
    ApplyDocument {
        document: WorkspaceDocument,
    },
    ApplyPatch {
        patch: WorkspacePatch,
    },
    PatchView {
        surface_id: String,
        patch: SharedViewPatch,
    },
    OpenFile {
        surface_id: String,
        path: String,
        #[serde(default)]
        title: Option<String>,
    },
    CloseSurface {
        surface_id: String,
    },
    SetFrame {
        frame: Rect,
    },
    SetDisplayTarget {
        target: DisplayTarget,
    },
    PlaceSurface {
        surface_id: String,
        display_ordinal: usize,
        region: DisplayRegion,
        #[serde(default = "default_surface_margin")]
        margin: u16,
    },
    SetOverlays {
        overlays: OverlaySettings,
    },
    SetVisibility {
        visible: bool,
    },
    Focus,
    Navigate {
        surface_id: String,
        target: Navigation,
    },
    SetTheme {
        project_id: String,
    },
    SetWallpaper {
        request: WallpaperRequest,
    },
    UpsertIndicator {
        indicator: StatusIndicator,
    },
    DismissIndicator {
        indicator_id: String,
    },
    SetMesh {
        mesh: MeshSnapshot,
    },
    SetPresence {
        presence: PresenceSnapshot,
    },
    ShowPresencePreview {
        preview: PresencePreview,
    },
    DismissPresencePreview,
    SetScreensaver {
        settings: ScreensaverSettings,
    },
    Shutdown,
    SetBackground {
        path: PathBuf,
    },
    ClearBackground,
    OpenStream {
        surface_id: String,
        url: String,
    },
    OpenWidget {
        surface_id: String,
        spec_path: PathBuf,
    },
    OpenOperationalView {
        view_id: String,
    },
    WorkspaceSave {
        name: String,
    },
    WorkspaceSwitch {
        name: String,
    },
    WorkspaceSync {
        projects: Vec<ProjectWorkspaceSpec>,
    },
    ScreensaverShader {
        index: usize,
    },
    /// Set the *document* background shader of a named project's workspace.
    ///
    /// [`Command::ScreensaverShader`] names a neighbouring but different
    /// surface: the privacy screensaver, which is only drawn while the
    /// screensaver is enabled. What a human sees on an ordinary desk is
    /// `Background::Shader` on the presented document, and until this command
    /// existed the F2 picker was the only thing in Canvas that could set it —
    /// so an agent asking for a shader got three `ok` replies and an unchanged
    /// desk. The project is named rather than implied so that landing the
    /// change on a workspace nobody is looking at is reported, not silent.
    SetBackgroundShader {
        project_id: String,
        shader_id: BackgroundShader,
        #[serde(default = "default_background_shader_speed")]
        speed: f32,
    },
    OpenDataViz {
        surface_id: String,
        file: PathBuf,
        chart_type: String,
    },
    Screenshot {
        surface_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Rect>,
    },
}

/// Public project metadata resolved by an authenticated catalog owner.
///
/// Canvas persists this bounded projection beside workspace documents. It does
/// not contact Alfalfa or copy private project context.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectWorkspaceSpec {
    pub project_id: String,
    pub name: String,
    pub status: String,
    pub theme: ThemeSelection,
}

impl ProjectWorkspaceSpec {
    pub fn validate(&self) -> Result<(), String> {
        if !workspace_name(&self.project_id) {
            return Err("invalid project workspace id".into());
        }
        if !bounded_printable(&self.name, 256) {
            return Err("project workspace name must contain 1-256 printable bytes".into());
        }
        if !bounded_printable(&self.status, 64) {
            return Err("project workspace status must contain 1-64 printable bytes".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StatusIndicator {
    pub id: String,
    pub title: String,
    pub state: IndicatorState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnostic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndicatorState {
    Ready,
    Transcribing,
    Listening,
    Thinking,
    Speaking,
}

/// Complete, declarative state for Canvas' animated privacy background.
///
/// Animation is disabled by default so a runtime never starts expensive frame
/// work until orchestration explicitly asks for it. The frame limit is part of
/// intent, which makes performance behavior inspectable and reproducible.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreensaverSettings {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub show_time: bool,
    #[serde(default)]
    pub show_fps: bool,
    #[serde(default = "default_screensaver_fps")]
    pub max_fps: u16,
}

impl Default for ScreensaverSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            show_time: false,
            show_fps: false,
            max_fps: DEFAULT_SCREENSAVER_FPS,
        }
    }
}

impl ScreensaverSettings {
    pub fn validate(self) -> Result<(), String> {
        if !(1..=MAX_SCREENSAVER_FPS).contains(&self.max_fps) {
            return Err(format!(
                "screensaver max_fps must be between 1 and {MAX_SCREENSAVER_FPS}"
            ));
        }
        Ok(())
    }
}

const fn default_screensaver_fps() -> u16 {
    DEFAULT_SCREENSAVER_FPS
}

const fn default_background_shader_speed() -> f32 {
    DEFAULT_BACKGROUND_SHADER_SPEED
}

const fn default_surface_margin() -> u16 {
    24
}

impl StatusIndicator {
    pub fn validate(&self) -> Result<(), String> {
        if !identifier(&self.id) {
            return Err("invalid status indicator id".into());
        }
        if !bounded_printable(&self.title, MAX_INDICATOR_TITLE_BYTES) {
            return Err("status indicator title must contain 1-128 printable bytes".into());
        }
        if self
            .caption
            .as_ref()
            .is_some_and(|caption| !bounded_printable(caption, MAX_INDICATOR_CAPTION_BYTES))
        {
            return Err("status indicator caption must contain 1-512 printable bytes".into());
        }
        if self
            .detail
            .as_ref()
            .is_some_and(|detail| !bounded_printable(detail, MAX_INDICATOR_DETAIL_BYTES))
        {
            return Err("status indicator detail must contain 1-512 printable bytes".into());
        }
        if self.diagnostic.as_ref().is_some_and(|diagnostic| {
            !bounded_printable(diagnostic, MAX_INDICATOR_DIAGNOSTIC_BYTES)
        }) {
            return Err("status indicator diagnostic must contain 1-512 printable bytes".into());
        }
        Ok(())
    }
}

/// State of one computing-mesh node as reported by the orchestrator.
///
/// The states track the pause/resume semantics: a hidden UI, a paused agent,
/// a suspended (frozen, resumable) compute node, and a stopped process are
/// distinct conditions and must render distinctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeshNodeState {
    /// Node is up and doing work.
    Active,
    /// Node is up but its work is paused (agent idle, human gate, etc).
    Paused,
    /// Compute is suspended (frozen but resumable).
    Suspended,
    /// The node's process has stopped.
    Stopped,
    /// Node is up but its UI is hidden (locked, screensaver, headless).
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshNode {
    pub id: String,
    pub label: String,
    pub state: MeshNodeState,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MeshSnapshot {
    pub nodes: Vec<MeshNode>,
}

impl MeshNode {
    pub fn validate(&self) -> Result<(), String> {
        if !identifier(&self.id) {
            return Err("invalid mesh node id".into());
        }
        if !bounded_printable(&self.label, MAX_MESH_NODE_LABEL_BYTES) {
            return Err("mesh node label must contain 1-64 printable bytes".into());
        }
        if self
            .detail
            .as_ref()
            .is_some_and(|detail| !bounded_printable(detail, MAX_MESH_NODE_DETAIL_BYTES))
        {
            return Err("mesh node detail must contain 1-128 printable bytes".into());
        }
        Ok(())
    }
}

impl MeshSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if self.nodes.len() > MAX_MESH_NODES {
            return Err(format!(
                "mesh snapshot may carry at most {MAX_MESH_NODES} nodes"
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for node in &self.nodes {
            node.validate()?;
            if !ids.insert(node.id.as_str()) {
                return Err(format!("duplicate mesh node id {}", node.id));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PresenceSignalState {
    Unavailable,
    Unknown,
    Detected,
    Verified,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceSignal {
    pub state: PresenceSignalState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    pub age_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceOccupant {
    pub id: String,
    pub display_name: String,
    pub presence: PresenceSignal,
    pub face: PresenceSignal,
    pub voice: PresenceSignal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresenceSnapshot {
    pub zone: String,
    pub observed_at_unix_ms: u64,
    pub expires_after_ms: u64,
    pub occupied: bool,
    pub occupants: Vec<PresenceOccupant>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresencePreviewFace {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub occupant_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub confidence: f32,
    pub bounds: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresencePreview {
    pub captured_at_unix_ms: u64,
    pub width: u32,
    pub height: u32,
    pub linger_ms: u64,
    pub jpeg_base64: String,
    pub faces: Vec<PresencePreviewFace>,
}

impl PresenceSignal {
    fn validate(&self) -> Result<(), String> {
        if self
            .confidence
            .is_some_and(|confidence| !confidence.is_finite() || !(0.0..=1.0).contains(&confidence))
        {
            return Err("presence confidence must be finite and between zero and one".into());
        }
        if self.age_ms > 24 * 60 * 60 * 1000 {
            return Err("presence signal age exceeds 24 hours".into());
        }
        Ok(())
    }
}

impl PresenceSnapshot {
    pub fn validate(&self) -> Result<(), String> {
        if !bounded_printable(&self.zone, MAX_PRESENCE_TEXT_BYTES) {
            return Err("presence zone must contain 1-64 printable bytes".into());
        }
        if !(250..=300_000).contains(&self.expires_after_ms) {
            return Err("presence expiry must be between 250ms and five minutes".into());
        }
        if self.occupants.len() > MAX_PRESENCE_OCCUPANTS {
            return Err(format!(
                "presence snapshot may carry at most {MAX_PRESENCE_OCCUPANTS} occupants"
            ));
        }
        let mut ids = std::collections::BTreeSet::new();
        for occupant in &self.occupants {
            if !presence_identifier(&occupant.id)
                || !bounded_printable(&occupant.display_name, MAX_PRESENCE_TEXT_BYTES)
                || !ids.insert(occupant.id.as_str())
            {
                return Err("presence occupant IDs and names must be valid and unique".into());
            }
            occupant.presence.validate()?;
            occupant.face.validate()?;
            occupant.voice.validate()?;
        }
        if !self.occupied && !self.occupants.is_empty() {
            return Err("an unoccupied presence snapshot cannot contain occupants".into());
        }
        Ok(())
    }

    /// Strip the full occupant roster (names, IDs, face data) so external callers
    /// see only whether the zone is occupied, never who is in it.
    pub fn redact(&self) -> Self {
        PresenceSnapshot {
            zone: self.zone.clone(),
            observed_at_unix_ms: self.observed_at_unix_ms,
            expires_after_ms: self.expires_after_ms,
            occupied: self.occupied,
            occupants: Vec::new(),
        }
    }
}

impl PresencePreview {
    pub fn validate(&self) -> Result<(), String> {
        if self.width == 0
            || self.height == 0
            || self.width > 1280
            || self.height > 1280
            || !(250..=30_000).contains(&self.linger_ms)
            || self.jpeg_base64.is_empty()
            || self.jpeg_base64.len() > MAX_PRESENCE_PREVIEW_BASE64_BYTES
            || self.faces.is_empty()
            || self.faces.len() > MAX_PRESENCE_FACES
        {
            return Err("invalid bounded presence preview".into());
        }
        for face in &self.faces {
            if face
                .occupant_id
                .as_ref()
                .is_some_and(|id| !presence_identifier(id))
                || face
                    .display_name
                    .as_ref()
                    .is_some_and(|name| !bounded_printable(name, MAX_PRESENCE_TEXT_BYTES))
                || !face.confidence.is_finite()
                || !(0.0..=1.0).contains(&face.confidence)
                || face
                    .bounds
                    .iter()
                    .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
                || face.bounds[0] >= face.bounds[2]
                || face.bounds[1] >= face.bounds[3]
            {
                return Err("invalid presence preview face".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Navigation {
    Next,
    Previous,
    Page { index: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlReply {
    pub schema_version: u32,
    pub request_id: String,
    pub revision: u64,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
#[allow(clippy::large_enum_variant)]
pub enum Outcome {
    Ok { receipt: Receipt },
    Error { code: ErrorCode, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Conflict,
    NotFound,
    Unsupported,
    ResourceLimit,
    Platform,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Receipt {
    DocumentSnapshot {
        document: Box<WorkspaceDocument>,
        snapshot: RuntimeSnapshot,
    },
    Snapshot {
        snapshot: RuntimeSnapshot,
    },
    DocumentApplied {
        workspace_id: String,
    },
    SurfaceOpened {
        surface: SurfaceSnapshot,
    },
    ViewPatched {
        surface_id: String,
        view: Box<SharedViewDocument>,
    },
    OperationalViewOpened {
        view_id: String,
        surface_ids: Vec<String>,
    },
    SurfaceClosed {
        surface_id: String,
    },
    FrameSet {
        frame: Rect,
    },
    DisplayTargetSet {
        display_ids: Vec<String>,
        frame: Rect,
    },
    SurfacePlaced {
        surface_id: String,
        display_id: String,
        bounds: NormalizedRect,
    },
    OverlaysSet {
        overlays: OverlaySettings,
    },
    VisibilitySet {
        visible: bool,
    },
    Focused,
    Navigated {
        surface_id: String,
        page_index: u32,
        page_count: u32,
    },
    ThemeSet {
        project_id: String,
        theme: ThemeRef,
    },
    WallpaperSet {
        receipt: WallpaperReceipt,
    },
    IndicatorUpserted {
        indicator_id: String,
    },
    IndicatorDismissed {
        indicator_id: String,
    },
    MeshSet {
        node_count: usize,
    },
    PresenceSet {
        zone: String,
        occupant_count: usize,
    },
    PresencePreviewShown {
        face_count: usize,
    },
    PresencePreviewDismissed,
    ScreensaverUpdated {
        settings: ScreensaverSettings,
    },
    ShuttingDown,
    WorkspaceSaved {
        name: String,
    },
    WorkspaceSwitched {
        name: String,
    },
    WorkspacesSynced {
        created: usize,
        updated: usize,
        preserved: usize,
        total: usize,
    },
    ScreensaverShaderSet {
        index: usize,
        name: String,
        /// Optional so a reply from an older runtime still parses; every
        /// current runtime fills it in.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        visible: Option<VisibleBackground>,
    },
    BackgroundShaderSet {
        visible: VisibleBackground,
    },
    ScreenshotCaptured {
        path: PathBuf,
        sha256: String,
        width: u32,
        height: u32,
    },
}

/// What a person is actually looking at once a mutation has landed.
///
/// A receipt that echoes the field it wrote cannot tell "done" from "done
/// somewhere nobody is looking". `OK rev=2189 shader=7 (Flame)` came back three
/// times for a screensaver index while the desk went on drawing another
/// project's document background, and every reply was truthful about the write
/// and useless about the result. Naming the project the change landed on, the
/// project on screen, and the shader being drawn makes that gap visible in the
/// first reply instead of the third.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisibleBackground {
    /// The project whose workspace the mutation was written to.
    pub project_id: String,
    /// The project Canvas is presenting. Equal to `project_id` when the change
    /// is on screen, and different when it is not — which is the whole point.
    pub active_project_id: String,
    /// The presented project's document background.
    pub background: Background,
    /// Whether the privacy screensaver is covering that background.
    pub screensaver_enabled: bool,
    /// The shader the desk is drawing right now, if any: the screensaver's
    /// while it is enabled, otherwise the presented document's.
    pub drawn_shader: Option<BackgroundShader>,
}

impl VisibleBackground {
    /// Whether the mutation landed on the workspace that is on screen.
    pub fn on_screen(&self) -> bool {
        self.project_id == self.active_project_id
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSnapshot {
    pub workspace_id: String,
    pub project_id: String,
    pub revision: u64,
    pub frame: Rect,
    pub focused_surface_id: Option<String>,
    pub surfaces: Vec<SurfaceSnapshot>,
    #[serde(default)]
    pub surface_bounds: BTreeMap<String, Rect>,
    #[serde(default)]
    pub views: Vec<MountedViewSnapshot>,
    #[serde(default)]
    pub active_indicators: Vec<StatusIndicator>,
    #[serde(default)]
    pub mesh: Option<MeshSnapshot>,
    #[serde(default)]
    pub presence: Option<PresenceSnapshot>,
    #[serde(default)]
    pub presence_preview_active: bool,
    #[serde(default)]
    pub screensaver: ScreensaverSnapshot,
    #[serde(default)]
    pub workspaces: Vec<String>,
    #[serde(default)]
    pub workspace_summaries: Vec<ProjectWorkspaceSnapshot>,
    #[serde(default)]
    pub minimized_surface_ids: Vec<String>,
    #[serde(default)]
    pub maximized_surface_ids: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub display_frames: Vec<Rect>,
    #[serde(default)]
    pub displays: Vec<DisplayInfo>,
    #[serde(default)]
    pub selected_display_ids: Vec<String>,
    #[serde(default)]
    pub overlays: OverlaySettings,
    #[serde(default = "default_true")]
    pub visible: bool,
}

impl RuntimeSnapshot {
    /// Redact the presence roster before sending to external callers. The full
    /// occupant list (names, IDs, face data) is only meaningful on the wall
    /// display itself; every MCP/HTTP client sees "occupied" without identities.
    pub fn redact_presence(&self) -> RuntimeSnapshot {
        let presence = self.presence.as_ref().map(|p| p.redact());
        RuntimeSnapshot {
            presence,
            ..self.clone()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MountedViewSnapshot {
    pub surface_id: String,
    /// Absolute desktop coordinates of the visibly rendered Canvas surface.
    pub bounds: Rect,
    pub view: SharedViewDocument,
}

fn default_true() -> bool {
    true
}

/// Host-local placement policy for runtime overlays. This remains separate
/// from the portable workspace document and can be driven by Lua, MCP, or the
/// raw socket without duplicating renderer placement logic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub struct OverlaySettings {
    pub presence_rail: OverlayTarget,
    pub presence_preview: OverlayTarget,
    pub presence_orb: OverlayTarget,
    pub system_hud: OverlayTarget,
    pub fpl_mark: OverlayTarget,
    pub shortcut_help: OverlayTarget,
}

impl Default for OverlaySettings {
    fn default() -> Self {
        Self {
            presence_rail: OverlayTarget::Leftmost,
            presence_preview: OverlayTarget::Leftmost,
            presence_orb: OverlayTarget::Hidden,
            system_hud: OverlayTarget::Hidden,
            fpl_mark: OverlayTarget::Leftmost,
            shortcut_help: OverlayTarget::Rightmost,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum OverlayTarget {
    Hidden,
    All,
    Leftmost,
    Center,
    Rightmost,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectWorkspaceSnapshot {
    pub project_id: String,
    pub name: String,
    pub status: String,
    pub surface_count: usize,
    pub active: bool,
    pub theme: ThemeSelection,
    pub background: Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreensaverSnapshot {
    #[serde(flatten)]
    pub settings: ScreensaverSettings,
    /// Rolling measured cadence of animation frames requested by Canvas.
    pub measured_fps: f32,
}

impl Default for ScreensaverSnapshot {
    fn default() -> Self {
        Self {
            settings: ScreensaverSettings::default(),
            measured_fps: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceSnapshot {
    pub id: String,
    pub title: String,
    pub content: ContentKind,
    pub asset_sha256: Option<String>,
    pub page_index: Option<u32>,
    pub page_count: Option<u32>,
    pub source_pixels: Option<[u32; 2]>,
    /// Decoded or rasterized texture dimensions uploaded to the renderer.
    pub rendered_pixels: Option<[u32; 2]>,
    /// Logical draw dimensions after applying the display scale factor.
    pub display_logical: Option<[f64; 2]>,
    pub scale_factor: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentKind {
    Empty,
    Image,
    Pdf,
    Workspace,
    Markdown,
    Terminal,
    Scene,
    Stream,
    DataViz,
    Widget,
}

impl ControlRequest {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != CONTROL_VERSION {
            return Err("unsupported Canvas control schema_version".into());
        }
        if !request_id(&self.request_id) {
            return Err("request_id must be 1–64 lowercase identifier characters".into());
        }
        match &self.command {
            Command::OpenFile {
                surface_id,
                path,
                title,
            } => {
                if !identifier(surface_id) {
                    return Err("invalid Canvas surface id".into());
                }
                if path.is_empty() || path.len() > 4096 || path.contains('\0') {
                    return Err("file path must contain 1–4096 non-NUL bytes".into());
                }
                if title.as_ref().is_some_and(|value| !visible(value)) {
                    return Err("file title must be printable and bounded".into());
                }
            }
            Command::CloseSurface { surface_id }
            | Command::Navigate { surface_id, .. }
            | Command::PatchView { surface_id, .. } => {
                if !identifier(surface_id) {
                    return Err("invalid Canvas surface id".into());
                }
            }
            Command::SetFrame { frame } => validate_frame(*frame)?,
            Command::SetDisplayTarget { target } => match target {
                DisplayTarget::All => {}
                DisplayTarget::Exact { display_ids } => {
                    if display_ids.is_empty() || display_ids.len() > crate::display::MAX_DISPLAYS {
                        return Err("display target must contain 1-16 IDs".into());
                    }
                    let mut unique = std::collections::BTreeSet::new();
                    if display_ids.iter().any(|id| {
                        id.is_empty()
                            || id.len() > 128
                            || id.chars().any(char::is_control)
                            || !unique.insert(id.as_str())
                    }) {
                        return Err("display target IDs must be bounded and unique".into());
                    }
                }
                DisplayTarget::Ordinals { ordinals } => {
                    if ordinals.is_empty() || ordinals.len() > crate::display::MAX_DISPLAYS {
                        return Err("display ordinals must contain 1-16 values".into());
                    }
                    let mut unique = std::collections::BTreeSet::new();
                    if ordinals.iter().any(|ordinal| !unique.insert(*ordinal))
                        || (ordinals.contains(&0) && ordinals != &[0])
                    {
                        return Err(
                            "display ordinals must be unique; 0 means all and must be used alone"
                                .into(),
                        );
                    }
                }
            },
            Command::PlaceSurface {
                surface_id,
                display_ordinal,
                margin,
                ..
            } => {
                if !identifier(surface_id) {
                    return Err("invalid Canvas surface id".into());
                }
                if *display_ordinal == 0 || *display_ordinal > crate::display::MAX_DISPLAYS {
                    return Err("display ordinal must be between 1 and 16".into());
                }
                if *margin > MAX_SURFACE_PLACEMENT_MARGIN {
                    return Err(format!(
                        "surface placement margin must not exceed {MAX_SURFACE_PLACEMENT_MARGIN}"
                    ));
                }
            }
            Command::SetOverlays { .. } | Command::SetVisibility { .. } => {}
            Command::SetTheme { project_id } => {
                if !workspace_name(project_id) {
                    return Err("invalid project id".into());
                }
            }
            Command::SetWallpaper { request } => {
                if !identifier(&request.project)
                    || request.display_id.is_empty()
                    || request.display_id.len() > 128
                {
                    return Err("invalid wallpaper project or display id".into());
                }
            }
            Command::UpsertIndicator { indicator } => indicator.validate()?,
            Command::DismissIndicator { indicator_id } => {
                if !identifier(indicator_id) {
                    return Err("invalid status indicator id".into());
                }
            }
            Command::SetMesh { mesh } => mesh.validate()?,
            Command::SetPresence { presence } => presence.validate()?,
            Command::ShowPresencePreview { preview } => preview.validate()?,
            Command::SetScreensaver { settings } => settings.validate()?,
            Command::WorkspaceSync { projects } => {
                if projects.is_empty() || projects.len() > MAX_PROJECT_WORKSPACES {
                    return Err(format!(
                        "workspace sync must contain 1-{MAX_PROJECT_WORKSPACES} projects"
                    ));
                }
                let mut ids = std::collections::BTreeSet::new();
                for project in projects {
                    project.validate()?;
                    if !ids.insert(project.project_id.as_str()) {
                        return Err("workspace sync contains duplicate project ids".into());
                    }
                }
            }
            Command::WorkspaceSave { name } | Command::WorkspaceSwitch { name } => {
                if !workspace_name(name) {
                    return Err("invalid project workspace id".into());
                }
            }
            Command::SetBackgroundShader {
                project_id, speed, ..
            } => {
                if !workspace_name(project_id) {
                    return Err("invalid project id".into());
                }
                if !speed.is_finite() || *speed < 0.0 || *speed > MAX_BACKGROUND_SHADER_SPEED {
                    return Err(format!(
                        "background shader speed must be between 0 and {MAX_BACKGROUND_SHADER_SPEED}"
                    ));
                }
            }
            Command::OpenOperationalView { view_id } => {
                crate::operational_views::load(view_id).map_err(|error| error.to_string())?;
            }
            Command::ApplyDocument { .. }
            | Command::ApplyPatch { .. }
            | Command::Inspect
            | Command::ReadDocument
            | Command::Focus
            | Command::DismissPresencePreview
            | Command::Shutdown
            | Command::ScreensaverShader { .. }
            | Command::OpenDataViz { .. }
            | Command::Screenshot { .. }
            | Command::SetBackground { .. }
            | Command::ClearBackground
            | Command::OpenStream { .. }
            | Command::OpenWidget { .. } => {}
        }
        Ok(())
    }
}

pub fn validate_frame(frame: Rect) -> Result<(), String> {
    if frame.width < 320 || frame.height < 240 || frame.width > 16_384 || frame.height > 16_384 {
        return Err("Canvas frame must be between 320x240 and 16384x16384".into());
    }
    let right = i64::from(frame.x) + i64::from(frame.width);
    let bottom = i64::from(frame.y) + i64::from(frame.height);
    if right < i64::from(i32::MIN)
        || right > i64::from(i32::MAX)
        || bottom < i64::from(i32::MIN)
        || bottom > i64::from(i32::MAX)
    {
        return Err("Canvas frame overflows desktop coordinates".into());
    }
    Ok(())
}

/// Grammar for workspace *names*: they key the store and arrive from Alfalfa,
/// whose project ids are human-authored and often capitalized (`JARVIS`,
/// `LibreChat`). Still closed to anything path-like or whitespace; only the
/// leading lowercase rule differs from `identifier`.
fn workspace_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_lowercase()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn request_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
        })
}

fn presence_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.is_ascii()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'@'))
}

fn visible(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
}

fn bounded_printable(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

pub fn read_frame(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    BufReader::new(reader.take(MAX_CONTROL_MESSAGE + 1))
        .read_until(b'\n', &mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_CONTROL_MESSAGE || bytes.last() != Some(&b'\n') {
        return Err("Canvas control message exceeds limit or lacks newline".into());
    }
    Ok(bytes)
}

#[cfg(unix)]
pub fn socket_path() -> Result<PathBuf, String> {
    #[cfg(target_os = "macos")]
    let path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("HOME is unavailable")?
        .join("Library/Application Support/Dock/canvas/control.sock");
    #[cfg(not(target_os = "macos"))]
    let path = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or("XDG_RUNTIME_DIR is unavailable")?
        .join("dock/canvas/control.sock");
    Ok(path)
}

#[cfg(unix)]
pub fn send(request: &ControlRequest) -> Result<ControlReply, String> {
    use std::os::unix::{
        fs::{FileTypeExt, MetadataExt},
        net::UnixStream,
    };

    request.validate()?;
    let path = socket_path()?;
    let metadata = std::fs::symlink_metadata(&path)
        .map_err(|error| format!("Canvas runtime unavailable at {}: {error}", path.display()))?;
    if !metadata.file_type().is_socket()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("unsafe Canvas socket type, ownership, or permissions".into());
    }
    let mut stream = UnixStream::connect(path).map_err(|error| error.to_string())?;
    canvas_control::verify_peer(&stream)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|error| error.to_string())?;
    let mut bytes = serde_json::to_vec(request).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_CONTROL_MESSAGE {
        return Err("Canvas control request exceeds limit".into());
    }
    stream
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    let reply: ControlReply = serde_json::from_slice(&read_frame(stream)?)
        .map_err(|error| format!("invalid Canvas control reply: {error}"))?;
    if reply.schema_version != CONTROL_VERSION || reply.request_id != request.request_id {
        return Err("Canvas control reply does not match the request".into());
    }
    Ok(reply)
}

pub mod text {
    use super::*;

    pub fn tokenize(line: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        let mut quote_char = '\0';
        for ch in line.chars() {
            if in_quotes {
                if ch == quote_char {
                    in_quotes = false;
                } else if ch == '\\' {
                    // pass through escape char
                } else {
                    current.push(ch);
                }
            } else {
                match ch {
                    '"' | '\'' => {
                        in_quotes = true;
                        quote_char = ch;
                    }
                    ' ' | '\t' | '\n' | '\r' => {
                        if !current.is_empty() {
                            tokens.push(std::mem::take(&mut current));
                        }
                    }
                    '#' => break,
                    _ => current.push(ch),
                }
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }
        tokens
    }

    fn req(tokens: &[String], count: usize, usage: &str) -> Result<(), String> {
        if tokens.len() < count {
            Err(format!("expected: {usage}"))
        } else {
            Ok(())
        }
    }

    fn parse_i32(token: &str) -> Result<i32, String> {
        token
            .parse()
            .map_err(|_| format!("not an integer: {token}"))
    }

    fn parse_u32(token: &str) -> Result<u32, String> {
        token.parse().map_err(|_| format!("not a number: {token}"))
    }

    fn parse_u8(token: &str) -> Result<u8, String> {
        token
            .parse()
            .map_err(|_| format!("not a byte value (0-255): {token}"))
    }

    /// A shader named the way the wire names it (`aurora-drift`), or the
    /// renderer index `screensaver_shader` already takes. Both resolutions come
    /// from the shader table and serde rather than a name list here, so adding
    /// a shader does not need a second edit in the protocol.
    fn background_shader_from_token(token: &str) -> Result<BackgroundShader, String> {
        if let Ok(index) = token.parse::<usize>() {
            return crate::screensaver::SHADERS
                .get(index)
                .map(|shader| shader.id)
                .ok_or_else(|| format!("background shader index {index} is out of range"));
        }
        serde_json::from_value(serde_json::Value::String(token.to_owned()))
            .map_err(|_| format!("unknown background shader: {token}"))
    }

    fn state_from_name(token: &str) -> Result<IndicatorState, String> {
        match token {
            "ready" => Ok(IndicatorState::Ready),
            "transcribing" => Ok(IndicatorState::Transcribing),
            "listening" => Ok(IndicatorState::Listening),
            "thinking" => Ok(IndicatorState::Thinking),
            "speaking" => Ok(IndicatorState::Speaking),
            _ => Err(format!("unknown indicator state: {token}")),
        }
    }

    fn build_command(tokens: &[String]) -> Result<Command, String> {
        if tokens.is_empty() {
            return Err("empty command".into());
        }
        match tokens[0].as_str() {
            "inspect" | "info" => Ok(Command::Inspect),
            "focus" => Ok(Command::Focus),
            "show" => Ok(Command::SetVisibility { visible: true }),
            "hide" => Ok(Command::SetVisibility { visible: false }),
            "shutdown" => Ok(Command::Shutdown),
            "ws-save" | "workspace-save" | "save-ws" => {
                req(tokens, 2, "ws-save <name>")?;
                Ok(Command::WorkspaceSave {
                    name: tokens[1].clone(),
                })
            }
            "ws-switch" | "workspace-switch" | "switch-ws" => {
                req(tokens, 2, "ws-switch <name>")?;
                Ok(Command::WorkspaceSwitch {
                    name: tokens[1].clone(),
                })
            }
            "screensaver_shader" | "shader" => {
                req(tokens, 2, "screensaver_shader <index>")?;
                tokens[1]
                    .parse::<usize>()
                    .map_err(|_| "invalid shader index".into())
                    .map(|index| Command::ScreensaverShader { index })
            }
            "bg-shader" | "set_background_shader" => {
                req(
                    tokens,
                    3,
                    "bg-shader <project_id> <shader-name|index> [speed]",
                )?;
                if !workspace_name(&tokens[1]) {
                    return Err("invalid project id".into());
                }
                let speed = match tokens.get(3) {
                    Some(token) => token
                        .parse::<f32>()
                        .map_err(|_| format!("not a speed: {token}"))?,
                    None => DEFAULT_BACKGROUND_SHADER_SPEED,
                };
                Ok(Command::SetBackgroundShader {
                    project_id: tokens[1].clone(),
                    shader_id: background_shader_from_token(&tokens[2])?,
                    speed,
                })
            }
            "open_dataviz" | "dataviz" | "chart" => {
                req(tokens, 4, "open_dataviz <chart_type> <surface_id> <file>")?;
                let chart_type = tokens[1].clone();
                let surface_id = tokens[2].clone();
                let file = std::path::PathBuf::from(&tokens[3]);
                Ok(Command::OpenDataViz {
                    surface_id,
                    file,
                    chart_type,
                })
            }
            "bg-image" | "bgimg" => {
                req(tokens, 2, "bg-image <path>")?;
                Ok(Command::SetBackground {
                    path: std::path::PathBuf::from(&tokens[1]),
                })
            }
            "bg-clear" | "bgclear" | "bg-default" => Ok(Command::ClearBackground),
            "open_stream" | "stream" => {
                req(tokens, 3, "open_stream <url> <surface_id>")?;
                Ok(Command::OpenStream {
                    surface_id: tokens[2].clone(),
                    url: tokens[1].clone(),
                })
            }
            "open_widget" | "widget" => {
                req(tokens, 3, "open_widget <spec_path> <surface_id>")?;
                Ok(Command::OpenWidget {
                    surface_id: tokens[2].clone(),
                    spec_path: std::path::PathBuf::from(&tokens[1]),
                })
            }
            "open_view" | "view" => {
                req(tokens, 2, "view <project/view>")?;
                Ok(Command::OpenOperationalView {
                    view_id: tokens[1].clone(),
                })
            }
            "bg" | "theme" => {
                req(tokens, 2, "bg <project_id>")?;
                if !workspace_name(&tokens[1]) {
                    return Err("invalid project id".into());
                }
                Ok(Command::SetTheme {
                    project_id: tokens[1].clone(),
                })
            }
            "move" | "mv" => {
                req(tokens, 5, "move <x> <y> <width> <height>")?;
                Ok(Command::SetFrame {
                    frame: Rect {
                        x: parse_i32(&tokens[1])?,
                        y: parse_i32(&tokens[2])?,
                        width: parse_u32(&tokens[3])?,
                        height: parse_u32(&tokens[4])?,
                    },
                })
            }
            "display-target" | "displays-use" => {
                req(tokens, 2, "display-target <all|display-id...>")?;
                let target = if tokens[1] == "all" {
                    if tokens.len() != 2 {
                        return Err("display-target all takes no display IDs".into());
                    }
                    DisplayTarget::All
                } else {
                    DisplayTarget::Exact {
                        display_ids: tokens[1..].to_vec(),
                    }
                };
                Ok(Command::SetDisplayTarget { target })
            }
            "close" | "rm" => {
                req(tokens, 2, "close <surface_id>")?;
                if !identifier(&tokens[1]) {
                    return Err("invalid surface id".into());
                }
                Ok(Command::CloseSurface {
                    surface_id: tokens[1].clone(),
                })
            }
            "nav" | "go" => {
                req(tokens, 3, "nav <surface_id> <next|prev|page N>")?;
                if !identifier(&tokens[1]) {
                    return Err("invalid surface id".into());
                }
                let target = match tokens[2].as_str() {
                    "next" | "n" => Navigation::Next,
                    "prev" | "previous" | "p" => Navigation::Previous,
                    "page" => {
                        req(tokens, 4, "nav <surface_id> page <index>")?;
                        Navigation::Page {
                            index: parse_u32(&tokens[3])?,
                        }
                    }
                    other => return Err(format!("unknown nav target: {other}")),
                };
                Ok(Command::Navigate {
                    surface_id: tokens[1].clone(),
                    target,
                })
            }
            "indicator" | "ind" => {
                req(
                    tokens,
                    2,
                    "indicator <id> <state> [caption]  |  indicator <id> off",
                )?;
                if !identifier(&tokens[1]) {
                    return Err("invalid indicator id".into());
                }
                if tokens.len() == 3 && tokens[2] == "off" {
                    return Ok(Command::DismissIndicator {
                        indicator_id: tokens[1].clone(),
                    });
                }
                req(tokens, 3, "indicator <id> <state> [caption]")?;
                let state = state_from_name(&tokens[2])?;
                let caption = if tokens.len() > 3 {
                    Some(tokens[3..].join(" "))
                } else {
                    None
                };
                Ok(Command::UpsertIndicator {
                    indicator: StatusIndicator {
                        id: tokens[1].clone(),
                        title: tokens[1].clone(),
                        state,
                        caption,
                        detail: None,
                        diagnostic: None,
                    },
                })
            }
            "open" => {
                req(tokens, 3, "open <surface_id> <path> [title]")?;
                if !identifier(&tokens[1]) {
                    return Err("invalid surface id".into());
                }
                let title = if tokens.len() > 3 {
                    Some(tokens[3..].join(" "))
                } else {
                    None
                };
                Ok(Command::OpenFile {
                    surface_id: tokens[1].clone(),
                    path: tokens[2].clone(),
                    title,
                })
            }
            "wallpaper" | "wp" => {
                req(tokens, 6, "wp <project> <display_id> <r> <g> <b>")?;
                if !workspace_name(&tokens[1]) {
                    return Err("invalid project id".into());
                }
                if tokens[2].is_empty() || tokens[2].len() > 128 {
                    return Err("display id must be 1-128 chars".into());
                }
                Ok(Command::SetWallpaper {
                    request: WallpaperRequest {
                        project: tokens[1].clone(),
                        display_id: tokens[2].clone(),
                        wallpaper: Wallpaper::Solid {
                            rgb: [
                                parse_u8(&tokens[3])?,
                                parse_u8(&tokens[4])?,
                                parse_u8(&tokens[5])?,
                            ],
                        },
                    },
                })
            }
            other => Err(format!("unknown command: {other}")),
        }
    }

    pub fn parse_line(line: &str) -> Result<ControlRequest, String> {
        let tokens = tokenize(line);
        if tokens.is_empty() {
            return Err("empty command".into());
        }
        let mut tokens = tokens;
        let expected_revision;
        if tokens[0] == "rev" {
            req(&tokens, 2, "rev <N> <command>")?;
            expected_revision = Some(parse_u32(&tokens[1])? as u64);
            tokens = tokens[2..].to_vec();
        } else {
            expected_revision = None;
        }
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: format!("text-{}", std::process::id()),
            expected_revision,
            actor: None,
            command: build_command(&tokens)?,
        };
        request.validate()?;
        Ok(request)
    }

    /// The result a person can see, not the field that was written.
    ///
    /// The `NOT on screen` half is the whole reason this exists: a reply that
    /// said only what it had written let three identical `ok`s describe a desk
    /// that never changed.
    fn format_visible_background(visible: &VisibleBackground) -> String {
        let landed = if visible.on_screen() {
            format!("project={} (on screen)", visible.project_id)
        } else {
            format!(
                "project={} NOT on screen, showing={}",
                visible.project_id, visible.active_project_id
            )
        };
        let drawn = match &visible.drawn_shader {
            Some(shader) => format!("{shader:?}"),
            None => "none".into(),
        };
        format!(
            "{landed} background={} drawing={drawn} screensaver={}",
            background_kind(&visible.background),
            visible.screensaver_enabled
        )
    }

    /// The background's own wire tag, so this label cannot drift from the enum.
    fn background_kind(background: &Background) -> String {
        serde_json::to_value(background)
            .ok()
            .and_then(|value| {
                value
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "unknown".into())
    }

    fn format_error_code(code: &ErrorCode) -> &'static str {
        match code {
            ErrorCode::InvalidRequest => "invalid_request",
            ErrorCode::Conflict => "conflict",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::ResourceLimit => "resource_limit",
            ErrorCode::Platform => "platform",
            ErrorCode::Internal => "internal",
        }
    }

    pub fn format_reply(reply: &ControlReply) -> String {
        match &reply.outcome {
            Outcome::Ok { receipt } => {
                let detail = match receipt {
                    Receipt::DocumentSnapshot { snapshot, .. } => format!(
                        "project={} frame={},{},{},{} focus={} surfaces={} indicators={}",
                        snapshot.project_id,
                        snapshot.frame.x,
                        snapshot.frame.y,
                        snapshot.frame.width,
                        snapshot.frame.height,
                        snapshot.focused_surface_id.as_deref().unwrap_or("-"),
                        snapshot
                            .surfaces
                            .iter()
                            .map(|s| s.id.as_str())
                            .collect::<Vec<_>>()
                            .join(","),
                        snapshot.active_indicators.len()
                    ),
                    Receipt::Snapshot { snapshot } => format!(
                        "project={} frame={},{},{},{} surfaces={} indicators={} events={} displays={} visible={}",
                        snapshot.project_id,
                        snapshot.frame.x,
                        snapshot.frame.y,
                        snapshot.frame.width,
                        snapshot.frame.height,
                        snapshot.surfaces.len(),
                        snapshot.active_indicators.len(),
                        snapshot.events.len(),
                        snapshot.display_frames.len(),
                        snapshot.visible
                    ),
                    Receipt::DocumentApplied { workspace_id } => {
                        format!("document={}", workspace_id)
                    }
                    Receipt::SurfaceOpened { surface } => {
                        format!("opened={}", surface.id)
                    }
                    Receipt::ViewPatched { surface_id, view } => {
                        format!("view={} revision={}", surface_id, view.revision)
                    }
                    Receipt::OperationalViewOpened {
                        view_id,
                        surface_ids,
                    } => format!("view={view_id} surfaces={}", surface_ids.join(",")),
                    Receipt::SurfaceClosed { surface_id } => {
                        format!("closed={}", surface_id)
                    }
                    Receipt::FrameSet { frame } => format!(
                        "frame={},{},{},{}",
                        frame.x, frame.y, frame.width, frame.height
                    ),
                    Receipt::DisplayTargetSet { display_ids, frame } => format!(
                        "displays={} frame={},{},{},{}",
                        display_ids.join(","),
                        frame.x,
                        frame.y,
                        frame.width,
                        frame.height
                    ),
                    Receipt::SurfacePlaced {
                        surface_id,
                        display_id,
                        ..
                    } => format!("placed={surface_id} display={display_id}"),
                    Receipt::OverlaysSet { overlays } => format!(
                        "overlays presence={:?} hud={:?}",
                        overlays.presence_rail, overlays.system_hud
                    ),
                    Receipt::VisibilitySet { visible } => format!("visible={visible}"),
                    Receipt::Focused => "focused".to_string(),
                    Receipt::Navigated {
                        surface_id,
                        page_index,
                        page_count,
                    } => format!("nav {} page {}/{}", surface_id, page_index + 1, page_count),
                    Receipt::ThemeSet { project_id, .. } => {
                        format!("theme={}", project_id)
                    }
                    Receipt::WallpaperSet { receipt } => {
                        format!("wallpaper={} {}", receipt.project, receipt.display_id)
                    }
                    Receipt::IndicatorUpserted { indicator_id } => {
                        format!("indicator={}", indicator_id)
                    }
                    Receipt::IndicatorDismissed { indicator_id } => {
                        format!("dismissed={}", indicator_id)
                    }
                    Receipt::MeshSet { node_count } => format!("mesh={node_count}"),
                    Receipt::ShuttingDown => "shutting down".to_string(),
                    Receipt::PresenceSet { .. } => "presence set".to_string(),
                    Receipt::ScreensaverUpdated { settings } => {
                        format!(
                            "screensaver enabled={} show_time={} show_fps={} max_fps={}",
                            settings.enabled,
                            settings.show_time,
                            settings.show_fps,
                            settings.max_fps
                        )
                    }
                    Receipt::PresencePreviewShown { .. } => "preview shown".to_string(),
                    Receipt::PresencePreviewDismissed => "preview dismissed".to_string(),
                    Receipt::WorkspaceSaved { name } => format!("saved={name}"),
                    Receipt::WorkspaceSwitched { name } => format!("switched={name}"),
                    Receipt::WorkspacesSynced {
                        created,
                        updated,
                        preserved,
                        total,
                    } => format!(
                        "workspaces={total} created={created} updated={updated} preserved={preserved}"
                    ),
                    Receipt::ScreensaverShaderSet {
                        index,
                        name,
                        visible,
                    } => match visible {
                        Some(visible) => format!(
                            "screensaver shader={index} ({name}) {}",
                            format_visible_background(visible)
                        ),
                        None => format!("shader={index} ({name})"),
                    },
                    Receipt::BackgroundShaderSet { visible } => {
                        format!("background {}", format_visible_background(visible))
                    }
                    Receipt::ScreenshotCaptured {
                        path,
                        sha256,
                        width,
                        height,
                    } => {
                        format!(
                            "screenshot path={} sha256={sha256} {}x{}",
                            path.display(),
                            width,
                            height
                        )
                    }
                };
                format!("OK rev={} {}", reply.revision, detail)
            }
            Outcome::Error { code, message } => {
                format!("ERR {} {}", format_error_code(code), message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framing_is_bounded_and_requires_newline() {
        assert_eq!(read_frame(&b"{}\n"[..]).unwrap(), b"{}\n");
        assert!(read_frame(&b"{}"[..]).is_err());
        assert!(read_frame(vec![b'x'; MAX_CONTROL_MESSAGE as usize + 1].as_slice()).is_err());
    }

    #[test]
    fn screensaver_defaults_off_and_rejects_unbounded_frame_rates() {
        let settings = ScreensaverSettings::default();
        assert!(!settings.enabled);
        assert_eq!(settings.max_fps, DEFAULT_SCREENSAVER_FPS);
        settings.validate().unwrap();

        assert!(
            ScreensaverSettings {
                max_fps: 0,
                ..settings
            }
            .validate()
            .is_err()
        );
        assert!(
            ScreensaverSettings {
                max_fps: MAX_SCREENSAVER_FPS + 1,
                ..settings
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn workspace_sync_requires_unique_bounded_project_definitions() {
        let project = ProjectWorkspaceSpec {
            project_id: "gh-1288853134".into(),
            name: "ohmOS".into(),
            status: "paused".into(),
            theme: ThemeSelection::Coherence {
                mode: canvas_core::ThemeMode::Dark,
                accent: canvas_core::Accent::Success,
            },
        };
        let request = |projects| ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "sync-projects".into(),
            expected_revision: Some(4),
            actor: None,
            command: Command::WorkspaceSync { projects },
        };
        request(vec![project.clone()]).validate().unwrap();
        assert!(request(Vec::new()).validate().is_err());
        assert!(
            request(vec![project.clone(), project])
                .validate()
                .unwrap_err()
                .contains("duplicate")
        );
    }

    #[test]
    fn protocol_rejects_unknown_fields_and_unsafe_geometry() {
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "move-1".into(),
            expected_revision: Some(4),
            actor: None,
            command: Command::SetFrame {
                frame: Rect {
                    x: 10,
                    y: 20,
                    width: 1200,
                    height: 800,
                },
            },
        };
        request.validate().unwrap();
        let json = serde_json::to_string(&request)
            .unwrap()
            .replace("\"width\":1200", "\"width\":1200,\"surprise\":true");
        assert!(serde_json::from_str::<ControlRequest>(&json).is_err());

        let mut invalid = request;
        invalid.command = Command::SetFrame {
            frame: Rect {
                x: i32::MAX,
                y: 0,
                width: 1000,
                height: 800,
            },
        };
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn indicator_wire_names_and_states_are_stable() {
        for (state, wire_name) in [
            (IndicatorState::Ready, "ready"),
            (IndicatorState::Transcribing, "transcribing"),
            (IndicatorState::Listening, "listening"),
            (IndicatorState::Thinking, "thinking"),
            (IndicatorState::Speaking, "speaking"),
        ] {
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{wire_name}\"")
            );
        }

        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "indicator-1".into(),
            expected_revision: None,
            actor: None,
            command: Command::UpsertIndicator {
                indicator: StatusIndicator {
                    id: "assistant-status".into(),
                    title: "Assistant".into(),
                    state: IndicatorState::Listening,
                    caption: Some("Listening...".into()),
                    detail: Some("I can hear you.".into()),
                    diagnostic: Some("STT=233ms".into()),
                },
            },
        };
        request.validate().unwrap();
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["command"]["command"], "upsert_indicator");
        assert_eq!(value["command"]["indicator"]["state"], "listening");
        assert_eq!(value["command"]["indicator"]["detail"], "I can hear you.");
        assert_eq!(value["command"]["indicator"]["diagnostic"], "STT=233ms");

        let dismiss = ControlRequest {
            command: Command::DismissIndicator {
                indicator_id: "assistant-status".into(),
            },
            ..request
        };
        let value = serde_json::to_value(dismiss).unwrap();
        assert_eq!(value["command"]["command"], "dismiss_indicator");
    }

    #[test]
    fn mesh_wire_names_and_states_are_stable() {
        for (state, wire_name) in [
            (MeshNodeState::Active, "active"),
            (MeshNodeState::Paused, "paused"),
            (MeshNodeState::Suspended, "suspended"),
            (MeshNodeState::Stopped, "stopped"),
            (MeshNodeState::Hidden, "hidden"),
        ] {
            assert_eq!(
                serde_json::to_string(&state).unwrap(),
                format!("\"{wire_name}\"")
            );
        }

        let mesh = MeshSnapshot {
            nodes: vec![MeshNode {
                id: "agora-one".into(),
                label: "Agora One".into(),
                state: MeshNodeState::Active,
                detail: Some("rendering".into()),
            }],
        };
        mesh.validate().unwrap();
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "mesh-1".into(),
            expected_revision: None,
            actor: None,
            command: Command::SetMesh { mesh },
        };
        request.validate().unwrap();
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(value["command"]["command"], "set_mesh");
        assert_eq!(value["command"]["mesh"]["nodes"][0]["state"], "active");
    }

    #[test]
    fn mesh_snapshot_is_bounded_and_duplicate_free() {
        let node = MeshNode {
            id: "node-a".into(),
            label: "A".into(),
            state: MeshNodeState::Active,
            detail: None,
        };
        let mut mesh = MeshSnapshot {
            nodes: vec![node.clone(); MAX_MESH_NODES],
        };
        mesh.nodes
            .iter_mut()
            .enumerate()
            .for_each(|(index, entry)| entry.id = format!("node-{index}"));
        mesh.validate().unwrap();

        mesh.nodes.push(node.clone());
        assert!(mesh.validate().is_err());

        mesh.nodes.pop();
        mesh.nodes[0].id = "node-b".into();
        mesh.nodes[1].id = "node-b".into();
        assert!(
            mesh.validate()
                .unwrap_err()
                .contains("duplicate mesh node id")
        );

        mesh.nodes.pop();
        mesh.nodes[0].detail = Some("x".repeat(MAX_MESH_NODE_DETAIL_BYTES + 1));
        assert!(mesh.validate().is_err());
        mesh.nodes[0].label = " ".into();
        assert!(mesh.validate().is_err());
    }

    #[test]
    fn presence_snapshot_keeps_modalities_independent_and_bounded() {
        let verified = PresenceSignal {
            state: PresenceSignalState::Verified,
            confidence: Some(0.91),
            age_ms: 120,
        };
        let unavailable = PresenceSignal {
            state: PresenceSignalState::Unavailable,
            confidence: None,
            age_ms: 120,
        };
        let avery = PresenceOccupant {
            id: "avery@fpl.dev".into(),
            display_name: "Avery".into(),
            presence: PresenceSignal {
                state: PresenceSignalState::Detected,
                ..verified.clone()
            },
            face: verified.clone(),
            voice: verified.clone(),
        };
        let ange = PresenceOccupant {
            id: "ange".into(),
            display_name: "Ange".into(),
            presence: PresenceSignal {
                state: PresenceSignalState::Detected,
                ..verified.clone()
            },
            face: verified,
            voice: unavailable,
        };
        let snapshot = PresenceSnapshot {
            zone: "bedroom".into(),
            observed_at_unix_ms: 1000,
            expires_after_ms: 5000,
            occupied: true,
            occupants: vec![avery, ange],
        };
        snapshot.validate().unwrap();
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "presence-1".into(),
            expected_revision: None,
            actor: None,
            command: Command::SetPresence {
                presence: snapshot.clone(),
            },
        };
        request.validate().unwrap();
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["command"]["command"], "set_presence");
        assert_eq!(
            value["command"]["presence"]["occupants"][1]["display_name"],
            "Ange"
        );

        let mut invalid = snapshot.clone();
        invalid.occupied = false;
        assert!(invalid.validate().is_err());
        invalid = snapshot.clone();
        invalid.occupants[1].id = invalid.occupants[0].id.clone();
        assert!(invalid.validate().is_err());
        invalid = snapshot;
        invalid.occupants[0].face.confidence = Some(f32::NAN);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn presence_preview_rejects_invalid_geometry_and_oversize_images() {
        let valid = PresencePreview {
            captured_at_unix_ms: 1000,
            width: 480,
            height: 360,
            linger_ms: 2500,
            jpeg_base64: "jpeg".into(),
            faces: vec![PresencePreviewFace {
                occupant_id: Some("avery@fpl.dev".into()),
                display_name: Some("Avery".into()),
                confidence: 0.9,
                bounds: [0.1, 0.1, 0.8, 0.9],
            }],
        };
        valid.validate().unwrap();
        let mut invalid = valid.clone();
        invalid.faces[0].bounds = [0.8, 0.1, 0.1, 0.9];
        assert!(invalid.validate().is_err());
        invalid = valid;
        invalid.jpeg_base64 = "x".repeat(MAX_PRESENCE_PREVIEW_BASE64_BYTES + 1);
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn workspace_names_accept_alfalfa_capitalization_and_reject_paths() {
        for good in ["JARVIS", "LibreChat", "dock", "a-B-9"] {
            let req = ControlRequest {
                schema_version: CONTROL_VERSION,
                request_id: "w".into(),
                expected_revision: None,
                actor: None,
                command: Command::WorkspaceSwitch { name: good.into() },
            };
            assert!(req.validate().is_ok(), "{good} must be a legal workspace name");
        }
        for bad in ["", "jar vis", "a/b", "..", "x.y"] {
            let req = ControlRequest {
                schema_version: CONTROL_VERSION,
                request_id: "w".into(),
                expected_revision: None,
                actor: None,
                command: Command::WorkspaceSwitch { name: bad.into() },
            };
            assert!(req.validate().is_err(), "{bad:?} must be refused");
        }
    }

    #[test]
    fn indicator_text_and_ids_are_bounded_and_printable() {
        let valid = StatusIndicator {
            id: "assistant-status".into(),
            title: "Assistant".into(),
            state: IndicatorState::Thinking,
            caption: Some("Working on it".into()),
            detail: Some("Checking the room".into()),
            diagnostic: Some("wake=0.91 | STT=233ms".into()),
        };
        valid.validate().unwrap();

        for invalid in [
            StatusIndicator {
                id: "Assistant_Status".into(),
                ..valid.clone()
            },
            StatusIndicator {
                title: " ".into(),
                ..valid.clone()
            },
            StatusIndicator {
                title: "x".repeat(MAX_INDICATOR_TITLE_BYTES + 1),
                ..valid.clone()
            },
            StatusIndicator {
                caption: Some("line one\nline two".into()),
                ..valid.clone()
            },
            StatusIndicator {
                caption: Some("x".repeat(MAX_INDICATOR_CAPTION_BYTES + 1)),
                ..valid.clone()
            },
            StatusIndicator {
                detail: Some("x".repeat(MAX_INDICATOR_DETAIL_BYTES + 1)),
                ..valid.clone()
            },
            StatusIndicator {
                diagnostic: Some("x".repeat(MAX_INDICATOR_DIAGNOSTIC_BYTES + 1)),
                ..valid.clone()
            },
        ] {
            assert!(invalid.validate().is_err(), "accepted {invalid:?}");
        }
    }

    #[test]
    fn text_command_round_trips_and_rejects_garbage() {
        use crate::control::text::*;
        let req = parse_line("inspect").unwrap();
        assert_eq!(req.command, Command::Inspect);
        assert_eq!(req.expected_revision, None);

        let req = parse_line("focus").unwrap();
        assert_eq!(req.command, Command::Focus);

        assert_eq!(
            parse_line("hide").unwrap().command,
            Command::SetVisibility { visible: false }
        );
        assert_eq!(
            parse_line("show").unwrap().command,
            Command::SetVisibility { visible: true }
        );

        let req = parse_line("rev 8 focus").unwrap();
        assert_eq!(req.expected_revision, Some(8));
        assert_eq!(req.command, Command::Focus);

        assert_eq!(
            parse_line("display-target all").unwrap().command,
            Command::SetDisplayTarget {
                target: DisplayTarget::All
            }
        );
        assert_eq!(
            parse_line("display-target left right").unwrap().command,
            Command::SetDisplayTarget {
                target: DisplayTarget::Exact {
                    display_ids: vec!["left".into(), "right".into()]
                }
            }
        );

        let req = parse_line("move 0 25 1920 1150").unwrap();        match req.command {
            Command::SetFrame { frame } => {
                assert_eq!(
                    frame,
                    Rect {
                        x: 0,
                        y: 25,
                        width: 1920,
                        height: 1150
                    }
                );
            }
            _ => panic!("expected SetFrame"),
        }

        let req = parse_line("bg dock").unwrap();
        assert_eq!(
            req.command,
            Command::SetTheme {
                project_id: "dock".into()
            }
        );

        let req = parse_line("indicator status speaking 'Socket live'").unwrap();
        match &req.command {
            Command::UpsertIndicator { indicator } => {
                assert_eq!(indicator.id, "status");
                assert_eq!(indicator.state, IndicatorState::Speaking);
                assert_eq!(indicator.caption.as_deref(), Some("Socket live"));
            }
            _ => panic!("expected UpsertIndicator"),
        }

        let req = parse_line("indicator status off").unwrap();
        match &req.command {
            Command::DismissIndicator { indicator_id } => {
                assert_eq!(indicator_id, "status");
            }
            _ => panic!("expected DismissIndicator"),
        }

        let req = parse_line("nav scene-view next").unwrap();
        match &req.command {
            Command::Navigate { surface_id, target } => {
                assert_eq!(surface_id, "scene-view");
                assert_eq!(*target, Navigation::Next);
            }
            _ => panic!("expected Navigate"),
        }

        let req = parse_line("nav scene-view page 3").unwrap();
        match &req.command {
            Command::Navigate { target, .. } => {
                assert_eq!(*target, Navigation::Page { index: 3 });
            }
            _ => panic!("expected Navigate"),
        }

        let req = parse_line("close studio-view").unwrap();
        match &req.command {
            Command::CloseSurface { surface_id } => {
                assert_eq!(surface_id, "studio-view");
            }
            _ => panic!("expected CloseSurface"),
        }

        let req = parse_line("wp dock display-1 10 20 30").unwrap();
        match &req.command {
            Command::SetWallpaper { request } => {
                assert_eq!(request.project, "dock");
                assert_eq!(request.display_id, "display-1");
                assert_eq!(request.wallpaper, Wallpaper::Solid { rgb: [10, 20, 30] });
            }
            _ => panic!("expected SetWallpaper"),
        }

        // Comments and quotes
        let req = parse_line("inspect # read state").unwrap();
        assert_eq!(req.command, Command::Inspect);

        assert!(parse_line("").is_err());
        assert!(parse_line("# just a comment").is_err());
        assert!(parse_line("bogus").is_err());
    }

    #[test]
    fn overlay_policy_round_trips_as_a_bounded_control_command() {
        let overlays = OverlaySettings::default();
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "overlay-policy".into(),
            expected_revision: Some(4),
            actor: None,
            command: Command::SetOverlays { overlays },
        };
        request.validate().unwrap();
        let json = serde_json::to_string(&request).unwrap();
        let decoded: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(
            serde_json::to_value(overlays).unwrap()["shortcut_help"],
            "rightmost"
        );
    }

    #[test]
    fn shared_view_patch_round_trips_over_the_control_socket() {
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "view-focus-region".into(),
            expected_revision: Some(8),
            actor: None,
            command: Command::PatchView {
                surface_id: "drawing".into(),
                patch: SharedViewPatch {
                    schema_version: crate::SCHEMA_VERSION,
                    view_id: "drawing".into(),
                    base_revision: 3,
                    revision: 4,
                    operations: vec![crate::view::ViewOperation::SetViewport {
                        viewport: crate::view::ViewRect {
                            x: 0.25,
                            y: 0.25,
                            width: 0.5,
                            height: 0.5,
                        },
                    }],
                },
            },
        };
        request.validate().unwrap();
        let encoded = serde_json::to_vec(&request).unwrap();
        let decoded: ControlRequest = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn background_shader_command_names_a_project_and_bounded_speed() {
        use crate::control::text::*;

        let request = parse_line("bg-shader dock aurora-drift 0.5").unwrap();
        assert_eq!(
            request.command,
            Command::SetBackgroundShader {
                project_id: "dock".into(),
                shader_id: BackgroundShader::AuroraDrift,
                speed: 0.5,
            }
        );
        assert_eq!(
            serde_json::to_value(&request).unwrap()["command"]["command"],
            "set_background_shader"
        );

        // An index means the same shader the renderer table means, so the
        // number an operator already has from `shader <n>` keeps working.
        let by_index = parse_line("bg-shader dock 0").unwrap();
        assert_eq!(
            by_index.command,
            Command::SetBackgroundShader {
                project_id: "dock".into(),
                shader_id: crate::screensaver::SHADERS[0].id,
                speed: DEFAULT_BACKGROUND_SHADER_SPEED,
            }
        );
        const { assert!(DEFAULT_BACKGROUND_SHADER_SPEED > 0.0) };

        assert!(parse_line("bg-shader dock no-such-shader").is_err());
        // Project ids are case-tolerant (Alfalfa capitalizes); path-like or
        // empty tokens stay refused.
        assert!(parse_line("bg-shader Dock tidepool").is_ok());
        assert!(parse_line("bg-shader do/ck tidepool").is_err());
        assert!(parse_line("bg-shader dock").is_err());
        assert!(
            parse_line(&format!(
                "bg-shader dock tidepool {}",
                MAX_BACKGROUND_SHADER_SPEED + 1.0
            ))
            .is_err()
        );

        // The screensaver command it is easily mistaken for still parses to the
        // command it always did.
        assert_eq!(
            parse_line("shader 7").unwrap().command,
            Command::ScreensaverShader { index: 7 }
        );
    }

    #[test]
    fn mutation_receipts_name_the_project_the_change_landed_on() {
        use crate::control::text::*;

        let off_screen = VisibleBackground {
            project_id: "studio".into(),
            active_project_id: "dock".into(),
            background: Background::Shader {
                shader_id: BackgroundShader::Tidepool,
                speed: 1.0,
            },
            screensaver_enabled: false,
            drawn_shader: Some(BackgroundShader::Tidepool),
        };
        assert!(!off_screen.on_screen());
        let reply = ControlReply {
            schema_version: CONTROL_VERSION,
            request_id: "bg-1".into(),
            revision: 2189,
            outcome: Outcome::Ok {
                receipt: Receipt::BackgroundShaderSet {
                    visible: off_screen.clone(),
                },
            },
        };
        let formatted = format_reply(&reply);
        assert!(formatted.contains("project=studio"), "{formatted}");
        assert!(formatted.contains("showing=dock"), "{formatted}");
        assert!(formatted.contains("NOT on screen"), "{formatted}");

        // The receipt that said only `shader=7` now says where that landed.
        let screensaver = ControlReply {
            outcome: Outcome::Ok {
                receipt: Receipt::ScreensaverShaderSet {
                    index: 7,
                    name: "Flame".into(),
                    visible: Some(VisibleBackground {
                        project_id: "dock".into(),
                        ..off_screen
                    }),
                },
            },
            ..reply
        };
        let formatted = format_reply(&screensaver);
        assert!(formatted.contains("shader=7 (Flame)"), "{formatted}");
        assert!(formatted.contains("project=dock"), "{formatted}");
        assert!(formatted.contains("screensaver=false"), "{formatted}");
    }

    #[test]
    fn receipts_stay_readable_without_the_visible_state_field() {
        // An older runtime's reply carries no `visible`; a client built from
        // this protocol must still parse and print it.
        let legacy = serde_json::json!({
            "kind": "screensaver_shader_set",
            "index": 3,
            "name": "Tidepool",
        });
        let receipt: Receipt = serde_json::from_value(legacy).unwrap();
        assert_eq!(
            receipt,
            Receipt::ScreensaverShaderSet {
                index: 3,
                name: "Tidepool".into(),
                visible: None,
            }
        );
        let reply = ControlReply {
            schema_version: CONTROL_VERSION,
            request_id: "legacy".into(),
            revision: 1,
            outcome: Outcome::Ok { receipt },
        };
        assert_eq!(text::format_reply(&reply), "OK rev=1 shader=3 (Tidepool)");
    }

    #[test]
    fn text_reply_formats_ok_and_err() {
        use crate::control::text::*;
        use canvas_core::layout::Rect;

        let ok = ControlReply {
            schema_version: CONTROL_VERSION,
            request_id: "test".into(),
            revision: 42,
            outcome: Outcome::Ok {
                receipt: Receipt::Focused,
            },
        };
        assert_eq!(format_reply(&ok), "OK rev=42 focused");

        let ok = ControlReply {
            schema_version: CONTROL_VERSION,
            request_id: "test".into(),
            revision: 42,
            outcome: Outcome::Ok {
                receipt: Receipt::FrameSet {
                    frame: Rect {
                        x: 0,
                        y: 25,
                        width: 1920,
                        height: 1150,
                    },
                },
            },
        };
        assert_eq!(format_reply(&ok), "OK rev=42 frame=0,25,1920,1150");

        let err = ControlReply {
            schema_version: CONTROL_VERSION,
            request_id: "test".into(),
            revision: 41,
            outcome: Outcome::Error {
                code: ErrorCode::Conflict,
                message: "stale".into(),
            },
        };
        assert_eq!(format_reply(&err), "ERR conflict stale");
    }

    #[test]
    fn presence_redaction_strips_occupant_names_and_ids() {
        let avery = PresenceOccupant {
            id: "avery@fpl.dev".into(),
            display_name: "Avery".into(),
            presence: PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            },
            face: PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            },
            voice: PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            },
        };
        let snapshot = PresenceSnapshot {
            zone: "bedroom".into(),
            observed_at_unix_ms: 1000,
            expires_after_ms: 5000,
            occupied: true,
            occupants: vec![avery],
        };
        let redacted = snapshot.redact();
        assert!(
            redacted.occupants.is_empty(),
            "redacted snapshot has no occupants"
        );
        assert!(redacted.occupied, "occupied flag survives redaction");
        assert_eq!(redacted.zone, "bedroom");
    }

    #[test]
    fn runtime_snapshot_redact_presence_clears_roster() {
        let occupant = PresenceOccupant {
            id: "avery@fpl.dev".into(),
            display_name: "Avery".into(),
            presence: PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            },
            face: PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            },
            voice: PresenceSignal {
                state: PresenceSignalState::Unavailable,
                confidence: None,
                age_ms: 0,
            },
        };
        let snapshot = RuntimeSnapshot {
            workspace_id: "ws".into(),
            project_id: "proj".into(),
            revision: 1,
            frame: Rect {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            },
            focused_surface_id: None,
            surfaces: vec![],
            surface_bounds: Default::default(),
            views: vec![],
            active_indicators: vec![],
            mesh: None,
            presence: Some(PresenceSnapshot {
                zone: "bedroom".into(),
                observed_at_unix_ms: 1000,
                expires_after_ms: 5000,
                occupied: true,
                occupants: vec![occupant],
            }),
            presence_preview_active: false,
            screensaver: ScreensaverSnapshot::default(),
            workspaces: vec![],
            workspace_summaries: vec![],
            minimized_surface_ids: vec![],
            maximized_surface_ids: vec![],
            events: vec![],
            display_frames: vec![],
            displays: vec![],
            selected_display_ids: vec![],
            overlays: OverlaySettings::default(),
            visible: true,
        };
        let redacted = snapshot.redact_presence();
        let redacted_presence = redacted.presence.expect("presence must survive redaction");
        assert!(
            redacted_presence.occupants.is_empty(),
            "no occupant names leak to callers"
        );
        // Sanity: the original is untouched
        assert_eq!(snapshot.presence.as_ref().unwrap().occupants.len(), 1);
    }
}
