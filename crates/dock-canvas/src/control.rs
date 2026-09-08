//! Bounded control protocol for a long-lived Canvas runtime.

use crate::{ThemeRef, WorkspaceDocument, WorkspacePatch};
use dock_core::{
    desktop::{Wallpaper, WallpaperReceipt, WallpaperRequest},
    layout::Rect,
};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::time::Duration;

pub const CONTROL_VERSION: u32 = 1;
pub const MAX_CONTROL_MESSAGE: u64 = 256 * 1024;
pub const MAX_ACTIVE_INDICATORS: usize = 16;
pub const MAX_MESH_NODES: usize = 8;
pub const MAX_PRESENCE_OCCUPANTS: usize = 8;
pub const MAX_PRESENCE_FACES: usize = 8;
pub const DEFAULT_SCREENSAVER_FPS: u16 = 30;
pub const MAX_SCREENSAVER_FPS: u16 = 60;
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
    pub command: Command,
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
    WorkspaceSave {
        name: String,
    },
    WorkspaceSwitch {
        name: String,
    },
    ScreensaverShader {
        index: usize,
    },
    OpenDataViz {
        surface_id: String,
        file: PathBuf,
        chart_type: String,
    },
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
    SurfaceClosed {
        surface_id: String,
    },
    FrameSet {
        frame: Rect,
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
    ScreensaverShaderSet {
        index: usize,
        name: String,
    },
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
    pub minimized_surface_ids: Vec<String>,
    #[serde(default)]
    pub maximized_surface_ids: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default)]
    pub display_frames: Vec<Rect>,
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
            Command::CloseSurface { surface_id } | Command::Navigate { surface_id, .. } => {
                if !identifier(surface_id) {
                    return Err("invalid Canvas surface id".into());
                }
            }
            Command::SetFrame { frame } => validate_frame(*frame)?,
            Command::SetTheme { project_id } => {
                if !identifier(project_id) {
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
            Command::ApplyDocument { .. }
            | Command::ApplyPatch { .. }
            | Command::Inspect
            | Command::ReadDocument
            | Command::Focus
            | Command::DismissPresencePreview
            | Command::Shutdown
            | Command::WorkspaceSave { .. }
            | Command::WorkspaceSwitch { .. }
            | Command::ScreensaverShader { .. }
            | Command::OpenDataViz { .. }
            | Command::SetBackground { .. }
            | Command::ClearBackground
            | Command::OpenStream { .. } => {}
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
    dock_control::verify_peer(&stream)?;
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
            "bg" | "theme" => {
                req(tokens, 2, "bg <project_id>")?;
                if !identifier(&tokens[1]) {
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
                if !identifier(&tokens[1]) {
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
            command: build_command(&tokens)?,
        };
        request.validate()?;
        Ok(request)
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
                        "project={} frame={},{},{},{} surfaces={} indicators={} events={} displays={}",
                        snapshot.project_id,
                        snapshot.frame.x,
                        snapshot.frame.y,
                        snapshot.frame.width,
                        snapshot.frame.height,
                        snapshot.surfaces.len(),
                        snapshot.active_indicators.len(),
                        snapshot.events.len(),
                        snapshot.display_frames.len()
                    ),
                    Receipt::DocumentApplied { workspace_id } => {
                        format!("document={}", workspace_id)
                    }
                    Receipt::SurfaceOpened { surface } => {
                        format!("opened={}", surface.id)
                    }
                    Receipt::SurfaceClosed { surface_id } => {
                        format!("closed={}", surface_id)
                    }
                    Receipt::FrameSet { frame } => format!(
                        "frame={},{},{},{}",
                        frame.x, frame.y, frame.width, frame.height
                    ),
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
                    Receipt::ScreensaverShaderSet { index, name } => {
                        format!("shader={index} ({name})")
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
    fn protocol_rejects_unknown_fields_and_unsafe_geometry() {
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "move-1".into(),
            expected_revision: Some(4),
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

        let req = parse_line("rev 8 focus").unwrap();
        assert_eq!(req.expected_revision, Some(8));
        assert_eq!(req.command, Command::Focus);

        let req = parse_line("move 0 25 1920 1150").unwrap();
        match req.command {
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
    fn text_reply_formats_ok_and_err() {
        use crate::control::text::*;
        use dock_core::layout::Rect;

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
}
