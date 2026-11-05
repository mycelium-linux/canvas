use ::canvas::DeliveryPolicy;
use ::canvas::chart::{ChartDocument, ChartKind};
use ::canvas::control::{
    CONTROL_VERSION, Command, ContentKind, ControlReply, ControlRequest, ErrorCode,
    MAX_ACTIVE_INDICATORS, MAX_CONTROL_MESSAGE, MeshSnapshot, MountedViewSnapshot, Navigation,
    Outcome, OverlaySettings, OverlayTarget, PresencePreview, PresenceSnapshot,
    ProjectWorkspaceSnapshot, Receipt, RuntimeSnapshot, ScreensaverSettings, ScreensaverSnapshot,
    StatusIndicator, SurfaceSnapshot, VisibleBackground,
};
use ::canvas::display::{
    DisplayInfo, DisplayRegion, DisplayTarget, DisplayTopology, ResolvedDisplayTarget,
};
use ::canvas::hud;
use ::canvas::interaction::{
    HitRegion, InteractionConfig, InteractionEngine, InteractionIntent, LogicalPoint, LogicalRect,
    PointerEvent, PointerPhase, ResizeHandle, SurfaceGeometry, hit_test,
};
use ::canvas::media::{
    LogicalSize, MediaCacheKey, MediaLimits, MediaPlan, MediaVariant, PixelSize, SourceDimensions,
    plan_pdf_page, plan_raster,
};
use ::canvas::view::{
    SharedViewDocument, SourcePosition, ViewNavigation, ViewOperation, ViewRect, ViewSource,
    VisualSourceKind, VisualSourceRef,
};
use ::canvas::workspace_store::{self, WorkspaceStore};
use ::canvas::{
    AssetRef, Axis, Background, BackgroundFit, DataBinding, DataSource, GradientStop, InputPort,
    InputTarget, Layout, NodeContent, NormalizedRect, PatchOperation, Placement, SCHEMA_VERSION,
    Scene, SceneNode, SemanticInk, Surface, SurfaceChrome, SurfaceContent, SurfaceTitleMode,
    ThemeRef, WorkspaceDocument, WorkspacePatch,
};
use base64::Engine as _;
use canvas_core::display_watch::{DisplayWatch, WatchStatus};
use canvas_core::global_hotkey::{GlobalHotkey, HotkeySpec};
use canvas_core::notification::Notification;
use canvas_core::{Catalog, ThemePalette, VisualTheme, desktop::WallpaperRequest, layout::Rect};
use iced::mouse;
use iced::widget::canvas::Canvas;
use iced::widget::{
    button, column, container, image as image_widget, markdown, mouse_area, row, scrollable, space,
    stack, text, tooltip,
};
use iced::{Color, ContentFit, Element, Length, Point, Rectangle, Size, Subscription, Task, Theme};
use image::{ImageReader, Limits};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufReader, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::{
    fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    net::UnixListener,
};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::{
    Arc, Mutex,
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::time::{Duration, Instant};
use unibus_media::CameraDetections;

mod default_background;
mod hid_output;
#[cfg(feature = "voice-popup")]
mod indicator;
mod keyboard_input;
mod notification;
mod presence;
use ::canvas::screensaver;
use ::canvas::stream_contract::{STREAM_FIRST_EVENT_TIMEOUT, STREAM_STALE_AFTER};
use keyboard_input::*;
mod terminal;
mod transition;
mod visual_overlays;
use visual_overlays::*;

const INITIAL_FRAME: Rect = Rect {
    x: 120,
    y: 100,
    width: 1280,
    height: 800,
};
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_IMAGE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_IMAGE_DIMENSION: u32 = 16_384;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_PDF_PAGES: u32 = 10_000;
/// Largest single-frame gap accepted by the screensaver delta-time clock.
const MAX_SCREENSAVER_FRAME_DELTA: Duration = Duration::from_millis(250);
const METRICS_REFRESH: Duration = Duration::from_secs(2);
const NOTIFICATION_LIFETIME: Duration = Duration::from_secs(9);
const MAX_VISIBLE_NOTIFICATIONS: usize = 4;

#[derive(Debug, Clone)]
struct TransientNotification {
    notification: Notification,
    shown_at: Instant,
}
const SURFACE_CONTROL_HIT_SIZE: f64 = 18.0;
const SURFACE_CONTROL_INSET: f64 = 4.0;
const SURFACE_RESIZE_HANDLE_SIZE: f64 = 10.0;
const CANVAS_TOGGLE_KEYS: &str = "Ctrl-Option-C";
const SNAP_LAYOUT_KEYS: &str = "Ctrl-Option-L";
const FIT_LAYOUT_KEYS: &str = "Ctrl-Option-F";
const JARVIS_TOGGLE_KEYS: &str = "Ctrl-Option-J";
const JARVIS_TEXT_KEYS: &str = "Ctrl-Option-T";
const JUMP_BACK_KEYS: &str = "Ctrl-O";
const JUMP_FORWARD_KEYS: &str = "Ctrl-Shift-O";
const SHORTCUT_HELP_KEYS: &str = "Ctrl-Option-?";
const FOCUSED_HELP_KEYS: &str = "?";
/// Opt-in appliance mode. A dedicated Canvas exhibit sets `CANVAS_KIOSK=1` to
/// keep the borderless fullscreen desktop; every other launch gets a normal
/// decorated window that the compositor maximizes.
const KIOSK_ENV: &str = "CANVAS_KIOSK";
/// Two presses of the desktop background inside this window count as a zoom
/// gesture on the host window (native title-bar double-click semantics).
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_SLOP: f64 = 6.0;

/// A position in the jump list: a project workspace and optionally an
/// operational view within it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JumpEntry {
    project_id: String,
    view_id: Option<String>,
}

impl JumpEntry {
    fn current(project_id: &str, view_id: Option<&str>) -> Self {
        Self {
            project_id: project_id.to_owned(),
            view_id: view_id.map(str::to_owned),
        }
    }
}

/// A bounded navigation history (vim jump-list model). Jumps are recorded on
/// project switch and operational view activation. The cursor points at the
/// current entry; back moves it toward 0, forward toward the tail.
struct JumpHistory {
    entries: Vec<JumpEntry>,
    cursor: usize,
}

impl JumpHistory {
    const MAX_DEPTH: usize = 100;

    fn new(initial: JumpEntry) -> Self {
        Self {
            entries: vec![initial],
            cursor: 0,
        }
    }

    /// Record a jump to `target`. Truncates any forward history first, as the
    /// vim jump list does: a new jump invalidates the path you would have gone
    /// forward to.
    fn jump(&mut self, target: JumpEntry) {
        let next = self.cursor + 1;
        self.entries.truncate(next);
        self.entries.push(target);
        self.cursor = next;
        if self.entries.len() > Self::MAX_DEPTH {
            self.entries.drain(0..self.entries.len() - Self::MAX_DEPTH);
            self.cursor = self
                .cursor
                .saturating_sub(self.entries.len() - Self::MAX_DEPTH);
        }
    }

    fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    fn can_go_forward(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    fn back(&mut self) -> Option<&JumpEntry> {
        if self.can_go_back() {
            self.cursor -= 1;
            self.entries.get(self.cursor)
        } else {
            None
        }
    }

    fn forward(&mut self) -> Option<&JumpEntry> {
        if self.can_go_forward() {
            self.cursor += 1;
            self.entries.get(self.cursor)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
struct Pending {
    request: ControlRequest,
    reply: SyncSender<ControlReply>,
}

type PendingReceiver = Arc<Mutex<Receiver<Pending>>>;

#[derive(Debug, Clone)]
struct LoadedContent {
    snapshot: SurfaceSnapshot,
    source: ContentSource,
    handle: iced::widget::image::Handle,
    asset: AssetRef,
    source_pixels: [u32; 2],
    media_plan: MediaPlan,
    upload_pixels: [u32; 2],
}

#[derive(Debug, Clone)]
/// What a Lua-authored widget currently draws on this wall.
///
/// Resolved off the frame path and retained, because a scene is only
/// reproducible from its inputs and the author never re-runs because a frame
/// passed: the same key must produce the same scene without touching a VM.
#[cfg(feature = "lua-scenes")]
enum ModuleSurface {
    Rendered {
        key: u64,
        scene: Scene,
        buttons: Vec<::canvas::lua_module::ModuleButton>,
        /// A rejected reload never replaces the last valid scene.
        last_error: Option<String>,
    },
    /// Why nothing is drawn, said on the surface.
    ///
    /// A module that failed to load and a widget with nothing in it are the same
    /// shape from across a room, and only one of them is a bug worth chasing.
    Failed { key: u64, reason: String },
}

#[cfg(feature = "lua-scenes")]
impl ModuleSurface {
    fn key(&self) -> u64 {
        match self {
            Self::Rendered { key, .. } | Self::Failed { key, .. } => *key,
        }
    }
}

#[derive(Debug, Clone)]
enum ContentSource {
    Image {
        _path: PathBuf,
    },
    Pdf {
        path: PathBuf,
        page_index: u32,
        page_count: u32,
    },
    Markdown {
        #[allow(dead_code)]
        path: PathBuf,
        items: Vec<markdown::Item>,
    },
    Terminal {
        #[allow(dead_code)]
        master_fd: i32,
        stdin_fd: i32,
        #[allow(dead_code)]
        pid: u32,
        buffer: String,
    },
    Scene {
        scene: Scene,
    },
    DataViz {
        chart: ChartDocument,
    },
    Stream {
        url: String,
    },
    /// Frame-replaying raster from `<runtime dir>/frames/<source_id>.jpg`.
    /// `seen` is the (mtime, len) that produced the current handle, so a tick
    /// reloads only when the bytes actually changed — never a steady timer.
    LiveImage {
        source_id: String,
        path: PathBuf,
        seen: Option<(std::time::SystemTime, u64)>,
    },
    /// A fixed-size RGBA frame produced by the transport-specific media edge.
    /// Canvas remains transport agnostic and only owns presentation.
    LiveVideo {
        source_id: String,
        path: PathBuf,
        width: u32,
        height: u32,
        orbitable: bool,
        timeline: bool,
        seen: Option<(std::time::SystemTime, u64)>,
    },
    ExternalSurface {
        source_id: String,
    },
    Widget {
        spec: canvas_widgets::WidgetSpec,
    },
    /// A line being typed on the wall, plus what has already been sent.
    ///
    /// `draft` lives here rather than in the document because a half-typed
    /// prompt is not part of the board: persisting and mirroring every
    /// keystroke would put partial words on every other screen and write the
    /// store on each character. `sent` is kept only so the surface can show
    /// that something left — the answers come back as their own surfaces.
    Prompt {
        target: String,
        placeholder: Option<String>,
        /// Pinned session, or `None` to let the target choose.
        session: Option<String>,
        draft: String,
        sent: Vec<String>,
        task_kinds: Vec<String>,
        task_targets: BTreeMap<String, String>,
        selected_kind: Option<String>,
        tasks: Vec<LocalTask>,
        active_task: Option<u64>,
    },
}

#[derive(Debug, Clone)]
struct LocalTask {
    id: u64,
    kind: String,
    prompt: String,
    state: String,
    detail: Option<String>,
    activity: Vec<String>,
}

/// Longest prompt a surface will accept.
///
/// A wall is not a text editor; this is a sentence, not a document. Bounded so
/// a key held down cannot grow the draft without limit.
const MAX_PROMPT_CHARS: usize = 400;

/// How many sent lines a prompt surface remembers.
const PROMPT_SENT_TAIL: usize = 3;

#[derive(Debug, Clone)]
struct RuntimeSurface {
    snapshot: SurfaceSnapshot,
    source: ContentSource,
    handle: iced::widget::image::Handle,
    source_pixels: [u32; 2],
    media_plan: MediaPlan,
    upload_pixels: [u32; 2],
}

#[derive(Debug, Clone)]
struct ExternalPointerCapture {
    surface_id: String,
    source_id: String,
}

/// A live frame decoded and planned off the main thread, ready to swap in.
/// JPEG decode is the expensive half of a `live_image` update; a Pi 4 wall
/// at camera rates must never pay it inside the event loop.
#[derive(Debug, Clone)]
struct PreparedFrame {
    handle: iced::widget::image::Handle,
    source_pixels: [u32; 2],
    media_plan: MediaPlan,
    upload_pixels: [u32; 2],
    sha256: Option<String>,
}

type LiveFrameResult = Result<PreparedFrame, String>;

fn canvas_media_edge_time_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

struct PresencePreviewVisual {
    value: PresencePreview,
    handle: iced::widget::image::Handle,
    started: Instant,
    last_frame: Instant,
}

type ContentLoadResult = Result<LoadedContent, Failure>;
type PageLoadResult = Result<(iced::widget::image::Handle, [u32; 2], MediaPlan), Failure>;

#[derive(Debug, Clone)]
struct LoadedBackground {
    asset: AssetRef,
    handle: iced::widget::image::Handle,
}

/// The view change a key press asks for, if any.
///
/// Modified presses are left alone: Ctrl-arrow already pages the workspace,
/// and a model surface must not steal it just because it has focus.
fn orbit_action(event: &iced::keyboard::Event) -> Option<canvas_model_viewer::OrbitAction> {
    use iced::keyboard::Event;
    let Event::KeyPressed {
        key,
        modifiers,
        text,
        ..
    } = event
    else {
        return None;
    };
    if modifiers.command() || modifiers.control() || modifiers.alt() {
        return None;
    }
    let name = match key {
        iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowLeft) => "ArrowLeft",
        iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowRight) => "ArrowRight",
        iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowUp) => "ArrowUp",
        iced::keyboard::Key::Named(iced::keyboard::key::Named::ArrowDown) => "ArrowDown",
        _ => text.as_ref().map(|text| text.as_str())?,
    };
    canvas_model_viewer::action_for_key(name)
}

fn hid_escape_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{
        Event,
        key::{Code, Physical},
    };
    matches!(
        event,
        Event::KeyPressed { physical_key: Physical::Code(Code::Escape), modifiers, .. }
            if modifiers.control() && modifiers.alt()
    )
}

/// Whether a press belongs to interactive content rather than surface layout.
///
/// Model video uses the whole visible frame for orbit, including the strip
/// that would otherwise be treated as an invisible title bar on a frameless
/// surface. Edge handles remain available for deliberate resizing.
fn content_claims_surface_drag(region: HitRegion, orbitable: bool) -> bool {
    orbitable && matches!(region, HitRegion::Body | HitRegion::TopBar)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

fn prompt_target_for_kind<'a>(
    default_target: &'a str,
    task_targets: &'a BTreeMap<String, String>,
    kind: &str,
) -> &'a str {
    task_targets
        .get(kind)
        .map(String::as_str)
        .unwrap_or(default_target)
}

/// Write a pose where the renderer, or an edge forwarding to it, will find it.
///
/// Latest-value, not a queue: the file is replaced whole. An older pose
/// arriving late would drag the camera backwards, so there is deliberately
/// nowhere for one to wait.
fn publish_model_camera(pose: &canvas_core::model_camera::ViewerCamera) -> anyhow::Result<()> {
    use anyhow::Context as _;
    let path = canvas_core::model_camera::camera_file(&pose.stream_id)
        .context("stream id cannot address a camera file")?;
    let body = serde_json::to_vec(pose).context("serialise camera pose")?;
    canvas_core::write_atomic(&path, &body).context("write camera pose")?;
    Ok(())
}

type TimelineAction = canvas_model_viewer::view::TimelineAction;

#[derive(Debug, Clone)]
enum Message {
    LauncherReply {
        generation: u64,
        target_id: String,
        activating: bool,
        result: Result<Receipt, String>,
    },
    Tick,
    #[cfg(all(unix, feature = "lua-scenes"))]
    NativeSwitcherTick,
    #[cfg(feature = "lua-scenes")]
    ModuleActivate {
        surface_id: String,
        target_id: String,
    },
    ShellBack,
    ShellForward,
    ShellProjects,
    ShellCanvasApps,
    ShellLauncherScope {
        generation: u64,
        scope: ::canvas::shell_launcher::Scope,
    },
    ShellFocusSurface(String),
    ShellDockActivate(String),
    ShellDockReply(Result<Receipt, String>),
    ShellPinToggle(String),
    ShellLauncherPick {
        generation: u64,
        revision: String,
        entry_id: String,
    },
    ShellNewTask,
    /// A gesture on a model surface. Carries the stream rather than the
    /// surface: the camera belongs to the thing being rendered, so two
    /// surfaces showing the same model turn together.
    ModelOrbit {
        stream_id: String,
        event: canvas_model_viewer::video::OrbitEvent,
    },
    /// A discrete face/corner chosen from the model's view cube.
    ModelOrbitAction {
        stream_id: String,
        action: canvas_model_viewer::OrbitAction,
    },
    ModelOrbitToggle {
        stream_id: String,
    },
    ModelTimeline {
        stream_id: String,
        action: TimelineAction,
    },
    PromptNew(String),
    PromptKindSelected {
        surface_id: String,
        kind: String,
    },
    PromptTaskSelected {
        surface_id: String,
        task_id: u64,
    },
    PdfNavigate {
        surface_id: String,
        target: Navigation,
    },
    WindowReady(iced::window::Id),
    WindowEvent(iced::window::Id, iced::window::Event),
    /// `captured` means a widget under the pointer already claimed the event.
    /// Window chrome is not a widget, so a press over a traffic light still has
    /// to reach the app even when the surface's own controls capture it — that
    /// filtering is why minimize and maximize did nothing on a live surface.
    MouseEvent(iced::window::Id, iced::mouse::Event, bool),
    RestoreSurface(String),
    BackgroundTileClicked(usize),
    LiveFrameLoaded {
        surface_id: String,
        stamp: (std::time::SystemTime, u64),
        result: Box<LiveFrameResult>,
    },
    ScaleFactorChanged(iced::window::Id, f32),
    ContentLoaded {
        pending: Box<Pending>,
        result: Box<ContentLoadResult>,
    },
    PageLoaded {
        pending: Option<Box<Pending>>,
        surface_id: String,
        page_index: u32,
        result: Box<PageLoadResult>,
    },
    BackgroundLoaded {
        pending: Box<Pending>,
        base_document_revision: u64,
        result: Result<LoadedBackground, Failure>,
    },
    WallpaperFinished {
        pending: Box<Pending>,
        result: Result<canvas_core::desktop::WallpaperReceipt, Failure>,
    },
    ScreenshotCaptured {
        pending: Box<Pending>,
        region: Option<Rect>,
        screenshot: iced::window::Screenshot,
    },
    LinkOpened(String),
    NativeAppLaunched {
        pending: Box<Pending>,
        entry_id: String,
        result: Result<(), String>,
    },
    TerminalInput(iced::keyboard::Event),
    ThemePickerSelect(usize),
    SurfaceHover(Option<String>),
}

#[derive(Debug, Clone)]
struct Failure {
    code: ErrorCode,
    message: String,
}

impl Failure {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::InvalidRequest,
            message: message.into(),
        }
    }

    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Unsupported,
            message: message.into(),
        }
    }

    fn platform(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::Platform,
            message: message.into(),
        }
    }

    fn resource(message: impl Into<String>) -> Self {
        Self {
            code: ErrorCode::ResourceLimit,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowAction {
    Close,
    Minimize,
    Maximize,
}

fn window_action_label(action: WindowAction, maximized: bool) -> &'static str {
    match action {
        WindowAction::Close => "Close",
        WindowAction::Minimize => "Minimize",
        WindowAction::Maximize if maximized => "Restore",
        WindowAction::Maximize => "Maximize",
    }
}

/// The surfaces a canvas layout presents, in layer order.
fn placement_surface_ids(layout: &Layout) -> Vec<String> {
    ::canvas::layout_tree::placements(layout)
        .into_iter()
        .map(|placement| placement.surface_id)
        .collect()
}

/// The surfaces a layout is actively showing. Parked stack members remain
/// part of the document but must not keep media decoders or external streams
/// alive while another member covers them.
fn visible_surface_ids(layout: &Layout) -> Vec<String> {
    match layout {
        Layout::Empty => Vec::new(),
        Layout::Surface { surface_id } => vec![surface_id.clone()],
        Layout::Split { first, second, .. } => {
            let mut ids = visible_surface_ids(first);
            ids.extend(visible_surface_ids(second));
            ids
        }
        Layout::Stack {
            active_surface_id, ..
        } => vec![active_surface_id.clone()],
        Layout::Canvas { placements } => placements
            .iter()
            .map(|placement| placement.surface_id.clone())
            .collect(),
    }
}

/// Resolve the one surface represented by a non-canvas layout.
///
/// Layout identity is authoritative. Focus is interaction state and can point
/// at a surface parked on another page, so it must never decide what a
/// `Surface` or `Stack` page renders.
fn single_surface_id<'a>(layout: &'a Layout, focused: Option<&'a str>) -> Option<&'a str> {
    match layout {
        Layout::Surface { surface_id } => Some(surface_id),
        Layout::Stack {
            active_surface_id, ..
        } => Some(active_surface_id),
        Layout::Split { .. } => focused,
        Layout::Empty | Layout::Canvas { .. } => None,
    }
}

/// Recover the operational-view identity already encoded in materialized
/// surface ids. This keeps restart state inferred from the document instead of
/// persisting a second view-name field that could drift from its layout.
fn inferred_snapped_view(document: &WorkspaceDocument) -> Option<String> {
    let Layout::Canvas { placements } = document.active_layout() else {
        return None;
    };
    if placements.is_empty() {
        return None;
    }
    ::canvas::operational_views::NAMES.iter().find_map(|name| {
        let prefix = format!("view-{}-", name.replace('/', "-"));
        placements
            .iter()
            .all(|placement| placement.surface_id.starts_with(&prefix))
            .then(|| (*name).to_owned())
    })
}

/// Names the widgets a frame is too small to draw legibly, for a status line.
///
/// Empty when everything fits, so it appends to a status message without one
/// needing to know whether there was anything to say.
fn undersized_note(undersized: &[String]) -> String {
    if undersized.is_empty() {
        String::new()
    } else {
        format!(
            " Too small to read at this size: {}.",
            undersized.join(", ")
        )
    }
}

fn surface_title_visible(mode: SurfaceTitleMode, focused: bool) -> bool {
    match mode {
        SurfaceTitleMode::Hidden => false,
        SurfaceTitleMode::Presentation => true,
        SurfaceTitleMode::Focused => focused,
        SurfaceTitleMode::Always => true,
    }
}

struct App {
    catalog: Catalog,
    document: WorkspaceDocument,
    runtime_revision: u64,
    /// Last list written, so an unchanged set costs nothing.
    last_wanted_video: Option<String>,
    /// Camera pose per model stream, keyed by stream rather than surface so
    /// Viewer-side state for every streamed model. The dedicated crate owns
    /// camera, orbit, timeline, throttling and view-cube semantics.
    model_viewer: canvas_model_viewer::state::Viewer,
    frame: Rect,
    display_frames: Vec<Rect>,
    displays: Vec<DisplayInfo>,
    /// The one owner of the display inventory: it takes the first enumeration
    /// and every settled one after it. `!Send`, like the `GlobalHotkey` fields
    /// below and for the same reason — the platform observer is registered on
    /// the GUI main thread and delivers there.
    display_watch: DisplayWatch<DisplayInfo>,
    selected_display_ids: Vec<String>,
    canvas_visible: bool,
    overlays: OverlaySettings,
    visibility_hotkey: Option<GlobalHotkey>,
    snap_hotkey: Option<GlobalHotkey>,
    fit_hotkey: Option<GlobalHotkey>,
    /// The active view's own answer to whether a display change reflows its
    /// arrangement. Views default to reflow; a hand-arranged workspace can
    /// declare `fixed` and keep its geometry while displays reveal parts of it.
    layout_policy: ::canvas::view::LayoutPolicy,
    /// The view whose saved layout Ctrl-Option-L snaps back to. Set whenever a
    /// view is materialized, so the key always refers to what is on screen.
    snapped_view: Option<String>,
    /// The arrangement in effect before the last snap. `Some` means we are
    /// currently showing the saved layout and a second press restores this.
    free_layout: Option<Layout>,
    jump_history: JumpHistory,
    jump_back_hotkey: Option<GlobalHotkey>,
    jump_forward_hotkey: Option<GlobalHotkey>,
    shortcuts_hotkey: Option<GlobalHotkey>,
    jarvis_hotkey: Option<GlobalHotkey>,
    jarvis_text_hotkey: Option<GlobalHotkey>,
    jarvis_text_visible: bool,
    jarvis_text: String,
    jarvis_text_reply: Option<String>,
    shortcuts_visible: bool,
    background_picker: Option<::canvas::background_picker::BackgroundPicker>,
    theme_picker: Option<::canvas::theme_picker::ThemePicker>,
    project_picker: Option<::canvas::project_picker::ProjectPicker>,
    project_switcher: Option<::canvas::shell_launcher::Launcher>,
    window_cycle: ::canvas::window_cycle::WindowCycle,
    window_cycle_workspace: Option<String>,
    dock_pins: ::canvas::shell_dock::Pins,
    dock_apps: Vec<::canvas::shell_launcher::Entry>,
    dock_icons: BTreeMap<String, iced::widget::image::Handle>,
    dock_icon_failures: BTreeSet<String>,
    dock_icon_worker: Option<
        std::sync::mpsc::Receiver<(
            Vec<String>,
            Vec<(String, Result<Option<iced::widget::image::Handle>, String>)>,
        )>,
    >,
    #[cfg(feature = "lua-scenes")]
    desktop_bar: ::canvas::shell_presentation::Presentation,
    #[cfg(all(unix, feature = "lua-scenes"))]
    native_switcher: Option<::canvas::native_switcher::Host>,
    #[cfg(feature = "lua-scenes")]
    desktop_dock: ::canvas::shell_presentation::Presentation,
    jarvis_visible: bool,
    hud_visible: bool,
    scale_factor: f64,
    receiver: PendingReceiver,
    _socket: SocketGuard,
    external_surfaces: ::canvas::external_surface_binding::ExternalSurfaceFeed,
    _external_surface_socket:
        Option<::canvas::external_surface_binding::ExternalSurfaceSocketGuard>,
    external_surface_pointer: Option<ExternalPointerCapture>,
    hid_output: hid_output::HidOutput,
    update_timings: ::canvas::performance::UpdateTimings,
    #[cfg(feature = "lua-scenes")]
    stream_inbox: ::canvas::stream_binding::StreamInbox,
    #[cfg(feature = "lua-scenes")]
    _stream_socket: Option<::canvas::stream_binding::StreamSocketGuard>,
    #[cfg(feature = "lua-scenes")]
    stream_sequences: BTreeMap<String, u64>,
    #[cfg(feature = "lua-scenes")]
    stream_started_at: Instant,
    #[cfg(feature = "lua-scenes")]
    stream_last_applied: BTreeMap<String, Instant>,
    #[cfg(feature = "lua-scenes")]
    stream_descriptors: BTreeMap<String, serde_json::Value>,
    #[cfg(feature = "lua-scenes")]
    stream_endpoints: BTreeMap<String, serde_json::Value>,
    /// Surface -> stamp of the frame currently being decoded off-thread.
    /// Blocks duplicate spawns and lets a stale arrival be discarded.
    live_pending: BTreeMap<String, (std::time::SystemTime, u64)>,
    /// Prepared live frames survive workspace switches. The disk JPEG is the
    /// producer cache; this is the decode/upload cache keyed by source id.
    live_frame_cache: BTreeMap<String, ((std::time::SystemTime, u64), PreparedFrame)>,
    camera_detections: BTreeMap<String, CameraDetections>,
    camera_detection_stamps: BTreeMap<String, (std::time::SystemTime, u64)>,
    /// This screen's position in the retained stream log. Held rather than
    /// implied by consumption, so a gap can be named instead of looking like a
    /// quiet producer.
    #[cfg(feature = "lua-scenes")]
    stream_position: u64,
    windows: BTreeMap<iced::window::Id, Rect>,
    decoded: BTreeMap<String, RuntimeSurface>,
    /// Lua-authored widgets, resolved off the frame path.
    ///
    /// Keyed by surface so an authored widget behaves like every other surface:
    /// it is placed, transitioned and themed by the document, and it is only the
    /// drawing that came from a script instead of from a Rust kind.
    #[cfg(feature = "lua-scenes")]
    modules: BTreeMap<String, ModuleSurface>,
    #[cfg(feature = "lua-scenes")]
    module_bus: ::canvas::module_bus::ModuleBus,
    /// The civil day the authored modules were resolved for.
    ///
    /// Held rather than recomputed because the point is to notice the change: a
    /// wall with a calendar on it must roll over at midnight without waiting for
    /// someone to touch the document.
    #[cfg(feature = "lua-scenes")]
    module_day: u32,
    shared_views: BTreeMap<String, SharedViewDocument>,
    indicators: BTreeMap<String, StatusIndicator>,
    notifications: Vec<TransientNotification>,
    mesh: Option<MeshSnapshot>,
    presence: Option<PresenceSnapshot>,
    presence_received: Option<Instant>,
    presence_preview: Option<PresencePreviewVisual>,
    visible_indicator_id: Option<String>,
    indicator_started: Instant,
    indicator_dismissing: Option<(StatusIndicator, Instant)>,
    voice_indicator_seen: Option<Instant>,
    metrics: hud::LocalMetrics,
    metrics_last: Instant,
    system: sysinfo::System,
    networks: sysinfo::Networks,
    status: String,
    interaction: InteractionEngine,
    snap_preview: Option<LogicalRect>,
    cursor: Option<LogicalPoint>,
    /// Offset added to raw pointer positions, so a global cursor can be turned
    /// back into window-local coordinates for hit-testing.
    cursor_offset: LogicalPoint,
    cursor_window: Option<iced::window::Id>,
    desktop_focused: bool,
    hovered_surface_id: Option<String>,
    focus_emphasis_started: Option<Instant>,
    primary_pointer_down: bool,
    desktop_press: Option<(Instant, LogicalPoint)>,
    kiosk: bool,
    minimized_surfaces: BTreeSet<String>,
    maximized_surfaces: BTreeSet<String>,
    transition: Option<transition::Transition>,
    workspaces: WorkspaceStore,
    workspace_store_path: PathBuf,
    active_project_workspace: Option<String>,
    background_assets: BTreeMap<String, iced::widget::image::Handle>,
    events: Vec<String>,
    screensaver_settings: ScreensaverSettings,
    screensaver_clock: screensaver::FrameClock,
    screensaver_index: usize,
    screensaver_piece_elapsed: Duration,
    screensaver_last_frame: Option<Instant>,
    screensaver_elapsed: Duration,
    /// A clock for the background picker's previews.
    ///
    /// Separate from `screensaver_elapsed` because that one is deliberately
    /// stopped when the workspace background is frozen — and a frozen desktop
    /// is exactly when someone opens the picker to choose a different one.
    /// Sharing it left every preview stuck at whatever instant the workspace
    /// stopped at, which reads as most of the shaders being broken.
    picker_elapsed: Duration,
    screensaver_fps: f32,
    /// Last wall-clock second used to invalidate live clock widgets. Iced's
    /// idle subscription alone does not guarantee a present when no model
    /// state changed, which left clocks visually frozen on quiet workspaces.
    last_clock_second: u64,
    last_clock_millis: u64,
    /// Content identity of runtime theme packs; changes trigger hot reload.
    theme_pack_fingerprint: String,
    theme_packs: Vec<::canvas::theme_pack::ThemePack>,
    theme_pack_ids: BTreeMap<String, String>,
    theme_pack_last_check: Instant,
    #[cfg(unix)]
    workspace_mirror: ::canvas::workspace_mirror::Publisher,
}

impl App {
    fn new(
        receiver: PendingReceiver,
        socket: SocketGuard,
        frame: Rect,
        workspace_store_path: PathBuf,
        kiosk: bool,
    ) -> Result<Self, String> {
        let base_catalog = Catalog::bundled().expect("bundled project catalog must remain valid");
        // An unusable store must not boot the wall blank in silence
        // (dock-8gr): salvage what parses, report every drop loudly, and
        // keep starting from defaults as the last resort — not the quiet one.
        let (workspaces, mut store_notes) =
            match workspace_store::load(&workspace_store_path, &base_catalog) {
                Ok(pair) => pair,
                Err(error) => {
                    eprintln!("Canvas workspace store unusable: {error}");
                    (
                        workspace_store::WorkspaceStore::default(),
                        vec![format!("workspace store unusable: {error}")],
                    )
                }
            };
        let mut catalog = workspaces
            .effective_catalog(&base_catalog)
            .map_err(|error| error.to_string())?;
        // Pull in the full Alfalfa project roster so Ctrl-P shows every project,
        // not just the 2 bundled ones. New projects get a default Coherence theme;
        // bundled projects keep their configured theme.
        Self::merge_alfalfa_projects(&mut catalog);
        let mut project_id = std::env::var("CANVAS_BOOT_PROJECT")
            .ok()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| {
                workspaces
                    .active_project_id
                    .clone()
                    .unwrap_or_else(|| catalog.projects[0].id.clone())
            });
        let mut restored_document = workspaces.get(&project_id).cloned();
        if let Some(document) = &restored_document
            && let Err(error) = document.validate(&catalog)
        {
            // The lenient tier keeps workspaces for discovered projects in the
            // store; only the one document rendering at boot must resolve its
            // theme right now, so the strict re-check happens after the
            // Alfalfa merge widened the catalog. Failing it falls back to a
            // default document — loudly, and before the first switch could
            // upsert anything over the stored one.
            store_notes.push(format!("active workspace {project_id} unusable: {error}"));
            restored_document = None;
            project_id = catalog.projects[0].id.clone();
        }
        if restored_document.is_none() && catalog.project(&project_id).is_err() {
            // Salvage may have dropped the active document entirely; a default
            // document for a project the catalog does not know would panic the
            // palette on the first frame. Land on a real project instead.
            store_notes.push(format!(
                "active project {project_id} has no usable workspace; falling back to {}",
                catalog.projects[0].id
            ));
            project_id = catalog.projects[0].id.clone();
        }
        for note in &store_notes {
            eprintln!("Canvas workspace store: {note}");
        }
        let active_project_workspace = restored_document
            .as_ref()
            .map(|document| document.project_id.clone());
        // Restore the saved display policy (display target + overlay anchors).
        // Validated against live topology; invalid entries fall back to defaults.
        let saved_policy = workspaces.display_policy();
        let saved_target = saved_policy.target.clone();
        let saved_overlays = saved_policy.overlays;
        let mut shortcut_errors = Vec::new();
        // One owner of the display inventory: the watch takes the first one and
        // every settled one after it, so nothing else enumerates and drifts.
        let display_watch = DisplayWatch::start(platform::displays);
        if let Some(error) = display_watch.last_error() {
            shortcut_errors.push(format!("display inventory unavailable: {error}"));
        }
        if let WatchStatus::Unobserved(_) = display_watch.status() {
            shortcut_errors.push(display_watch.status().to_string());
        }
        let displays = display_watch.displays().to_vec();
        let display_frames = displays.iter().map(|display| display.frame).collect();
        let display_ids: Vec<String> = displays.iter().map(|d| d.id.clone()).collect();
        let desktop_frame = match DisplayTopology::new(displays.clone())
            .and_then(|topology| topology.resolve(&saved_target))
        {
            Ok(resolved) => {
                if let Err(error) = saved_policy.validate(&display_ids) {
                    shortcut_errors.push(format!("Display policy rejected: {error}"));
                }
                resolved.frame
            }
            Err(error) => {
                shortcut_errors.push(format!("Display target unresolved: {error}"));
                DisplayTopology::new(displays.clone())
                    .and_then(|topology| topology.resolve(&DisplayTarget::All))
                    .map(|resolved| resolved.frame)
                    .unwrap_or(frame)
            }
        };
        let selected_display_ids = match DisplayTopology::new(displays.clone())
            .and_then(|topology| topology.resolve(&saved_target))
        {
            Ok(resolved) => resolved.display_ids,
            Err(_) => displays
                .iter()
                .find(|display| display.frame == frame)
                .map(|display| vec![display.id.clone()])
                .unwrap_or_default(),
        };
        #[cfg(feature = "lua-scenes")]
        let (stream_inbox, stream_socket) = if cfg!(test) {
            (::canvas::stream_binding::StreamInbox::default(), None)
        } else {
            let (inbox, guard) =
                ::canvas::stream_binding::start_server().map_err(|error| error.to_string())?;
            (inbox, Some(guard))
        };
        let (external_surfaces, external_surface_socket) = if cfg!(test) {
            (
                ::canvas::external_surface_binding::ExternalSurfaceFeed::default(),
                None,
            )
        } else {
            let (feed, guard) = ::canvas::external_surface_binding::start_server()?;
            (feed, Some(guard))
        };
        let visibility_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"Cnvs",
            id: 1,
            key_code: 0x08,                   // kVK_ANSI_C
            modifiers: (1 << 11) | (1 << 12), // optionKey | controlKey
            label: CANVAS_TOGGLE_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let fit_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"CnvF",
            id: 5,
            key_code: 0x03,                   // kVK_ANSI_F
            modifiers: (1 << 11) | (1 << 12), // optionKey | controlKey
            label: FIT_LAYOUT_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let snap_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"CnvL",
            id: 4,
            key_code: 0x25,                   // kVK_ANSI_L
            modifiers: (1 << 11) | (1 << 12), // optionKey | controlKey
            label: SNAP_LAYOUT_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let shortcuts_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"Cnvs",
            id: 2,
            key_code: 0x2c,                              // kVK_ANSI_Slash
            modifiers: (1 << 9) | (1 << 11) | (1 << 12), // shift | option | control
            label: SHORTCUT_HELP_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let jarvis_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"Cnvs",
            id: 3,
            key_code: 0x26,                   // kVK_ANSI_J
            modifiers: (1 << 11) | (1 << 12), // optionKey | controlKey
            label: JARVIS_TOGGLE_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let jarvis_text_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"Cnvs",
            id: 4,
            key_code: 0x13,                              // kVK_ANSI_T
            modifiers: (1 << 9) | (1 << 11) | (1 << 12), // shift | option | control
            label: JARVIS_TEXT_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let jump_back_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"Cnvs",
            id: 6,
            key_code: 0x1F,                   // kVK_ANSI_O
            modifiers: (1 << 11) | (1 << 12), // optionKey | controlKey
            label: JUMP_BACK_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let jump_forward_hotkey = match GlobalHotkey::register(HotkeySpec {
            signature: *b"Cnvs",
            id: 7,
            key_code: 0x1F,                              // kVK_ANSI_O
            modifiers: (1 << 9) | (1 << 11) | (1 << 12), // shift | option | control
            label: JUMP_FORWARD_KEYS,
        }) {
            Ok(hotkey) => Some(hotkey),
            Err(error) => {
                shortcut_errors.push(error);
                None
            }
        };
        let (theme_packs, theme_notes) = ::canvas::theme_pack::load();
        for note in theme_notes {
            store_notes.push(format!("theme pack unusable: {note}"));
        }
        let theme_pack_ids = workspaces
            .projects
            .keys()
            .filter_map(|project_id| {
                read_theme_pack_id(project_id).map(|id| (project_id.clone(), id))
            })
            .collect();
        let document = restored_document.unwrap_or_else(|| WorkspaceDocument {
            pages: Vec::new(),
            active_page: 0,
            schema_version: SCHEMA_VERSION,
            id: "canvas-runtime".into(),
            project_id: project_id.clone(),
            revision: 1,
            theme: ThemeRef::Project,
            background: ::canvas::Background::default(),
            chrome: SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: Layout::Empty,
            bindings: Vec::new(),
            focused_surface_id: None,
        });
        let snapped_view = inferred_snapped_view(&document);
        let layout_policy = snapped_view
            .as_deref()
            .and_then(|view_id| ::canvas::operational_views::load(view_id).ok())
            .map(|view| view.layout_policy)
            .unwrap_or_default();
        let titlebar_height =
            theme_chrome_for(&theme_packs, &theme_pack_ids, &document.project_id).titlebar_height;
        let interaction = interaction_engine(desktop_frame, document.chrome, titlebar_height);
        let mut app = Self {
            catalog,
            document,
            last_wanted_video: None,
            model_viewer: canvas_model_viewer::state::Viewer::default(),
            runtime_revision: 1,
            frame: desktop_frame,
            display_frames,
            displays,
            display_watch,
            selected_display_ids,
            canvas_visible: true,
            overlays: saved_overlays,
            visibility_hotkey,
            snap_hotkey,
            fit_hotkey,
            layout_policy,
            snapped_view,
            free_layout: None,
            jump_history: JumpHistory::new(JumpEntry::current(&project_id.clone(), None)),
            jump_back_hotkey,
            jump_forward_hotkey,
            shortcuts_hotkey,
            jarvis_hotkey,
            jarvis_text_hotkey,
            jarvis_text_visible: false,
            jarvis_text: String::new(),
            jarvis_text_reply: None,
            shortcuts_visible: false,
            background_picker: None,
            theme_picker: None,
            project_picker: None,
            project_switcher: None,
            window_cycle: Default::default(),
            window_cycle_workspace: None,
            dock_pins: ::canvas::shell_dock::Pins::load().map_err(|e| format!("Dock pins: {e}"))?,
            dock_apps: ::canvas::shell_launcher::applications(),
            dock_icons: BTreeMap::new(),
            dock_icon_failures: BTreeSet::new(),
            dock_icon_worker: None,
            #[cfg(feature = "lua-scenes")]
            desktop_bar: Default::default(),
            #[cfg(all(unix, feature = "lua-scenes"))]
            native_switcher: ::canvas::native_switcher::Host::from_environment(),
            #[cfg(feature = "lua-scenes")]
            desktop_dock: Default::default(),
            jarvis_visible: true,
            hud_visible: false,
            scale_factor: 1.0,
            receiver,
            _socket: socket,
            external_surfaces,
            _external_surface_socket: external_surface_socket,
            external_surface_pointer: None,
            hid_output: hid_output::HidOutput::default(),
            update_timings: ::canvas::performance::UpdateTimings::default(),
            #[cfg(feature = "lua-scenes")]
            stream_inbox,
            #[cfg(feature = "lua-scenes")]
            _stream_socket: stream_socket,
            #[cfg(feature = "lua-scenes")]
            stream_sequences: BTreeMap::new(),
            #[cfg(feature = "lua-scenes")]
            stream_started_at: Instant::now(),
            #[cfg(feature = "lua-scenes")]
            stream_last_applied: BTreeMap::new(),
            #[cfg(feature = "lua-scenes")]
            stream_descriptors: BTreeMap::new(),
            #[cfg(feature = "lua-scenes")]
            stream_endpoints: BTreeMap::new(),
            live_pending: BTreeMap::new(),
            live_frame_cache: BTreeMap::new(),
            camera_detections: BTreeMap::new(),
            camera_detection_stamps: BTreeMap::new(),
            #[cfg(feature = "lua-scenes")]
            stream_position: 0,
            windows: BTreeMap::new(),
            decoded: BTreeMap::new(),
            #[cfg(feature = "lua-scenes")]
            modules: BTreeMap::new(),
            #[cfg(feature = "lua-scenes")]
            module_bus: ::canvas::module_bus::ModuleBus::from_env(),
            #[cfg(feature = "lua-scenes")]
            module_day: ::canvas::hud::local_parts(now_unix_ms()).day,
            shared_views: BTreeMap::new(),
            indicators: BTreeMap::new(),
            notifications: Vec::new(),
            mesh: None,
            presence: None,
            presence_received: None,
            presence_preview: None,
            visible_indicator_id: None,
            indicator_started: Instant::now(),
            indicator_dismissing: None,
            voice_indicator_seen: None,
            metrics: hud::LocalMetrics {
                name: hostname(),
                cpu_percent: 0.0,
                cpu_cores: Vec::new(),
                ram_used: 0,
                ram_total: 0,
                net_down: 0,
                net_up: 0,
                disk_used: 0,
                disk_total: 0,
                top_processes: Vec::new(),
                uptime: Duration::from_secs(sysinfo::System::uptime()),
            },
            metrics_last: Instant::now(),
            system: sysinfo::System::new(),
            networks: sysinfo::Networks::new(),
            status: if shortcut_errors.is_empty() {
                "Canvas is ready; Ctrl+Option+C toggles its displays.".into()
            } else {
                format!(
                    "Canvas is ready; unavailable shortcut(s): {}",
                    shortcut_errors.join("; ")
                )
            },
            interaction,
            snap_preview: None,
            cursor: None,
            cursor_offset: LogicalPoint { x: 0.0, y: 0.0 },
            cursor_window: None,
            desktop_focused: false,
            hovered_surface_id: None,
            focus_emphasis_started: None,
            primary_pointer_down: false,
            desktop_press: None,
            kiosk,
            minimized_surfaces: BTreeSet::new(),
            maximized_surfaces: BTreeSet::new(),
            transition: None,
            workspaces,
            workspace_store_path,
            active_project_workspace,
            events: store_notes,
            background_assets: BTreeMap::new(),
            screensaver_settings: ScreensaverSettings::default(),
            screensaver_clock: screensaver::FrameClock::new(
                ScreensaverSettings::default().max_fps,
                Instant::now(),
            ),
            screensaver_index: 0,
            screensaver_piece_elapsed: Duration::ZERO,
            screensaver_last_frame: None,
            screensaver_elapsed: Duration::ZERO,
            picker_elapsed: Duration::ZERO,
            screensaver_fps: 0.0,
            last_clock_second: now_unix_ms() / 1_000,
            last_clock_millis: now_unix_ms(),
            theme_pack_fingerprint: ::canvas::theme_pack::fingerprint(),
            theme_packs,
            theme_pack_ids,
            theme_pack_last_check: Instant::now(),
            #[cfg(unix)]
            workspace_mirror: ::canvas::workspace_mirror::Publisher::new(),
        };
        app.publish_web_themes();
        app.publish_active_project();
        app.publish_wanted_video();
        app.ensure_scene_decoded();
        app.cache_dock_icons();
        #[cfg(feature = "lua-scenes")]
        app.refresh_desktop_bar(true);
        app.reset_background_animation();
        Ok(app)
    }

    /// Capture the presentation just before a workspace change so the view can
    /// interpolate from it. Call before mutating `document` or the theme.
    fn start_transition(&mut self) {
        let global_motion = canvas_core::appearance::current().motion;
        let pack_motion = self
            .selected_theme_pack(&self.document.project_id)
            .map(|pack| pack.motion);
        let motion = pack_motion
            .as_ref()
            .map_or(global_motion, |theme| theme.apply_to(global_motion));
        let curve = transition::Curve::from_theme(
            pack_motion
                .as_ref()
                .map_or("fluid", |theme| theme.easing.as_str()),
        );
        let from_placements = match self.document.active_layout() {
            Layout::Canvas { placements } => placements
                .iter()
                .map(|placement| {
                    let bounds = if self.maximized_surfaces.contains(&placement.surface_id) {
                        NormalizedRect {
                            x: 0.0,
                            y: 0.0,
                            width: 1.0,
                            height: 1.0,
                        }
                    } else {
                        placement.bounds
                    };
                    (
                        placement.surface_id.clone(),
                        [bounds.x, bounds.y, bounds.width, bounds.height],
                    )
                })
                .collect(),
            _ => std::collections::BTreeMap::new(),
        };
        self.transition = Some(transition::Transition::new(
            Instant::now(),
            Duration::from_millis(motion.transition_ms),
            curve,
            from_placements,
            self.project_palette(),
        ));
    }

    /// The palette the view should render: the live palette, or an
    /// interpolation toward it while a transition is in flight.
    fn presented_palette(&self) -> canvas_core::ThemePalette {
        let target = self
            .theme_picker
            .as_ref()
            .map(::canvas::theme_picker::ThemePicker::selection)
            .map(canvas_core::ThemeSelection::palette)
            .unwrap_or_else(|| self.project_palette());
        self.transition
            .as_ref()
            .map(|t| t.palette(target))
            .unwrap_or(target)
    }

    fn presented_visual_theme(&self) -> VisualTheme {
        let mut visual = self.effective_visual_theme(&self.document);
        visual.palette = self.presented_palette();
        visual
    }

    /// The normalized bounds the view should render for a surface: an
    /// interpolation from its previous bounds while a transition is in
    /// flight, otherwise the target.
    fn presented_bounds(&self, surface_id: &str, bounds: NormalizedRect) -> [f32; 4] {
        let target = [bounds.x, bounds.y, bounds.width, bounds.height];
        self.transition
            .as_ref()
            .map(|t| t.placement(surface_id, target))
            .unwrap_or(target)
    }

    fn tick_transition(&mut self) {
        if self
            .transition
            .as_ref()
            .is_some_and(transition::Transition::done)
        {
            self.transition = None;
        }
    }

    /// The rectangle the normalized layout is mapped across: the union of the
    /// displays Canvas currently occupies.
    ///
    /// Each display window is a viewport into this rectangle, so making it the
    /// selected displays rather than the whole desktop is what causes a layout
    /// to fill one monitor instead of showing a slice of a wider arrangement.
    fn occupied_frame(&self) -> Rect {
        let frames = self.selected_display_frames();
        let Some((first, rest)) = frames.split_first() else {
            return self.frame;
        };
        rest.iter().fold(*first, |acc, frame| {
            let x = acc.x.min(frame.x);
            let y = acc.y.min(frame.y);
            Rect {
                x,
                y,
                width: (acc.x + acc.width as i32)
                    .max(frame.x + frame.width as i32)
                    .saturating_sub(x) as u32,
                height: (acc.y + acc.height as i32)
                    .max(frame.y + frame.height as i32)
                    .saturating_sub(y) as u32,
            }
        })
    }

    /// Remaps the layout onto the displays Canvas is on, right now.
    ///
    /// This is the manual escape hatch for a view that declares `fixed`, and a
    /// no-op for one already filling its displays.
    fn fit_layout_to_displays(&mut self) {
        let target = self.occupied_frame();
        if target == self.frame {
            self.status = "Layout already fits the current displays.".into();
            return;
        }
        self.start_transition();
        self.frame = target;
        self.bump();
        self.status = format!(
            "Fitted layout to {}x{} across {} display(s).",
            target.width,
            target.height,
            self.selected_display_ids.len().max(1)
        );
        self.emit_event("layout_fitted".to_owned());
    }

    /// Snaps the layout back to the active view's saved arrangement, or
    /// restores what was on screen before the last snap.
    ///
    /// This is a peek, not a commit: moving surfaces around and pressing the key
    /// returns you to the saved layout, and pressing it again gives your own
    /// arrangement back. Nothing is written to the saved view, so the key can
    /// never destroy a layout someone arranged deliberately.
    fn toggle_snapped_layout(&mut self) {
        if let Some(previous) = self.free_layout.take() {
            self.start_transition();
            *self.document.active_layout_mut() = previous;
            self.bump();
            self.status = "Restored your arrangement.".into();
            self.emit_event("layout_restored".to_owned());
            return;
        }
        self.restore_snapped_layout();
    }

    fn begin_focus_emphasis(&mut self) {
        self.focus_emphasis_started = self
            .document
            .focused_surface_id
            .as_ref()
            .map(|_| Instant::now());
    }

    fn focus_emphasis(&self, surface_id: &str) -> f32 {
        if self.document.focused_surface_id.as_deref() != Some(surface_id) {
            return 0.0;
        }
        self.focus_emphasis_started
            .map(|started| focus_emphasis_curve(started.elapsed()))
            .unwrap_or(0.0)
    }

    /// Move between readable pages of the generated widget catalog.
    ///
    /// Every catalog surface remains resident; paging only replaces the six
    /// placements. That makes navigation synchronous and keeps filesystem
    /// knowledge in `canvasctl`, where the catalog is originally assembled.
    fn cycle_widget_catalog_page(&mut self, direction: i32) -> bool {
        if self.document.id != "widget-catalog-all" {
            return false;
        }
        let Layout::Canvas { placements } = self.document.active_layout() else {
            return false;
        };
        let page_size = placements.len().max(1);
        let page_count = self.document.surfaces.len().div_ceil(page_size);
        if page_count <= 1 {
            return true;
        }
        let current_first = placements
            .first()
            .and_then(|placement| {
                self.document
                    .surfaces
                    .iter()
                    .position(|surface| surface.id == placement.surface_id)
            })
            .unwrap_or(0);
        let current_page = current_first / page_size;
        let next_page = (current_page as i32 + direction).rem_euclid(page_count as i32) as usize;
        let first = next_page * page_size;
        let last = (first + page_size).min(self.document.surfaces.len());
        let ids = self.document.surfaces[first..last]
            .iter()
            .map(|surface| surface.id.clone())
            .collect::<Vec<_>>();
        let columns = if ids.len() <= 2 { ids.len() as u32 } else { 3 };
        self.start_transition();
        *self.document.active_layout_mut() = Layout::Canvas {
            placements: ::canvas::compose::grid(&ids, 1, columns),
        };
        self.document.revision = self.document.revision.saturating_add(1);
        self.free_layout = None;
        self.bump();
        self.status = format!("Widget catalog page {} / {}.", next_page + 1, page_count);
        self.emit_event(format!(
            "widget_catalog_page {} {}",
            next_page + 1,
            page_count
        ));
        true
    }

    /// Return to the active view's canonical layout without acting as a
    /// toggle. This is the button-safe form: repeated clicks are idempotent.
    fn restore_snapped_layout(&mut self) {
        let Some(view_id) = self.snapped_view.clone() else {
            self.restore_saved_workspace_layout();
            return;
        };
        match ::canvas::operational_views::materialize(&view_id, &self.document, self.view_fit()) {
            Ok(snapped) => {
                if snapped.document.layout == *self.document.active_layout() {
                    self.status = format!("Layout already matches {view_id}.");
                    return;
                }
                // Take the whole materialized document, not just its layout: the
                // saved layout references the view's surfaces, and if any were
                // closed since it was opened, a layout-only assignment would
                // reference a surface that no longer exists. materialize starts
                // from the current document, so existing surfaces are preserved.
                self.free_layout = Some(self.document.active_layout().clone());
                self.start_transition();
                self.document = snapped.document;
                self.bump();
                self.status = format!(
                    "Snapped to {view_id}.{}",
                    undersized_note(&snapped.undersized)
                );
                self.emit_event(format!("layout_snapped {view_id}"));
            }
            // A view that no longer loads must not strand the key in a state
            // where the stash is gone and the layout did not change.
            Err(error) => self.status = format!("Could not snap to {view_id}: {error}"),
        }
    }

    /// Restore the persisted per-project arrangement when the current screen
    /// is a project dashboard rather than a named operational view.
    fn restore_saved_workspace_layout(&mut self) {
        let Some(saved_layout) = self
            .workspaces
            .get(&self.document.project_id)
            .map(|saved| saved.layout.clone())
        else {
            self.status = "No saved workspace layout is available to snap to.".into();
            return;
        };
        if saved_layout == *self.document.active_layout() {
            self.status = "Layout already matches the saved workspace.".into();
            return;
        }
        let saved_ids = placement_surface_ids(&saved_layout)
            .into_iter()
            .collect::<BTreeSet<_>>();
        let current_ids = self
            .document
            .surfaces
            .iter()
            .map(|surface| surface.id.clone())
            .collect::<BTreeSet<_>>();
        if !saved_ids.is_subset(&current_ids) {
            self.status =
                "Saved workspace layout references surfaces that are no longer open.".into();
            return;
        }
        self.free_layout = Some(self.document.active_layout().clone());
        self.start_transition();
        *self.document.active_layout_mut() = saved_layout;
        self.document.revision = self.document.revision.saturating_add(1);
        self.bump();
        self.status = format!(
            "Snapped to the saved {} workspace.",
            self.document.project_id
        );
        self.emit_event(format!(
            "workspace_layout_snapped {}",
            self.document.project_id
        ));
    }

    /// Navigate to the previous entry in the jump list (Ctrl-O).
    ///
    /// Jumps are recorded on project switch and view activation. Going back
    /// moves the cursor toward the oldest entry; going forward toward the
    /// newest.
    fn jump_back(&mut self) {
        if !self.jump_history.can_go_back() {
            self.status = "No earlier location in the jump list.".into();
            return;
        }
        let entry = self.jump_history.back().expect("checked above").clone();
        let view = entry.view_id.as_deref();
        self.navigate_to(entry.project_id.as_str(), view);
        self.status = format!(
            "Jumped to '{}{}'.",
            entry.project_id,
            view.map(|v| format!(" / {v}")).unwrap_or_default()
        );
    }

    fn jump_forward(&mut self) {
        if !self.jump_history.can_go_forward() {
            self.status = "No later location in the jump list.".into();
            return;
        }
        let entry = self.jump_history.forward().expect("checked above").clone();
        let view = entry.view_id.as_deref();
        self.navigate_to(entry.project_id.as_str(), view);
        self.status = format!(
            "Jumped forward to '{}{}'.",
            entry.project_id,
            view.map(|v| format!(" / {v}")).unwrap_or_default()
        );
    }

    /// Apply a jump target: switch project workspace and optionally open a view.
    fn navigate_to(&mut self, project_id: &str, view_id: Option<&str>) {
        self.start_transition();
        if let Some(document) = self.workspaces.get(project_id).cloned() {
            self.document = document;
            self.active_project_workspace = Some(project_id.to_owned());
        }
        if let Some(view_id) = view_id
            && let Ok(opened) =
                ::canvas::operational_views::materialize(view_id, &self.document, self.view_fit())
        {
            self.snapped_view = Some(view_id.to_owned());
            self.document = opened.document;
            self.ensure_scene_decoded();
        }
        self.reset_background_animation();
        self.bump();
        self.emit_event(format!("jump_navigate {} {:?}", project_id, view_id));
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        let started = Instant::now();
        let previous_target = self.focused_hid_target();
        let task = self.update_inner(message);
        if previous_target != self.focused_hid_target() {
            if let Some(target) = previous_target {
                if let Err(error) = self.hid_output.release_if_active(&target) {
                    self.status = format!("Canvas HID release failed: {error}");
                }
            }
        }
        self.update_timings.record(started.elapsed());
        task
    }

    fn update_inner(&mut self, message: Message) -> Task<Message> {
        match message {
            #[cfg(all(unix, feature = "lua-scenes"))]
            Message::NativeSwitcherTick => {
                let visual = self.presented_visual_theme();
                let theme = ::canvas::lua_module::ModuleThemeTokens::from_visual(
                    &visual,
                    self.effective_theme(&self.document).accent(),
                );
                let Some(host) = &mut self.native_switcher else {
                    return Task::none();
                };
                if let Err(error) = host.refresh(theme) {
                    eprintln!("native switcher presentation: {error}");
                }
                if host.window.is_none() {
                    if let Some(snapshot) = &host.snapshot {
                        let mut settings = switcher_window_settings(snapshot.switcher.frame);
                        settings.transparent = true;
                        #[cfg(target_os = "linux")]
                        {
                            settings.platform_specific.application_id =
                                canvas_core::native_control::SWITCHER_APP_ID.into();
                        }
                        let (window, open) = iced::window::open(settings);
                        host.window = Some(window);
                        return open.map(|_| Message::NativeSwitcherTick);
                    }
                }
                Task::none()
            }
            Message::LauncherReply {
                generation,
                target_id,
                activating,
                result,
            } => {
                if !self.project_switcher.as_ref().is_some_and(|l| {
                    l.generation == generation && l.targets[l.target].id == target_id
                }) {
                    return Task::none();
                }
                if activating {
                    if let Some(l) = self.project_switcher.as_mut() {
                        l.busy = false;
                    }
                    match result {
                        Ok(Receipt::ShellActivated { .. }) => {
                            self.project_switcher = None;
                            self.status = "Opened on selected target.".into();
                        }
                        Ok(_) => self.status = "Unexpected launcher action receipt".into(),
                        Err(error) => {
                            self.status = error.clone();
                            if let Some(l) = self.project_switcher.as_mut() {
                                l.status = error;
                            }
                        }
                    }
                } else if let Some(l) = self.project_switcher.as_mut() {
                    let catalog = result.and_then(|r| match r {
                        Receipt::ShellCatalog { catalog } => Ok(catalog),
                        _ => Err("Target does not support launcher catalog".into()),
                    });
                    l.accept(generation, catalog, now_unix_ms());
                }
                self.refresh_launcher_scene();
                self.bump();
                Task::none()
            }
            #[cfg(feature = "lua-scenes")]
            Message::ModuleActivate {
                surface_id,
                target_id,
            } => {
                self.activate_module(&surface_id, &target_id);
                Task::none()
            }
            Message::ShellBack => {
                self.jump_back();
                Task::none()
            }
            Message::ShellForward => {
                self.jump_forward();
                Task::none()
            }
            Message::ShellProjects => self.toggle_project_switcher(),
            Message::ShellCanvasApps => {
                self.toggle_shell_launcher(::canvas::shell_launcher::Scope::Canvas)
            }
            Message::ShellLauncherScope { generation, scope } => {
                if let Some(picker) = self.project_switcher.as_mut()
                    && picker.generation == generation
                    && !picker.busy
                    && !picker.loading
                    && !picker.window_cycle
                {
                    picker.set_scope(scope);
                    self.refresh_launcher_scene();
                    self.bump();
                }
                Task::none()
            }
            Message::ShellLauncherPick {
                generation,
                revision,
                entry_id,
            } => {
                if self.window_cycle.active() {
                    if let Some(id) = entry_id.strip_prefix("surface:") {
                        let valid = self.project_switcher.as_ref().is_some_and(|picker| {
                            picker.generation == generation
                                && picker
                                    .catalog
                                    .as_ref()
                                    .is_some_and(|catalog| catalog.revision == revision)
                                && picker.matches().iter().any(|entry| entry.id == entry_id)
                        });
                        if valid
                            && self
                                .cycle_window_ids()
                                .iter()
                                .any(|candidate| candidate == id)
                        {
                            self.window_cycle.cancel();
                            self.project_switcher = None;
                            self.focus_shell_surface(id);
                        }
                    }
                    return Task::none();
                }
                if self.project_switcher.as_mut().is_some_and(|picker| {
                    picker.select_entry(generation, &revision, &entry_id, now_unix_ms())
                }) {
                    return self.activate_launcher_choice();
                }
                Task::none()
            }
            Message::ShellFocusSurface(id) => {
                if self.window_cycle.active() {
                    self.window_cycle.cancel();
                    self.project_switcher = None;
                }
                self.focus_shell_surface(&id);
                Task::none()
            }
            Message::ShellDockActivate(id) => {
                let catalog = self.launcher_catalog();
                let command = Command::ShellActivate {
                    catalog_revision: catalog.revision,
                    entry_id: id,
                };
                let target = ::canvas::shell_launcher::Target {
                    id: "local".into(),
                    label: "This screen".into(),
                    mycelium_peer: None,
                    unix_user: None,
                };
                let (send, receive) = iced::futures::channel::oneshot::channel();
                std::thread::spawn(move || {
                    let _ = send.send(::canvas::shell_launcher::request(&target, command));
                });
                Task::perform(
                    async move {
                        receive
                            .await
                            .unwrap_or_else(|_| Err("Dock launch worker stopped".into()))
                    },
                    Message::ShellDockReply,
                )
            }
            Message::ShellDockReply(result) => {
                self.status = match result {
                    Ok(Receipt::ShellActivated { .. }) => {
                        "Native application opened; capture remains separate.".into()
                    }
                    Ok(_) => "Unexpected dock action receipt".into(),
                    Err(error) => format!("Dock action failed: {error}"),
                };
                self.bump();
                Task::none()
            }
            Message::ShellPinToggle(id) => {
                let pinned = !self.dock_pins.0.contains(&id);
                let catalog = self.launcher_catalog();
                self.status = match self.set_dock_pin(&id, pinned, &catalog.entries) {
                    Ok(()) => if pinned {
                        "Application pinned to this machine's dock"
                    } else {
                        "Application unpinned"
                    }
                    .into(),
                    Err(error) => format!("Dock pin failed: {error}"),
                };
                self.bump();
                Task::none()
            }
            Message::ShellNewTask => {
                let prompt = self.decoded.iter().find_map(|(id, surface)| {
                    matches!(
                        &surface.source,
                        ContentSource::Prompt { task_kinds, .. } if !task_kinds.is_empty()
                    )
                    .then(|| id.clone())
                });
                match prompt {
                    Some(surface_id) => {
                        let destination_page = {
                            self.document.layouts().position(|layout| {
                                flatten_layout(layout)
                                    .iter()
                                    .any(|placement| placement.surface_id == surface_id)
                            })
                        };
                        if let Some(page) = destination_page {
                            self.document.active_page = page;
                        }
                        self.update(Message::PromptNew(surface_id))
                    }
                    None => {
                        self.status = "This workspace has no task creator.".into();
                        self.bump();
                        Task::none()
                    }
                }
            }
            Message::ModelOrbit { stream_id, event } => {
                self.handle_model_orbit(&stream_id, event);
                Task::none()
            }
            Message::ModelOrbitAction { stream_id, action } => {
                self.apply_orbit_action(&stream_id, action)
            }
            Message::ModelOrbitToggle { stream_id } => {
                self.toggle_model_orbit(&stream_id);
                Task::none()
            }
            Message::ModelTimeline { stream_id, action } => {
                self.apply_model_timeline(&stream_id, action)
            }
            Message::PdfNavigate { surface_id, target } => {
                self.navigate_pdf(surface_id, target, None)
            }
            Message::Tick => {
                if self.expire_voice_indicator(Instant::now()) {
                    self.bump();
                }
                // The dismiss animation comes from the gfx `voice_popup`
                // module, which is only available behind `voice-popup`. Without
                // it, drop the transient state immediately so it cannot keep
                // the animation subscription alive forever.
                #[cfg(feature = "voice-popup")]
                if self
                    .indicator_dismissing
                    .as_ref()
                    .is_some_and(|(_, since)| {
                        since.elapsed().as_secs_f32() >= fpl_gfx::voice_popup::DISMISS_SECONDS
                    })
                {
                    self.indicator_dismissing = None;
                }
                #[cfg(not(feature = "voice-popup"))]
                {
                    self.indicator_dismissing = None;
                }
                #[cfg(feature = "lua-scenes")]
                self.drain_module_bus();
                let previous_notification_count = self.notifications.len();
                self.notifications
                    .retain(|item| item.shown_at.elapsed() < NOTIFICATION_LIFETIME);
                if self.notifications.len() != previous_notification_count {
                    self.bump();
                }
                if self
                    .focus_emphasis_started
                    .is_some_and(|started| started.elapsed() >= FOCUS_EMPHASIS_DURATION)
                {
                    self.focus_emphasis_started = None;
                    self.bump();
                }
                if self.theme_pack_last_check.elapsed() >= Duration::from_secs(1) {
                    self.theme_pack_last_check = Instant::now();
                    let theme_pack_fingerprint = ::canvas::theme_pack::fingerprint();
                    if theme_pack_fingerprint != self.theme_pack_fingerprint {
                        self.theme_pack_fingerprint = theme_pack_fingerprint;
                        let (packs, notes) = ::canvas::theme_pack::load();
                        self.theme_packs = packs;
                        for note in notes {
                            self.events.push(format!("theme_pack_invalid {note}"));
                        }
                        self.publish_web_themes();
                        self.refresh_interaction_geometry();
                        self.bump();
                    }
                }
                let clock_second = now_unix_ms() / 1_000;
                if clock_second != self.last_clock_second {
                    #[cfg(feature = "lua-scenes")]
                    self.refresh_desktop_bar(true);
                    if self.project_switcher.is_some() {
                        self.refresh_launcher_scene();
                    }
                    self.last_clock_second = clock_second;
                    let has_clock = self.document.surfaces.iter().any(|surface| {
                        matches!(
                            surface.content,
                            SurfaceContent::Widget {
                                spec: canvas_widgets::WidgetSpec {
                                    kind: canvas_widgets::WidgetKind::Clock { .. },
                                    ..
                                }
                            }
                        )
                    });
                    #[cfg(feature = "lua-scenes")]
                    if !self.modules.is_empty() {
                        // One cheap digest poll per second makes authored UI
                        // iterative without putting file I/O or a Lua VM on the
                        // frame path.
                        self.refresh_modules();
                    }
                    let _ = has_clock;
                }
                let clock_millis = now_unix_ms();
                #[cfg(feature = "lua-scenes")]
                if ::canvas::hud::local_parts(clock_millis).day != self.module_day {
                    // Midnight is a document change nobody made. Without this the
                    // wall would show yesterday's calendar until someone touched it.
                    self.module_day = ::canvas::hud::local_parts(clock_millis).day;
                    self.refresh_modules();
                }
                if self.widget_animation_interval().is_some_and(|interval| {
                    clock_millis.saturating_sub(self.last_clock_millis)
                        >= interval.as_millis() as u64
                }) {
                    self.last_clock_millis = clock_millis;
                }
                let toggle_visibility = self
                    .visibility_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                let snap_layout = self
                    .snap_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                let fit_layout = self
                    .fit_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                let toggle_shortcuts = self
                    .shortcuts_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                let toggle_jarvis = self
                    .jarvis_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                let toggle_jarvis_text = self
                    .jarvis_text_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                let jump_back = self
                    .jump_back_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                let jump_forward = self
                    .jump_forward_hotkey
                    .as_ref()
                    .is_some_and(GlobalHotkey::poll_pressed);
                if self.hud_visible && self.metrics_last.elapsed() >= METRICS_REFRESH {
                    self.refresh_metrics();
                }
                self.tick_transition();
                self.refresh_screensaver(Instant::now());
                if self.presence_preview.as_ref().is_some_and(|preview| {
                    preview.last_frame.elapsed() >= Duration::from_millis(preview.value.linger_ms)
                }) {
                    self.presence_preview = None;
                    self.bump();
                }
                self.refresh_terminal();
                self.refresh_camera_detections();
                self.reconcile_external_stream_visibility();
                #[cfg(feature = "lua-scenes")]
                {
                    self.drain_stream_events();
                    self.refresh_stream_health();
                }
                let mut tasks = vec![self.drain_commands(), self.reload_live_images()];
                if !self.window_cycle.active()
                    && let Some((target, generation)) = self
                        .project_switcher
                        .as_mut()
                        .and_then(|l| l.refresh_due(now_unix_ms()))
                {
                    tasks.push(launcher_task(
                        target,
                        Command::ShellCatalog,
                        generation,
                        false,
                    ));
                }
                if let Some(settled) = self.display_watch.poll() {
                    tasks.push(self.apply_display_change(settled));
                }
                if toggle_visibility {
                    tasks.push(self.set_canvas_visible(!self.canvas_visible));
                }
                if snap_layout {
                    self.toggle_snapped_layout();
                }
                if fit_layout {
                    self.fit_layout_to_displays();
                }
                if toggle_shortcuts {
                    tasks.push(self.toggle_shortcut_help());
                }
                if toggle_jarvis {
                    self.jarvis_visible = !self.jarvis_visible;
                    self.bump();
                    self.emit_event(format!(
                        "jarvis_overlay {}",
                        if self.jarvis_visible {
                            "shown"
                        } else {
                            "hidden"
                        }
                    ));
                }
                if toggle_jarvis_text {
                    self.toggle_jarvis_text();
                }
                if jump_back {
                    self.jump_back();
                }
                if jump_forward {
                    self.jump_forward();
                }
                Task::batch(tasks)
            }
            Message::WindowReady(window) => {
                if let Err(error) = platform::make_windows_shareable(self.kiosk) {
                    self.status = format!("Canvas capture setup failed: {error}");
                    eprintln!("{}", self.status);
                }
                let frame = self.windows.get(&window).copied().unwrap_or(self.frame);
                let scale = iced::window::scale_factor(window)
                    .map(move |scale| Message::ScaleFactorChanged(window, scale));
                if self.kiosk {
                    // A kiosk exhibit owns the whole output: borderless and
                    // fullscreen, sized directly to the selected display.
                    // Native macOS fullscreen creates a Space transition that
                    // can stall this utility-style window's event loop, so fill
                    // the display directly and keep control/stream sockets
                    // responsive.
                    return iced::window::set_mode(window, iced::window::Mode::Fullscreen)
                        .chain(iced::window::move_to(
                            window,
                            Point::new(frame.x as f32, frame.y as f32),
                        ))
                        .chain(iced::window::resize(
                            window,
                            Size::new(frame.width as f32, frame.height as f32),
                        ))
                        .chain(scale);
                }
                // A decorated window keeps the compositor's zoom semantics: the
                // title-bar double-click maximizes and restores, while
                // fullscreen stays reserved for kiosk launches. Place the window
                // on its display first, then request the maximize state; a
                // resize here would fight the compositor's zoomed frame.
                iced::window::move_to(window, Point::new(frame.x as f32, frame.y as f32))
                    .chain(iced::window::maximize(window, true))
                    .chain(scale)
            }
            Message::WindowEvent(window, event) => {
                #[cfg(all(unix, feature = "lua-scenes"))]
                if self
                    .native_switcher
                    .as_ref()
                    .is_some_and(|host| host.window == Some(window))
                {
                    // Presentation-only overlay never participates in desktop focus bookkeeping.
                    if matches!(event, iced::window::Event::Closed) {
                        if let Some(host) = &mut self.native_switcher {
                            host.window = None;
                        }
                    }
                    return Task::none();
                }
                let mut metrics_changed = false;
                match event {
                    // Closing the window is a real quit on macOS. Without this
                    // the decorated titlebar's close button silently did nothing
                    // and the app looked like it had force-reopened.
                    iced::window::Event::CloseRequested if cfg!(target_os = "macos") => {
                        return iced::exit();
                    }
                    iced::window::Event::Unfocused => {
                        if self.window_cycle.active() {
                            self.window_cycle.cancel();
                            self.project_switcher = None;
                        }
                        if let Some(target_id) = self.focused_hid_target() {
                            if let Err(error) = self.hid_output.release(&target_id) {
                                self.status = format!("Canvas HID release failed: {error}");
                            }
                        }
                        self.desktop_focused = true;
                    }
                    iced::window::Event::Moved(point) => {
                        if let Some(frame) = self.windows.get_mut(&window) {
                            frame.x = point.x.round() as i32;
                            frame.y = point.y.round() as i32;
                        }
                    }
                    iced::window::Event::Resized(size) => {
                        if let Some(frame) = self.windows.get_mut(&window) {
                            frame.width = size.width.round().max(1.0) as u32;
                            frame.height = size.height.round().max(1.0) as u32;
                        }
                        metrics_changed = true;
                    }
                    iced::window::Event::Rescaled(scale) => {
                        self.scale_factor = f64::from(scale);
                        metrics_changed = true;
                    }
                    _ => {}
                }
                if metrics_changed {
                    if self.interaction.captured_pointer().is_none() {
                        self.interaction = interaction_engine(
                            self.frame,
                            self.document.chrome,
                            self.window_chrome().titlebar_height,
                        );
                    }
                    self.replan_rasters();
                }
                Task::none()
            }
            Message::MouseEvent(window, event, captured) => {
                if self.project_switcher.is_some() {
                    return Task::none();
                }
                self.handle_mouse(window, event, captured)
            }
            Message::RestoreSurface(surface_id) => {
                if self.minimized_surfaces.remove(&surface_id) {
                    self.desktop_focused = false;
                    self.start_transition();
                    let revision = self.runtime_revision;
                    self.apply_interaction_intents(vec![
                        InteractionIntent::Focus {
                            surface_id: surface_id.clone(),
                        },
                        InteractionIntent::PromoteToFront {
                            surface_id: surface_id.clone(),
                        },
                    ]);
                    if self.runtime_revision == revision {
                        self.bump();
                    }
                    self.status = format!("Restored {surface_id}.");
                    self.emit_event(format!("surface_restored {surface_id}"));
                }
                Task::none()
            }
            Message::BackgroundTileClicked(index) => {
                // Click to highlight, click again to commit — the pointer takes
                // the same two steps the keyboard does, so a stray click cannot
                // restyle the desktop. The decision is taken before the borrow
                // ends so that applying it can touch the rest of `self`.
                let commit = match self.background_picker.as_mut() {
                    Some(picker) if picker.is_selected(index) => Some(picker.background()),
                    Some(picker) => {
                        picker.select(index);
                        None
                    }
                    None => return Task::none(),
                };
                match commit {
                    Some(background) => {
                        self.background_picker = None;
                        self.apply_picked_background(background)
                    }
                    None => {
                        self.bump();
                        Task::none()
                    }
                }
            }
            Message::ScaleFactorChanged(_window, scale) => {
                self.scale_factor = f64::from(scale);
                self.replan_rasters();
                Task::none()
            }
            Message::PromptNew(surface_id) => {
                if let Some(surface) = self.decoded.get_mut(&surface_id)
                    && let ContentSource::Prompt {
                        selected_kind,
                        active_task,
                        draft,
                        ..
                    } = &mut surface.source
                {
                    *selected_kind = None;
                    *active_task = None;
                    draft.clear();
                    self.document.focused_surface_id = Some(surface_id);
                    self.bump();
                }
                Task::none()
            }
            Message::PromptKindSelected { surface_id, kind } => {
                if let Some(surface) = self.decoded.get_mut(&surface_id)
                    && let ContentSource::Prompt {
                        task_kinds,
                        selected_kind,
                        active_task,
                        draft,
                        ..
                    } = &mut surface.source
                    && task_kinds.iter().any(|candidate| candidate == &kind)
                {
                    *selected_kind = Some(kind);
                    *active_task = None;
                    draft.clear();
                    self.document.focused_surface_id = Some(surface_id);
                    self.bump();
                }
                Task::none()
            }
            Message::PromptTaskSelected {
                surface_id,
                task_id,
            } => self.select_prompt_task(&surface_id, task_id),
            Message::LinkOpened(url) => {
                self.status = match platform::open_url(&url) {
                    Ok(()) => format!("Opened link {url}."),
                    Err(error) => format!("Could not open link {url}: {error}"),
                };
                Task::none()
            }
            Message::ThemePickerSelect(index) => {
                let Some(picker) = self.theme_picker.as_mut() else {
                    return Task::none();
                };
                let selected_index = picker.selection_index();
                if index == selected_index {
                    let theme = picker.selection();
                    let pack_id = picker.selected_pack_id().map(str::to_string);
                    self.theme_picker = None;
                    self.apply_picked_theme(theme, pack_id)
                } else {
                    picker.select(index);
                    Task::none()
                }
            }
            Message::SurfaceHover(surface_id) => {
                if self.hovered_surface_id != surface_id {
                    self.hovered_surface_id = surface_id;
                    self.bump();
                }
                Task::none()
            }
            Message::TerminalInput(kbd_event) => {
                if self.handle_window_cycle(&kbd_event) {
                    return Task::none();
                }
                // Canvas-native text input to JARVIS. The text appears on the
                // wall as an overlay, not a macOS dialog.
                if self.jarvis_text_visible {
                    return self.handle_jarvis_text_input(kbd_event);
                }
                // First refusal while open: arrows and Enter mean something
                // different inside a picker than they do on the desktop, and
                // anything it does not claim still falls through below.
                if let Some(picker) = self.background_picker.as_mut() {
                    use ::canvas::background_picker::Action;
                    match picker.handle(&kbd_event) {
                        Action::Consumed => {
                            self.bump();
                            return Task::none();
                        }
                        Action::Apply => {
                            let background = picker.background();
                            self.background_picker = None;
                            return self.apply_picked_background(background);
                        }
                        Action::Cancel => {
                            self.background_picker = None;
                            self.status = "Kept the current background.".into();
                            self.bump();
                            return Task::none();
                        }
                        Action::Ignored => {}
                    }
                }
                if let Some(picker) = self.theme_picker.as_mut() {
                    use ::canvas::theme_picker::Action;
                    match picker.handle(&kbd_event) {
                        Action::Consumed => {
                            self.transition = None;
                            self.bump();
                            return Task::none();
                        }
                        Action::Apply => {
                            let theme = picker.selection();
                            let pack_id = picker.selected_pack_id().map(str::to_string);
                            self.theme_picker = None;
                            return self.apply_picked_theme(theme, pack_id);
                        }
                        Action::Cancel => {
                            self.theme_picker = None;
                            self.transition = None;
                            self.status = "Kept the current theme.".into();
                            self.bump();
                            return Task::none();
                        }
                        Action::Ignored => {}
                    }
                }
                if let Some(picker) = self.project_picker.as_mut() {
                    use ::canvas::project_picker::Action;
                    match picker.handle(&kbd_event, &self.catalog) {
                        Action::Consumed => {
                            self.bump();
                            return Task::none();
                        }
                        Action::Apply(project) => {
                            self.project_picker = None;
                            return self.switch_project(project);
                        }
                        Action::Cancel => {
                            self.project_picker = None;
                            self.status = "Kept the current project.".into();
                            self.bump();
                            return Task::none();
                        }
                        Action::Ignored => {}
                    }
                }
                if let Some(switcher) = self.project_switcher.as_mut() {
                    use ::canvas::shell_launcher::Input as SwitchAction;
                    match switcher.handle(&kbd_event) {
                        SwitchAction::Consumed => {
                            self.refresh_launcher_scene();
                            self.bump();
                            return Task::none();
                        }
                        SwitchAction::Apply => {
                            return self.activate_launcher_choice();
                        }
                        SwitchAction::Cancel => {
                            self.project_switcher = None;
                            self.status = "Kept the current project.".into();
                            self.bump();
                            return Task::none();
                        }
                        SwitchAction::Reload => {
                            let target = switcher.targets[switcher.target].clone();
                            let generation = switcher.generation;
                            self.refresh_launcher_scene();
                            self.bump();
                            return launcher_task(target, Command::ShellCatalog, generation, false);
                        }
                    }
                }
                if snap_layout_requested(&kbd_event) {
                    self.toggle_snapped_layout();
                    return Task::none();
                }
                if let Some(direction) = widget_catalog_page_direction(&kbd_event)
                    && self.cycle_widget_catalog_page(direction)
                {
                    return Task::none();
                }
                if background_picker_requested(&kbd_event) {
                    return self.toggle_background_picker();
                }
                if theme_picker_requested(&kbd_event) {
                    return self.toggle_theme_picker();
                }
                if project_picker_requested(&kbd_event) {
                    return self.toggle_project_switcher();
                }
                if switcher_requested(&kbd_event) {
                    return self.toggle_project_switcher();
                }
                if new_task_requested(&kbd_event) {
                    let prompt = self.decoded.iter().find_map(|(id, surface)| {
                        matches!(
                            &surface.source,
                            ContentSource::Prompt { task_kinds, .. } if !task_kinds.is_empty()
                        )
                        .then(|| id.clone())
                    });
                    if let Some(surface_id) = prompt {
                        self.document.active_page = 0;
                        return self.update(Message::PromptNew(surface_id));
                    }
                    self.status = "This workspace has no task creator.".into();
                    self.bump();
                    return Task::none();
                }
                if diagnostics_toggle_requested(&kbd_event) {
                    self.hud_visible = !self.hud_visible;
                    self.status = format!(
                        "Diagnostics {}.",
                        if self.hud_visible { "shown" } else { "hidden" }
                    );
                    self.emit_event(format!(
                        "diagnostics_overlay {}",
                        if self.hud_visible { "shown" } else { "hidden" }
                    ));
                    return Task::none();
                }
                if self.desktop_focused && shortcut_help_requested(&kbd_event) {
                    return self.toggle_shortcut_help();
                }
                if let Some(direction) = page_cycle_direction(&kbd_event) {
                    return self.cycle_page(direction);
                }
                if let Some(direction) = workspace_cycle_direction(&kbd_event) {
                    return self.cycle_project_workspace(direction);
                }
                if let Some(ordinal) = display_shortcut(&kbd_event) {
                    return self.toggle_display_ordinal(ordinal);
                }
                // A focused model takes the view keys. Gated on focus because
                // these are bare digits and arrows: claimed unconditionally
                // they would eat what someone is typing into the prompt on
                // the same board.
                if let Some(stream) = self.focused_model_stream()
                    && let Some(action) = orbit_action(&kbd_event)
                {
                    return self.apply_orbit_action(&stream, action);
                }
                // A focused prompt takes the keys before the terminal arm,
                // so typing lands in the prompt rather than being swallowed.
                if let Some(id) = self.document.focused_surface_id.clone() {
                    if self.decoded.get(&id).is_some_and(|surface| {
                        matches!(surface.source, ContentSource::Prompt { .. })
                    }) {
                        return self.handle_prompt_key(&id, kbd_event);
                    }
                }
                if let Some(target_id) = self.focused_hid_target() {
                    if hid_escape_requested(&kbd_event) {
                        if let Err(error) = self.hid_output.release(&target_id) {
                            self.status = format!("Canvas HID release failed: {error}");
                        } else {
                            self.desktop_focused = true;
                            self.status = "Released remote input (Ctrl+Option+Escape).".into();
                        }
                        return Task::none();
                    }
                    match self.hid_output.key(&target_id, &kbd_event) {
                        Ok(true) => return Task::none(),
                        Ok(false) => {}
                        Err(error) => {
                            self.status = format!("Canvas HID output failed: {error}");
                            return Task::none();
                        }
                    }
                }
                #[allow(clippy::collapsible_if)]
                if let Some(surface) = self
                    .document
                    .focused_surface_id
                    .clone()
                    .and_then(|id| self.decoded.get_mut(&id))
                {
                    if let ContentSource::Terminal { stdin_fd, .. } = &mut surface.source {
                        let bytes = key_to_bytes(&kbd_event);
                        if !bytes.is_empty() {
                            let _ = terminal::write_fd(*stdin_fd, &bytes);
                        }
                    }
                }
                Task::none()
            }
            Message::LiveFrameLoaded {
                surface_id,
                stamp,
                result,
            } => {
                self.apply_live_frame(&surface_id, stamp, *result);
                Task::none()
            }
            Message::ContentLoaded { pending, result } => {
                let pending = *pending;
                match *result {
                    Ok(loaded) => {
                        let id = loaded.snapshot.id.clone();
                        let title = loaded.snapshot.title.clone();
                        let shared_view = shared_view_for_loaded(&loaded);
                        let surface = Surface {
                            id: id.clone(),
                            title,
                            content: SurfaceContent::Asset {
                                asset: loaded.asset,
                            },
                            inputs: Vec::new(),
                        };
                        if let Some(existing) = self
                            .document
                            .surfaces
                            .iter_mut()
                            .find(|candidate| candidate.id == id)
                        {
                            *existing = surface;
                        } else {
                            self.document.surfaces.push(surface);
                        }
                        self.decoded.insert(
                            id.clone(),
                            RuntimeSurface {
                                snapshot: loaded.snapshot.clone(),
                                source: loaded.source,
                                handle: loaded.handle,
                                source_pixels: loaded.source_pixels,
                                media_plan: loaded.media_plan,
                                upload_pixels: loaded.upload_pixels,
                            },
                        );
                        if let Some(view) = shared_view {
                            self.shared_views.insert(id.clone(), view);
                        } else {
                            self.shared_views.remove(&id);
                        }
                        self.document.focused_surface_id = Some(id.clone());
                        self.rebuild_stack();
                        self.bump();
                        self.status = format!("Opened {}.", loaded.snapshot.title);
                        self.emit_event(format!("surface_opened {}", loaded.snapshot.id));
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::SurfaceOpened {
                                    surface: loaded.snapshot,
                                },
                            },
                        );
                    }
                    Err(failure) => self.fail(pending, failure),
                }
                Task::none()
            }
            Message::BackgroundLoaded {
                pending,
                base_document_revision,
                result,
            } => {
                let pending = *pending;
                if self.document.revision != base_document_revision {
                    self.fail(
                        pending,
                        Failure {
                            code: ErrorCode::Conflict,
                            message: format!(
                                "workspace changed while the background loaded; expected document revision {base_document_revision}, current revision is {}",
                                self.document.revision
                            ),
                        },
                    );
                    return Task::none();
                }
                match result {
                    Ok(loaded) => {
                        let sha256 = loaded.asset.sha256.clone();
                        self.background_assets.insert(sha256.clone(), loaded.handle);
                        let background = Background::Image {
                            asset: loaded.asset,
                            fit: BackgroundFit::Cover,
                        };
                        match self.apply_background(background) {
                            Ok(()) => self.answer(
                                pending,
                                Outcome::Ok {
                                    receipt: Receipt::DocumentApplied {
                                        workspace_id: self.document.id.clone(),
                                    },
                                },
                            ),
                            Err(failure) => {
                                self.background_assets.remove(&sha256);
                                self.fail(pending, failure);
                            }
                        }
                    }
                    Err(failure) => self.fail(pending, failure),
                }
                Task::none()
            }
            Message::PageLoaded {
                pending,
                surface_id,
                page_index,
                result,
            } => {
                let pending = pending.map(|pending| *pending);
                match *result {
                    Ok((handle, source_pixels, media_plan)) => {
                        let Some(surface) = self.decoded.get_mut(&surface_id) else {
                            let failure = Failure {
                                code: ErrorCode::NotFound,
                                message: format!("Canvas surface disappeared: {surface_id}"),
                            };
                            self.finish_pdf_failure(pending, failure);
                            return Task::none();
                        };
                        let ContentSource::Pdf {
                            page_index: current,
                            page_count,
                            ..
                        } = &mut surface.source
                        else {
                            self.finish_pdf_failure(
                                pending,
                                Failure::unsupported("surface is not a PDF"),
                            );
                            return Task::none();
                        };
                        *current = page_index;
                        surface.snapshot.page_index = Some(page_index);
                        surface.snapshot.source_pixels = Some(source_pixels);
                        apply_plan_diagnostics(&mut surface.snapshot, &media_plan, source_pixels);
                        surface.handle = handle;
                        surface.source_pixels = source_pixels;
                        surface.media_plan = media_plan;
                        surface.upload_pixels = source_pixels;
                        let page_count = *page_count;
                        if let Some(view) = self.shared_views.get_mut(&surface_id) {
                            view.revision = view.revision.saturating_add(1);
                            view.navigation = ViewNavigation::Page { index: page_index };
                            view.viewport = ViewRect::default();
                            view.attention = None;
                        }
                        self.document.focused_surface_id = Some(surface_id.clone());
                        self.bump();
                        self.status = format!("Page {} of {}.", page_index + 1, page_count);
                        if let Some(pending) = pending {
                            self.answer(
                                pending,
                                Outcome::Ok {
                                    receipt: Receipt::Navigated {
                                        surface_id,
                                        page_index,
                                        page_count,
                                    },
                                },
                            );
                        }
                    }
                    Err(failure) => self.finish_pdf_failure(pending, failure),
                }
                Task::none()
            }
            Message::WallpaperFinished { pending, result } => {
                let pending = *pending;
                match result {
                    Ok(receipt) => {
                        self.bump();
                        self.status = format!("Wallpaper set on {}.", receipt.display_id);
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::WallpaperSet { receipt },
                            },
                        );
                    }
                    Err(failure) => self.fail(pending, failure),
                }
                Task::none()
            }
            Message::NativeAppLaunched {
                pending,
                entry_id,
                result,
            } => {
                match result {
                    Ok(()) => self.answer(
                        *pending,
                        Outcome::Ok {
                            receipt: Receipt::ShellActivated { entry_id },
                        },
                    ),
                    Err(error) => self.fail(*pending, Failure::resource(error)),
                }
                Task::none()
            }
            Message::ScreenshotCaptured {
                pending,
                region,
                screenshot: captured,
            } => {
                let pending = *pending;
                let cropped = if let Some(region) = region {
                    let sf = captured.scale_factor;
                    let crop_region = Rectangle::<u32> {
                        x: (region.x as f32 * sf) as u32,
                        y: (region.y as f32 * sf) as u32,
                        width: (region.width as f32 * sf) as u32,
                        height: (region.height as f32 * sf) as u32,
                    };
                    match captured.crop(crop_region) {
                        Ok(cropped) => cropped,
                        Err(_) => captured.clone(),
                    }
                } else {
                    captured.clone()
                };
                let width = cropped.size.width;
                let height = cropped.size.height;
                let img = image::RgbaImage::from_raw(width, height, cropped.rgba.to_vec());
                let result = if let Some(img) = img {
                    let path = std::env::temp_dir().join(format!(
                        "canvas-screenshot-{}-{}.png",
                        std::process::id(),
                        pending.request.request_id
                    ));
                    let file = std::fs::File::create(&path);
                    if let Ok(mut file) = file {
                        let write_result = img.write_to(&mut file, image::ImageFormat::Png);
                        if write_result.is_ok() {
                            let digest = hash_file(&path).unwrap_or_else(|_| "error".to_string());
                            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                            Ok((path, digest, width, height, size))
                        } else {
                            Err("PNG encode failed")
                        }
                    } else {
                        Err("Could not create temp file")
                    }
                } else {
                    Err("Screenshot has no RGBA data")
                };
                match result {
                    Ok((path, sha256, width, height, _)) => {
                        let _ = width;
                        let _ = height;
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::ScreenshotCaptured {
                                    path,
                                    sha256,
                                    width,
                                    height,
                                },
                            },
                        );
                    }
                    Err(error) => {
                        self.fail(pending, Failure::platform(error.to_string()));
                    }
                }
                Task::none()
            }
        }
    }

    fn drain_commands(&mut self) -> Task<Message> {
        let mut pending = Vec::new();
        {
            let receiver = self.receiver.lock().expect("Canvas command queue poisoned");
            for _ in 0..16 {
                match receiver.try_recv() {
                    Ok(command) => pending.push(command),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        self.status = "Canvas control socket stopped.".into();
                        break;
                    }
                }
            }
        }
        Task::batch(pending.into_iter().map(|pending| self.execute(pending)))
    }

    #[cfg(feature = "lua-scenes")]
    fn drain_stream_events(&mut self) {
        const MAX_EVENTS_PER_FRAME: usize = 256;
        let read = self
            .stream_inbox
            .read_from(self.stream_position, MAX_EVENTS_PER_FRAME);
        self.stream_position = read.position;
        if read.missed > 0 {
            // Fail loud: this screen is now discontinuous with what it last
            // drew, and a widget left at state=ready would show a stale value
            // as though it were current.
            let missed = read.missed;
            eprintln!("Canvas missed {missed} stream events - fell behind the retained window");
            self.emit_event(format!("stream_gap missed={missed}"));
        }
        let mut events = read.events;
        if events.is_empty() {
            return;
        }
        let now = ::canvas::stream_binding::now_ms();
        self.stream_endpoints.retain(|_, announcement| {
            announcement
                .get("valid_until_unix_ms")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|valid_until| valid_until > now)
        });
        let descriptor_events = events
            .extract_if(.., |event| {
                event.contract == stream_descriptors::STREAM_DESCRIPTOR_KIND
            })
            .collect::<Vec<_>>();
        for event in descriptor_events {
            match serde_json::from_value::<stream_descriptors::StreamDescriptorV1>(event.payload) {
                Ok(descriptor) => match descriptor.validate() {
                    Ok(()) => {
                        let key = format!("{}/{}", event.owner, descriptor.stream_id);
                        let mut value = serde_json::to_value(descriptor)
                            .expect("validated stream descriptor serializes");
                        value["owner"] = serde_json::Value::String(event.owner);
                        self.stream_descriptors.insert(key, value);
                    }
                    Err(error) => {
                        eprintln!("Canvas rejected stream descriptor: {error}");
                        self.emit_event(format!("stream_descriptor_rejected error={error}"));
                    }
                },
                Err(error) => {
                    eprintln!("Canvas rejected stream descriptor: {error}");
                    self.emit_event(format!("stream_descriptor_rejected error={error}"));
                }
            }
        }
        let endpoint_events = events
            .extract_if(.., |event| {
                event.contract == stream_descriptors::ENDPOINT_ANNOUNCEMENT_KIND
            })
            .collect::<Vec<_>>();
        for event in endpoint_events {
            match serde_json::from_value::<stream_descriptors::EndpointAnnouncementV1>(
                event.payload,
            ) {
                Ok(announcement) => match announcement.validate(now) {
                    Ok(()) => {
                        let key = format!("{}/{}", event.owner, announcement.stream_id);
                        self.stream_endpoints.insert(
                            key,
                            serde_json::to_value(announcement)
                                .expect("validated endpoint announcement serializes"),
                        );
                    }
                    Err(error) => {
                        self.emit_event(format!("stream_endpoints_rejected error={error}"));
                    }
                },
                Err(error) => {
                    self.emit_event(format!("stream_endpoints_rejected error={error}"));
                }
            }
        }
        // Task lifecycle is Canvas chrome rather than a widget presentation.
        // Consume the generic status contract before Lua-bound widget events;
        // tool-specific build facts still flow through their Lua binders.
        let task_events = events
            .extract_if(.., |event| event.contract == canvas_core::task::CONTRACT)
            .collect::<Vec<_>>();
        for event in task_events {
            match serde_json::from_value::<canvas_core::task::TaskStatus>(event.payload) {
                Ok(status) => self.apply_task_status(status),
                Err(error) => {
                    eprintln!("Canvas rejected task status: {error}");
                    self.emit_event(format!("task_status_rejected error={error}"));
                }
            }
        }
        let notification_events = events
            .extract_if(.., |event| {
                event.contract == canvas_core::notification::CONTRACT
            })
            .collect::<Vec<_>>();
        for event in notification_events {
            match serde_json::from_value::<Notification>(event.payload) {
                Ok(notification) => {
                    if let Err(error) = notification.validate() {
                        eprintln!("Canvas rejected notification: {error}");
                        self.emit_event(format!("notification_rejected error={error}"));
                    } else {
                        self.present_notification(notification);
                    }
                }
                Err(error) => {
                    eprintln!("Canvas rejected notification: {error}");
                    self.emit_event(format!("notification_rejected error={error}"));
                }
            }
        }
        // Briefings are whole-canvas programs, not widget bindings. Render
        // their Lua beats through the same sandbox, then apply the resulting
        // visual patch atomically before ordinary stream bindings run.
        let briefing_events = events
            .extract_if(.., |event| {
                event.contract == ::canvas::stream_binding::BRIEFING_CONTRACT
            })
            .collect::<Vec<_>>();
        for event in briefing_events {
            match ::canvas::briefing::render_beat(&event) {
                Ok(beat) => self.apply_briefing_visuals(beat),
                Err(error) => {
                    eprintln!("Canvas rejected briefing beat: {error}");
                    self.emit_event(format!("briefing_rejected error={error}"));
                }
            }
        }
        if events.is_empty() {
            return;
        }
        let bindings = self.document.bindings.clone();
        let mut applied = 0_u64;
        let mut rejected = 0_u64;
        let mut oldest_age = 0_u64;

        for binding in bindings {
            let project_id = &self.document.project_id;
            let mut matching = events
                .iter()
                .filter(|event| {
                    // Events that carry a project_id are scoped: they only land
                    // in the workspace whose project_id matches. This prevents
                    // project X's agent activity from being drawn into project Y's
                    // surface on a shared wall.
                    if let Some(event_project) = &event.project_id
                        && *event_project != *project_id
                    {
                        return false;
                    }
                    event.owner == binding.source.owner
                        && event.stream == binding.source.stream
                        && event.contract == binding.source.contract
                })
                .collect::<Vec<_>>();
            if matching.is_empty() {
                continue;
            }
            match binding.delivery {
                DeliveryPolicy::Latest => {
                    if let Some(latest) =
                        matching.iter().copied().max_by_key(|event| event.sequence)
                    {
                        matching = vec![latest];
                    }
                }
                DeliveryPolicy::Ordered {
                    max_batch,
                    max_age_ms,
                } => {
                    matching.retain(|event| event.age_ms(now) <= u64::from(max_age_ms));
                    matching.sort_by_key(|event| event.sequence);
                    let excess = matching.len().saturating_sub(usize::from(max_batch));
                    matching.drain(..excess);
                }
            }

            let marked = self.stream_sequences.get(&binding.id).copied().unwrap_or(0);
            let newest = matching
                .iter()
                .map(|event| event.sequence)
                .max()
                .unwrap_or_default();
            let previous = effective_stream_watermark(marked, newest);
            if previous != marked {
                eprintln!(
                    "Canvas binding {} restarted upstream at sequence {newest} (was {marked})",
                    binding.id
                );
            }
            matching.retain(|event| event.sequence > previous);
            if matching.is_empty() {
                continue;
            }
            let Some(adapter) = binding.adapter.as_deref() else {
                rejected = rejected.saturating_add(matching.len() as u64);
                eprintln!("Canvas binding {} has no stream adapter", binding.id);
                continue;
            };
            let Some(surface) = self.decoded.get_mut(&binding.target.surface_id) else {
                rejected = rejected.saturating_add(matching.len() as u64);
                continue;
            };
            let result = match &mut surface.source {
                ContentSource::Widget { spec } => {
                    ::canvas::stream_binding::apply_named_adapter_batch(adapter, &matching, spec)
                }
                ContentSource::LiveVideo { source_id, .. } => {
                    let last = matching.last().expect("non-empty stream batch");
                    ::canvas::stream_binding::apply_named_timeline_adapter(adapter, last).map(
                        |mut timeline| {
                            // The surface remains authoritative for routing; a
                            // binder cannot redirect controls to another model.
                            timeline.stream_id.clone_from(source_id);
                            self.model_viewer.set_timeline(timeline);
                        },
                    )
                }
                _ => Err(::canvas::Error(format!(
                    "binding {} targets unsupported non-widget surface",
                    binding.id
                ))),
            };
            match result {
                Ok(()) => {
                    let last = matching.last().expect("non-empty stream batch");
                    self.stream_sequences
                        .insert(binding.id.clone(), last.sequence);
                    self.stream_last_applied
                        .insert(binding.id.clone(), Instant::now());
                    applied = applied.saturating_add(matching.len() as u64);
                    oldest_age = oldest_age.max(
                        matching
                            .iter()
                            .map(|event| event.age_ms(now))
                            .max()
                            .unwrap_or(0),
                    );
                }
                Err(error) => {
                    rejected = rejected.saturating_add(matching.len() as u64);
                    eprintln!("Canvas rejected {} batch: {error}", binding.id);
                    if let ContentSource::Widget { spec } = &mut surface.source {
                        spec.data.state = canvas_widgets::WidgetDataState::Error;
                        spec.data.detail = Some(format!("Binder rejected payload: {error}"));
                    }
                }
            }
        }

        if applied > 0 || rejected > 0 {
            self.bump();
            let stats = self.stream_inbox.stats();
            self.emit_event(format!(
                "stream_tick applied={applied} rejected={rejected} age_ms={oldest_age} \
                 position={} retained={} evicted={}",
                self.stream_position, stats.retained, stats.evicted
            ));
        }
    }

    #[cfg(feature = "lua-scenes")]
    fn apply_task_status(&mut self, status: canvas_core::task::TaskStatus) {
        let Some(id) = status
            .task_id
            .strip_prefix("local-")
            .and_then(|id| id.parse::<u64>().ok())
        else {
            self.emit_event(format!("task_status_unknown id={}", status.task_id));
            return;
        };
        let mut applied = false;
        for surface in self.decoded.values_mut() {
            let ContentSource::Prompt { tasks, .. } = &mut surface.source else {
                continue;
            };
            if let Some(task) = tasks.iter_mut().find(|task| task.id == id) {
                task.state = status.state.clone();
                task.detail = status.detail.clone().or_else(|| {
                    status
                        .session
                        .as_deref()
                        .map(|session| format!("Delivered to {} · {session}", status.target))
                });
                task.activity = status.activity.clone();
                applied = true;
            }
        }
        if applied {
            self.bump();
            self.emit_event(format!(
                "task_status id={} state={}",
                status.task_id, status.state
            ));
        }
    }

    #[cfg(feature = "lua-scenes")]
    fn refresh_stream_health(&mut self) {
        let mut changed = false;
        for binding in &self.document.bindings {
            let Some(surface) = self.decoded.get_mut(&binding.target.surface_id) else {
                continue;
            };
            let ContentSource::Widget { spec } = &mut surface.source else {
                continue;
            };
            let last_age = self
                .stream_last_applied
                .get(&binding.id)
                .map(Instant::elapsed);
            match stream_health_transition(
                spec.data.state,
                self.stream_started_at.elapsed(),
                last_age,
            ) {
                Some(canvas_widgets::WidgetDataState::Stale) => {
                    let age = last_age.expect("stale transition requires an observation");
                    spec.data.state = canvas_widgets::WidgetDataState::Stale;
                    spec.data.detail = Some(format!(
                        "{} stream has not updated for {}s",
                        binding.source.owner,
                        age.as_secs()
                    ));
                    changed = true;
                }
                Some(canvas_widgets::WidgetDataState::Disconnected) => {
                    spec.data.state = canvas_widgets::WidgetDataState::Disconnected;
                    spec.data.detail = Some(format!(
                        "No first event from {}/{}",
                        binding.source.owner, binding.source.stream
                    ));
                    changed = true;
                }
                _ => {}
            }
        }
        if changed {
            self.bump();
        }
    }

    #[cfg(feature = "lua-scenes")]
    fn apply_briefing_visuals(&mut self, beat: canvas_widgets::BriefingBeat) {
        use canvas_widgets::PatchOp as BriefingPatch;

        let mut operations = Vec::new();
        for patch in beat.pre_actions.iter().chain(&beat.patches) {
            if matches!(patch, BriefingPatch::SetMeshNode { .. }) {
                continue;
            }
            let operation = serde_json::to_value(patch)
                .map_err(|error| error.to_string())
                .and_then(|value| {
                    serde_json::from_value::<PatchOperation>(value)
                        .map_err(|error| error.to_string())
                });
            match operation {
                Ok(operation) => operations.push(operation),
                Err(error) => {
                    eprintln!("Canvas rejected briefing visual patch: {error}");
                    self.emit_event(format!("briefing_patch_rejected error={error}"));
                    return;
                }
            }
        }
        if operations.is_empty() {
            return;
        }
        let patch = WorkspacePatch {
            schema_version: SCHEMA_VERSION,
            workspace_id: self.document.id.clone(),
            base_revision: self.document.revision,
            revision: self.document.revision.saturating_add(1),
            operations,
        };
        match self.document.apply_patch(&patch, &self.catalog) {
            Ok(document) => {
                let background_changed = self.document.background != document.background;
                self.start_transition();
                self.document = document;
                self.prune_decoded();
                self.ensure_scene_decoded();
                if background_changed {
                    self.reset_background_animation();
                }
                self.bump();
                self.status = beat
                    .message
                    .unwrap_or_else(|| format!("Briefing beat {}.", beat.beat));
                self.emit_event(format!(
                    "briefing_beat beat={} done={}",
                    beat.beat, beat.done
                ));
            }
            Err(error) => {
                eprintln!("Canvas rejected briefing workspace patch: {error}");
                self.emit_event(format!("briefing_patch_rejected error={error}"));
            }
        }
    }

    fn replace_host_windows(&mut self, frames: impl IntoIterator<Item = Rect>) -> Task<Message> {
        let mut tasks = self
            .windows
            .keys()
            .copied()
            .map(iced::window::close)
            .collect::<Vec<Task<Message>>>();
        self.windows.clear();
        for frame in frames {
            let (window, open) = iced::window::open(canvas_window_settings(frame, self.kiosk));
            self.windows.insert(window, frame);
            tasks.push(open.map(Message::WindowReady));
        }
        Task::batch(tasks)
    }

    fn selected_display_frames(&self) -> Vec<Rect> {
        let frames = self
            .selected_display_ids
            .iter()
            .filter_map(|id| self.displays.iter().find(|display| display.id == *id))
            .map(|display| display.frame)
            .collect::<Vec<_>>();
        if frames.is_empty() {
            vec![self.frame]
        } else {
            frames
        }
    }

    fn apply_display_target(
        &mut self,
        target: &DisplayTarget,
    ) -> Result<(ResolvedDisplayTarget, Task<Message>), Failure> {
        let topology = DisplayTopology::new(self.displays.clone()).map_err(Failure::platform)?;
        let resolved = topology.resolve(target).map_err(Failure::invalid)?;
        let frames = resolved
            .display_ids
            .iter()
            .filter_map(|id| topology.get(id).map(|display| display.frame))
            .collect::<Vec<_>>();
        self.selected_display_ids = resolved.display_ids.clone();
        // Persist the resolved display policy so it survives restart.
        self.workspaces
            .set_display_policy(workspace_store::DisplayPolicy {
                target: target.clone(),
                overlays: self.overlays,
            });
        let _ = workspace_store::save(&self.workspace_store_path, &self.workspaces, &self.catalog);
        // A reflow view fills whatever Canvas now occupies; a fixed one keeps
        // its geometry on the whole desktop and lets displays reveal parts of
        // it. Without this the frame stays the full desktop and selecting one
        // display shows a slice of a wider arrangement rather than fitting it.
        if self.layout_policy == ::canvas::view::LayoutPolicy::Reflow {
            self.frame = resolved.frame;
        }
        self.bump();
        self.status = format!(
            "Canvas spans {} display(s) at {},{} {}x{}.",
            resolved.display_ids.len(),
            resolved.frame.x,
            resolved.frame.y,
            resolved.frame.width,
            resolved.frame.height
        );
        let task = if self.canvas_visible {
            self.replace_host_windows(frames)
        } else {
            Task::none()
        };
        Ok((resolved, task))
    }

    /// React to a settled change in the display topology.
    ///
    /// The watch has already coalesced the burst and re-enumerated, so this is
    /// the whole of Canvas's answer to a monitor being plugged in, woken, or
    /// switched off — including the case that used to be terminal, where Canvas
    /// started before any display answered and stayed blank until a restart.
    fn apply_display_change(&mut self, settled: Result<Vec<DisplayInfo>, String>) -> Task<Message> {
        let displays = match settled {
            Ok(displays) => displays,
            Err(error) => {
                self.status = format!("Display inventory failed: {error}");
                return Task::none();
            }
        };
        // The fit the active view was last laid out against. Captured before the
        // frame moves, so an arrangement someone changed by hand can be told
        // from one that is still the view's own.
        let previous_fit = self.view_fit();
        self.displays = displays;
        self.display_frames = self.displays.iter().map(|display| display.frame).collect();
        // Keep the displays the operator had selected where they survived the
        // change; a selection that is entirely gone — including the startup case
        // where there were none — falls back to the whole desktop.
        let surviving = self
            .selected_display_ids
            .iter()
            .filter(|id| self.displays.iter().any(|display| display.id == **id))
            .cloned()
            .collect::<Vec<_>>();
        let target = if surviving.is_empty() {
            DisplayTarget::All
        } else {
            DisplayTarget::Exact {
                display_ids: surviving,
            }
        };
        // apply_display_target sets selected_display_ids and (for a reflow view)
        // self.frame, bumps the revision, and reopens the host windows on the
        // new frames, which is what makes a Canvas that started with displays=0
        // finally draw.
        match self.apply_display_target(&target) {
            Ok((_, task)) => {
                self.emit_event(format!("displays_changed {}", self.displays.len()));
                self.refit_active_view(previous_fit);
                task
            }
            Err(failure) => {
                self.status = format!(
                    "Displays changed but the layout could not follow: {}",
                    failure.message
                );
                Task::none()
            }
        }
    }

    /// Recompute the active view against the room Canvas now has.
    ///
    /// `apply_display_target` has already applied the view's own layout policy —
    /// a reflow view's frame followed the displays, a fixed one's deliberately
    /// did not — and normalized geometry rides along with the frame. What it
    /// cannot do is re-run the readable-minimum fit, because minimums are
    /// logical pixels: unchanged proportions on a smaller frame still put
    /// widgets under them. So a reflow view comes out of here fitted to its new
    /// frame, and a fixed view comes out untouched, which is what fixed means.
    fn refit_active_view(&mut self, previous: ::canvas::operational_views::Fit) {
        let Some(view_id) = self.snapped_view.clone() else {
            return;
        };
        // Only a view still showing its own arrangement is recomputed. Someone
        // who moved surfaces since it opened keeps what they arranged: a monitor
        // waking up is not a reason to undo their work.
        let untouched =
            ::canvas::operational_views::materialize(&view_id, &self.document, previous)
                .is_ok_and(|before| before.document.layout == *self.document.active_layout());
        if !untouched {
            return;
        }
        match ::canvas::operational_views::materialize(&view_id, &self.document, self.view_fit()) {
            Ok(fitted) => {
                if fitted.document == self.document {
                    return;
                }
                let surface_ids = placement_surface_ids(fitted.document.active_layout());
                self.start_transition();
                self.document = fitted.document;
                for surface_id in &surface_ids {
                    self.decoded.remove(surface_id);
                }
                self.ensure_scene_decoded();
                // A stashed arrangement was drawn for displays that may no
                // longer exist, so snapping back to it would restore geometry
                // for a topology that is gone.
                self.free_layout = None;
                // A decoded video frame is presentation state, not a document
                // mutation. Advancing the optimistic workspace revision here
                // made saved layouts race 24 fps video and analog-clock ticks.
                // The Tick update already schedules a redraw after this handle
                // swap; reserve `runtime_revision` for control-plane changes.
                self.status = format!(
                    "Re-fitted {view_id} to {} display(s).{}",
                    self.selected_display_ids.len().max(1),
                    undersized_note(&fitted.undersized)
                );
                self.emit_event(format!("operational_view_refitted {view_id}"));
            }
            Err(error) => {
                self.status =
                    format!("Displays changed but {view_id} could not be re-fitted: {error}");
            }
        }
    }

    /// The room a view has to lay itself out in right now: the frame Canvas
    /// currently occupies, at the type scale it is drawn in.
    fn view_fit(&self) -> ::canvas::operational_views::Fit {
        ::canvas::operational_views::Fit {
            width: self.frame.width as f32,
            height: self.frame.height as f32,
            metrics: self.presented_visual_theme().metrics,
        }
    }

    fn toggle_display_ordinal(&mut self, ordinal: usize) -> Task<Message> {
        let target = if ordinal == 0 {
            DisplayTarget::All
        } else {
            let mut ordered = self.displays.iter().collect::<Vec<_>>();
            ordered.sort_by_key(|display| (display.frame.x, display.frame.y));
            let Some(display) = ordered.get(ordinal - 1) else {
                self.status = format!("Display {ordinal} is not connected.");
                return Task::none();
            };
            let mut ids = self.selected_display_ids.clone();
            if let Some(index) = ids.iter().position(|id| id == &display.id) {
                if ids.len() == 1 {
                    self.status = "Canvas must remain on at least one display; use the global Canvas toggle to hide all.".into();
                    return Task::none();
                }
                ids.remove(index);
            } else {
                ids.push(display.id.clone());
            }
            DisplayTarget::Exact { display_ids: ids }
        };
        match self.apply_display_target(&target) {
            Ok((_resolved, task)) => task,
            Err(failure) => {
                self.status = format!("Display selection rejected: {}", failure.message);
                Task::none()
            }
        }
    }

    fn place_surface_on_display(
        &mut self,
        surface_id: &str,
        display_ordinal: usize,
        region: DisplayRegion,
        margin: u16,
    ) -> Result<(String, NormalizedRect, Task<Message>), Failure> {
        if !self
            .document
            .surfaces
            .iter()
            .any(|surface| surface.id == surface_id)
        {
            return Err(Failure {
                code: ErrorCode::NotFound,
                message: format!("unknown Canvas surface: {surface_id}"),
            });
        }
        let mut ordered = self.displays.clone();
        ordered.sort_by_key(|display| (display.frame.x, display.frame.y));
        let display = ordered
            .get(display_ordinal.saturating_sub(1))
            .ok_or_else(|| Failure::invalid(format!("display {display_ordinal} is not connected")))?
            .clone();
        let bounds = region
            .normalized_bounds(display.frame, self.frame, margin)
            .map_err(Failure::invalid)?;
        let mut task = Task::none();
        if !self.selected_display_ids.contains(&display.id) {
            let mut ids = self.selected_display_ids.clone();
            ids.push(display.id.clone());
            let (_, display_task) =
                self.apply_display_target(&DisplayTarget::Exact { display_ids: ids })?;
            task = display_task;
        }
        let mut placements = flatten_layout(self.document.active_layout());
        let next_layer = placements
            .iter()
            .map(|placement| placement.layer)
            .max()
            .unwrap_or(-1)
            .saturating_add(1);
        if let Some(placement) = placements
            .iter_mut()
            .find(|placement| placement.surface_id == surface_id)
        {
            placement.bounds = bounds;
            placement.layer = next_layer;
        } else {
            placements.push(Placement {
                surface_id: surface_id.into(),
                bounds,
                layer: next_layer,
            });
        }
        self.start_transition();
        *self.document.active_layout_mut() = Layout::Canvas { placements };
        self.document.focused_surface_id = Some(surface_id.into());
        self.document.revision = self.document.revision.saturating_add(1);
        self.bump();
        self.status = format!("Placed {surface_id} on display {display_ordinal} ({region:?}).");
        self.emit_event(format!(
            "surface_placed {surface_id} display={} region={region:?}",
            display.id
        ));
        Ok((display.id, bounds, task))
    }

    /// Type into the focused prompt surface.
    ///
    /// Handled by hand rather than with an iced text input because Canvas
    /// already routes keys to the focused surface itself (that is how terminals
    /// receive input), and a second, widget-owned focus would compete with it.
    fn handle_prompt_key(
        &mut self,
        surface_id: &str,
        event: iced::keyboard::Event,
    ) -> Task<Message> {
        use iced::keyboard::{Event, Key, key::Named};
        let Event::KeyPressed { key, text, .. } = event else {
            return Task::none();
        };
        let Some(surface) = self.decoded.get_mut(surface_id) else {
            return Task::none();
        };
        let ContentSource::Prompt {
            target,
            session,
            draft,
            sent,
            task_kinds,
            task_targets,
            selected_kind,
            tasks,
            active_task,
            ..
        } = &mut surface.source
        else {
            return Task::none();
        };

        if !task_kinds.is_empty() && selected_kind.is_none() {
            return Task::none();
        }

        match key {
            Key::Named(Named::Backspace) => {
                draft.pop();
            }
            Key::Named(Named::Escape) => draft.clear(),
            Key::Named(Named::Enter) => {
                let line = draft.trim().to_string();
                if line.is_empty() {
                    return Task::none();
                }
                let session = session.clone();
                draft.clear();
                sent.push(line.clone());
                let kind = selected_kind.take().unwrap_or_else(|| "Prompt".to_string());
                let target = prompt_target_for_kind(target, task_targets, &kind).to_string();
                let task_id = unix_ms();
                tasks.push(LocalTask {
                    id: task_id,
                    kind: kind.clone(),
                    prompt: line.clone(),
                    state: "queued".into(),
                    detail: Some("Waiting for the local delivery bridge".into()),
                    activity: Vec::new(),
                });
                *active_task = Some(task_id);
                // Only the tail is kept: this is a receipt that something was
                // sent, not a transcript. Answers arrive as their own surfaces.
                while sent.len() > PROMPT_SENT_TAIL {
                    sent.remove(0);
                }
                return self.submit_prompt(&target, session.as_deref(), &line, &kind, task_id);
            }
            Key::Named(Named::Space) => draft.push(' '),
            _ => {
                // `text` is what the layout actually produced, so punctuation
                // and non-US layouts arrive correctly instead of being
                // reconstructed from the keycode.
                if let Some(text) = text {
                    if draft.chars().count() < MAX_PROMPT_CHARS {
                        draft.extend(text.chars().filter(|character| !character.is_control()));
                    }
                }
            }
        }
        Task::none()
    }

    /// Hand a finished prompt to the bridge.
    ///
    /// Canvas has no bus client — its outbound idiom is an atomic write under
    /// the runtime root — so the prompt is spooled as one file per submission
    /// and `canvas-edge`, which already owns the canvas/bus boundary, publishes
    /// and removes it. Keeping the wall off the bus is what stops a display
    /// from becoming a client of every tool that might answer.
    fn submit_prompt(
        &mut self,
        target: &str,
        session: Option<&str>,
        line: &str,
        task_kind: &str,
        task_id: u64,
    ) -> Task<Message> {
        let Some(root) = ::canvas::runtime_config::root() else {
            self.status = "No runtime directory: the prompt was not sent.".into();
            return Task::none();
        };
        let directory = root.join("prompts");
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.status = format!("Could not spool the prompt: {error}");
            self.emit_event(format!("prompt_spool_failed {error}"));
            return Task::none();
        }
        // Sequence first so the bridge drains in submission order, and the
        // runtime revision disambiguates two submissions in the same
        // millisecond.
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_millis() as u64)
            .unwrap_or_default();
        let name = format!("{at_ms:013}-{:06}-{target}.json", self.runtime_revision);
        let mut body = serde_json::json!({
            "schema_version": 1,
            "target": target,
            "text": line,
            "at_unix_ms": at_ms,
            "project_id": self.document.project_id,
            "task": {
                "id": format!("local-{task_id}"),
                "kind": task_kind,
                "state": "submitted"
            },
        });
        // Absent rather than null when unpinned: the tool then applies its own
        // notion of "current", which is the whole point of not pinning.
        if let Some(session) = session {
            body["session"] = serde_json::Value::String(session.to_string());
        }
        match canvas_core::write_atomic(&directory.join(&name), body.to_string().as_bytes()) {
            Ok(()) => {
                self.status = format!("Sent to {target}.");
                self.emit_event(format!("prompt_sent {target}"));
            }
            Err(error) => {
                // Loud: the line is gone from the draft, so silence here would
                // read as sent.
                self.status = format!("Could not send to {target}: {error}");
                self.emit_event(format!("prompt_spool_failed {error}"));
            }
        }
        Task::none()
    }

    /// Move to another page of the same workspace.
    ///
    /// Deliberately not a workspace switch: the surface pool is untouched, so
    /// every parked stream keeps decoding and the board is simply drawn from a
    /// different arrangement. That is the point of pages — a second screenful
    /// that costs nothing to reach.
    fn cycle_page(&mut self, direction: i32) -> Task<Message> {
        if self.document.page_count() < 2 {
            self.status = "This workspace has one page.".into();
            return Task::none();
        }
        let index = self.document.step_page(direction);
        let name = self.document.active_page_name().to_string();
        self.status = format!(
            "Page {} of {} — {name}",
            index + 1,
            self.document.page_count()
        );
        self.start_transition();
        // Surfaces are shared across pages, so nothing is decoded or dropped
        // here. Anything the incoming page shows that has never been drawn
        // decodes on demand, exactly as it would on first paint.
        self.ensure_scene_decoded();
        // The visible set changed even though the document did not, so the
        // media edge needs telling or it keeps feeding the page you left.
        self.publish_wanted_video();
        self.bump();
        self.emit_event(format!("page_switched {index}"));
        Task::none()
    }

    /// Open a task receipt wherever its owning prompt surface currently lives.
    ///
    /// The global task strip is visible across every workspace page, while a
    /// prompt surface may be parked on another page or minimized. Selecting a
    /// receipt therefore navigates first and selects second; merely changing
    /// `active_task` would leave the user looking at an unchanged page.
    fn select_prompt_task(&mut self, surface_id: &str, task_id: u64) -> Task<Message> {
        let owns_task = self.decoded.get(surface_id).is_some_and(|surface| {
            matches!(
                &surface.source,
                ContentSource::Prompt { tasks, .. }
                    if tasks.iter().any(|task| task.id == task_id)
            )
        });
        if !owns_task {
            return Task::none();
        }

        let destination_page = self.document.layouts().position(|layout| {
            flatten_layout(layout)
                .iter()
                .any(|placement| placement.surface_id == surface_id)
        });
        let page_changed =
            destination_page.is_some_and(|page| page != self.document.active_page_index());
        if page_changed {
            self.start_transition();
            self.document.active_page = destination_page.expect("page was present");
        }

        let Some(surface) = self.decoded.get_mut(surface_id) else {
            return Task::none();
        };
        let ContentSource::Prompt {
            active_task,
            selected_kind,
            ..
        } = &mut surface.source
        else {
            return Task::none();
        };
        *active_task = Some(task_id);
        *selected_kind = None;
        self.minimized_surfaces.remove(surface_id);
        self.document.focused_surface_id = Some(surface_id.to_string());

        if page_changed {
            self.ensure_scene_decoded();
            self.publish_wanted_video();
        }
        self.status = format!("Opened task on {}.", self.document.active_page_name());
        self.bump();
        self.emit_event(format!("task_opened {surface_id} {task_id}"));
        Task::none()
    }

    fn cycle_project_workspace(&mut self, direction: i32) -> Task<Message> {
        let projects = self
            .workspaces
            .project_ids()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if projects.len() < 2 {
            self.status = "No other project workspace is available.".into();
            return Task::none();
        }
        let current = projects
            .iter()
            .position(|project| project == &self.document.project_id)
            .unwrap_or(0) as i32;
        let next = (current + direction).rem_euclid(projects.len() as i32) as usize;
        self.switch_project(projects[next].clone())
    }

    fn switch_project(&mut self, project: String) -> Task<Message> {
        // If a saved workspace exists for this project, switch to it.
        // If not (e.g. a newly discovered Alfalfa project), create a new
        // workspace document with that project's theme and apply it.
        // This makes Ctrl-P → Enter work for all projects without prior setup.
        if self.workspaces.get(&project).is_some() {
            let (reply, _replies) = mpsc::sync_channel(1);
            self.execute(Pending {
                request: ControlRequest {
                    schema_version: CONTROL_VERSION,
                    request_id: "key-workspace".into(),
                    expected_revision: Some(self.runtime_revision),
                    actor: None,
                    command: Command::WorkspaceSwitch { name: project },
                },
                reply,
            })
        } else {
            let project_id = project.clone();
            // First visit to this project: create a workspace with a default
            // repo-status table surface so the user sees live data immediately.
            let document = WorkspaceDocument {
                pages: Vec::new(),
                active_page: 0,
                schema_version: SCHEMA_VERSION,
                // Store keys may carry Alfalfa's capitalization; document ids
                // follow the lowercase identifier grammar.
                id: format!("{}-active", project_id.to_ascii_lowercase()),
                project_id: project_id.clone(),
                revision: 1,
                theme: ThemeRef::Project,
                background: Background::Theme,
                chrome: SurfaceChrome::default(),
                surfaces: vec![Surface {
                    id: "repo-status".into(),
                    title: format!("{}: Active Work", project_id),
                    content: SurfaceContent::Widget {
                        spec: canvas_widgets::WidgetSpec {
                            schema_version: canvas_widgets::WIDGET_SCHEMA_VERSION,
                            id: "repo-status".into(),
                            kind: canvas_widgets::WidgetKind::Table {
                                title: format!("{}: Active Work", project_id),
                                columns: vec![
                                    "State".into(),
                                    "Project".into(),
                                    "Idle".into(),
                                    "Ready".into(),
                                    "Backlog".into(),
                                ],
                                rows: Vec::new(),
                            },
                            theme: None,
                            refresh_ms: None,
                            chrome: canvas_widgets::WidgetChrome::DragOnly,
                            data: canvas_widgets::WidgetData::default(),
                        },
                    },
                    inputs: vec![InputPort {
                        id: "repos".into(),
                        contract: "canvas.widget.snapshot.v1".into(),
                    }],
                }],
                layout: Layout::Canvas {
                    placements: vec![crate::Placement {
                        surface_id: "repo-status".into(),
                        bounds: crate::NormalizedRect {
                            x: 0.02,
                            y: 0.1,
                            width: 0.96,
                            height: 0.8,
                        },
                        layer: 0,
                    }],
                },
                bindings: vec![DataBinding {
                    id: "repo-feed".into(),
                    source: DataSource {
                        owner: "alfalfa".into(),
                        stream: "repo-status".into(),
                        contract: "canvas.widget.snapshot.v1".into(),
                    },
                    target: InputTarget {
                        surface_id: "repo-status".into(),
                        input_id: "repos".into(),
                    },
                    adapter: Some("lockout-v1".into()),
                    delivery: DeliveryPolicy::default(),
                }],
                focused_surface_id: Some("repo-status".into()),
            };
            let (reply, _replies) = mpsc::sync_channel(1);
            self.execute(Pending {
                request: ControlRequest {
                    schema_version: CONTROL_VERSION,
                    request_id: "key-workspace-new".into(),
                    expected_revision: None,
                    actor: None,
                    command: Command::ApplyDocument { document },
                },
                reply,
            })
        }
    }

    /// Fetch all projects from the local Alfalfa index and merge them into
    /// the catalog. This makes Ctrl-P show the full project roster, not just
    /// the 2 bundled entries in config/projects.json.
    fn merge_alfalfa_projects(catalog: &mut Catalog) {
        let alfalfa_url = std::env::var("FPL_ALFALFA_URL")
            .ok()
            .unwrap_or_else(|| "http://127.0.0.1:8787".to_string());
        let addr = alfalfa_url.strip_prefix("http://").unwrap_or(&alfalfa_url);
        let timeout = std::time::Duration::from_secs(3);
        let Some(projects) = alfalfa_get(addr, "/api/v1/projects", timeout) else {
            return;
        };
        if !merge_projects_body(catalog, &projects) {
            // An empty projects array is not a roster: fall back to repos, on
            // its own connection (the first requested `Connection: close`).
            // Each repo's id is a project id in Alfalfa's model.
            if let Some(repos) = alfalfa_get(addr, "/api/v1/repos", timeout) {
                merge_repos_body(catalog, &repos);
            }
        }
    }

    fn set_canvas_visible(&mut self, visible: bool) -> Task<Message> {
        if self.canvas_visible == visible {
            return Task::none();
        }
        self.canvas_visible = visible;
        self.bump();
        self.emit_event(format!(
            "canvas_visibility {}",
            if visible { "shown" } else { "hidden" }
        ));
        if visible {
            self.status = "Canvas is visible on its selected displays.".into();
            self.replace_host_windows(self.selected_display_frames())
        } else {
            self.status = "Canvas is hidden; Ctrl+Option+C restores it.".into();
            let tasks = self
                .windows
                .keys()
                .copied()
                .map(iced::window::close)
                .collect::<Vec<Task<Message>>>();
            self.windows.clear();
            Task::batch(tasks)
        }
    }

    fn toggle_background_picker(&mut self) -> Task<Message> {
        self.theme_picker = None;
        self.background_picker = match self.background_picker {
            Some(_) => None,
            None => Some(::canvas::background_picker::BackgroundPicker::opened_from(
                &self.document.background,
            )),
        };
        let open = self.background_picker.is_some();
        self.status = if open {
            "Choose a background: ←→↑↓, −/+ speed, Enter to apply.".into()
        } else {
            "Kept the current background.".into()
        };
        self.bump();
        self.emit_event(format!(
            "background_picker {}",
            if open { "shown" } else { "hidden" }
        ));
        if open && !self.canvas_visible {
            self.set_canvas_visible(true)
        } else {
            Task::none()
        }
    }

    fn toggle_theme_picker(&mut self) -> Task<Message> {
        self.project_picker = None;
        self.background_picker = None;
        self.theme_picker = match self.theme_picker {
            Some(_) => None,
            None => Some(::canvas::theme_picker::ThemePicker::opened_from(
                self.effective_theme(&self.document),
                &self.theme_packs,
            )),
        };
        let open = self.theme_picker.is_some();
        self.transition = None;
        self.status = if open {
            "Choose a theme: ←→↑↓, Enter to apply.".into()
        } else {
            "Kept the current theme.".into()
        };
        self.bump();
        self.emit_event(format!(
            "theme_picker {}",
            if open { "shown" } else { "hidden" }
        ));
        if open && !self.canvas_visible {
            self.set_canvas_visible(true)
        } else {
            Task::none()
        }
    }

    fn toggle_project_switcher(&mut self) -> Task<Message> {
        self.toggle_shell_launcher(::canvas::shell_launcher::Scope::Workspaces)
    }

    fn toggle_shell_launcher(&mut self, scope: ::canvas::shell_launcher::Scope) -> Task<Message> {
        // An explicit shell choice cancels an Alt-Tab preview, not its destination.
        self.window_cycle.cancel();
        if self
            .project_switcher
            .take()
            .is_some_and(|picker| picker.scope == scope && !picker.window_cycle)
        {
            self.bump();
            return Task::none();
        }
        self.background_picker = None;
        self.theme_picker = None;
        self.project_picker = None;
        let mut launcher = match ::canvas::shell_launcher::Launcher::new() {
            Ok(l) => l,
            Err(e) => {
                self.status = e;
                self.bump();
                return Task::none();
            }
        };
        launcher.scope = scope;
        launcher.canvas_surface_ids = Some(
            ::canvas::shell_launcher::dock_surfaces(&self.document)
                .map(|s| s.id.clone())
                .collect(),
        );
        let target = launcher.targets[launcher.target].clone();
        let generation = launcher.generation;
        self.project_switcher = Some(launcher);
        self.status = "Launcher · type to search, Tab target, Enter open.".into();
        self.desktop_focused = true;
        self.refresh_launcher_scene();
        self.bump();
        self.emit_event(format!("project_switcher {}", "shown"));
        launcher_task(target, Command::ShellCatalog, generation, false)
    }

    fn activate_launcher_choice(&mut self) -> Task<Message> {
        let Some(picker) = self.project_switcher.as_mut() else {
            return Task::none();
        };
        if picker.busy {
            return Task::none();
        }
        let action = picker.action(now_unix_ms());
        let generation = picker.generation;
        picker.busy = action.is_some();
        picker.status = if action.is_some() {
            "Opening on selected screen…"
        } else {
            "Unavailable / stale · F5 to refresh"
        }
        .into();
        self.refresh_launcher_scene();
        self.bump();
        action
            .map(|(target, command)| launcher_task(target, command, generation, true))
            .unwrap_or_else(Task::none)
    }

    fn refresh_launcher_scene(&mut self) {
        #[cfg(feature = "lua-scenes")]
        {
            let visual = self.presented_visual_theme();
            let accent = self.effective_theme(&self.document).accent();
            let theme = ::canvas::lua_module::ModuleThemeTokens::from_visual(&visual, accent);
            let canvas_surface_ids = ::canvas::shell_launcher::dock_surfaces(&self.document)
                .map(|s| s.id.clone())
                .collect();
            if let Some(launcher) = self.project_switcher.as_mut() {
                launcher.canvas_surface_ids = Some(canvas_surface_ids);
                launcher.viewport = ::canvas::shell_geometry::sheet_size(
                    launcher.window_cycle,
                    launcher.matches().len(),
                    [self.frame.width as f32, self.frame.height as f32],
                );
                if let Err(error) = launcher.render(theme, now_unix_ms()) {
                    launcher.status = format!("Launcher Lua error: {error}");
                    self.status = launcher.status.clone();
                }
            }
        }
    }

    #[cfg(feature = "lua-scenes")]
    fn refresh_desktop_bar(&mut self, poll_files: bool) {
        self.cache_dock_icons();
        use ::canvas::lua_module::{
            ModuleInputSnapshot, ModuleInputValue as Value, ModuleThemeTokens,
        };
        let visual = self.presented_visual_theme();
        let accent = self.effective_theme(&self.document).accent();
        let now = canvas_widgets::live::chrono_now();
        let sources = self
            .decoded
            .values()
            .filter(|s| matches!(s.source, ContentSource::LiveVideo { .. }))
            .count();
        let creator = self.decoded.values().any(|s| matches!(&s.source, ContentSource::Prompt { task_kinds, .. } if !task_kinds.is_empty()));
        let values = BTreeMap::from([
            (
                "width".into(),
                Value::Number(f64::from(self.frame.width.max(1))),
            ),
            (
                "height".into(),
                Value::Number(f64::from(::canvas::DESKTOP_BAR_HEIGHT)),
            ),
            (
                "padding".into(),
                Value::Number(f64::from(fpl_gfx::chrome::BAR_PADDING)),
            ),
            (
                "labelsize".into(),
                Value::Number(f64::from(fpl_gfx::chrome::BAR_TEXT_SIZE)),
            ),
            (
                "workspace".into(),
                Value::Text(
                    workspace_identity_name(&self.catalog, &self.document.project_id)
                        .chars()
                        .take(24)
                        .collect::<String>(),
                ),
            ),
            (
                "page".into(),
                Value::Text(
                    self.document
                        .active_page_name()
                        .chars()
                        .take(20)
                        .collect::<String>(),
                ),
            ),
            (
                "clock".into(),
                Value::Text(format!(
                    "{:02}:{:02}:{:02}",
                    now.hour, now.minute, now.second
                )),
            ),
            ("sources".into(), Value::Number(sources as f64)),
            (
                "back".into(),
                Value::Boolean(self.jump_history.can_go_back()),
            ),
            (
                "forward".into(),
                Value::Boolean(self.jump_history.can_go_forward()),
            ),
            ("create".into(), Value::Boolean(creator)),
        ]);
        let snapshot = ModuleInputSnapshot {
            revision: 1,
            now_ms: (now_unix_ms() / 1000 * 1000).max(1),
            values,
        };
        let theme = ModuleThemeTokens::from_visual(&visual, accent);
        let artifact = ::canvas::shell_presentation::DESKTOP_BAR;
        let result = if poll_files {
            self.desktop_bar.refresh_files(artifact, snapshot, theme)
        } else {
            self.desktop_bar.refresh_current(artifact, snapshot, theme)
        };
        if let Err(error) = result {
            self.status = format!("Desktop bar Lua error (last good retained): {error}");
        }
        self.refresh_desktop_dock(poll_files);
    }

    #[cfg(feature = "lua-scenes")]
    fn refresh_desktop_dock(&mut self, poll_files: bool) {
        use ::canvas::lua_module::{
            ModuleInputSnapshot, ModuleInputValue as Value, ModuleThemeTokens,
        };
        let entries = self.dock_entries();
        let mut values = BTreeMap::from([
            (
                "width".into(),
                Value::Number(f64::from(self.frame.width.max(1))),
            ),
            (
                "height".into(),
                Value::Number(f64::from(::canvas::shell_geometry::DOCK_HEIGHT)),
            ),
            ("count".into(), Value::Number(entries.len() as f64)),
        ]);
        for (n, entry) in entries.iter().take(6).enumerate() {
            let key = format!("item{n}");
            let surface_id = entry.id.strip_prefix("surface:");
            values.insert(format!("{key}-id"), Value::Text(entry.id.clone()));
            values.insert(
                format!("{key}-title"),
                Value::Text(entry.title.chars().take(16).collect()),
            );
            values.insert(
                format!("{key}-focused"),
                Value::Boolean(
                    !self.desktop_focused
                        && surface_id.is_some_and(|id| {
                            self.document.focused_surface_id.as_deref() == Some(id)
                        }),
                ),
            );
            values.insert(
                format!("{key}-minimized"),
                Value::Boolean(surface_id.is_some_and(|id| self.minimized_surfaces.contains(id))),
            );
            values.insert(format!("{key}-available"), Value::Boolean(entry.available));
            values.insert(format!("{key}-open"), Value::Boolean(surface_id.is_some()));
        }
        let visual = self.presented_visual_theme();
        let theme =
            ModuleThemeTokens::from_visual(&visual, self.effective_theme(&self.document).accent());
        let snapshot = ModuleInputSnapshot {
            revision: 1,
            now_ms: 1,
            values,
        };
        let artifact = ::canvas::shell_presentation::DESKTOP_DOCK;
        let result = if poll_files {
            self.desktop_dock.refresh_files(artifact, snapshot, theme)
        } else {
            self.desktop_dock.refresh_current(artifact, snapshot, theme)
        };
        if let Err(error) = result {
            self.status = format!("Desktop dock Lua error (last good retained): {error}");
        }
    }

    fn cache_dock_icons(&mut self) {
        self.dock_icons
            .retain(|id, _| self.dock_pins.0.contains(id));
        self.dock_icon_failures
            .retain(|id| self.dock_pins.0.contains(id));
        if let Some(worker) = &self.dock_icon_worker {
            match worker.try_recv() {
                Ok((pins, results)) => {
                    self.dock_icon_worker = None;
                    // A pin edit while lookup was running invalidates that job.
                    if pins == self.dock_pins.0 {
                        for (id, result) in results {
                            match result {
                                Ok(Some(icon)) => {
                                    self.dock_icons.insert(id, icon);
                                }
                                Ok(None) => {
                                    self.dock_icon_failures.insert(id);
                                }
                                Err(error) => {
                                    eprintln!("Dock icon unavailable for {id}: {error}");
                                    self.dock_icon_failures.insert(id);
                                }
                            }
                        }
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.dock_icon_worker = None;
                    self.dock_icon_failures
                        .extend(self.dock_pins.0.iter().cloned());
                    self.status = "Dock icon worker stopped; semantic icons retained".into();
                }
            }
        }
        let jobs: Vec<_> = self
            .dock_apps
            .iter()
            .filter(|entry| {
                self.dock_pins.0.contains(&entry.id)
                    && !self.dock_icons.contains_key(&entry.id)
                    && !self.dock_icon_failures.contains(&entry.id)
            })
            .cloned()
            .collect();
        if jobs.is_empty() {
            return;
        }
        let pins = self.dock_pins.0.clone();
        let (send, receive) = std::sync::mpsc::channel();
        self.dock_icon_worker = Some(receive);
        // Exactly one lookup job may be outstanding. No AppKit or decoding work
        // runs in the update/view thread; failed lookups are negatively cached.
        std::thread::spawn(move || {
            let results = jobs
                .into_iter()
                .map(|entry| {
                    let result = ::canvas::shell_dock::icon(&entry);
                    (entry.id, result)
                })
                .collect();
            let _ = send.send((pins, results));
        });
    }

    fn set_dock_pin(
        &mut self,
        id: &str,
        pinned: bool,
        entries: &[::canvas::shell_launcher::Entry],
    ) -> Result<(), String> {
        let next = self.dock_pins.updated(id, pinned, entries)?;
        next.save()?; // Durable write before visible mutation.
        self.dock_pins = next;
        self.dock_apps = entries
            .iter()
            .filter(|e| {
                matches!(
                    e.choice,
                    ::canvas::shell_launcher::Choice::Application { .. }
                )
            })
            .cloned()
            .collect();
        self.cache_dock_icons();
        #[cfg(feature = "lua-scenes")]
        self.refresh_desktop_dock(false);
        Ok(())
    }

    fn dock_entries(&self) -> Vec<::canvas::shell_launcher::Entry> {
        use ::canvas::shell_launcher::{Choice, Entry};
        // Native pins remain saved and available in the explicit native launcher.
        // The Canvas dock represents Canvas windows, not host applications.
        ::canvas::shell_launcher::dock_surfaces(&self.document)
            .map(|s| Entry {
                id: format!("surface:{}", s.id),
                title: s.title.clone(),
                detail: "WINDOW".into(),
                available: self.decoded.contains_key(&s.id),
                choice: Choice::Surface { id: s.id.clone() },
            })
            .collect()
    }

    fn focus_shell_surface(&mut self, id: &str) {
        if !self.decoded.contains_key(id) {
            return;
        }
        self.minimized_surfaces.remove(id);
        let page = self
            .document
            .layouts()
            .position(|layout| flatten_layout(layout).iter().any(|p| p.surface_id == id));
        if let Some(page) = page {
            self.document.active_page = page;
        }
        self.desktop_focused = false;
        self.apply_interaction_intents(vec![
            InteractionIntent::Focus {
                surface_id: id.into(),
            },
            InteractionIntent::PromoteToFront {
                surface_id: id.into(),
            },
        ]);
        self.bump();
    }

    fn cycle_window_ids(&self) -> Vec<String> {
        ::canvas::shell_launcher::dock_surfaces(&self.document)
            .filter(|surface| self.decoded.contains_key(&surface.id))
            .map(|surface| surface.id.clone())
            .collect()
    }

    fn handle_window_cycle(&mut self, event: &iced::keyboard::Event) -> bool {
        use iced::keyboard::{Event, Key, key::Named};
        let cycle = matches!(event, Event::KeyPressed {
            key: Key::Named(Named::Tab), modifiers, ..
        } if modifiers.alt() && !modifiers.control() && !modifiers.logo());
        if cycle {
            // Never commandeer an existing modal picker.
            if !self.window_cycle.active()
                && (self.project_switcher.is_some()
                    || self.background_picker.is_some()
                    || self.theme_picker.is_some()
                    || self.project_picker.is_some()
                    || self.jarvis_text_visible)
            {
                return false;
            }
            let reverse = matches!(event, Event::KeyPressed { modifiers, .. } if modifiers.shift());
            if !self.window_cycle.active()
                && let Some(target) = self.focused_hid_target()
            {
                // Alt may already have reached the old target before Tab arrived.
                if let Err(error) = self.hid_output.release(&target) {
                    self.status = format!("Cannot switch windows: HID release failed: {error}");
                    return true;
                }
            }
            let ids = self.cycle_window_ids();
            if !self.window_cycle.active() {
                self.window_cycle_workspace = Some(self.document.project_id.clone());
            }
            let Some(selected) = self.window_cycle.cycle(&ids, reverse).map(str::to_owned) else {
                return true;
            };
            if self.project_switcher.is_none() {
                let mut launcher = match ::canvas::shell_launcher::Launcher::new() {
                    Ok(launcher) => launcher,
                    Err(error) => {
                        self.window_cycle.cancel();
                        self.status = error;
                        return true;
                    }
                };
                let entries = self
                    .window_cycle
                    .order()
                    .iter()
                    .filter_map(|id| {
                        let surface = self
                            .document
                            .surfaces
                            .iter()
                            .find(|surface| &surface.id == id)?;
                        Some(::canvas::shell_launcher::Entry {
                            id: format!("surface:{id}"),
                            title: surface.title.clone(),
                            detail: "WINDOW · recent".into(),
                            available: true,
                            choice: ::canvas::shell_launcher::Choice::Surface { id: id.clone() },
                        })
                    })
                    .collect();
                launcher.window_cycle = true;
                launcher.accept(
                    launcher.generation,
                    Ok(::canvas::shell_launcher::Catalog {
                        revision: "window-cycle".into(),
                        entries,
                    }),
                    now_unix_ms(),
                );
                self.project_switcher = Some(launcher);
            }
            if let Some(launcher) = self.project_switcher.as_mut() {
                launcher.selected = launcher
                    .matches()
                    .iter()
                    .position(|entry| entry.id == format!("surface:{selected}"))
                    .unwrap_or(0);
                launcher.status =
                    "Alt-Tab · Shift reverses · release Alt to focus · Esc cancels".into();
            }
            self.refresh_launcher_scene();
            self.bump();
            return true;
        }
        if !self.window_cycle.active() {
            return false;
        }
        if matches!(
            event,
            Event::KeyPressed {
                key: Key::Named(Named::Escape),
                ..
            }
        ) {
            self.window_cycle.cancel();
            self.project_switcher = None;
            self.bump();
        } else if matches!(event, Event::ModifiersChanged(modifiers) if !modifiers.alt())
            || matches!(
                event,
                Event::KeyPressed {
                    key: Key::Named(Named::Enter),
                    ..
                }
            )
        {
            let ids = self.cycle_window_ids();
            let selected = self.window_cycle.commit(&ids);
            self.project_switcher = None;
            if let Some(id) = selected {
                self.focus_shell_surface(&id);
            } else {
                self.status = "Selected window closed; focus unchanged.".into();
                self.bump();
            }
        }
        true
    }

    fn apply_picked_theme(
        &mut self,
        theme: canvas_core::ThemeSelection,
        pack_id: Option<String>,
    ) -> Task<Message> {
        let previous = self.document.theme;
        self.document.theme = ThemeRef::Selection { theme };
        if let Err(error) = self.document.validate(&self.catalog) {
            self.document.theme = previous;
            self.status = format!("Could not apply that theme: {error}");
            self.bump();
            return Task::none();
        }
        if self.active_project_workspace.as_deref() == Some(self.document.project_id.as_str()) {
            let mut next = self.workspaces.clone();
            next.upsert(self.document.clone());
            if let Err(error) =
                workspace_store::save(&self.workspace_store_path, &next, &self.catalog)
            {
                self.document.theme = previous;
                self.status = format!("Could not save that theme: {error}");
                self.bump();
                return Task::none();
            }
            self.workspaces = next;
        }
        let selection_path =
            PathBuf::from("themes/selections").join(format!("{}.txt", self.document.project_id));
        if let Some(pack_id) = &pack_id {
            if let Err(error) =
                ::canvas::runtime_config::write(&selection_path, pack_id.as_bytes(), 48)
            {
                self.events
                    .push(format!("theme_pack_selection_failed {error}"));
            }
            self.theme_pack_ids
                .insert(self.document.project_id.clone(), pack_id.clone());
        } else if let Some(root) = ::canvas::runtime_config::root() {
            let _ = std::fs::remove_file(root.join(selection_path));
            self.theme_pack_ids.remove(&self.document.project_id);
        }
        self.publish_web_themes();
        let selected = theme.to_project_theme();
        self.status = selected.map_or_else(
            || {
                pack_id.map_or_else(
                    || "Theme set to imported palette.".into(),
                    |id| format!("Theme set to {id}."),
                )
            },
            |theme| format!("Theme set to {:?} {:?}.", theme.mode, theme.accent),
        );
        self.emit_event("theme_applied");
        self.bump();
        Task::none()
    }

    /// Commit a background to the workspace the human is standing in.
    ///
    /// Persisted immediately rather than on switch: the point of choosing is
    /// that it survives, and a background that quietly reverted on the next
    /// restart would look like the picker had not worked at all.
    ///
    /// Shared with the control protocol so `bg-shader` and F2 land the same
    /// change the same way; the picker reports failures in the status line and
    /// the socket reports them as an error outcome.
    fn commit_background(&mut self, background: Background) -> Result<(), Failure> {
        let previous = std::mem::replace(&mut self.document.background, background);
        if let Err(error) = self.document.validate(&self.catalog) {
            self.document.background = previous;
            return Err(Failure::invalid(format!(
                "Could not apply that background: {error}"
            )));
        }
        self.screensaver_index =
            screensaver::workspace_index(&self.document.id, &self.document.background);
        self.screensaver_piece_elapsed = Duration::ZERO;

        // Only write it through when this document is the saved workspace it
        // claims to be; otherwise the picker would invent a stored workspace
        // for a document that never had one.
        if self.active_project_workspace.as_deref() == Some(self.document.project_id.as_str()) {
            let mut next = self.workspaces.clone();
            next.upsert(self.document.clone());
            if let Err(error) =
                workspace_store::save(&self.workspace_store_path, &next, &self.catalog)
            {
                self.document.background = previous;
                return Err(Failure::resource(format!(
                    "Could not save that background: {error}"
                )));
            }
            self.workspaces = next;
        }
        self.status = format!(
            "Background set to {}.",
            screensaver::SHADERS[self.screensaver_index % screensaver::SHADERS.len()].name
        );
        self.emit_event("background_applied");
        self.bump();
        Ok(())
    }

    fn apply_picked_background(&mut self, background: Background) -> Task<Message> {
        if let Err(failure) = self.commit_background(background) {
            self.status = failure.message;
            self.bump();
        }
        Task::none()
    }

    /// Write a background onto a stored workspace that is not on screen.
    ///
    /// The document is not the live one, so nothing here touches the shader
    /// clock: the change is real and persisted, and becomes visible when that
    /// project is switched to.
    fn commit_stored_background(
        &mut self,
        project_id: &str,
        background: Background,
    ) -> Result<(), Failure> {
        let mut document = self
            .workspaces
            .get(project_id)
            .cloned()
            .ok_or_else(|| Failure {
                code: ErrorCode::NotFound,
                message: format!("workspace '{project_id}' not found"),
            })?;
        document.background = background;
        document.revision = document.revision.saturating_add(1);
        document.validate(&self.catalog).map_err(|error| {
            Failure::invalid(format!("Could not apply that background: {error}"))
        })?;
        let mut next = self.workspaces.clone();
        next.upsert(document);
        workspace_store::save(&self.workspace_store_path, &next, &self.catalog).map_err(
            |error| Failure::resource(format!("Could not save that background: {error}")),
        )?;
        self.workspaces = next;
        self.status = format!("Background set on project workspace '{project_id}'.");
        self.emit_event("background_applied");
        self.bump();
        Ok(())
    }

    /// What a person is looking at, told from the point of view of a change
    /// that has just landed on `project_id`.
    ///
    /// The presented document is the authority, not the field the mutation
    /// wrote: those are the same thing only when the change landed on the
    /// project that is on screen, and a receipt that cannot tell the two apart
    /// is how three `ok` replies described a desk that never changed.
    fn visible_background(&self, project_id: &str) -> VisibleBackground {
        let drawn_shader = if self.screensaver_settings.enabled {
            screensaver::SHADERS
                .get(self.screensaver_index)
                .map(|shader| shader.id)
        } else {
            match &self.document.background {
                Background::Shader { shader_id, .. } => Some(*shader_id),
                _ => None,
            }
        };
        VisibleBackground {
            project_id: project_id.to_owned(),
            active_project_id: self.document.project_id.clone(),
            background: self.document.background.clone(),
            screensaver_enabled: self.screensaver_settings.enabled,
            drawn_shader,
        }
    }

    fn toggle_shortcut_help(&mut self) -> Task<Message> {
        self.shortcuts_visible = !self.shortcuts_visible;
        self.bump();
        self.emit_event(format!(
            "shortcut_help {}",
            if self.shortcuts_visible {
                "shown"
            } else {
                "hidden"
            }
        ));
        if self.shortcuts_visible && !self.canvas_visible {
            self.set_canvas_visible(true)
        } else {
            Task::none()
        }
    }

    fn execute(&mut self, pending: Pending) -> Task<Message> {
        if let Err(message) = pending.request.validate() {
            self.fail(pending, Failure::invalid(message));
            return Task::none();
        }
        if !matches!(
            pending.request.command,
            Command::Inspect
                | Command::ShellCatalog
                | Command::ShellDockPins
                | Command::Performance
                | Command::StreamDescriptors
                | Command::ReadDocument
        ) && pending
            .request
            .expected_revision
            .is_some_and(|expected| expected != self.runtime_revision)
        {
            let expected = pending.request.expected_revision.unwrap();
            self.fail(
                pending,
                Failure {
                    code: ErrorCode::Conflict,
                    message: format!(
                        "stale Canvas revision {expected}; current revision is {}",
                        self.runtime_revision
                    ),
                },
            );
            return Task::none();
        }

        match pending.request.command.clone() {
            Command::ShellCatalog => {
                Self::merge_alfalfa_projects(&mut self.catalog);
                let catalog = self.launcher_catalog();
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ShellCatalog { catalog },
                    },
                );
                Task::none()
            }
            Command::ShellDockPins => {
                let pins = self.dock_pins.0.clone();
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ShellDockPins { pins },
                    },
                );
                Task::none()
            }
            Command::ShellDockPin {
                catalog_revision,
                entry_id,
                pinned,
            } => {
                let catalog = self.launcher_catalog();
                if catalog.revision != catalog_revision {
                    self.fail(
                        pending,
                        Failure {
                            code: ErrorCode::Conflict,
                            message: "Dock catalog changed; refresh this target before pinning."
                                .into(),
                        },
                    );
                } else if let Err(error) = self.set_dock_pin(&entry_id, pinned, &catalog.entries) {
                    self.fail(pending, Failure::resource(error));
                } else {
                    self.bump();
                    let pins = self.dock_pins.0.clone();
                    self.answer(
                        pending,
                        Outcome::Ok {
                            receipt: Receipt::ShellDockPins { pins },
                        },
                    );
                }
                Task::none()
            }
            Command::ShellActivate {
                catalog_revision,
                entry_id,
            } => {
                use ::canvas::shell_launcher::Choice;
                let catalog = self.launcher_catalog();
                let Some(entry) = catalog
                    .entries
                    .iter()
                    .find(|e| e.id == entry_id && e.available)
                    .cloned()
                    .filter(|_| catalog.revision == catalog_revision)
                else {
                    self.fail(pending,Failure{code:ErrorCode::Conflict,message:"Launcher catalog changed or choice unavailable; refresh this target.".into()});
                    return Task::none();
                };
                let task = match entry.choice {
                    Choice::Workspace { id } => {
                        let task = self.switch_project(id.clone());
                        if self.document.project_id != id {
                            self.fail(pending, Failure::resource(self.status.clone()));
                            return task;
                        }
                        task
                    }
                    Choice::Surface { id } => {
                        self.focus_shell_surface(&id);
                        Task::none()
                    }
                    Choice::Application { path } => {
                        let (send, receive) = iced::futures::channel::oneshot::channel();
                        std::thread::spawn(move || {
                            let _ = send.send(::canvas::shell_launcher::launch(&path));
                        });
                        return Task::perform(
                            async move {
                                receive.await.unwrap_or_else(|_| {
                                    Err("Native application worker stopped".into())
                                })
                            },
                            move |result| Message::NativeAppLaunched {
                                pending: Box::new(pending.clone()),
                                entry_id: entry_id.clone(),
                                result,
                            },
                        );
                    }
                };
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ShellActivated { entry_id },
                    },
                );
                task
            }
            Command::ReadDocument => {
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::DocumentSnapshot {
                            document: Box::new(self.document.clone()),
                            snapshot: self.snapshot(),
                        },
                    },
                );
                Task::none()
            }
            Command::Inspect => {
                let snapshot = self.snapshot();
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::Snapshot { snapshot },
                    },
                );
                Task::none()
            }
            Command::Performance => {
                let mut metrics = self.update_timings.snapshot();
                metrics["pending_raster_decodes"] = serde_json::json!(self.live_pending.len());
                metrics["video_sources"] = serde_json::json!(self.decoded.values().filter_map(|surface| {
                    let ContentSource::LiveVideo { source_id, path, width, height, .. } = &surface.source else { return None; };
                    let age = std::fs::metadata(path).ok()
                        .and_then(|metadata| metadata.modified().ok())
                        .and_then(|modified| modified.elapsed().ok())
                        .map(|elapsed| elapsed.as_millis().min(u64::MAX as u128) as u64);
                    Some(serde_json::json!({
                        "source_id": source_id,
                        "width": width,
                        "height": height,
                        "published_frame_age_ms": age,
                        "state": if age.is_none() { "missing" } else if age.is_some_and(|age| age > 2000) { "stale" } else { "recent" },
                    }))
                }).collect::<Vec<_>>());
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::Performance { metrics },
                    },
                );
                Task::none()
            }
            Command::StreamDescriptors => {
                #[cfg(feature = "lua-scenes")]
                let descriptors = self
                    .stream_descriptors
                    .iter()
                    .map(|(key, descriptor)| {
                        let mut descriptor = descriptor.clone();
                        descriptor["endpoints"] = self
                            .stream_endpoints
                            .get(key)
                            .and_then(|value| value.get("endpoints"))
                            .cloned()
                            .unwrap_or_else(|| serde_json::json!([]));
                        descriptor
                    })
                    .collect();
                #[cfg(not(feature = "lua-scenes"))]
                let descriptors = Vec::new();
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::StreamDescriptors { descriptors },
                    },
                );
                Task::none()
            }
            Command::BrowserKey {
                key,
                shift,
                control,
                alt,
                meta,
            } => {
                let Some(event) = browser_key_event(&key, shift, control, alt, meta) else {
                    self.fail(
                        pending,
                        Failure::invalid(format!("unsupported browser key {key}")),
                    );
                    return Task::none();
                };
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::BrowserKeyAccepted,
                    },
                );
                Task::done(Message::TerminalInput(event))
            }
            Command::BrowserPointer { phase, x, y, delta } => {
                self.handle_browser_pointer(&phase, x, y, delta);
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::BrowserPointerAccepted,
                    },
                );
                Task::none()
            }
            Command::ApplyDocument { document } => {
                if let Err(error) = document.validate(&self.catalog) {
                    self.fail(pending, Failure::invalid(error.to_string()));
                    return Task::none();
                }
                if let Err(failure) = self.ensure_background_available(&document) {
                    self.fail(pending, failure);
                    return Task::none();
                }
                // Applying a project workspace is a durable edit, not a
                // temporary preview. If we only replace `self.document`, the
                // next project switch resurrects the old stored copy and the
                // user's controls/layout appear to vanish. Persist before
                // presenting so an I/O failure cannot leave screen and store
                // disagreeing about which revision is current.
                let mut next = self.workspaces.clone();
                next.upsert(document.clone());
                next.active_project_id = Some(document.project_id.clone());
                if let Err(error) =
                    workspace_store::save(&self.workspace_store_path, &next, &self.catalog)
                {
                    self.fail(pending, Failure::resource(error.to_string()));
                    return Task::none();
                }
                let previous_focus = self.document.focused_surface_id.clone();
                let presentation_changed = self.document.id != document.id
                    || self.document.background != document.background;
                let workspace_id = document.id.clone();
                self.start_transition();
                self.document = document;
                self.workspaces = next;
                if self.document.focused_surface_id.is_some()
                    && (self.document.focused_surface_id != previous_focus || presentation_changed)
                {
                    self.begin_focus_emphasis();
                }
                self.refresh_interaction_geometry();
                self.prune_decoded();
                self.active_project_workspace = Some(self.document.project_id.clone());
                self.ensure_scene_decoded();
                if presentation_changed {
                    self.reset_background_animation();
                }
                self.bump();
                self.status = format!("Applied workspace {workspace_id}.");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::DocumentApplied { workspace_id },
                    },
                );
                Task::none()
            }
            Command::ApplyPatch { patch } => {
                let emphasizes_focus = patch.operations.iter().any(|operation| {
                    matches!(
                        operation,
                        PatchOperation::SetFocus {
                            surface_id: Some(_)
                        }
                    )
                });
                match self.document.apply_patch(&patch, &self.catalog) {
                    Ok(document) => {
                        if let Err(failure) = self.ensure_background_available(&document) {
                            self.fail(pending, failure);
                            return Task::none();
                        }
                        let previous_focus = self.document.focused_surface_id.clone();
                        let background_changed = self.document.background != document.background;
                        self.start_transition();
                        self.document = document;
                        if self.document.focused_surface_id != previous_focus || emphasizes_focus {
                            self.begin_focus_emphasis();
                        }
                        self.refresh_interaction_geometry();
                        self.prune_decoded();
                        self.ensure_scene_decoded();
                        if background_changed {
                            self.reset_background_animation();
                        }
                        self.bump();
                        self.status = format!("Patched workspace {}.", self.document.id);
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::DocumentApplied {
                                    workspace_id: self.document.id.clone(),
                                },
                            },
                        );
                    }
                    Err(error) => self.fail(
                        pending,
                        Failure {
                            code: ErrorCode::Conflict,
                            message: error.to_string(),
                        },
                    ),
                }
                Task::none()
            }
            Command::PatchView { surface_id, patch } => {
                if let Some(index) = patch.operations.iter().find_map(|operation| {
                    if let ViewOperation::SetNavigation {
                        navigation: ViewNavigation::Page { index },
                    } = operation
                    {
                        Some(*index)
                    } else {
                        None
                    }
                }) && self
                    .decoded
                    .get(&surface_id)
                    .and_then(|surface| surface.snapshot.page_index)
                    != Some(index)
                {
                    self.fail(
                        pending,
                        Failure::unsupported(
                            "navigate the PDF page before patching its shared viewport",
                        ),
                    );
                    return Task::none();
                }
                let Some(current) = self.shared_views.get(&surface_id) else {
                    self.fail(
                        pending,
                        Failure {
                            code: ErrorCode::NotFound,
                            message: format!("Canvas surface has no shared view: {surface_id}"),
                        },
                    );
                    return Task::none();
                };
                match current.apply_patch(&patch) {
                    Ok(view) => {
                        self.shared_views.insert(surface_id.clone(), view.clone());
                        self.document.focused_surface_id = Some(surface_id.clone());
                        self.bump();
                        self.status = format!(
                            "Shared view {} advanced to revision {}.",
                            view.id, view.revision
                        );
                        self.emit_event(format!("view_patched {} {}", surface_id, view.revision));
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::ViewPatched {
                                    surface_id,
                                    view: Box::new(view),
                                },
                            },
                        );
                    }
                    Err(error) => self.fail(
                        pending,
                        Failure {
                            code: ErrorCode::Conflict,
                            message: error.to_string(),
                        },
                    ),
                }
                Task::none()
            }
            Command::OpenFile {
                surface_id,
                path,
                title,
            } => {
                let viewport = self.media_viewport();
                let scale_factor = self.scale_factor;
                Task::perform(
                    async move {
                        load_content(
                            surface_id,
                            PathBuf::from(path),
                            title,
                            viewport,
                            scale_factor,
                        )
                    },
                    move |result| Message::ContentLoaded {
                        pending: Box::new(pending),
                        result: Box::new(result),
                    },
                )
            }
            Command::CloseSurface { surface_id } => {
                if !self
                    .document
                    .surfaces
                    .iter()
                    .any(|surface| surface.id == surface_id)
                {
                    self.fail(
                        pending,
                        Failure {
                            code: ErrorCode::NotFound,
                            message: format!("unknown Canvas surface: {surface_id}"),
                        },
                    );
                    return Task::none();
                }
                self.document
                    .surfaces
                    .retain(|surface| surface.id != surface_id);
                self.document
                    .bindings
                    .retain(|binding| binding.target.surface_id != surface_id);
                self.decoded.remove(&surface_id);
                self.shared_views.remove(&surface_id);
                if self.document.focused_surface_id.as_ref() == Some(&surface_id) {
                    self.document.focused_surface_id = self
                        .document
                        .surfaces
                        .last()
                        .map(|surface| surface.id.clone());
                }
                self.rebuild_stack();
                self.bump();
                self.status = format!("Closed {surface_id}.");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::SurfaceClosed { surface_id },
                    },
                );
                Task::none()
            }
            Command::SetFrame { frame } => {
                if !self.display_frames.is_empty()
                    && !platform::frame_within_displays(frame, &self.display_frames)
                {
                    self.fail(
                        pending,
                        Failure::invalid("Canvas frame must remain inside connected displays"),
                    );
                    return Task::none();
                }
                self.frame = frame;
                self.selected_display_ids = self
                    .displays
                    .iter()
                    .find(|display| display.frame == frame)
                    .map(|display| vec![display.id.clone()])
                    .unwrap_or_default();
                if self.interaction.captured_pointer().is_none() {
                    self.interaction = interaction_engine(
                        frame,
                        self.document.chrome,
                        self.window_chrome().titlebar_height,
                    );
                }
                self.replan_rasters();
                self.bump();
                self.status = format!(
                    "Canvas moved to {},{} at {}x{}.",
                    frame.x, frame.y, frame.width, frame.height
                );
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::FrameSet { frame },
                    },
                );
                if self.canvas_visible {
                    self.replace_host_windows([frame])
                } else {
                    Task::none()
                }
            }
            Command::SetDisplayTarget { target } => {
                let (resolved, task) = match self.apply_display_target(&target) {
                    Ok(result) => result,
                    Err(failure) => {
                        self.fail(pending, failure);
                        return Task::none();
                    }
                };
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::DisplayTargetSet {
                            display_ids: resolved.display_ids,
                            frame: resolved.frame,
                        },
                    },
                );
                task
            }
            Command::PlaceSurface {
                surface_id,
                display_ordinal,
                region,
                margin,
            } => {
                let (display_id, bounds, task) = match self.place_surface_on_display(
                    &surface_id,
                    display_ordinal,
                    region,
                    margin,
                ) {
                    Ok(result) => result,
                    Err(failure) => {
                        self.fail(pending, failure);
                        return Task::none();
                    }
                };
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::SurfacePlaced {
                            surface_id,
                            display_id,
                            bounds,
                        },
                    },
                );
                task
            }
            Command::SetOverlays { overlays } => {
                self.overlays = overlays;
                // Persist the updated overlay policy alongside the display target.
                self.workspaces
                    .set_display_policy(workspace_store::DisplayPolicy {
                        target: self.workspaces.display_policy().target,
                        overlays,
                    });
                let _ = workspace_store::save(
                    &self.workspace_store_path,
                    &self.workspaces,
                    &self.catalog,
                );
                self.bump();
                self.status = "Updated Canvas overlay placement.".into();
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::OverlaysSet { overlays },
                    },
                );
                Task::none()
            }
            Command::SetVisibility { visible } => {
                let task = self.set_canvas_visible(visible);
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::VisibilitySet { visible },
                    },
                );
                task
            }
            Command::Focus => {
                if let Err(error) = platform::make_windows_shareable(self.kiosk) {
                    self.fail(pending, Failure::platform(error));
                    return Task::none();
                }
                self.bump();
                self.status = "Canvas focused.".into();
                platform::activate_for_focus();
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::Focused,
                    },
                );
                Task::batch(self.windows.keys().copied().map(iced::window::gain_focus))
            }
            Command::Navigate { surface_id, target } => {
                self.navigate_pdf(surface_id, target, Some(pending))
            }
            Command::SetTheme { project_id } => {
                let mut document = self.document.clone();
                document.project_id.clone_from(&project_id);
                document.theme = ThemeRef::Project;
                if let Err(error) = document.validate(&self.catalog) {
                    self.fail(pending, Failure::invalid(error.to_string()));
                    return Task::none();
                }
                document.revision += 1;
                self.start_transition();
                self.document = document;
                self.active_project_workspace = None;
                self.ensure_scene_decoded();
                self.bump();
                self.status = format!("Using {project_id} theme.");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ThemeSet {
                            project_id,
                            theme: ThemeRef::Project,
                        },
                    },
                );
                Task::none()
            }
            Command::SetWallpaper { request } => {
                Task::perform(async move { set_wallpaper(request) }, move |result| {
                    Message::WallpaperFinished {
                        pending: Box::new(pending),
                        result,
                    }
                })
            }
            Command::UpsertIndicator { indicator } => {
                let indicator_id = indicator.id.clone();
                if !self.indicators.contains_key(&indicator_id)
                    && self.indicators.len() >= MAX_ACTIVE_INDICATORS
                {
                    self.fail(
                        pending,
                        Failure::resource(format!(
                            "Canvas supports at most {MAX_ACTIVE_INDICATORS} active indicators"
                        )),
                    );
                    return Task::none();
                }

                let changed = self.indicators.get(&indicator_id) != Some(&indicator)
                    || self.visible_indicator_id.as_ref() != Some(&indicator_id);
                let was_visible = self.visible_indicator_id.is_some();
                if indicator_id == canvas_workspace::observer::VOICE_INDICATOR_ID {
                    // Freshness is independent of reveal and caption changes.
                    self.voice_indicator_seen = Some(Instant::now());
                }
                self.indicators.insert(indicator_id.clone(), indicator);
                self.visible_indicator_id = Some(indicator_id.clone());
                self.indicator_dismissing = None;
                if changed {
                    // A partial caption/state update must not restart the reveal.
                    if !was_visible {
                        self.indicator_started = Instant::now();
                    }
                    self.bump();
                    self.status = format!("Updated indicator {indicator_id}.");
                }
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::IndicatorUpserted { indicator_id },
                    },
                );
                Task::none()
            }
            Command::DismissIndicator { indicator_id } => {
                let removed = self.dismiss_status_indicator(&indicator_id, Instant::now());
                if removed {
                    self.bump();
                    self.status = format!("Dismissed indicator {indicator_id}.");
                }
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::IndicatorDismissed { indicator_id },
                    },
                );
                Task::none()
            }
            Command::ShowNotification { notification } => {
                let notification_id = notification.id.clone();
                self.present_notification(notification);
                self.status = format!("Showing notification {notification_id}.");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::NotificationShown { notification_id },
                    },
                );
                Task::none()
            }
            Command::SetMesh { mesh } => {
                let node_count = mesh.nodes.len();
                if self.mesh.as_ref() != Some(&mesh) {
                    self.mesh = Some(mesh);
                    self.bump();
                    self.status = format!("Updated {node_count} mesh nodes.");
                }
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::MeshSet { node_count },
                    },
                );
                Task::none()
            }
            Command::SetPresence { presence } => {
                let zone = presence.zone.clone();
                let occupant_count = presence.occupants.len();
                let was_occupied = self.presence.as_ref().is_some_and(|p| p.occupied);
                self.presence = Some(presence);
                self.presence_received = Some(Instant::now());
                if !was_occupied && self.presence.as_ref().is_some_and(|p| p.occupied) {
                    self.screensaver_settings.enabled = false;
                    // Wake the displays on arrival: make Canvas visible if it was hidden.
                    if !self.canvas_visible {
                        let wake = self.set_canvas_visible(true);
                        self.bump();
                        self.emit_event("presence_arrival_wake".to_string());
                        return wake;
                    }
                    self.bump();
                    self.emit_event("presence_present".to_string());
                    self.status = "Presence detected — privacy screensaver cleared.".into();
                } else if was_occupied && !self.presence.as_ref().is_some_and(|p| p.occupied) {
                    let prev_enabled = self.screensaver_settings.enabled;
                    if !prev_enabled {
                        self.screensaver_settings.enabled = true;
                        self.screensaver_piece_elapsed = Duration::ZERO;
                        self.screensaver_elapsed = Duration::ZERO;
                        self.screensaver_last_frame = Some(Instant::now());
                    }
                    // Sleep the displays on departure: hide Canvas if occupied.
                    if self.canvas_visible {
                        let sleep = self.set_canvas_visible(false);
                        self.bump();
                        self.emit_event("presence_departure_sleep".to_string());
                        return sleep;
                    }
                    self.bump();
                    self.emit_event("presence_away".to_string());
                    self.status = "User away — privacy screensaver activated.".into();
                } else {
                    self.bump();
                    self.status = format!("Updated presence in {zone}.");
                }
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::PresenceSet {
                            zone,
                            occupant_count,
                        },
                    },
                );
                Task::none()
            }
            Command::ShowPresencePreview { preview } => {
                let face_count = preview.faces.len();
                match decode_presence_preview(preview) {
                    Ok(mut preview) => {
                        let replace = self.presence_preview.as_ref().is_none_or(|current| {
                            preview.value.captured_at_unix_ms >= current.value.captured_at_unix_ms
                        });
                        if replace {
                            if let Some(current) = self.presence_preview.as_ref() {
                                preview.started = current.started;
                            }
                            self.presence_preview = Some(preview);
                        }
                        self.bump();
                        self.status = format!("Showing {face_count} detected faces.");
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::PresencePreviewShown { face_count },
                            },
                        );
                    }
                    Err(error) => self.fail(pending, error),
                }
                Task::none()
            }
            Command::DismissPresencePreview => {
                if self.presence_preview.take().is_some() {
                    self.bump();
                    self.status = "Dismissed presence preview.".into();
                }
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::PresencePreviewDismissed,
                    },
                );
                Task::none()
            }
            Command::Shutdown => {
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ShuttingDown,
                    },
                );
                iced::exit()
            }
            Command::SetScreensaver { settings } => {
                if self.screensaver_settings != settings {
                    self.screensaver_clock
                        .set_max_fps(settings.max_fps, Instant::now());
                    self.screensaver_settings = settings;
                    if settings.enabled {
                        self.screensaver_piece_elapsed = Duration::ZERO;
                        self.screensaver_elapsed = Duration::ZERO;
                        self.screensaver_last_frame = Some(Instant::now());
                    }
                    self.bump();
                    self.status = format!(
                        "Screensaver {} at up to {} FPS.",
                        if settings.enabled {
                            "enabled"
                        } else {
                            "disabled"
                        },
                        settings.max_fps
                    );
                }
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ScreensaverUpdated { settings },
                    },
                );
                Task::none()
            }
            Command::WorkspaceSave { name } => {
                if name != self.document.project_id {
                    self.fail(
                        pending,
                        Failure::invalid(format!(
                            "project workspace name must equal project_id {:?}",
                            self.document.project_id
                        )),
                    );
                    return Task::none();
                }
                let mut next = self.workspaces.clone();
                next.upsert(self.document.clone());
                next.active_project_id = Some(name.clone());
                if let Err(error) =
                    workspace_store::save(&self.workspace_store_path, &next, &self.catalog)
                {
                    self.fail(pending, Failure::resource(error.to_string()));
                    return Task::none();
                }
                self.workspaces = next;
                self.active_project_workspace = Some(name.clone());
                self.bump();
                self.status = format!("Saved project workspace '{name}'.");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::WorkspaceSaved { name },
                    },
                );
                Task::none()
            }
            Command::WorkspaceSwitch { name } => {
                if self.workspaces.get(&name).is_some() {
                    let mut next = self.workspaces.clone();
                    if self.active_project_workspace.as_deref()
                        == Some(self.document.project_id.as_str())
                    {
                        next.upsert(self.document.clone());
                    }
                    // Navigation, so it goes through `activate` and leaves a
                    // trail. Save and sync below set the active project without
                    // one: neither is a place the human chose to leave, and
                    // treating them as such would make "go back" mean whichever
                    // project last happened to be written.
                    next.activate(&name);
                    if let Err(error) =
                        workspace_store::save(&self.workspace_store_path, &next, &self.catalog)
                    {
                        self.fail(pending, Failure::resource(error.to_string()));
                        return Task::none();
                    }
                    let document = next
                        .get(&name)
                        .cloned()
                        .expect("validated project workspace disappeared");
                    if let Err(failure) = self.ensure_background_available(&document) {
                        self.fail(pending, failure);
                        return Task::none();
                    }
                    self.workspaces = next;
                    self.start_transition();
                    self.document = document;
                    self.snapped_view = inferred_snapped_view(&self.document);
                    self.free_layout = None;
                    self.layout_policy = self
                        .snapped_view
                        .as_deref()
                        .and_then(|view_id| ::canvas::operational_views::load(view_id).ok())
                        .map(|view| view.layout_policy)
                        .unwrap_or_default();
                    self.active_project_workspace = Some(name.clone());
                    self.jump_history.jump(JumpEntry::current(&name, None));
                    self.publish_web_themes();
                    self.reset_workspace_runtime();
                    self.ensure_scene_decoded();
                    self.reset_background_animation();
                    self.bump();
                    self.status = format!("Switched to workspace '{name}'.");
                    self.answer(
                        pending,
                        Outcome::Ok {
                            receipt: Receipt::WorkspaceSwitched { name },
                        },
                    );
                    Task::none()
                } else {
                    // Provision a new workspace with a repo-status table so the
                    // user sees live data immediately instead of an empty screen.
                    let project_id = name.clone();
                    let document = WorkspaceDocument {
                        pages: Vec::new(),
                        active_page: 0,
                        schema_version: SCHEMA_VERSION,
                        // Store keys may carry Alfalfa's capitalization; document ids
                        // follow the lowercase identifier grammar.
                        id: format!("{}-active", project_id.to_ascii_lowercase()),
                        project_id: project_id.clone(),
                        revision: 1,
                        theme: ThemeRef::Project,
                        background: Background::Theme,
                        chrome: SurfaceChrome::default(),
                        surfaces: vec![Surface {
                            id: "repo-status".into(),
                            title: format!("{}: Active Work", project_id),
                            content: SurfaceContent::Widget {
                                spec: canvas_widgets::WidgetSpec {
                                    schema_version: canvas_widgets::WIDGET_SCHEMA_VERSION,
                                    id: "repo-status".into(),
                                    kind: canvas_widgets::WidgetKind::Table {
                                        title: format!("{}: Active Work", project_id),
                                        columns: vec![
                                            "State".into(),
                                            "Project".into(),
                                            "Idle".into(),
                                            "Ready".into(),
                                            "Backlog".into(),
                                        ],
                                        rows: Vec::new(),
                                    },
                                    theme: None,
                                    refresh_ms: None,
                                    chrome: canvas_widgets::WidgetChrome::DragOnly,
                                    data: canvas_widgets::WidgetData::default(),
                                },
                            },
                            inputs: vec![InputPort {
                                id: "repos".into(),
                                contract: "canvas.widget.snapshot.v1".into(),
                            }],
                        }],
                        layout: Layout::Canvas {
                            placements: vec![Placement {
                                surface_id: "repo-status".into(),
                                bounds: NormalizedRect {
                                    x: 0.02,
                                    y: 0.1,
                                    width: 0.96,
                                    height: 0.8,
                                },
                                layer: 0,
                            }],
                        },
                        bindings: vec![DataBinding {
                            id: "repo-feed".into(),
                            source: DataSource {
                                owner: "alfalfa".into(),
                                stream: "repo-status".into(),
                                contract: "canvas.widget.snapshot.v1".into(),
                            },
                            target: InputTarget {
                                surface_id: "repo-status".into(),
                                input_id: "repos".into(),
                            },
                            adapter: Some("lockout-v1".into()),
                            delivery: DeliveryPolicy::default(),
                        }],
                        focused_surface_id: Some("repo-status".into()),
                    };
                    self.workspaces
                        .projects
                        .insert(project_id.clone(), document.clone());
                    self.workspaces.active_project_id = Some(name.clone());
                    if let Err(error) = workspace_store::save(
                        &self.workspace_store_path,
                        &self.workspaces,
                        &self.catalog,
                    ) {
                        self.fail(pending, Failure::platform(error.to_string()));
                        return Task::none();
                    }
                    self.start_transition();
                    self.document = document;
                    self.snapped_view = inferred_snapped_view(&self.document);
                    self.free_layout = None;
                    self.layout_policy = ::canvas::view::LayoutPolicy::default();
                    self.active_project_workspace = Some(name.clone());
                    self.publish_web_themes();
                    self.reset_workspace_runtime();
                    self.ensure_scene_decoded();
                    self.bump();
                    self.status =
                        format!("Provisioned workspace '{name}' with a repo-status table.");
                    self.emit_event(format!("workspace_provisioned {name}"));
                    Self::merge_alfalfa_projects(&mut self.catalog);
                    self.answer(
                        pending,
                        Outcome::Ok {
                            receipt: Receipt::WorkspaceSwitched { name },
                        },
                    );
                    Task::none()
                }
            }
            Command::WorkspaceRead { name } => {
                let Some(document) = self.workspaces.get(&name).cloned() else {
                    self.fail(
                        pending,
                        Failure::invalid(format!("unknown project workspace: {name}")),
                    );
                    return Task::none();
                };
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::WorkspaceDocument {
                            document: Box::new(document),
                            assigned_output: self
                                .workspaces
                                .assigned_output(&name)
                                .map(str::to_owned),
                        },
                    },
                );
                Task::none()
            }
            Command::WorkspaceImport {
                document,
                assigned_output,
                local_output,
                activate,
            } => {
                let name = document.project_id.clone();
                match self.import_portable_workspace(
                    *document,
                    assigned_output.clone(),
                    local_output,
                    activate,
                ) {
                    Ok(active) => {
                        self.status = format!(
                            "Imported workspace '{name}' for {}.",
                            assigned_output.as_deref().unwrap_or("the shared pool")
                        );
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::WorkspaceImported {
                                    name,
                                    assigned_output,
                                    active,
                                },
                            },
                        );
                    }
                    Err(failure) => self.fail(pending, failure),
                }
                Task::none()
            }
            Command::WorkspaceSync { projects } => {
                let base_catalog =
                    Catalog::bundled().expect("bundled project catalog must remain valid");
                let mut next = self.workspaces.clone();
                next.upsert(self.document.clone());
                next.active_project_id = Some(self.document.project_id.clone());
                let report = match next.sync(projects, &base_catalog) {
                    Ok(report) => report,
                    Err(error) => {
                        self.fail(pending, Failure::invalid(error.to_string()));
                        return Task::none();
                    }
                };
                let catalog = match next.effective_catalog(&base_catalog) {
                    Ok(catalog) => catalog,
                    Err(error) => {
                        self.fail(pending, Failure::invalid(error.to_string()));
                        return Task::none();
                    }
                };
                if let Err(error) =
                    workspace_store::save(&self.workspace_store_path, &next, &base_catalog)
                {
                    self.fail(pending, Failure::resource(error.to_string()));
                    return Task::none();
                }
                self.workspaces = next;
                self.catalog = catalog;
                self.active_project_workspace = Some(self.document.project_id.clone());
                self.publish_web_themes();
                self.bump();
                self.status = format!("Synchronized {} project workspaces.", report.total);
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::WorkspacesSynced {
                            created: report.created,
                            updated: report.updated,
                            preserved: report.preserved,
                            total: report.total,
                        },
                    },
                );
                Task::none()
            }
            Command::WorkspaceReset { project_id } => {
                let base_catalog =
                    Catalog::bundled().expect("bundled project catalog must remain valid");
                let mut next = self.workspaces.clone();
                next.upsert(self.document.clone());
                let document = match next.reset_to_default(&project_id, &base_catalog) {
                    Ok(document) => document,
                    Err(error) => {
                        self.fail(pending, Failure::invalid(error.to_string()));
                        return Task::none();
                    }
                };
                if let Err(error) =
                    workspace_store::save(&self.workspace_store_path, &next, &base_catalog)
                {
                    self.fail(pending, Failure::resource(error.to_string()));
                    return Task::none();
                }
                self.workspaces = next;
                if self.document.project_id == project_id {
                    self.start_transition();
                    self.document = document;
                    self.reset_workspace_runtime();
                    self.ensure_scene_decoded();
                }
                self.publish_web_themes();
                self.bump();
                self.status = format!("Reset project workspace '{project_id}'.");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::WorkspaceReset { project_id },
                    },
                );
                Task::none()
            }
            Command::SetBackground { path } => {
                let base_document_revision = self.document.revision;
                Task::perform(async move { load_background(path) }, move |result| {
                    Message::BackgroundLoaded {
                        pending: Box::new(pending),
                        base_document_revision,
                        result,
                    }
                })
            }
            Command::ClearBackground => {
                match self.apply_background(Background::Theme) {
                    Ok(()) => self.answer(
                        pending,
                        Outcome::Ok {
                            receipt: Receipt::DocumentApplied {
                                workspace_id: self.document.id.clone(),
                            },
                        },
                    ),
                    Err(failure) => self.fail(pending, failure),
                }
                self.emit_event("background_changed theme".to_string());
                Task::none()
            }
            Command::OpenStream { surface_id, url } => {
                let snapshot = SurfaceSnapshot {
                    id: surface_id.clone(),
                    title: format!("Stream: {url}"),
                    content: ContentKind::Stream,
                    asset_sha256: None,
                    page_index: None,
                    page_count: None,
                    source_pixels: None,
                    rendered_pixels: None,
                    display_logical: None,
                    scale_factor: None,
                };
                let runtime = RuntimeSurface {
                    snapshot,
                    source: ContentSource::Stream { url: url.clone() },
                    handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
                    source_pixels: [1, 1],
                    media_plan: MediaPlan {
                        viewport_logical: LogicalSize {
                            width: 1920.0,
                            height: 1080.0,
                        },
                        source_dimensions: SourceDimensions::RasterPixels(PixelSize {
                            width: 1,
                            height: 1,
                        }),
                        target_px: PixelSize {
                            width: 1,
                            height: 1,
                        },
                        display_logical: LogicalSize {
                            width: 1920.0,
                            height: 1080.0,
                        },
                        scale_factor: 1.0,
                        cache_key: MediaCacheKey {
                            source_id: format!("stream:{surface_id}"),
                            variant: MediaVariant::Raster,
                            pixel_size: PixelSize {
                                width: 1,
                                height: 1,
                            },
                        },
                    },
                    upload_pixels: [1, 1],
                };
                self.decoded.insert(surface_id.clone(), runtime);
                self.start_transition();
                self.bump();
                self.emit_event(format!("stream_opened {surface_id}"));
                self.status = format!("Opened stream {url} on {surface_id}.");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::SurfaceOpened {
                            surface: self
                                .decoded
                                .get(&surface_id)
                                .map(|s| s.snapshot.clone())
                                .unwrap_or_else(|| SurfaceSnapshot {
                                    id: surface_id.clone(),
                                    title: format!("Stream: {url}"),
                                    content: ContentKind::Stream,
                                    asset_sha256: None,
                                    page_index: None,
                                    page_count: None,
                                    source_pixels: None,
                                    rendered_pixels: None,
                                    display_logical: None,
                                    scale_factor: None,
                                }),
                        },
                    },
                );
                Task::none()
            }
            Command::OpenLiveVideo {
                surface_id,
                source_id,
                width,
                height,
            } => {
                let content = SurfaceContent::LiveVideo {
                    source_id: source_id.clone(),
                    width,
                    height,
                    orbitable: false,
                    timeline: false,
                };
                let Some(path) = canvas_core::video_frame_file(&source_id) else {
                    self.fail(pending, Failure::invalid("Invalid live video source id"));
                    return Task::none();
                };
                let surface = Surface {
                    id: surface_id.clone(),
                    title: source_id.clone(),
                    content,
                    inputs: Vec::new(),
                };
                if let Some(existing) = self
                    .document
                    .surfaces
                    .iter_mut()
                    .find(|candidate| candidate.id == surface_id)
                {
                    *existing = surface;
                } else {
                    self.document.surfaces.push(surface);
                }
                // A native video is a real Canvas surface, not merely a
                // decoder subscription. Ensure a newly opened source has
                // geometry immediately, while preserving every existing
                // hand-arranged placement.
                self.rebuild_stack();
                self.decoded.remove(&surface_id);
                self.ensure_scene_decoded();
                self.start_transition();
                self.bump();
                self.emit_event(format!(
                    "live_video_opened {surface_id} {source_id} {width}x{height}"
                ));
                self.status = format!("Opened live video {source_id} on {surface_id}.");
                let snapshot = self
                    .decoded
                    .get(&surface_id)
                    .map(|surface| surface.snapshot.clone());
                let _ = path;
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::SurfaceOpened {
                            surface: snapshot.expect("live video runtime was inserted"),
                        },
                    },
                );
                Task::none()
            }
            Command::OpenExternalSurface {
                surface_id,
                source_id,
            } => {
                let surface = Surface {
                    id: surface_id.clone(),
                    title: source_id.clone(),
                    content: SurfaceContent::ExternalSurface {
                        source_id: source_id.clone(),
                    },
                    inputs: Vec::new(),
                };
                if let Some(existing) = self
                    .document
                    .surfaces
                    .iter_mut()
                    .find(|candidate| candidate.id == surface_id)
                {
                    *existing = surface;
                } else {
                    self.document.surfaces.push(surface);
                }
                self.rebuild_stack();
                self.decoded.remove(&surface_id);
                self.ensure_scene_decoded();
                self.start_transition();
                self.bump();
                self.emit_event(format!("external_surface_opened {surface_id} {source_id}"));
                self.status = format!("Opened external surface {source_id} on {surface_id}.");
                let snapshot = self
                    .decoded
                    .get(&surface_id)
                    .map(|surface| surface.snapshot.clone())
                    .expect("external surface runtime was inserted");
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::SurfaceOpened { surface: snapshot },
                    },
                );
                Task::none()
            }
            Command::OpenWidget {
                surface_id,
                spec_path,
            } => {
                // Constrain widget specs to a sanctioned directory so an agent
                // cannot open arbitrary file paths through the widget renderer.
                let widget_dir = std::env::var("HOME")
                    .map(|h| std::path::PathBuf::from(h).join(".canvas/widgets"))
                    .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/.canvas/widgets"));
                let canonical_spec = match std::fs::canonicalize(&spec_path) {
                    Ok(path) => path,
                    Err(error) => {
                        self.fail(
                            pending,
                            Failure::invalid(format!(
                                "Cannot read widget spec at {spec_path:?}: {error}"
                            )),
                        );
                        return Task::none();
                    }
                };
                if !canonical_spec.starts_with(&widget_dir) {
                    self.fail(
                        pending,
                        Failure::invalid(format!(
                            "Widget spec {spec_path:?} must live under {widget_dir:?}"
                        )),
                    );
                    return Task::none();
                }
                match canvas_widgets::WidgetSpec::from_file(&canonical_spec) {
                    Ok(spec) => {
                        let title = spec.id.clone();
                        let snapshot = SurfaceSnapshot {
                            id: surface_id.clone(),
                            title: title.clone(),
                            content: ContentKind::Widget,
                            asset_sha256: None,
                            page_index: None,
                            page_count: None,
                            source_pixels: None,
                            rendered_pixels: None,
                            display_logical: None,
                            scale_factor: None,
                        };
                        let runtime = RuntimeSurface {
                            snapshot: snapshot.clone(),
                            source: ContentSource::Widget { spec: spec.clone() },
                            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
                            source_pixels: [1, 1],
                            media_plan: MediaPlan {
                                viewport_logical: LogicalSize {
                                    width: 1920.0,
                                    height: 1080.0,
                                },
                                source_dimensions: SourceDimensions::RasterPixels(PixelSize {
                                    width: 1,
                                    height: 1,
                                }),
                                target_px: PixelSize {
                                    width: 1,
                                    height: 1,
                                },
                                display_logical: LogicalSize {
                                    width: 1920.0,
                                    height: 1080.0,
                                },
                                scale_factor: 1.0,
                                cache_key: MediaCacheKey {
                                    source_id: format!("widget:{surface_id}"),
                                    variant: MediaVariant::Raster,
                                    pixel_size: PixelSize {
                                        width: 1,
                                        height: 1,
                                    },
                                },
                            },
                            upload_pixels: [1, 1],
                        };
                        self.decoded.insert(surface_id.clone(), runtime);
                        self.start_transition();
                        if let Some(surface) = self
                            .document
                            .surfaces
                            .iter_mut()
                            .find(|surface| surface.id == surface_id)
                        {
                            surface.title = title.clone();
                            surface.content = SurfaceContent::Widget { spec: spec.clone() };
                        } else {
                            self.document.surfaces.push(Surface {
                                id: surface_id.clone(),
                                title,
                                content: SurfaceContent::Widget { spec: spec.clone() },
                                inputs: if matches!(
                                    &spec.kind,
                                    canvas_widgets::WidgetKind::Quant { .. }
                                ) {
                                    vec![InputPort {
                                        id: "candles".into(),
                                        contract: "canvas.quant.candle.v1".into(),
                                    }]
                                } else {
                                    Vec::new()
                                },
                            });
                            let mut placements = match self.document.active_layout() {
                                Layout::Canvas { placements } => placements.clone(),
                                Layout::Surface { surface_id: sid } => {
                                    vec![Placement {
                                        surface_id: sid.clone(),
                                        bounds: NormalizedRect {
                                            x: 0.0,
                                            y: 0.0,
                                            width: 1.0,
                                            height: 1.0,
                                        },
                                        layer: 0,
                                    }]
                                }
                                _ => Vec::new(),
                            };
                            if !placements.iter().any(|p| p.surface_id == surface_id) {
                                placements.push(Placement {
                                    surface_id: surface_id.clone(),
                                    bounds: NormalizedRect {
                                        x: 0.0,
                                        y: 0.0,
                                        width: 1.0,
                                        height: 1.0,
                                    },
                                    layer: placements.len() as i32,
                                });
                            }
                            *self.document.active_layout_mut() = Layout::Canvas { placements };
                        }
                        self.document.revision = self.document.revision.saturating_add(1);
                        self.bump();
                        self.emit_event(format!("widget_opened {surface_id}"));
                        self.status = format!("Opened widget surface {surface_id}.");
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::SurfaceOpened { surface: snapshot },
                            },
                        );
                        Task::none()
                    }
                    Err(error) => {
                        self.fail(pending, Failure::invalid(error.0));
                        Task::none()
                    }
                }
            }
            Command::OpenOperationalView { view_id } => {
                match ::canvas::operational_views::materialize(
                    &view_id,
                    &self.document,
                    self.view_fit(),
                ) {
                    Ok(opened) => {
                        if let Err(error) = opened.document.validate(&self.catalog) {
                            self.fail(pending, Failure::invalid(error.to_string()));
                            return Task::none();
                        }
                        let surface_ids = placement_surface_ids(opened.document.active_layout());
                        let changed = opened.document != self.document;
                        if changed {
                            self.start_transition();
                            self.document = opened.document;
                            for surface_id in &surface_ids {
                                self.decoded.remove(surface_id);
                            }
                            self.ensure_scene_decoded();
                            self.bump();
                        }
                        // A newly opened view becomes what the snap key returns
                        // to, and any stashed arrangement belonged to the old
                        // view, so it must not survive the switch.
                        self.snapped_view = Some(view_id.clone());
                        self.free_layout = None;
                        self.jump_history.jump(JumpEntry::current(
                            &self.document.project_id,
                            Some(&view_id),
                        ));
                        self.layout_policy = ::canvas::operational_views::load(&view_id)
                            .map(|view| view.layout_policy)
                            .unwrap_or_default();
                        self.status = format!(
                            "Opened operational view {view_id}.{}",
                            undersized_note(&opened.undersized)
                        );
                        if changed {
                            self.emit_event(format!("operational_view_opened {view_id}"));
                        }
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::OperationalViewOpened {
                                    view_id,
                                    surface_ids,
                                },
                            },
                        );
                        Task::none()
                    }
                    Err(error) => {
                        self.fail(pending, Failure::invalid(error.to_string()));
                        Task::none()
                    }
                }
            }
            Command::OpenDataViz {
                surface_id,
                file,
                chart_type,
            } => {
                let parsed = parse_dataviz_file(&file, &chart_type);
                match parsed {
                    Ok(chart) => {
                        let chart_content = chart.clone();
                        let snapshot = SurfaceSnapshot {
                            id: surface_id.clone(),
                            title: file
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into(),
                            content: ContentKind::DataViz,
                            asset_sha256: None,
                            page_index: None,
                            page_count: None,
                            source_pixels: None,
                            rendered_pixels: None,
                            display_logical: None,
                            scale_factor: None,
                        };
                        let runtime = RuntimeSurface {
                            snapshot,
                            source: ContentSource::DataViz { chart },
                            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
                            source_pixels: [1, 1],
                            media_plan: MediaPlan {
                                viewport_logical: self.media_viewport(),
                                source_dimensions: SourceDimensions::RasterPixels(PixelSize {
                                    width: 1,
                                    height: 1,
                                }),
                                target_px: PixelSize {
                                    width: 1,
                                    height: 1,
                                },
                                display_logical: self.media_viewport(),
                                scale_factor: self.scale_factor,
                                cache_key: MediaCacheKey {
                                    source_id: format!("dataviz:{surface_id}"),
                                    variant: MediaVariant::Raster,
                                    pixel_size: PixelSize {
                                        width: 1,
                                        height: 1,
                                    },
                                },
                            },
                            upload_pixels: [1, 1],
                        };
                        let runtime_title = runtime.snapshot.title.clone();
                        let runtime_snapshot = runtime.snapshot.clone();
                        self.decoded.insert(surface_id.clone(), runtime);
                        self.start_transition();
                        if let Some(surface) = self
                            .document
                            .surfaces
                            .iter_mut()
                            .find(|surface| surface.id == surface_id)
                        {
                            surface.title = runtime_title.clone();
                            surface.content = SurfaceContent::Chart {
                                chart: chart_content.clone(),
                            };
                        } else {
                            self.document.surfaces.push(Surface {
                                id: surface_id.clone(),
                                title: runtime_title,
                                content: SurfaceContent::Chart {
                                    chart: chart_content,
                                },
                                inputs: Vec::new(),
                            });
                        }
                        let mut placements = match self.document.active_layout() {
                            Layout::Canvas { placements } => placements.clone(),
                            Layout::Surface { surface_id: sid } => vec![Placement {
                                surface_id: sid.clone(),
                                bounds: NormalizedRect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: 1.0,
                                    height: 1.0,
                                },
                                layer: 0,
                            }],
                            _ => Vec::new(),
                        };
                        if !placements.iter().any(|p| p.surface_id == surface_id) {
                            placements.push(Placement {
                                surface_id: surface_id.clone(),
                                bounds: NormalizedRect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: 1.0,
                                    height: 1.0,
                                },
                                layer: placements.len() as i32,
                            });
                        }
                        *self.document.active_layout_mut() = Layout::Canvas { placements };
                        self.bump();
                        self.status = format!("Opened data viz surface {surface_id}.");
                        self.emit_event(format!("surface_opened {surface_id}"));
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::SurfaceOpened {
                                    surface: runtime_snapshot,
                                },
                            },
                        );
                    }
                    Err(error) => self.fail(pending, Failure::invalid(error)),
                }
                Task::none()
            }
            Command::ScreensaverShader { index } => {
                let Some(shader) = screensaver::SHADERS.get(index) else {
                    self.fail(
                        pending,
                        Failure::invalid(format!(
                            "screensaver shader index {index} is out of range"
                        )),
                    );
                    return Task::none();
                };
                self.screensaver_index = index;
                self.screensaver_piece_elapsed = Duration::ZERO;
                self.screensaver_elapsed = Duration::ZERO;
                self.screensaver_last_frame = Some(Instant::now());
                self.bump();
                self.status = format!("Using screensaver shader {}.", shader.name);
                let visible = self.visible_background(&self.document.project_id);
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ScreensaverShaderSet {
                            index,
                            name: shader.name.to_string(),
                            visible: Some(visible),
                        },
                    },
                );
                Task::none()
            }
            Command::SetBackgroundShader {
                project_id,
                shader_id,
                speed,
            } => {
                let background = Background::Shader { shader_id, speed };
                let committed = if project_id == self.document.project_id {
                    // The same call F2 makes, so the protocol can say exactly
                    // what a human at the keyboard can say and nothing else.
                    self.commit_background(background)
                } else {
                    self.commit_stored_background(&project_id, background)
                };
                match committed {
                    Ok(()) => {
                        let visible = self.visible_background(&project_id);
                        self.answer(
                            pending,
                            Outcome::Ok {
                                receipt: Receipt::BackgroundShaderSet { visible },
                            },
                        );
                    }
                    Err(failure) => self.fail(pending, failure),
                }
                Task::none()
            }
            Command::Screenshot { surface_id, region } => {
                let window_id = self.windows.keys().next().copied();
                let Some(window_id) = window_id else {
                    self.fail(
                        pending,
                        Failure::platform("no Canvas window to screenshot".to_string()),
                    );
                    return Task::none();
                };
                let resolved_region = surface_id
                    .as_ref()
                    .and_then(|id| self.snapshot().surface_bounds.get(id).copied());
                let capture_region = region.or(resolved_region);
                let mut pending = Some(pending);
                iced::window::screenshot(window_id).map(move |screenshot| {
                    let pending = pending.take().expect("screenshot task runs once");
                    Message::ScreenshotCaptured {
                        pending: Box::new(pending),
                        region: capture_region,
                        screenshot,
                    }
                })
            }
        }
    }

    fn present_notification(&mut self, notification: Notification) {
        self.notifications
            .retain(|item| item.notification.id != notification.id);
        self.notifications.push(TransientNotification {
            notification,
            shown_at: Instant::now(),
        });
        if self.notifications.len() > MAX_VISIBLE_NOTIFICATIONS {
            self.notifications.remove(0);
        }
        self.bump();
    }

    fn refresh_terminal(&mut self) {
        let mut changed = false;
        for surface in self.decoded.values_mut() {
            if let ContentSource::Terminal {
                master_fd, buffer, ..
            } = &mut surface.source
            {
                let data = terminal::read_fd(*master_fd);
                if let Some(text) = String::from_utf8(data).ok().filter(|t| !t.is_empty()) {
                    buffer.push_str(&text);
                    changed = true;
                }
            }
        }
        if changed {
            self.bump();
        }
    }

    fn refresh_metrics(&mut self) {
        let elapsed = self.metrics_last.elapsed().as_secs_f64().max(1.0);
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::All,
            false,
            sysinfo::ProcessRefreshKind::nothing().with_cpu(),
        );
        self.networks.refresh(true);

        let cpus = self.system.cpus();
        let cpu_percent = if cpus.is_empty() {
            0.0
        } else {
            cpus.iter().map(sysinfo::Cpu::cpu_usage).sum::<f32>() / cpus.len() as f32
        };
        let cpu_cores: Vec<f32> = cpus
            .iter()
            .take(hud::MAX_HUD_CORES)
            .map(sysinfo::Cpu::cpu_usage)
            .map(|usage| usage.clamp(0.0, 100.0))
            .collect();
        let (ram_used, ram_total) = (self.system.used_memory(), self.system.total_memory());

        let (mut down, mut up) = (0u64, 0u64);
        for network in self.networks.list().values() {
            down = down.saturating_add(network.received());
            up = up.saturating_add(network.transmitted());
        }
        let net_down = down / elapsed as u64;
        let net_up = up / elapsed as u64;

        let (disk_used, disk_total) = sysinfo::Disks::new_with_refreshed_list()
            .iter()
            .find(|disk| disk.mount_point() == std::path::Path::new("/"))
            .map(|disk| {
                (
                    disk.total_space().saturating_sub(disk.available_space()),
                    disk.total_space(),
                )
            })
            .unwrap_or_default();

        let mut top: Vec<hud::TopProcess> = self
            .system
            .processes()
            .values()
            .map(|process| hud::TopProcess {
                name: process.name().to_string_lossy().chars().take(24).collect(),
                cpu_percent: process.cpu_usage(),
            })
            .filter(|process| !process.name.trim().is_empty())
            .collect();
        top.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        top.truncate(hud::MAX_HUD_PROCESSES);

        self.metrics = hud::LocalMetrics {
            name: self.metrics.name.clone(),
            cpu_percent: cpu_percent.clamp(0.0, 100.0),
            cpu_cores,
            ram_used,
            ram_total,
            net_down,
            net_up,
            disk_used,
            disk_total,
            top_processes: top,
            uptime: Duration::from_secs(sysinfo::System::uptime()),
        };
        self.metrics_last = Instant::now();
    }

    fn refresh_screensaver(&mut self, now: Instant) {
        let document_shader = match &self.document.background {
            Background::Shader { shader_id, speed } if *speed > 0.0 => Some((*shader_id, *speed)),
            _ => None,
        };
        // Only the animated background picker consumes `picker_elapsed`.
        // Treating the static project switcher as animated forced the entire
        // Canvas through the shader cadence while Cmd-K was open, making the
        // very control used to switch projects feel needlessly heavy.
        let picker_open = self.background_picker.is_some();
        if (!self.screensaver_settings.enabled && document_shader.is_none() && !picker_open)
            || !self.screensaver_clock.record_if_due(now)
        {
            return;
        }
        // Delta-time: advance the shader clock by the real gap since the last
        // rendered frame, clamped so a pause resumes smoothly instead of
        // jumping the animation forward by the whole gap.
        let delta = self
            .screensaver_last_frame
            .map(|last| now.duration_since(last).min(MAX_SCREENSAVER_FRAME_DELTA))
            .unwrap_or(Duration::ZERO);
        self.screensaver_last_frame = Some(now);
        if self.screensaver_settings.enabled {
            (self.screensaver_index, self.screensaver_piece_elapsed) = screensaver::advance_elapsed(
                self.screensaver_index,
                self.screensaver_piece_elapsed,
                delta,
                MAX_SCREENSAVER_FRAME_DELTA,
            );
        } else if let Some((shader_id, speed)) = document_shader {
            self.screensaver_index = screensaver::index_for(shader_id);
            self.screensaver_piece_elapsed += delta.mul_f32(speed);
        }
        // Advances on its own so the previews move even when the workspace
        // background does not.
        if picker_open {
            self.picker_elapsed += delta;
        }
        self.screensaver_elapsed = self.screensaver_piece_elapsed;
        self.screensaver_fps = self.screensaver_clock.measured_fps();
    }

    fn launcher_catalog(&self) -> ::canvas::shell_launcher::Catalog {
        let mut catalog = ::canvas::shell_launcher::Catalog::collect(&self.snapshot());
        for project in &self.catalog.projects {
            if !catalog.entries.iter().any(|e|matches!(&e.choice,::canvas::shell_launcher::Choice::Workspace{id} if id==&project.id)) {
                catalog.entries.push(::canvas::shell_launcher::Entry{id:format!("workspace:{}",project.id),title:project.name.clone(),detail:"WORKSPACE · available".into(),available:true,choice: ::canvas::shell_launcher::Choice::Workspace{id:project.id.clone()}});
            }
        }
        catalog.entries.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then_with(|| a.id.cmp(&b.id))
        });
        catalog
            .entries
            .truncate(::canvas::shell_launcher::MAX_ENTRIES);
        for entry in &mut catalog.entries {
            let ::canvas::shell_launcher::Choice::Surface { id } = &entry.choice else {
                continue;
            };
            if let Some(decoded) = self.decoded.get(id) {
                let path = match &decoded.source {
                    ContentSource::LiveVideo { path, .. }
                    | ContentSource::LiveImage { path, .. } => Some(path),
                    _ => None,
                };
                if let Some(path) = path {
                    let recent = std::fs::metadata(path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|age| age <= Duration::from_secs(2));
                    entry.available = recent;
                    entry.detail = if recent {
                        "WINDOW · live stream"
                    } else {
                        "WINDOW · disconnected / stale"
                    }
                    .into();
                }
            }
        }
        catalog.revision = format!(
            "{:x}",
            Sha256::digest(
                serde_json::to_vec(&(&self.document.project_id, &catalog.entries))
                    .expect("catalog serializes")
            )
        );
        catalog
    }

    fn dismiss_status_indicator(&mut self, id: &str, now: Instant) -> bool {
        let removed = self.indicators.remove(id);
        let did_remove = removed.is_some();
        if self.visible_indicator_id.as_deref() == Some(id) {
            self.visible_indicator_id = self.indicators.keys().next_back().cloned();
            if self.visible_indicator_id.is_none() {
                self.indicator_dismissing = removed.map(|value| (value, now));
            }
        }
        if id == canvas_workspace::observer::VOICE_INDICATOR_ID {
            self.voice_indicator_seen = None;
        }
        did_remove
    }

    fn expire_voice_indicator(&mut self, now: Instant) -> bool {
        let expiry = Duration::from_millis(u64::from(canvas_workspace::observer::MAX_TTL_MS));
        if self
            .voice_indicator_seen
            .is_some_and(|seen| now.saturating_duration_since(seen) >= expiry)
        {
            return self
                .dismiss_status_indicator(canvas_workspace::observer::VOICE_INDICATOR_ID, now);
        }
        false
    }

    fn snapshot(&self) -> RuntimeSnapshot {
        RuntimeSnapshot {
            workspace_id: self.document.id.clone(),
            project_id: self.document.project_id.clone(),
            revision: self.runtime_revision,
            frame: self.frame,
            focused_surface_id: self.document.focused_surface_id.clone(),
            surfaces: self
                .document
                .surfaces
                .iter()
                .map(|surface| {
                    self.decoded
                        .get(&surface.id)
                        .map(|decoded| decoded.snapshot.clone())
                        .unwrap_or_else(|| SurfaceSnapshot {
                            id: surface.id.clone(),
                            title: surface.title.clone(),
                            content: ContentKind::Workspace,
                            asset_sha256: match &surface.content {
                                SurfaceContent::Asset { asset } => Some(asset.sha256.clone()),
                                _ => None,
                            },
                            page_index: None,
                            page_count: None,
                            source_pixels: None,
                            rendered_pixels: None,
                            display_logical: None,
                            scale_factor: None,
                        })
                })
                .collect(),
            surface_bounds: self
                .document
                .surfaces
                .iter()
                .filter_map(|surface| {
                    Some((surface.id.clone(), self.surface_screen_bounds(&surface.id)?))
                })
                .collect(),
            views: self
                .document
                .surfaces
                .iter()
                .filter_map(|surface| {
                    Some(MountedViewSnapshot {
                        surface_id: surface.id.clone(),
                        bounds: self.surface_screen_bounds(&surface.id)?,
                        view: self.shared_views.get(&surface.id)?.clone(),
                    })
                })
                .collect(),
            active_indicators: self.indicators.values().cloned().collect(),
            mesh: self.mesh.clone(),
            presence: assemble_presence(self.presence.as_ref(), now_unix_ms()),
            presence_preview_active: self.presence_preview.is_some(),
            screensaver: ScreensaverSnapshot {
                settings: self.screensaver_settings,
                measured_fps: self.screensaver_clock.measured_fps(),
            },
            workspaces: self.workspaces.project_ids().map(str::to_owned).collect(),
            workspace_summaries: self
                .workspaces
                .projects
                .iter()
                .filter_map(|(project_id, document)| {
                    let project = self.catalog.project(project_id).ok()?;
                    Some(ProjectWorkspaceSnapshot {
                        project_id: project_id.clone(),
                        name: project.name.clone(),
                        status: self
                            .workspaces
                            .definitions
                            .get(project_id)
                            .map(|definition| definition.status.clone())
                            .unwrap_or_else(|| "active".into()),
                        surface_count: document.surfaces.len(),
                        active: self.document.project_id == *project_id,
                        assigned_output: self
                            .workspaces
                            .assigned_output(project_id)
                            .map(str::to_owned),
                        theme: project.theme,
                        background: document.background.clone(),
                    })
                })
                .collect(),
            minimized_surface_ids: self.minimized_surfaces.iter().cloned().collect(),
            maximized_surface_ids: self.maximized_surfaces.iter().cloned().collect(),
            events: self.events.clone(),
            display_frames: self.display_frames.clone(),
            displays: self.displays.clone(),
            selected_display_ids: self.selected_display_ids.clone(),
            overlays: self.overlays,
            visible: self.canvas_visible,
        }
    }

    fn surface_screen_bounds(&self, surface_id: &str) -> Option<Rect> {
        if self.minimized_surfaces.contains(surface_id) {
            return None;
        }
        let local = match self.document.active_layout() {
            Layout::Canvas { placements } => {
                let placement = placements
                    .iter()
                    .find(|placement| placement.surface_id == surface_id)?;
                let target = if self.maximized_surfaces.contains(surface_id) {
                    NormalizedRect {
                        x: 0.0,
                        y: 0.0,
                        width: 1.0,
                        height: 1.0,
                    }
                } else {
                    placement.bounds
                };
                let animated = self.presented_bounds(surface_id, target);
                visible_surface_bounds(
                    NormalizedRect {
                        x: animated[0],
                        y: animated[1],
                        width: animated[2],
                        height: animated[3],
                    },
                    self.frame,
                    self.document.chrome,
                )
            }
            layout => layout_surface_bounds(
                layout,
                surface_id,
                LogicalRect {
                    x: 0.0,
                    y: 0.0,
                    width: f64::from(self.frame.width),
                    height: f64::from(self.frame.height),
                },
            )?,
        };
        logical_to_screen_rect(local, self.frame)
    }

    fn emit_event(&mut self, event: impl Into<String>) {
        self.events.push(event.into());
        if self.events.len() > 100 {
            self.events.remove(0);
        }
    }

    fn handle_mouse(
        &mut self,
        window: iced::window::Id,
        event: iced::mouse::Event,
        captured: bool,
    ) -> Task<Message> {
        use iced::mouse::{Button, Event};

        let hid_event = event;
        let viewport = self.windows.get(&window).copied().unwrap_or(self.frame);
        let offset = LogicalPoint {
            x: f64::from(viewport.x - self.frame.x),
            y: f64::from(viewport.y - self.frame.y),
        };
        self.cursor_offset = offset;

        match event {
            Event::ButtonPressed(Button::Back) if self.focused_hid_target().is_none() => {
                self.jump_back();
                return Task::none();
            }
            Event::ButtonPressed(Button::Forward) if self.focused_hid_target().is_none() => {
                self.jump_forward();
                return Task::none();
            }
            _ => {}
        }

        if let Event::ButtonPressed(Button::Left) = &event
            && let Some(position) = self.cursor
            && self.activate_chrome_control(LogicalPoint {
                x: position.x - offset.x,
                y: position.y - offset.y,
            })
        {
            return Task::none();
        }
        // A widget claimed this press. Chrome has already had its chance above,
        // and content must not also act on it.
        if captured && matches!(event, Event::ButtonPressed(_)) {
            return Task::none();
        }

        let phase = match event {
            Event::CursorMoved { position } => {
                self.cursor = Some(LogicalPoint {
                    x: f64::from(position.x) + offset.x,
                    y: f64::from(position.y) + offset.y,
                });
                self.cursor_window = Some(window);
                self.primary_pointer_down.then_some(PointerPhase::Move)
            }
            Event::ButtonPressed(Button::Left) => {
                self.primary_pointer_down = true;
                Some(PointerPhase::Press)
            }
            Event::ButtonReleased(Button::Left) => {
                self.primary_pointer_down = false;
                Some(PointerPhase::Release)
            }
            Event::CursorLeft => {
                if self.cursor_window != Some(window) {
                    None
                } else if self.primary_pointer_down {
                    // The pointer left this display mid-drag (moved onto
                    // another window or monitor). The button-release will
                    // likely be delivered elsewhere or lost entirely, so
                    // commit the gesture now: this both docks an edge/corner
                    // drag that stalled off-frame and prevents the captured
                    // gesture from wedging all later input.
                    self.primary_pointer_down = false;
                    if self.external_surface_pointer.is_some()
                        && let Some(position) = self.cursor
                    {
                        let surfaces = self.interaction_surfaces();
                        self.handle_external_surface_pointer(
                            PointerPhase::Cancel,
                            position,
                            &surfaces,
                        );
                    }
                    self.cursor = None;
                    self.cursor_window = None;
                    let intents = self.interaction.force_release();
                    self.apply_interaction_intents(intents);
                    let preview = self.interaction.snap_preview();
                    if preview != self.snap_preview {
                        self.snap_preview = preview;
                        self.bump();
                    }
                    return Task::none();
                } else {
                    self.cursor = None;
                    self.cursor_window = None;
                    None
                }
            }
            _ => None,
        };
        let surfaces = self.interaction_surfaces();
        if let Some(position) = self.cursor
            && self.handle_hid_pointer(&hid_event, position, &surfaces)
        {
            return Task::none();
        }
        let (Some(phase), Some(position)) = (phase, self.cursor) else {
            if let Event::WheelScrolled { delta } = event
                && let Some(position) = self.cursor
                && let Ok(Some(target)) = hit_test(
                    &surfaces,
                    position,
                    SURFACE_RESIZE_HANDLE_SIZE,
                    f64::from(self.window_chrome().titlebar_height),
                )
                && let Some(decoded) = self.decoded.get(&target.surface_id)
                && let ContentSource::ExternalSurface { source_id } = &decoded.source
                && let Some(geometry) = surfaces.iter().find(|s| s.surface_id == target.surface_id)
            {
                let (dx, dy) = match delta {
                    iced::mouse::ScrollDelta::Lines { x, y } => (x * 40.0, y * 40.0),
                    iced::mouse::ScrollDelta::Pixels { x, y } => (x, y),
                };
                let x = ((position.x - geometry.bounds.x) / geometry.bounds.width).clamp(0.0, 1.0)
                    as f32;
                let y = ((position.y - geometry.bounds.y) / geometry.bounds.height).clamp(0.0, 1.0)
                    as f32;
                if let Err(error) = self.external_surfaces.send_scroll(
                    source_id,
                    x,
                    y,
                    dx.clamp(-4096.0, 4096.0),
                    dy.clamp(-4096.0, 4096.0),
                ) {
                    self.status = format!("Canvas external scroll failed: {error}");
                }
            }
            return Task::none();
        };
        if self.handle_external_surface_pointer(phase, position, &surfaces) {
            return Task::none();
        }
        if phase == PointerPhase::Press
            && let Ok(Some(target)) = hit_test(
                &surfaces,
                position,
                SURFACE_RESIZE_HANDLE_SIZE,
                f64::from(self.window_chrome().titlebar_height),
            )
            && self.decoded.get(&target.surface_id).is_some_and(|surface| {
                let orbitable = matches!(
                    &surface.source,
                    ContentSource::LiveVideo {
                        orbitable: true,
                        ..
                    }
                );
                content_claims_surface_drag(target.region, orbitable)
            })
        {
            // Focus and z-order are still ordinary surface concerns, but no
            // layout gesture is captured: subsequent movement belongs solely
            // to the retained-video orbit hook.
            self.desktop_focused = false;
            self.apply_interaction_intents(vec![
                InteractionIntent::Focus {
                    surface_id: target.surface_id.clone(),
                },
                InteractionIntent::PromoteToFront {
                    surface_id: target.surface_id,
                },
            ]);
            return Task::none();
        }
        match self.interaction.handle(
            PointerEvent {
                pointer_id: 0,
                phase,
                position,
            },
            &surfaces,
        ) {
            Ok(intents) => {
                if phase == PointerPhase::Press {
                    self.desktop_focused = intents.is_empty();
                    // Double-clicking bare desktop zooms the decorated host
                    // window exactly like a title-bar double-click: maximize or
                    // restore, never fullscreen. Kiosk owns the whole output, so
                    // it has no windowed zoom state to toggle.
                    let zoom = if self.desktop_focused && !self.kiosk {
                        self.desktop_zoom_gesture(position)
                    } else {
                        self.desktop_press = None;
                        false
                    };
                    self.apply_interaction_intents(intents);
                    if zoom {
                        self.status = "Zoomed the Canvas window.".into();
                        let preview = self.interaction.snap_preview();
                        if preview != self.snap_preview {
                            self.snap_preview = preview;
                            self.bump();
                        }
                        return iced::window::toggle_maximize(window);
                    }
                } else {
                    self.apply_interaction_intents(intents);
                }
                let preview = self.interaction.snap_preview();
                if preview != self.snap_preview {
                    self.snap_preview = preview;
                    self.bump();
                }
            }
            Err(error) => self.status = format!("Canvas pointer rejected: {error}"),
        }
        Task::none()
    }

    /// Fold an empty-desktop press into the host window's double-click zoom
    /// gesture. Returns true when the press completes a double-click.
    fn desktop_zoom_gesture(&mut self, position: LogicalPoint) -> bool {
        let now = Instant::now();
        let zoom = double_click(self.desktop_press.take(), now, position);
        self.desktop_press = (!zoom).then_some((now, position));
        zoom
    }

    fn focused_hid_target(&self) -> Option<String> {
        if self.desktop_focused || self.project_switcher.is_some() {
            return None;
        }
        let surface_id = self.document.focused_surface_id.as_deref()?;
        self.hid_target_for_surface(surface_id)
    }

    fn hid_target_for_surface(&self, surface_id: &str) -> Option<String> {
        self.document
            .surfaces
            .iter()
            .find(|surface| surface.id == surface_id)?
            .inputs
            .iter()
            .find(|input| input.contract == "input.desktop_state.v1")
            .map(|input| input.id.clone())
    }

    fn handle_hid_pointer(
        &mut self,
        event: &iced::mouse::Event,
        position: LogicalPoint,
        surfaces: &[SurfaceGeometry],
    ) -> bool {
        let pressed = matches!(event, iced::mouse::Event::ButtonPressed(_));
        if !pressed && self.desktop_focused {
            return false;
        }
        let surface_id = if pressed {
            let Ok(Some(target)) = hit_test(
                surfaces,
                position,
                SURFACE_RESIZE_HANDLE_SIZE,
                f64::from(self.window_chrome().titlebar_height),
            ) else {
                return false;
            };
            let has_title = self.document.chrome.title != SurfaceTitleMode::Hidden
                && !matches!(
                    self.widget_chrome(&target.surface_id),
                    Some(
                        canvas_widgets::WidgetChrome::Frameless
                            | canvas_widgets::WidgetChrome::Integrated
                    )
                );
            let content_region = target.region == HitRegion::Body
                || (!has_title && target.region == HitRegion::TopBar);
            if !content_region || self.hid_target_for_surface(&target.surface_id).is_none() {
                return false;
            }
            target.surface_id
        } else {
            let Some(surface_id) = self.document.focused_surface_id.clone() else {
                return false;
            };
            if self.hid_target_for_surface(&surface_id).is_none() {
                return false;
            }
            surface_id
        };
        let Some(geometry) = surfaces
            .iter()
            .find(|surface| surface.surface_id == surface_id)
        else {
            return false;
        };
        let Some(loaded) = self.decoded.get(&surface_id) else {
            return false;
        };
        let title_height = if self.document.chrome.title == SurfaceTitleMode::Hidden
            || matches!(
                self.widget_chrome(&surface_id),
                Some(
                    canvas_widgets::WidgetChrome::Frameless
                        | canvas_widgets::WidgetChrome::Integrated
                )
            ) {
            0.0
        } else {
            f64::from(self.window_chrome().titlebar_height)
        };
        let source_pixels = stream_source_pixels(loaded);
        let Some(content) =
            ::canvas::stream_geometry::fitted_content(geometry.bounds, source_pixels, title_height)
        else {
            return false;
        };
        let inside = position.x >= content.x
            && position.x <= content.x + content.width
            && position.y >= content.y
            && position.y <= content.y + content.height;
        if !inside && !matches!(event, iced::mouse::Event::ButtonReleased(_)) {
            return false;
        }
        if pressed {
            self.desktop_focused = false;
            self.apply_interaction_intents(vec![
                InteractionIntent::Focus {
                    surface_id: surface_id.clone(),
                },
                InteractionIntent::PromoteToFront {
                    surface_id: surface_id.clone(),
                },
            ]);
        }
        let Some(target_id) = self.hid_target_for_surface(&surface_id) else {
            return false;
        };
        let x = (position.x - content.x) / content.width;
        let y = (position.y - content.y) / content.height;
        if let Err(error) = self.hid_output.pointer(&target_id, x, y, event) {
            self.status = format!("Canvas HID pointer failed: {error}");
        }
        true
    }

    fn interaction_surfaces(&self) -> Vec<SurfaceGeometry> {
        let Layout::Canvas { placements } = self.document.active_layout() else {
            return self
                .document
                .surfaces
                .iter()
                .filter_map(|surface| {
                    if self.minimized_surfaces.contains(&surface.id) {
                        return None;
                    }
                    let bounds = layout_surface_bounds(
                        self.document.active_layout(),
                        &surface.id,
                        LogicalRect {
                            x: 0.0,
                            y: 0.0,
                            width: f64::from(self.frame.width),
                            height: f64::from(self.frame.height),
                        },
                    )?;
                    Some(SurfaceGeometry {
                        surface_id: surface.id.clone(),
                        bounds,
                        z_order: 0,
                    })
                })
                .collect();
        };
        let shell_height = self
            .overlay_on_viewport(self.overlays.fpl_mark, self.frame)
            .then_some(::canvas::DESKTOP_BAR_HEIGHT)
            .unwrap_or_default();
        let content_frame = Rect {
            height: self.frame.height.saturating_sub(shell_height).max(1),
            ..self.frame
        };
        placements
            .iter()
            .filter(|placement| !self.minimized_surfaces.contains(&placement.surface_id))
            .map(|placement| SurfaceGeometry {
                surface_id: placement.surface_id.clone(),
                bounds: self.presented_surface_geometry(
                    &placement.surface_id,
                    if self.maximized_surfaces.contains(&placement.surface_id) {
                        NormalizedRect {
                            x: 0.0,
                            y: 0.0,
                            width: 1.0,
                            height: 1.0,
                        }
                    } else {
                        placement.bounds
                    },
                    content_frame,
                    shell_height,
                ),
                z_order: if self.maximized_surfaces.contains(&placement.surface_id) {
                    i64::MAX
                } else {
                    i64::from(placement.layer)
                },
            })
            .collect()
    }

    fn presented_surface_geometry(
        &self,
        surface_id: &str,
        target: NormalizedRect,
        content_frame: Rect,
        shell_height: u32,
    ) -> LogicalRect {
        let animated = self.presented_bounds(surface_id, target);
        let mut bounds = visible_surface_bounds(
            NormalizedRect {
                x: animated[0],
                y: animated[1],
                width: animated[2],
                height: animated[3],
            },
            content_frame,
            self.document.chrome,
        );
        bounds.y += f64::from(shell_height);
        bounds
    }

    fn handle_external_surface_pointer(
        &mut self,
        phase: PointerPhase,
        position: LogicalPoint,
        surfaces: &[SurfaceGeometry],
    ) -> bool {
        use canvas_core::external_surface::ExternalSurfaceInputPhase;

        let capture = if phase == PointerPhase::Press {
            let Ok(Some(target)) = hit_test(
                surfaces,
                position,
                SURFACE_RESIZE_HANDLE_SIZE,
                f64::from(self.window_chrome().titlebar_height),
            ) else {
                return false;
            };
            if !matches!(target.region, HitRegion::Body | HitRegion::TopBar) {
                return false;
            }
            let Some(source_id) = self.decoded.get(&target.surface_id).and_then(|surface| {
                let ContentSource::ExternalSurface { source_id } = &surface.source else {
                    return None;
                };
                self.external_surfaces
                    .frame(source_id)
                    .filter(|frame| frame.descriptor.input.is_some())
                    .map(|_| source_id.clone())
            }) else {
                return false;
            };
            let capture = ExternalPointerCapture {
                surface_id: target.surface_id.clone(),
                source_id,
            };
            self.external_surface_pointer = Some(capture.clone());
            self.desktop_focused = false;
            self.apply_interaction_intents(vec![
                InteractionIntent::Focus {
                    surface_id: target.surface_id.clone(),
                },
                InteractionIntent::PromoteToFront {
                    surface_id: target.surface_id,
                },
            ]);
            capture
        } else {
            let Some(capture) = self.external_surface_pointer.clone() else {
                return false;
            };
            capture
        };

        let Some(geometry) = surfaces
            .iter()
            .find(|surface| surface.surface_id == capture.surface_id)
        else {
            self.external_surface_pointer = None;
            return true;
        };
        let x = ((position.x - geometry.bounds.x) / geometry.bounds.width).clamp(0.0, 1.0) as f32;
        let y = ((position.y - geometry.bounds.y) / geometry.bounds.height).clamp(0.0, 1.0) as f32;
        let input_phase = match phase {
            PointerPhase::Press => ExternalSurfaceInputPhase::Press,
            PointerPhase::Move => ExternalSurfaceInputPhase::Move,
            PointerPhase::Release | PointerPhase::Cancel => ExternalSurfaceInputPhase::Release,
        };
        if let Err(error) = self
            .external_surfaces
            .send_input(&capture.source_id, input_phase, x, y)
        {
            self.status = format!("Canvas external-surface input failed: {error}");
        }
        if matches!(phase, PointerPhase::Release | PointerPhase::Cancel) {
            self.external_surface_pointer = None;
        }
        true
    }

    fn widget_chrome(&self, surface_id: &str) -> Option<canvas_widgets::WidgetChrome> {
        let surface = self.decoded.get(surface_id)?;
        match &surface.source {
            ContentSource::Widget { spec } => Some(spec.effective_chrome()),
            // Native video is already a complete visual frame. Treating it as
            // an application window wastes pixels and makes a letterboxed
            // 16:9 stream look like content inside a differently-shaped card.
            ContentSource::LiveVideo { .. } | ContentSource::ExternalSurface { .. } => {
                Some(canvas_widgets::WidgetChrome::Frameless)
            }
            _ => None,
        }
    }

    /// Act on a click on a window control at `position`.
    ///
    /// Shared by the native pointer path and the browser/control path so a
    /// click means the same thing whichever host delivered it.
    fn activate_chrome_control(&mut self, position: LogicalPoint) -> bool {
        let previous = self.cursor;
        self.cursor = Some(position);
        let hit = self.hit_test_surface_control();
        self.cursor = previous;
        let Some((surface_id, action)) = hit else {
            return false;
        };
        self.desktop_press = None;
        self.desktop_focused = false;
        match action {
            WindowAction::Close => {
                if self.document.surfaces.iter().any(|s| s.id == surface_id) {
                    self.document.surfaces.retain(|s| s.id != surface_id);
                    self.document
                        .bindings
                        .retain(|b| b.target.surface_id != surface_id);
                    self.decoded.remove(&surface_id);
                    self.shared_views.remove(&surface_id);
                    self.minimized_surfaces.remove(&surface_id);
                    self.maximized_surfaces.remove(&surface_id);
                    self.rebuild_stack();
                    if self.document.focused_surface_id.as_ref() == Some(&surface_id) {
                        self.document.focused_surface_id =
                            self.document.surfaces.last().map(|s| s.id.clone());
                    }
                    self.bump();
                    self.status = format!("Closed {surface_id}.");
                }
            }
            WindowAction::Minimize => {
                if self.minimized_surfaces.insert(surface_id.clone()) {
                    self.bump();
                    self.status = format!("Minimized {surface_id}.");
                }
            }
            WindowAction::Maximize => {
                self.start_transition();
                self.status = if self.maximized_surfaces.remove(&surface_id) {
                    format!("Restored {surface_id}.")
                } else {
                    self.maximized_surfaces.clear();
                    self.maximized_surfaces.insert(surface_id.clone());
                    format!("Maximized {surface_id}.")
                };
                self.bump();
            }
        }
        true
    }

    fn hit_test_surface_control(&self) -> Option<(String, WindowAction)> {
        if self.document.chrome.title == SurfaceTitleMode::Hidden {
            return None;
        }
        // Frame-local, like every bound this function compares against. The
        // callers convert from a window/global point; subtracting a second time
        // here shifted every hit by the window offset, which is why a click on a
        // traffic light landed on nothing.
        let position = self.cursor?;
        // Surfaces are laid out in the *content* frame; the desktop bar is not
        // part of it. Measuring controls against the full frame offsets every
        // hit by the bar's height, which is why the traffic lights did nothing.
        let shell_height = self
            .overlay_on_viewport(self.overlays.fpl_mark, self.frame)
            .then_some(::canvas::DESKTOP_BAR_HEIGHT)
            .unwrap_or_default();
        let frame = Rect {
            height: self.frame.height.saturating_sub(shell_height).max(1),
            ..self.frame
        };
        let titlebar_height = f64::from(self.window_chrome().titlebar_height);
        let Layout::Canvas { placements } = self.document.active_layout() else {
            return None;
        };
        let mut placements = placements.iter().collect::<Vec<_>>();
        placements.sort_by_key(|placement| {
            std::cmp::Reverse((
                self.maximized_surfaces.contains(&placement.surface_id),
                placement.layer,
            ))
        });
        for placement in placements {
            // A surface that draws a title bar owns clickable controls. Only the
            // two chromes that draw *no* title bar opt out; `Integrated` used to
            // be skipped here while still drawing traffic lights, which is a
            // control that looks pressable and is not.
            if matches!(
                self.widget_chrome(&placement.surface_id),
                Some(
                    canvas_widgets::WidgetChrome::DragOnly
                        | canvas_widgets::WidgetChrome::Frameless
                )
            ) {
                continue;
            }
            let bounds = visible_surface_bounds(
                if self.maximized_surfaces.contains(&placement.surface_id) {
                    NormalizedRect {
                        x: 0.0,
                        y: 0.0,
                        width: 1.0,
                        height: 1.0,
                    }
                } else {
                    placement.bounds
                },
                frame,
                self.document.chrome,
            );
            // Drawing places every surface below the desktop bar; hit-testing
            // has to add the same offset or the traffic lights are 44px above
            // where they appear.
            let top = bounds.y + f64::from(shell_height);
            let bottom = top + titlebar_height;
            let left = bounds.x;
            let right_edge = bounds.x + bounds.width;
            if position.x < left
                || position.x > right_edge
                || position.y < top
                || position.y > bottom
            {
                continue;
            }
            let close_x = left + SURFACE_CONTROL_INSET;
            let minimize_x = close_x + SURFACE_CONTROL_HIT_SIZE;
            let maximize_x = minimize_x + SURFACE_CONTROL_HIT_SIZE;
            let sid = placement.surface_id.clone();
            if position.x >= close_x && position.x < close_x + SURFACE_CONTROL_HIT_SIZE {
                return Some((sid, WindowAction::Close));
            }
            if position.x >= minimize_x && position.x < minimize_x + SURFACE_CONTROL_HIT_SIZE {
                return Some((sid, WindowAction::Minimize));
            }
            if position.x >= maximize_x && position.x < maximize_x + SURFACE_CONTROL_HIT_SIZE {
                return Some((sid, WindowAction::Maximize));
            }
        }
        None
    }

    fn pointer_interaction(&self, window: iced::window::Id) -> mouse::Interaction {
        if self.cursor_window != Some(window) {
            return mouse::Interaction::None;
        }
        if self.hit_test_surface_control().is_some() {
            return mouse::Interaction::Pointer;
        }
        let Some(cursor) = self.cursor else {
            return mouse::Interaction::None;
        };
        let surfaces = self.interaction_surfaces();
        let Ok(Some(target)) = hit_test(
            &surfaces,
            cursor,
            SURFACE_RESIZE_HANDLE_SIZE,
            f64::from(self.window_chrome().titlebar_height),
        ) else {
            return mouse::Interaction::None;
        };
        match target.region {
            HitRegion::Resize { handle } => resize_cursor(handle),
            HitRegion::Body | HitRegion::TopBar => mouse::Interaction::None,
        }
    }

    fn apply_interaction_intents(&mut self, intents: Vec<InteractionIntent>) {
        let mut changed = false;
        for intent in intents {
            match intent {
                InteractionIntent::Focus { surface_id } => {
                    self.desktop_focused = false;
                    if self.document.focused_surface_id.as_ref() != Some(&surface_id) {
                        self.document.focused_surface_id = Some(surface_id);
                        self.begin_focus_emphasis();
                        changed = true;
                    }
                }
                InteractionIntent::PromoteToFront { surface_id } => {
                    let Layout::Canvas { placements } = self.document.active_layout_mut() else {
                        continue;
                    };
                    let top = placements
                        .iter()
                        .map(|placement| placement.layer)
                        .max()
                        .unwrap_or(0);
                    if let Some(placement) = placements
                        .iter_mut()
                        .find(|placement| placement.surface_id == surface_id)
                        && placement.layer != top
                    {
                        placement.layer = top.saturating_add(1);
                        changed = true;
                    }
                }
                InteractionIntent::SetBounds { surface_id, bounds } => {
                    let frame = self.frame;
                    let chrome = self.document.chrome;
                    let Layout::Canvas { placements } = self.document.active_layout_mut() else {
                        continue;
                    };
                    if let Some(placement) = placements
                        .iter_mut()
                        .find(|placement| placement.surface_id == surface_id)
                    {
                        let normalized = normalize_visible_surface_bounds(bounds, frame, chrome);
                        if placement.bounds != normalized {
                            placement.bounds = normalized;
                            changed = true;
                        }
                    }
                }
                InteractionIntent::CapturePointer { .. }
                | InteractionIntent::ReleasePointer { .. } => {}
            }
        }
        if changed {
            self.document.revision = self.document.revision.saturating_add(1);
            self.bump();
        }
    }

    fn answer(&self, pending: Pending, outcome: Outcome) {
        let _ = pending.reply.send(ControlReply {
            schema_version: CONTROL_VERSION,
            request_id: pending.request.request_id,
            revision: self.runtime_revision,
            outcome,
        });
    }

    fn fail(&self, pending: Pending, failure: Failure) {
        self.answer(
            pending,
            Outcome::Error {
                code: failure.code,
                message: failure.message,
            },
        );
    }

    fn bump(&mut self) {
        if self.window_cycle.active()
            && self.window_cycle_workspace.as_deref() != Some(self.document.project_id.as_str())
        {
            self.window_cycle.cancel();
            self.project_switcher = None;
        }
        if let Some(id) = self.document.focused_surface_id.as_deref() {
            self.window_cycle.observe_focus(id);
        }
        #[cfg(feature = "lua-scenes")]
        self.refresh_desktop_bar(false);
        self.runtime_revision = self.runtime_revision.saturating_add(1);
        // Every document change lands here, which is the only place that sees
        // all of them: applying a board, switching project and paging all
        // change what is on screen, and hooking them individually missed the
        // one that mattered. Cheap because it only writes when the set differs.
        self.publish_wanted_video();
        #[cfg(unix)]
        {
            let assigned_output = self
                .workspaces
                .assigned_output(&self.document.project_id)
                .map(str::to_owned);
            self.workspace_mirror
                .publish_if_changed(&self.document, assigned_output.as_deref());
        }
    }

    /// Retain one complete workspace generation and reconcile whether this
    /// output should present it. The edge decides `activate` from its own
    /// stable output identity; Canvas owns persistence and presentation.
    fn import_portable_workspace(
        &mut self,
        document: WorkspaceDocument,
        assigned_output: Option<String>,
        local_output: Option<String>,
        activate: bool,
    ) -> Result<bool, Failure> {
        let project_id = document.project_id.clone();
        let mut next = self.workspaces.clone();
        if !next.definitions.contains_key(&project_id) {
            let theme = self
                .catalog
                .project(&project_id)
                .map(|project| project.theme)
                .unwrap_or(canvas_core::ThemeSelection::Coherence {
                    mode: canvas_core::ThemeMode::Dark,
                    accent: canvas_core::Accent::Info,
                });
            next.definitions.insert(
                project_id.clone(),
                ::canvas::control::ProjectWorkspaceSpec {
                    project_id: project_id.clone(),
                    name: project_id.replace('-', " "),
                    status: "active".into(),
                    theme,
                },
            );
        }
        if self.active_project_workspace.as_deref() == Some(self.document.project_id.as_str()) {
            next.upsert(self.document.clone());
        }
        next.upsert(document.clone());
        next.assign_output(&project_id, assigned_output);

        let replacement = if activate {
            next.activate(&project_id);
            Some(document)
        } else if self.document.project_id == project_id {
            let fallback = next.previous_project_id().map(str::to_owned).or_else(|| {
                next.project_ids()
                    .find(|id| *id != project_id)
                    .map(str::to_owned)
            });
            let fallback = fallback.unwrap_or_else(|| {
                let idle = "canvas-idle".to_string();
                next.definitions.entry(idle.clone()).or_insert_with(|| {
                    ::canvas::control::ProjectWorkspaceSpec {
                        project_id: idle.clone(),
                        name: "Canvas Idle".into(),
                        status: "system".into(),
                        theme: canvas_core::ThemeSelection::Coherence {
                            mode: canvas_core::ThemeMode::Dark,
                            accent: canvas_core::Accent::Info,
                        },
                    }
                });
                next.projects
                    .entry(idle.clone())
                    .or_insert_with(|| WorkspaceDocument {
                        schema_version: SCHEMA_VERSION,
                        id: "canvas-idle-workspace".into(),
                        project_id: idle.clone(),
                        revision: 1,
                        theme: ThemeRef::Project,
                        background: Background::Theme,
                        chrome: SurfaceChrome::default(),
                        surfaces: Vec::new(),
                        layout: Layout::Empty,
                        pages: Vec::new(),
                        active_page: 0,
                        bindings: Vec::new(),
                        focused_surface_id: None,
                    });
                idle
            });
            next.activate(&fallback);
            next.assign_output(&fallback, local_output);
            next.get(&fallback).cloned()
        } else {
            None
        };

        let base_catalog = Catalog::bundled().expect("bundled project catalog must remain valid");
        let catalog = next
            .effective_catalog(&base_catalog)
            .map_err(|error| Failure::invalid(error.to_string()))?;
        workspace_store::save(&self.workspace_store_path, &next, &base_catalog)
            .map_err(|error| Failure::resource(error.to_string()))?;
        self.workspaces = next;
        self.catalog = catalog;

        let became_active = replacement.is_some();
        if let Some(document) = replacement {
            self.ensure_background_available(&document)?;
            self.start_transition();
            self.document = document;
            self.snapped_view = inferred_snapped_view(&self.document);
            self.free_layout = None;
            self.layout_policy = self
                .snapped_view
                .as_deref()
                .and_then(|view_id| ::canvas::operational_views::load(view_id).ok())
                .map(|view| view.layout_policy)
                .unwrap_or_default();
            self.active_project_workspace = Some(self.document.project_id.clone());
            self.publish_web_themes();
            self.reset_workspace_runtime();
            self.ensure_scene_decoded();
            self.reset_background_animation();
        }
        self.bump();
        Ok(became_active && self.document.project_id == project_id)
    }

    /// Open a Canvas-native text input overlay for JARVIS. Unlike the
    /// osascript dialog, this renders inside the Canvas surface itself — the
    /// text appears on the wall, not as a separate macOS window.
    fn handle_jarvis_text_input(&mut self, kbd_event: iced::keyboard::Event) -> Task<Message> {
        use iced::keyboard::{Event, Key, key::Named};
        let Event::KeyPressed { key, text, .. } = kbd_event else {
            return Task::none();
        };
        match key {
            Key::Named(Named::Backspace) => {
                let _ = self.jarvis_text.pop();
                Task::none()
            }
            Key::Named(Named::Enter) => {
                self.submit_jarvis_text();
                Task::none()
            }
            Key::Named(Named::Escape) => {
                self.jarvis_text_visible = false;
                self.jarvis_text.clear();
                self.bump();
                Task::none()
            }
            Key::Character(c) => {
                if let Some(text) = text {
                    self.jarvis_text.push_str(&text);
                }
                let _ = c;
                Task::none()
            }
            _ => Task::none(),
        }
    }

    fn toggle_jarvis_text(&mut self) {
        self.jarvis_text_visible = !self.jarvis_text_visible;
        self.jarvis_text.clear();
        self.status = if self.jarvis_text_visible {
            "Type your message to JARVIS. Enter to send, Esc to cancel.".into()
        } else {
            "JARVIS text dialog closed.".into()
        };
        self.bump();
        self.emit_event(format!(
            "jarvis_text {}",
            if self.jarvis_text_visible {
                "shown"
            } else {
                "hidden"
            }
        ));
    }

    /// Submit the visible jarvis_text to jarvis text-turn and capture the reply.
    fn submit_jarvis_text(&mut self) {
        let text = self.jarvis_text.clone();
        if text.is_empty() {
            return;
        }
        self.jarvis_text_visible = false;
        self.jarvis_text.clear();
        self.status = "Sending to JARVIS...".into();
        let home = std::env::var("HOME").unwrap_or_default();
        let jarvis_root = std::path::PathBuf::from(&home).join("Documents/src/fpl/JARVIS");
        let jarvis_bin = jarvis_root.join("target/debug/jarvis");
        if jarvis_bin.exists() {
            match std::process::Command::new(&jarvis_bin)
                .arg("text-turn")
                .arg(&text)
                .current_dir(&jarvis_root)
                .env(
                    "JARVIS_VOICE_TRANSCRIPT_LOG_DIR",
                    format!("{}/.jarvis/transcripts", home),
                )
                .output()
            {
                Ok(output) => {
                    let reply = String::from_utf8_lossy(&output.stdout).to_string();
                    self.jarvis_text_reply = Some(reply.trim().to_string());
                    self.status = format!(
                        "JARVIS replied: {}",
                        self.jarvis_text_reply.as_deref().unwrap_or("")
                    );
                }
                Err(_) => {
                    self.status = "JARVIS binary error".to_string();
                }
            }
        } else {
            self.status = "JARVIS binary not found".into();
        }
        self.bump();
        self.emit_event("jarvis_text_submitted".to_string());
    }

    fn apply_background(&mut self, background: Background) -> Result<(), Failure> {
        let patch = WorkspacePatch {
            schema_version: SCHEMA_VERSION,
            workspace_id: self.document.id.clone(),
            base_revision: self.document.revision,
            revision: self.document.revision.saturating_add(1),
            operations: vec![PatchOperation::SetBackground { background }],
        };
        let next = self
            .document
            .apply_patch(&patch, &self.catalog)
            .map_err(|error| Failure::invalid(error.to_string()))?;
        self.ensure_background_available(&next)?;
        self.document = next;
        self.reset_background_animation();
        self.bump();
        self.status = "Canvas background updated.".into();
        self.emit_event("background_changed image".to_string());
        Ok(())
    }

    fn ensure_background_available(&self, document: &WorkspaceDocument) -> Result<(), Failure> {
        if let Background::Image { asset, .. } = &document.background
            && !self.background_assets.contains_key(&asset.sha256)
        {
            return Err(Failure::invalid(format!(
                "background asset {} has not been imported into this Canvas runtime",
                asset.sha256
            )));
        }
        Ok(())
    }

    fn reset_background_animation(&mut self) {
        self.screensaver_piece_elapsed = Duration::ZERO;
        self.screensaver_elapsed = Duration::ZERO;
        self.screensaver_last_frame = Some(Instant::now());
        self.screensaver_index =
            screensaver::workspace_index(&self.document.id, &self.document.background);
    }

    fn rebuild_stack(&mut self) {
        let ids = self
            .document
            .surfaces
            .iter()
            .map(|surface| surface.id.clone())
            .collect::<Vec<_>>();
        if ids.is_empty() {
            *self.document.active_layout_mut() = Layout::Empty;
            self.document.revision = self.document.revision.saturating_add(1);
            return;
        }
        let existing = match self.document.active_layout() {
            Layout::Canvas { placements } => placements
                .iter()
                .map(|placement| (placement.surface_id.clone(), placement.clone()))
                .collect::<BTreeMap<_, _>>(),
            _ => BTreeMap::new(),
        };
        let aspect = f64::from(self.frame.width.max(1)) / f64::from(self.frame.height.max(1));
        if existing.is_empty() {
            // A stack nobody has arranged yet earns a composition, not a
            // pile: the golden mosaic fills the wall in one deliberate pass
            // instead of cascading centered rectangles over each other.
            *self.document.active_layout_mut() = Layout::Canvas {
                placements: ::canvas::compose::compose(
                    ::canvas::compose::LayoutMode::Golden,
                    &ids,
                    aspect as f32,
                ),
            };
            self.document.revision = self.document.revision.saturating_add(1);
            return;
        }
        let placements = ids
            .into_iter()
            .enumerate()
            .map(|(index, surface_id)| {
                existing.get(&surface_id).cloned().unwrap_or_else(|| {
                    // The arrangement is already spoken for, so a newcomer
                    // lands as a golden-sized tile with the old cascade
                    // nudge; overlap with hand-placed surfaces is the
                    // user's to resolve, and it promotes to front.
                    let cascade = (index % 6) as f32 * 0.025;
                    Placement {
                        surface_id,
                        bounds: NormalizedRect {
                            x: ((1.0 - 0.618) * 0.5 + cascade).min(1.0 - 0.618),
                            y: ((1.0 - 0.618) * 0.5 + cascade).min(1.0 - 0.618),
                            width: 0.618,
                            height: 0.618,
                        },
                        layer: index as i32,
                    }
                })
            })
            .collect();
        *self.document.active_layout_mut() = Layout::Canvas { placements };
        self.document.revision = self.document.revision.saturating_add(1);
    }

    fn prune_decoded(&mut self) {
        self.decoded.retain(|id, decoded| {
            self.document.surfaces.iter().any(|surface| {
                surface.id == *id
                    && match &surface.content {
                        SurfaceContent::Asset { asset } => {
                            decoded.snapshot.asset_sha256.as_ref() == Some(&asset.sha256)
                        }
                        // A bound widget is *meant* to diverge from its
                        // template: stream bindings mutate the retained spec
                        // with live values. Comparing the whole spec therefore
                        // discarded the live widget on every document
                        // re-application, so the surface fell back to its
                        // placeholder and then refilled — the decision widgets
                        // flashed "loading" and then the new state instead of
                        // updating in place. Identity is the widget's id and
                        // family, as it already is for live video below.
                        SurfaceContent::Widget { spec } => {
                            let ContentSource::Widget { spec: decoded_spec } =
                                &mut decoded.source
                            else {
                                return false;
                            };
                            let same_widget = decoded_spec.id == spec.id
                                && std::mem::discriminant(&decoded_spec.kind)
                                    == std::mem::discriminant(&spec.kind);
                            if same_widget {
                                // The retained value/data belongs to the live
                                // stream, while chrome belongs to the applied
                                // workspace. Keeping the former must not make
                                // presentation edits appear to do nothing.
                                decoded_spec.chrome = spec.chrome;
                                decoded_spec.refresh_ms = spec.refresh_ms;
                            }
                            same_widget
                        }
                        SurfaceContent::Chart { chart } => matches!(
                            &decoded.source,
                            ContentSource::DataViz { chart: decoded_chart } if decoded_chart == chart
                        ),
                        SurfaceContent::Scene { scene } => matches!(
                            &decoded.source,
                            ContentSource::Scene { scene: decoded_scene } if decoded_scene == scene
                        ),
                        SurfaceContent::LiveImage { source_id } => matches!(
                            &decoded.source,
                            ContentSource::LiveImage {
                                source_id: decoded_source_id,
                                ..
                            } if decoded_source_id == source_id
                        ),
                        SurfaceContent::LiveVideo {
                            source_id,
                            width,
                            height,
                            orbitable,
                            timeline,
                        } => matches!(
                            &decoded.source,
                            ContentSource::LiveVideo {
                                source_id: decoded_source_id,
                                width: decoded_width,
                                height: decoded_height,
                                orbitable: decoded_orbitable,
                                timeline: decoded_timeline,
                                ..
                            } if decoded_source_id == source_id
                                && decoded_width == width
                                && decoded_height == height
                                && decoded_orbitable == orbitable
                                && decoded_timeline == timeline
                        ),
                        // Identity is the target, never the draft: the draft is
                        // runtime state the document does not carry, so
                        // comparing the whole source would discard a
                        // half-typed prompt on every document re-application —
                        // the words would vanish mid-sentence.
                        SurfaceContent::Prompt {
                            target, session, ..
                        } => matches!(
                            &decoded.source,
                            ContentSource::Prompt {
                                target: decoded_target,
                                session: decoded_session,
                                ..
                            } if decoded_target == target && decoded_session == session
                        ),
                        _ => true,
                    }
            })
        });
        self.shared_views
            .retain(|id, _| self.decoded.contains_key(id));
    }

    /// A workspace document is a presentation identity boundary. Surface and
    /// binding ids are intentionally reusable across projects (every inherited
    /// dashboard has `project-html`, `project-goals`, and friends), so retaining
    /// runtime state by those local ids would display the departed project's
    /// pixels and values in the incoming workspace.
    fn reset_workspace_runtime(&mut self) {
        self.refresh_interaction_geometry();
        self.decoded.clear();
        self.shared_views.clear();
        self.live_pending.clear();
        self.publish_active_project();
        self.publish_wanted_video();
        #[cfg(feature = "lua-scenes")]
        {
            self.stream_sequences.clear();
            self.stream_last_applied.clear();
            self.stream_started_at = Instant::now();
            // A workspace switch is a new reader of retained state. Replaying
            // the tail hydrates its widgets immediately; waiting only for new
            // packets leaves a quiet project saying "Loading" indefinitely.
            self.stream_position = self.stream_inbox.replay_tail(256);
            // Bindings mutate decoded runtime surfaces, not the persisted
            // loading-state document. Decode the incoming workspace before
            // replaying or every cached event is rejected as a missing target.
            self.ensure_scene_decoded();
            self.drain_stream_events();
        }
    }

    fn refresh_interaction_geometry(&mut self) {
        if self.interaction.captured_pointer().is_none() {
            let shell_height = self
                .overlay_on_viewport(self.overlays.fpl_mark, self.frame)
                .then_some(::canvas::DESKTOP_BAR_HEIGHT)
                .unwrap_or_default();
            self.interaction = interaction_engine_with_shell(
                self.frame,
                self.document.chrome,
                self.window_chrome().titlebar_height,
                shell_height,
            );
        }
    }

    /// Publish the video sources this Canvas is actually showing.
    ///
    /// The media edge decodes whatever is announced, which means a camera on a
    /// board nobody is looking at still burns a decoder. On the Pi that is not
    /// merely wasteful: its H.264 decoding is hardware, and the buffer pool ran
    /// out once there were enough streams, after which nothing decoded at all.
    /// Only Canvas knows what is on screen, so only Canvas can say.
    ///
    /// Written as a plain list under the runtime root, the same way the active
    /// project is published. An empty or missing file means "no preference",
    /// never "nothing" — a reader that cannot find this must keep decoding
    /// rather than blank the wall.
    fn navigate_pdf(
        &mut self,
        surface_id: String,
        target: Navigation,
        pending: Option<Pending>,
    ) -> Task<Message> {
        let Some(surface) = self.decoded.get(&surface_id) else {
            self.finish_pdf_failure(
                pending,
                Failure {
                    code: ErrorCode::NotFound,
                    message: format!("unknown decoded Canvas surface: {surface_id}"),
                },
            );
            return Task::none();
        };
        let ContentSource::Pdf {
            path,
            page_index,
            page_count,
        } = &surface.source
        else {
            self.finish_pdf_failure(pending, Failure::unsupported("surface is not navigable"));
            return Task::none();
        };
        let next = match target {
            Navigation::Next => page_index.saturating_add(1),
            Navigation::Previous => page_index.saturating_sub(1),
            Navigation::Page { index } => index,
        };
        if next >= *page_count {
            self.finish_pdf_failure(
                pending,
                Failure::invalid(format!(
                    "PDF page index {next} is outside 0..{}",
                    page_count
                )),
            );
            return Task::none();
        }
        let path = path.clone();
        let source_id = surface
            .snapshot
            .asset_sha256
            .clone()
            .unwrap_or_else(|| surface_id.clone());
        let viewport = self.media_viewport();
        let scale_factor = self.scale_factor;
        Task::perform(
            async move {
                let (handle, pixels) = render_pdf_page(&path, next)?;
                let plan = plan_pdf_page(
                    source_id,
                    next,
                    LogicalSize {
                        width: f64::from(pixels[0]),
                        height: f64::from(pixels[1]),
                    },
                    viewport,
                    scale_factor,
                    MediaLimits::default(),
                )
                .map_err(|error| Failure::resource(error.to_string()))?;
                Ok((handle, pixels, plan))
            },
            move |result| Message::PageLoaded {
                pending: pending.map(Box::new),
                surface_id,
                page_index: next,
                result: Box::new(result),
            },
        )
    }

    fn finish_pdf_failure(&mut self, pending: Option<Pending>, failure: Failure) {
        if let Some(pending) = pending {
            self.fail(pending, failure);
        } else {
            self.status = failure.message.clone();
            eprintln!("Canvas: PDF navigation failed: {}", failure.message);
        }
    }

    fn handle_model_orbit(
        &mut self,
        stream_id: &str,
        event: canvas_model_viewer::video::OrbitEvent,
    ) {
        use canvas_model_viewer::state::{Gesture, Input};
        use canvas_model_viewer::video::OrbitEvent;
        let gesture = match event {
            OrbitEvent::Press(at) => Gesture::Press(at),
            OrbitEvent::Drag(to, size) => Gesture::Drag(to, size),
            OrbitEvent::Release => Gesture::Release,
            OrbitEvent::Zoom(steps) => Gesture::Zoom(steps),
        };
        self.apply_model_viewer_input(stream_id, Input::Gesture(gesture));
    }

    fn apply_model_viewer_input(
        &mut self,
        stream_id: &str,
        input: canvas_model_viewer::state::Input,
    ) {
        let update = self
            .model_viewer
            .apply(stream_id, input, Instant::now(), unix_ms());
        if let Some(output) = update.output {
            if let Err(error) = publish_model_camera(&output.pose) {
                eprintln!(
                    "Canvas: could not publish model viewer state for {stream_id}: {error:#}"
                );
            }
            self.status = output.status;
        }
        if update.changed {
            // The Neo owns the interaction chrome. Repaint its cube for every
            // accepted gesture even when Studio publication is rate-limited.
            self.bump();
        }
    }

    fn handle_browser_pointer(&mut self, phase: &str, x: f32, y: f32, delta: f32) {
        if self.project_switcher.is_some() {
            return;
        }
        use canvas_model_viewer::video::OrbitEvent;

        let snapshot = self.snapshot();
        let global = iced::Point::new(
            self.frame.x as f32 + x * self.frame.width as f32,
            self.frame.y as f32 + y * self.frame.height as f32,
        );
        // A stream can remain grabbed if its surface disappears before the
        // browser delivers pointer-up (workspace switches are the common
        // case). Never let that stale controller capture gestures intended
        // for a model that is visible in the current document.
        let grabbed = snapshot
            .surface_bounds
            .iter()
            .find_map(|(surface_id, bounds)| {
                let surface = self
                    .document
                    .surfaces
                    .iter()
                    .find(|surface| &surface.id == surface_id)?;
                let SurfaceContent::LiveVideo {
                    source_id,
                    orbitable: true,
                    ..
                } = &surface.content
                else {
                    return None;
                };
                self.model_viewer
                    .is_grabbed(source_id)
                    .then(|| (source_id.clone(), *bounds))
            });
        // Window controls are chrome, not content: a click on a traffic light
        // must act before any surface sees it.
        if phase == "press"
            && self.activate_chrome_control(LogicalPoint {
                // `x`/`y` are normalized to the window, and chrome hit-testing
                // works in window-local space.
                x: f64::from(x) * f64::from(self.frame.width),
                y: f64::from(y) * f64::from(self.frame.height),
            })
        {
            return;
        }

        let hit = snapshot
            .surface_bounds
            .iter()
            .find_map(|(surface_id, bounds)| {
                let inside = global.x >= bounds.x as f32
                    && global.y >= bounds.y as f32
                    && global.x <= (bounds.x + bounds.width as i32) as f32
                    && global.y <= (bounds.y + bounds.height as i32) as f32;
                if !inside {
                    return None;
                }
                let surface = self
                    .document
                    .surfaces
                    .iter()
                    .find(|surface| &surface.id == surface_id)?;
                let SurfaceContent::LiveVideo {
                    source_id,
                    orbitable: true,
                    ..
                } = &surface.content
                else {
                    return None;
                };
                Some((source_id.clone(), *bounds))
            });
        let target = if let Some(grabbed) = grabbed {
            Some(grabbed)
        } else {
            hit
        };
        let Some((stream, bounds)) = target else {
            return;
        };
        let local = iced::Point::new(global.x - bounds.x as f32, global.y - bounds.y as f32);
        let size = iced::Size::new(bounds.width as f32, bounds.height as f32);
        let event = match phase {
            "press" => OrbitEvent::Press(local),
            "drag" => OrbitEvent::Drag(local, size),
            "release" => OrbitEvent::Release,
            "zoom" => OrbitEvent::Zoom(delta),
            _ => return,
        };
        self.handle_model_orbit(&stream, event);
    }

    /// The model stream the focused surface is showing, if it is showing one.
    fn focused_model_stream(&self) -> Option<String> {
        let id = self.document.focused_surface_id.as_ref()?;
        match &self.decoded.get(id)?.source {
            ContentSource::LiveVideo {
                source_id,
                orbitable: true,
                ..
            } => Some(source_id.clone()),
            _ => None,
        }
    }

    /// Apply a keyed view change and publish it.
    ///
    /// Always published, never throttled: these arrive one key press at a
    /// time, so there is no burst to thin out, and dropping one would leave
    /// the screen showing a view the renderer was never told about.
    fn apply_orbit_action(
        &mut self,
        stream_id: &str,
        action: canvas_model_viewer::OrbitAction,
    ) -> Task<Message> {
        self.apply_model_viewer_input(stream_id, canvas_model_viewer::state::Input::Camera(action));
        Task::none()
    }

    fn apply_model_timeline(&mut self, stream_id: &str, action: TimelineAction) -> Task<Message> {
        self.apply_model_viewer_input(
            stream_id,
            canvas_model_viewer::state::Input::Timeline(action),
        );
        Task::none()
    }

    fn toggle_model_orbit(&mut self, stream_id: &str) {
        self.apply_model_viewer_input(stream_id, canvas_model_viewer::state::Input::ToggleOrbit);
    }

    fn publish_wanted_video(&mut self) {
        let Some(root) = canvas_core::runtime_root() else {
            return;
        };
        let placed = visible_surface_ids(self.document.active_layout());
        let mut wanted: Vec<&str> = self
            .document
            .surfaces
            .iter()
            .filter(|surface| placed.iter().any(|id| id == &surface.id))
            .filter_map(|surface| match &surface.content {
                SurfaceContent::LiveVideo { source_id, .. } => Some(source_id.as_str()),
                SurfaceContent::LiveImage { source_id } => Some(source_id.as_str()),
                _ => None,
            })
            .collect();
        wanted.sort_unstable();
        wanted.dedup();
        let body = wanted.join("\n");
        if self.last_wanted_video.as_deref() == Some(body.as_str()) {
            return;
        }
        self.last_wanted_video = Some(body.clone());
        if let Err(error) =
            canvas_core::write_atomic(&root.join("state/wanted-video"), body.as_bytes())
        {
            self.events
                .push(format!("wanted_video_publish_failed {error}"));
        }
    }

    fn reconcile_external_stream_visibility(&mut self) {
        let placed = if self.canvas_visible {
            visible_surface_ids(self.document.active_layout())
        } else {
            Vec::new()
        };
        let visible_sources = self
            .document
            .surfaces
            .iter()
            .filter(|surface| {
                placed.iter().any(|id| id == &surface.id)
                    && !self.minimized_surfaces.contains(&surface.id)
            })
            .filter_map(|surface| match &surface.content {
                SurfaceContent::ExternalSurface { source_id } => Some(source_id.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        for error in self
            .external_surfaces
            .reconcile_stream_visibility(&visible_sources)
        {
            eprintln!("Canvas external-surface stream activation failed: {error}");
            self.emit_event(format!("external_stream_activation_failed error={error}"));
        }
    }

    fn publish_active_project(&mut self) {
        let Some(root) = canvas_core::runtime_root() else {
            return;
        };
        if let Err(error) = canvas_core::write_atomic(
            &root.join("state/active-project"),
            self.document.project_id.as_bytes(),
        ) {
            self.events
                .push(format!("active_project_publish_failed {error}"));
        }
    }

    /// Publish the renderer-neutral Coherence contract consumed by external
    /// presentation engines such as Blitz. These files are derived snapshots,
    /// never another theme source: Canvas resolves project inheritance and
    /// selected palettes, while renderers only consume semantic roles.
    fn publish_web_themes(&mut self) {
        let Some(root) = canvas_core::runtime_root() else {
            return;
        };
        let directory = root.join("themes");
        let appearance = canvas_core::appearance::current();
        for (project_id, document) in &self.workspaces.projects {
            if !canvas_core::is_safe_frame_id(project_id) {
                continue;
            }
            let visual = self.effective_visual_theme(document);
            let chart_colors = canvas_widgets::live::series_colors(&visual)
                .into_iter()
                .map(|color| {
                    format!(
                        "#{:02x}{:02x}{:02x}",
                        (color.r * 255.0).round() as u8,
                        (color.g * 255.0).round() as u8,
                        (color.b * 255.0).round() as u8,
                    )
                })
                .collect::<Vec<_>>();
            let pack = self.selected_theme_pack(project_id);
            let mode = match visual.mode {
                canvas_core::ThemeMode::Light => "light",
                canvas_core::ThemeMode::Dark => "dark",
            };
            let snapshot = serde_json::json!({
                "schema_version": 1,
                "project_id": project_id,
                "mode": mode,
                "palette": visual.palette,
                "charts": { "categorical": chart_colors },
                "surface_raised": format!("#{:02x}{:02x}{:02x}", visual.surface_raised[0], visual.surface_raised[1], visual.surface_raised[2]),
                "surface_accent": format!("#{:02x}{:02x}{:02x}", visual.surface_accent[0], visual.surface_accent[1], visual.surface_accent[2]),
                "focus": format!("#{:02x}{:02x}{:02x}", visual.focus[0], visual.focus[1], visual.focus[2]),
                "metrics": {
                    "space_1": visual.metrics.space_1, "space_2": visual.metrics.space_2,
                    "space_3": visual.metrics.space_3, "space_4": visual.metrics.space_4,
                    "space_5": visual.metrics.space_5, "radius_sm": visual.metrics.radius_sm,
                    "radius_md": visual.metrics.radius_md, "radius_lg": visual.metrics.radius_lg,
                    "small": visual.metrics.small, "body": visual.metrics.body,
                    "title": visual.metrics.title, "hero": visual.metrics.hero,
                },
                "appearance": appearance,
                "typography": pack.as_ref().map(|pack| serde_json::json!({
                    "font_family": pack.typography.font_family,
                    "mono_family": pack.typography.mono_family,
                    "letter_spacing": pack.typography.letter_spacing,
                })).unwrap_or_else(|| serde_json::json!({
                    "font_family": "system-ui, sans-serif",
                    "mono_family": "ui-monospace, monospace",
                    "letter_spacing": 0.0,
                })),
                "effects": pack.as_ref().map(|pack| serde_json::json!({
                    "profile": pack.id, "glow": pack.effects.glow,
                    "scanlines": pack.effects.scanlines, "noise": pack.effects.noise,
                    "corner_style": pack.effects.corner_style, "density": pack.effects.density,
                })).unwrap_or_else(|| serde_json::json!({
                    "profile": "coherence", "glow": 0.0, "scanlines": 0.0,
                    "noise": 0.0, "corner_style": "rounded", "density": 1.0,
                })),
            });
            let Ok(bytes) = serde_json::to_vec_pretty(&snapshot) else {
                continue;
            };
            let path = directory.join(format!("{project_id}.json"));
            if let Err(error) = canvas_core::write_atomic(&path, &bytes) {
                self.events
                    .push(format!("theme_snapshot_failed {project_id}: {error}"));
            }
        }
    }

    fn ensure_scene_decoded(&mut self) {
        for surface in &self.document.surfaces {
            if self.decoded.contains_key(&surface.id) {
                continue;
            }
            let (content, source) = match &surface.content {
                SurfaceContent::Scene { scene } => (
                    ContentKind::Scene,
                    ContentSource::Scene {
                        scene: scene.clone(),
                    },
                ),
                SurfaceContent::Chart { chart } => (
                    ContentKind::DataViz,
                    ContentSource::DataViz {
                        chart: chart.clone(),
                    },
                ),
                SurfaceContent::Widget { spec } => (
                    ContentKind::Widget,
                    ContentSource::Widget { spec: spec.clone() },
                ),
                SurfaceContent::Prompt {
                    target,
                    placeholder,
                    session,
                    task_kinds,
                    task_targets,
                } => (
                    ContentKind::Widget,
                    ContentSource::Prompt {
                        target: target.clone(),
                        placeholder: placeholder.clone(),
                        session: session.clone(),
                        draft: String::new(),
                        sent: Vec::new(),
                        task_kinds: task_kinds.clone(),
                        task_targets: task_targets.clone(),
                        selected_kind: None,
                        tasks: Vec::new(),
                        active_task: None,
                    },
                ),
                SurfaceContent::LiveImage { source_id } => {
                    let Some(root) = ::canvas::runtime_config::root() else {
                        continue;
                    };
                    let Some(path) = canvas_core::frame_file(&root, source_id) else {
                        continue;
                    };
                    (
                        ContentKind::Image,
                        ContentSource::LiveImage {
                            source_id: source_id.clone(),
                            path,
                            seen: None,
                        },
                    )
                }
                SurfaceContent::LiveVideo {
                    source_id,
                    width,
                    height,
                    orbitable,
                    timeline,
                } => {
                    let Some(path) = canvas_core::video_frame_file(source_id) else {
                        continue;
                    };
                    (
                        ContentKind::Image,
                        ContentSource::LiveVideo {
                            source_id: source_id.clone(),
                            path,
                            width: *width,
                            height: *height,
                            orbitable: *orbitable,
                            timeline: *timeline,
                            seen: None,
                        },
                    )
                }
                SurfaceContent::ExternalSurface { source_id } => (
                    ContentKind::Image,
                    ContentSource::ExternalSurface {
                        source_id: source_id.clone(),
                    },
                ),
                // Content this build cannot draw still gets a tile. Rendering it
                // as markdown reuses a renderer that already exists rather than
                // adding a path solely to say "no": the surface keeps its place
                // in the layout, and a person reading it learns which kind is
                // missing and that the fix is a newer Canvas — rather than
                // finding an empty rectangle and guessing.
                SurfaceContent::Unsupported(_) => {
                    let Some(reason) = surface.content.unsupported_reason() else {
                        continue;
                    };
                    (
                        ContentKind::Markdown,
                        ContentSource::Markdown {
                            path: PathBuf::new(),
                            items: markdown::parse(&reason).collect(),
                        },
                    )
                }
                _ => continue,
            };
            let snapshot = SurfaceSnapshot {
                id: surface.id.clone(),
                title: surface.title.clone(),
                content,
                asset_sha256: None,
                page_index: None,
                page_count: None,
                source_pixels: None,
                rendered_pixels: None,
                display_logical: None,
                scale_factor: None,
            };
            let mut runtime = RuntimeSurface {
                snapshot,
                source,
                handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
                source_pixels: [1, 1],
                media_plan: MediaPlan {
                    viewport_logical: LogicalSize {
                        width: 1920.0,
                        height: 1080.0,
                    },
                    source_dimensions: SourceDimensions::RasterPixels(PixelSize {
                        width: 1,
                        height: 1,
                    }),
                    target_px: PixelSize {
                        width: 1,
                        height: 1,
                    },
                    display_logical: LogicalSize {
                        width: 1920.0,
                        height: 1080.0,
                    },
                    scale_factor: 1.0,
                    cache_key: MediaCacheKey {
                        source_id: "<scene>".to_string(),
                        variant: MediaVariant::Raster,
                        pixel_size: PixelSize {
                            width: 1,
                            height: 1,
                        },
                    },
                },
                upload_pixels: [1, 1],
            };
            if let ContentSource::LiveImage {
                source_id,
                path,
                seen,
            } = &mut runtime.source
                && let Ok(metadata) = std::fs::metadata(&*path)
            {
                let stamp = (
                    metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                    metadata.len(),
                );
                if let Some((cached_stamp, cached)) = self.live_frame_cache.get(source_id).cloned()
                {
                    let same_content = cached_stamp == stamp
                        || cached.sha256.as_ref().is_some_and(|expected| {
                            hash_file(path).ok().as_ref() == Some(expected)
                        });
                    if same_content {
                        *seen = Some(stamp);
                        runtime.handle = cached.handle.clone();
                        runtime.source_pixels = cached.source_pixels;
                        runtime.upload_pixels = cached.upload_pixels;
                        runtime.media_plan = cached.media_plan.clone();
                        runtime.snapshot.asset_sha256 = cached.sha256.clone();
                        apply_plan_diagnostics(
                            &mut runtime.snapshot,
                            &cached.media_plan,
                            cached.upload_pixels,
                        );
                        self.live_frame_cache
                            .insert(source_id.clone(), (stamp, cached));
                    }
                }
            }
            self.decoded.insert(surface.id.clone(), runtime);
        }
        // Authored widgets are part of the document, so they are reconciled with
        // it. Every caller of this function gets them, which is the point: a
        // module that only resolved on one of twenty-four paths would be a wall
        // that shows an authored widget until someone switches workspace.
        #[cfg(feature = "lua-scenes")]
        self.refresh_modules();
    }

    fn project_palette(&self) -> ThemePalette {
        self.effective_theme(&self.document).palette()
    }

    fn selected_theme_pack(&self, project_id: &str) -> Option<::canvas::theme_pack::ThemePack> {
        let id = self.theme_pack_ids.get(project_id)?;
        self.theme_packs.iter().find(|pack| &pack.id == id).cloned()
    }

    fn window_chrome(&self) -> ::canvas::theme_pack::ThemeChrome {
        theme_chrome_for(
            &self.theme_packs,
            &self.theme_pack_ids,
            &self.document.project_id,
        )
    }

    fn effective_theme(&self, document: &WorkspaceDocument) -> canvas_core::ThemeSelection {
        self.selected_theme_pack(&document.project_id)
            .map(|pack| canvas_core::ThemeSelection::Palette {
                palette: pack.palette,
            })
            .unwrap_or_else(|| {
                document
                    .resolved_theme(&self.catalog)
                    .expect("validated Canvas document theme")
            })
    }

    fn effective_visual_theme(&self, document: &WorkspaceDocument) -> VisualTheme {
        let mut visual = self.effective_theme(document).visual_theme();
        if let Some(pack) = self.selected_theme_pack(&document.project_id) {
            if let Ok(categorical) = pack.charts.categorical.clone().try_into() {
                visual.categorical = Some(categorical);
            }
            let scale = match pack.effects.corner_style.as_str() {
                "square" => 0.0,
                "cut" => 0.28,
                _ => 1.0,
            };
            visual.metrics.radius_xs *= scale;
            visual.metrics.radius_sm *= scale;
            visual.metrics.radius_md *= scale;
            visual.metrics.radius_lg *= scale;
            visual.metrics.radius *= scale;
        }
        visual
    }

    fn media_viewport(&self) -> LogicalSize {
        LogicalSize {
            width: f64::from(self.frame.width.max(320)),
            height: f64::from(self.frame.height.max(240)),
        }
    }

    /// Resolve every authored module into a scene, and forget surfaces that have
    /// left the document.
    ///
    /// Runs when the document changes and when the civil day turns over — an
    /// authored calendar that does not roll at midnight is a wall that lies once a
    /// day. Never per frame: the retained key means a steady wall starts no VM.
    #[cfg(feature = "lua-scenes")]
    fn drain_module_bus(&mut self) {
        for event in self.module_bus.poll() {
            if event.kind == "canvas.widget.snapshot.v1" {
                match serde_json::from_value::<::canvas::stream_contract::StreamEnvelope>(
                    event.body,
                )
                .map_err(|error| error.to_string())
                .and_then(|event| {
                    event
                        .validate()
                        .map(|_| event)
                        .map_err(|error| error.to_string())
                }) {
                    Ok(event) => self.stream_inbox.push(event),
                    Err(error) => self.status = format!("Module stream rejected: {error}"),
                }
            } else if event.kind == "canvas.module.action.result.v1" {
                self.status = self
                    .module_bus
                    .receipt(&event)
                    .unwrap_or_else(|error| format!("Module receipt rejected: {error}"));
            }
        }
        if let Some(error) = self.module_bus.error.take() {
            eprintln!("Canvas module bus: {error}");
            self.status = format!("Module bus: {error}");
        }
    }

    #[cfg(feature = "lua-scenes")]
    fn activate_module(&mut self, surface_id: &str, target_id: &str) {
        // Re-evaluate the current artifact and state: a stale button cannot
        // replay the body retained before a reload or newer controller revision.
        let Some(surface) = self.decoded.get(surface_id) else {
            return;
        };
        let ContentSource::Widget { spec } = &surface.source else {
            return;
        };
        let canvas_widgets::WidgetKind::Module { module, inputs } = &spec.kind else {
            return;
        };
        let visual = self.presented_visual_theme();
        let theme = ::canvas::lua_module::ModuleThemeTokens::from_visual(
            &visual,
            self.effective_theme(&self.document).accent(),
        );
        let result = resolve_module_event(
            module,
            inputs,
            theme,
            now_unix_ms(),
            Some(::canvas::lua_module::ModuleEvent::Activate {
                target_id: target_id.to_owned(),
            }),
        );
        match result {
            Ok(retained) => {
                // Two destinations. `canvas.state` is the surface's own small
                // memory: a stateless view runs in a fresh VM on every render, so
                // a toggle has to live somewhere the host owns until the Spore
                // does. Anything else is an intent for a controller and travels
                // on the bus.
                let mut updates: Vec<(String, String)> = Vec::new();
                for action in retained.actions {
                    if action.destination == "canvas.state" {
                        let value = action
                            .body
                            .get("value")
                            .and_then(|value| value.as_str())
                            .unwrap_or("true")
                            .to_owned();
                        updates.push((action.kind, value));
                    } else if action.destination == "canvas.shell" {
                        self.apply_shell_action(&action);
                    } else {
                        match self.module_bus.publish(action) {
                            Ok(request_id) => {
                                self.status = format!(
                                    "Module command queued · {request_id} (awaiting controller)"
                                )
                            }
                            Err(error) => self.status = format!("Module command failed: {error}"),
                        }
                    }
                }
                let changed = self.remember_module_state(surface_id, updates);
                if changed {
                    self.refresh_modules();
                }
            }
            Err(error) => self.status = format!("Module activation rejected: {error}"),
        }
    }

    /// Shell intents: the dock and bar ask the compositor for things it already
    /// owns. Publishing these to the module bus would require a controller to
    /// echo them back, which is why a dock press did nothing.
    fn apply_shell_action(&mut self, action: &::canvas::lua_module::ModuleAction) {
        let Some(target) = action.body.get("surface_id").and_then(|id| id.as_str()) else {
            return;
        };
        match action.kind.as_str() {
            "focus" | "select" => {
                if self.minimized_surfaces.remove(target) {
                    self.bump();
                } else if self.document.focused_surface_id.as_deref() == Some(target) {
                    // Pressing the tile of the window you are already looking at
                    // collapses it, which is what a dock icon does on every
                    // desktop people already know.
                    self.minimized_surfaces.insert(target.to_owned());
                    self.bump();
                }
                self.document.focused_surface_id = Some(target.to_owned());
                self.bump();
            }
            _ => {}
        }
    }

    /// Write an action's payload into the surface's own inputs, which is the
    /// state a stateless view is handed back on its next render. Returns whether
    /// anything actually changed, so a repeated press does not re-run Lua.
    #[cfg(feature = "lua-scenes")]
    fn remember_module_state(&mut self, surface_id: &str, updates: Vec<(String, String)>) -> bool {
        if updates.is_empty() {
            return false;
        }
        let Some(surface) = self.decoded.get_mut(surface_id) else {
            return false;
        };
        let ContentSource::Widget { spec } = &mut surface.source else {
            return false;
        };
        let canvas_widgets::WidgetKind::Module { inputs, .. } = &mut spec.kind else {
            return false;
        };
        let mut changed = false;
        for (key, value) in updates {
            let next = canvas_widgets::WidgetInput::Text { value };
            if inputs.get(&key) != Some(&next) {
                inputs.insert(key, next);
                changed = true;
            }
        }
        changed
    }

    #[cfg(feature = "lua-scenes")]
    fn module_button_layer(
        &self,
        surface_id: &str,
        buttons: &[::canvas::lua_module::ModuleButton],
        viewport: LogicalSize,
    ) -> Element<'static, Message> {
        let visual = self.presented_visual_theme();
        // A control is pressable whenever it is enabled. The bus is how an
        // action reaches a *controller*; an action addressed to the surface's own
        // state needs no transport, and gating both on the bus is what made every
        // authored interface a picture of an interface.
        let children = self.native_module_buttons(buttons, viewport, |control| {
            control.enabled.then(|| Message::ModuleActivate {
                surface_id: surface_id.to_owned(),
                target_id: control.id.clone(),
            })
        });
        let mut children = children;
        if !buttons.is_empty() && !self.module_bus.connected() {
            children.push(
                container(
                    text("CONTROLS OFFLINE · Unibus session unavailable")
                        .size(visual.metrics.micro)
                        .color(color(visual.palette.warning)),
                )
                .padding(iced::Padding {
                    top: (viewport.height as f32 - 22.0).max(0.0),
                    left: 12.0,
                    right: 0.0,
                    bottom: 0.0,
                })
                .into(),
            );
        }
        stack(children).into()
    }

    #[cfg(feature = "lua-scenes")]
    fn native_module_buttons(
        &self,
        buttons: &[::canvas::lua_module::ModuleButton],
        viewport: LogicalSize,
        action: impl Fn(&::canvas::lua_module::ModuleButton) -> Option<Message>,
    ) -> Vec<Element<'static, Message>> {
        self.native_module_buttons_selected(buttons, viewport, action, |_| false)
    }

    #[cfg(feature = "lua-scenes")]
    fn native_module_buttons_selected(
        &self,
        buttons: &[::canvas::lua_module::ModuleButton],
        viewport: LogicalSize,
        action: impl Fn(&::canvas::lua_module::ModuleButton) -> Option<Message>,
        selected: impl Fn(&::canvas::lua_module::ModuleButton) -> bool,
    ) -> Vec<Element<'static, Message>> {
        let visual = self.presented_visual_theme();
        buttons
            .iter()
            .map(|control| {
                let [x, y] = scene_node_position([control.x, control.y, 0.0], viewport);
                let width = control.width * viewport.width as f32 / 8.0;
                let height = control.height * viewport.height as f32 / 8.0;
                let selected = selected(control);
                let appearance = control.appearance;
                let message = action(control);
                let content: Element<'static, Message> = if appearance
                    == ::canvas::lua_module::ModuleButtonAppearance::Invisible
                {
                    // The scene already draws this control; the host contributes
                    // the press and nothing else.
                    iced::widget::space::horizontal().into()
                } else if let Some(icon) = control.icon {
                    let id = match &message {
                        Some(Message::ShellFocusSurface(id)) => Some(id.as_str()),
                        Some(Message::ShellLauncherPick { entry_id, .. }) => {
                            entry_id.strip_prefix("surface:")
                        }
                        _ => None,
                    };
                    let preview = match &message {
                        Some(Message::ShellDockActivate(id)) => {
                            self.dock_icons.get(id).map(|handle| {
                                image_widget(handle.clone())
                                    .content_fit(ContentFit::Contain)
                                    .width(Length::Fill)
                                    .height(Length::Fill)
                                    .into()
                            })
                        }
                        _ => None,
                    }
                    .or_else(|| {
                        id.and_then(|id| self.decoded.get(id)).and_then(|surface| {
                            match &surface.source {
                                ContentSource::ExternalSurface { source_id } => self
                                    .external_surfaces
                                    .frame(source_id)
                                    .map(canvas_model_viewer::video::external_view),
                                ContentSource::Image { .. } | ContentSource::LiveImage { .. } => {
                                    Some(
                                        image_widget(surface.handle.clone())
                                            .content_fit(ContentFit::Contain)
                                            .width(Length::Fill)
                                            .height(Length::Fill)
                                            .into(),
                                    )
                                }
                                _ => None,
                            }
                        })
                    });
                    let picture: Element<'static, Message> =
                        container(preview.unwrap_or_else(|| {
                            canvas_widgets::live::app_chrome::shell_icon(
                                icon,
                                control.icon_size,
                                color(if control.enabled {
                                    visual.palette.text
                                } else {
                                    visual.palette.text_muted
                                }),
                            )
                        }))
                        .width(control.icon_size.min(width - 8.0).max(8.0))
                        .height(control.icon_size.min(height - 8.0).max(8.0))
                        .align_x(iced::alignment::Horizontal::Center)
                        .align_y(iced::alignment::Vertical::Center)
                        .into();
                    let content = if control.icon_caption {
                        let mut caption: String = control.label.chars().take(10).collect();
                        if control.label.chars().count() > 10 {
                            caption.push('…');
                        }
                        column![picture, text(caption).size(visual.metrics.small).center()]
                            .spacing(10)
                            .align_x(iced::Alignment::Center)
                            .into()
                    } else {
                        picture
                    };
                    container(content).center(Length::Fill).into()
                } else {
                    text(control.label.clone())
                        .size(fpl_gfx::chrome::BAR_TEXT_SIZE)
                        .center()
                        .into()
                };
                let item = button(content)
                    .width(width)
                    .height(height)
                    .on_press_maybe(control.enabled.then(|| message.clone()).flatten())
                    .style(move |_, status| match appearance {
                        ::canvas::lua_module::ModuleButtonAppearance::Standard => {
                            canvas_widgets::live::app_chrome::cube_button_style(
                                visual, selected, status,
                            )
                        }
                        ::canvas::lua_module::ModuleButtonAppearance::Quiet => {
                            canvas_widgets::live::app_chrome::quiet_button_style(
                                visual, selected, status,
                            )
                        }
                        ::canvas::lua_module::ModuleButtonAppearance::Invisible => {
                            iced::widget::button::Style {
                                background: None,
                                ..Default::default()
                            }
                        }
                    });
                let pin_id = match &message {
                    Some(Message::ShellDockActivate(id)) => Some(id.clone()),
                    Some(Message::ShellLauncherPick { entry_id, .. })
                        if entry_id.starts_with("app:")
                            && self
                                .project_switcher
                                .as_ref()
                                .is_some_and(|p| p.targets[p.target].id == "local") =>
                    {
                        Some(entry_id.clone())
                    }
                    _ => None,
                };
                let item: Element<'static, Message> = if control.icon.is_some() {
                    iced::widget::tooltip(
                        item,
                        text(control.label.clone()).size(visual.metrics.small),
                        iced::widget::tooltip::Position::Top,
                    )
                    .delay(Duration::from_millis(450))
                    .into()
                } else {
                    item.into()
                };
                let item = if let Some(id) = pin_id {
                    iced::widget::mouse_area(item)
                        .on_right_press(Message::ShellPinToggle(id))
                        .into()
                } else {
                    item
                };
                container(item)
                    .padding(iced::Padding {
                        top: y,
                        left: x,
                        right: 0.0,
                        bottom: 0.0,
                    })
                    .into()
            })
            .collect()
    }

    #[cfg(feature = "lua-scenes")]
    fn refresh_modules(&mut self) {
        let now_ms = now_unix_ms();
        let visual = self.presented_visual_theme();
        let accent = self.effective_theme(&self.document).accent();
        let theme = ::canvas::lua_module::ModuleThemeTokens::from_visual(&visual, accent);
        let today = ::canvas::hud::local_parts(now_ms);
        let mut targets = Vec::new();
        for (id, surface) in &self.decoded {
            if let ContentSource::Widget { spec } = &surface.source
                && let canvas_widgets::WidgetKind::Module { module, inputs } = &spec.kind
            {
                let artifact_revision = module_artifact_revision(module);
                let key = module_key(
                    module,
                    inputs,
                    &visual,
                    today.year,
                    today.month,
                    today.day,
                    artifact_revision,
                );
                // Explicit low-frequency freshness refresh, off the draw path.
                let key = spec
                    .refresh_ms
                    .map_or(key, |refresh| key ^ (now_ms / refresh.max(1_000)));
                targets.push((id.clone(), module.clone(), inputs.clone(), key));
            }
        }
        let live: BTreeSet<String> = targets.iter().map(|(id, _, _, _)| id.clone()).collect();
        for (id, name, inputs, key) in targets {
            if self.modules.get(&id).is_some_and(|done| done.key() == key) {
                continue;
            }
            let resolved = match resolve_module(&name, &inputs, theme, now_ms) {
                Ok(scene) => ModuleSurface::Rendered {
                    key,
                    buttons: scene.buttons,
                    scene: scene.scene,
                    last_error: None,
                },
                Err(reason) => {
                    // Loud on the console as well as on the wall: this is the one
                    // failure mode where a silent fallback would look like a
                    // working widget.
                    eprintln!("Canvas module {name} on surface {id}: {reason}");
                    match self.modules.get(&id) {
                        Some(ModuleSurface::Rendered { scene, buttons, .. }) => {
                            ModuleSurface::Rendered {
                                key,
                                scene: scene.clone(),
                                buttons: buttons.clone(),
                                last_error: Some(reason),
                            }
                        }
                        _ => ModuleSurface::Failed { key, reason },
                    }
                }
            };
            self.modules.insert(id, resolved);
        }
        self.modules.retain(|id, _| live.contains(id));
    }

    /// Draw a scene's text nodes in 2D, sized and coloured from the presented theme.
    ///
    /// Shared by document-authored scenes and resolved Lua modules, so an
    /// authored widget can never look like a different toolkit than a placed one.
    fn scene_layer(&self, scene: &Scene, viewport: LogicalSize) -> Element<'static, Message> {
        self.scene_layer_filtered(scene, viewport, false)
    }

    /// Lua owns the shell panel geometry; native chrome adds the shared border
    /// and shadow without drawing its opaque ground a second time.
    fn shell_scene_layer(&self, scene: &Scene, viewport: LogicalSize) -> Element<'static, Message> {
        let Some(node) = scene.nodes.iter().find(|node| node.id == "ground") else {
            return self.scene_layer(scene, viewport);
        };
        let NodeContent::Rect {
            ink,
            opacity,
            radius,
        } = node.content
        else {
            return self.scene_layer(scene, viewport);
        };
        let [x, y] = scene_node_position(node.transform.translation, viewport);
        let palette = self.presented_palette();
        let mut fill = color(scene_ink(ink, &palette));
        fill.a = opacity;
        let border = color(palette.border);
        let panel = container(space::horizontal())
            .width(node.transform.scale[0].abs() * viewport.width as f32 / 8.0)
            .height(node.transform.scale[1].abs() * viewport.height as f32 / 8.0)
            .style(move |_| container::Style {
                background: Some(fill.into()),
                border: iced::Border {
                    color: border,
                    width: 1.0,
                    radius: radius.into(),
                },
                shadow: iced::Shadow {
                    color: Color::from_rgba(0.0, 0.0, 0.0, 0.24),
                    offset: iced::Vector::new(0.0, 4.0),
                    blur_radius: 18.0,
                },
                ..Default::default()
            });
        stack![
            container(panel).padding(iced::Padding {
                top: y,
                left: x,
                ..Default::default()
            }),
            self.scene_layer_filtered(scene, viewport, true)
        ]
        .into()
    }

    fn scene_layer_filtered(
        &self,
        scene: &Scene,
        viewport: LogicalSize,
        shell_ground: bool,
    ) -> Element<'static, Message> {
        let visual = self.presented_visual_theme();
        let palette = visual.palette;
        let metrics = visual.metrics;
        // Flat primitives are retained scene nodes. Group/Mesh/Camera/Light
        // nodes contribute transforms without inventing a second layout owner.
        let mut sorted: Vec<usize> = scene
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                (!shell_ground || node.id != "ground")
                    && matches!(
                        node.content,
                        NodeContent::Text { .. } | NodeContent::Rect { .. }
                    )
            })
            .map(|(index, _)| index)
            .collect();
        // Farther nodes draw first so closer ones appear on top.
        sorted.sort_by(|a, b| {
            scene.nodes[*a].transform.translation[2]
                .total_cmp(&scene.nodes[*b].transform.translation[2])
        });
        let children: Vec<Element<'static, Message>> = sorted
            .iter()
            .map(|&index| {
                let node = &scene.nodes[index];
                let parents = scene_parent_chain(index, &scene.nodes);
                let (translation, _rotation, scale) =
                    scene_node_effective(index, &scene.nodes, &parents);
                let [pos_x, pos_y] = scene_node_position(translation, viewport);
                let z_scale = scene_node_scale(translation[2]);
                let content: Element<'static, Message> = match &node.content {
                    NodeContent::Text { text: label, ink } => {
                        // Font size is a length, not an area: multiplying X
                        // and Y made a 2x Lua label grow 4x and clip its frame.
                        let font_size =
                            scene_text_size(scale[1], metrics.body, z_scale, metrics.micro);
                        container(
                            text(label.clone())
                                .size(font_size)
                                .color(color(scene_ink(*ink, &palette))),
                        )
                        .into()
                    }
                    NodeContent::Rect {
                        ink,
                        opacity,
                        radius,
                    } => {
                        let width = (scale[0].abs() * viewport.width as f32 / 8.0).max(1.0);
                        let height = (scale[1].abs() * viewport.height as f32 / 8.0).max(1.0);
                        let mut fill = color(scene_ink(*ink, &palette));
                        fill.a = *opacity;
                        let radius = *radius;
                        container(space::horizontal())
                            .width(width)
                            .height(height)
                            .style(move |_| container::Style {
                                background: Some(fill.into()),
                                border: iced::Border {
                                    radius: radius.into(),
                                    ..Default::default()
                                },
                                ..Default::default()
                            })
                            .into()
                    }
                    _ => container(text("")).into(),
                };
                container(content)
                    .width(Length::Shrink)
                    .height(Length::Shrink)
                    .padding(iced::Padding {
                        top: pos_y,
                        right: 0.0,
                        bottom: 0.0,
                        left: pos_x,
                    })
                    .into()
            })
            .collect();
        // A `Stack` takes its intrinsic size from its *first* child, so shrink
        // nodes would be laid out inside node #1's box and clipped. The scene
        // owns the whole surface; fill it and let each node pad to its own
        // position.
        container(stack(children).width(Length::Fill).height(Length::Fill)).into()
    }

    fn surface_widget<'a>(
        &self,
        surface: &'a RuntimeSurface,
        width: Length,
        height: Length,
        viewport: LogicalSize,
    ) -> Element<'a, Message> {
        match &surface.source {
            ContentSource::LiveVideo {
                source_id,
                path,
                orbitable,
                timeline,
                ..
            } if !path.is_file() => {
                let loading = self.unloaded_media(
                    &surface.snapshot.title,
                    source_id,
                    Length::Fill,
                    Length::Fill,
                );
                if *orbitable || *timeline {
                    column![
                        container(loading).width(Length::Fill).height(Length::Fill),
                        self.model_toolbar(source_id, *orbitable, *timeline),
                    ]
                    .width(width)
                    .height(height)
                    .into()
                } else {
                    container(loading).width(width).height(height).into()
                }
            }
            ContentSource::LiveImage {
                source_id,
                seen: None,
                ..
            } => self.unloaded_media(&surface.snapshot.title, source_id, width, height),
            ContentSource::Markdown { items, .. } => container(
                markdown::view(items, markdown::Settings::from(&iced::Theme::Dark))
                    .map(Message::LinkOpened),
            )
            .width(width)
            .height(height)
            .into(),
            ContentSource::Terminal { buffer, .. } => {
                let palette = self.project_palette();
                scrollable(text(buffer).color(color(palette.text)).width(Length::Fill))
                    .width(width)
                    .height(height)
                    .into()
            }
            ContentSource::Scene { scene } => container(self.scene_layer(scene, viewport))
                .width(width)
                .height(height)
                .into(),
            ContentSource::DataViz { chart } => {
                let visual = self.presented_visual_theme();
                let palette = visual.palette;
                Canvas::new(ChartProgram {
                    chart: chart.clone(),
                    axis_color: color(palette.text),
                    muted_color: color(palette.text_muted),
                    surface_color: color(palette.surface),
                    // Categorical identity, not status: a ramp of
                    // `[primary, success, danger, warning]` made series three
                    // and four claim "failed" and "degraded" by list position.
                    series_colors: canvas_widgets::live::series_colors(&visual),
                    metrics: visual.metrics,
                })
                .width(width)
                .height(height)
                .into()
            }
            ContentSource::Prompt {
                target,
                placeholder,
                session,
                draft,
                sent,
                task_kinds,
                task_targets,
                selected_kind,
                tasks,
                active_task,
            } => {
                let palette = self.project_palette();
                let focused =
                    self.document.focused_surface_id.as_deref() == Some(&surface.snapshot.id);
                // Which session is named outright: on a wall showing several
                // agents, "→ operator" alone cannot tell you which of them is
                // about to receive what you type.
                let addressed_target = selected_kind.as_ref().map_or(target.as_str(), |kind| {
                    prompt_target_for_kind(target, task_targets, kind)
                });
                let addressed = match session {
                    Some(session) => format!("→ {addressed_target} · {session}"),
                    None => format!("→ {addressed_target} · current session"),
                };
                let surface_id = surface.snapshot.id.clone();
                let mut header = row![
                    text(if task_kinds.is_empty() {
                        addressed.clone()
                    } else {
                        "PROJECT TASKS".into()
                    })
                    .size(13)
                    .color(color(palette.text_muted)),
                    space::horizontal(),
                ]
                .align_y(iced::Alignment::Center);
                if !task_kinds.is_empty() {
                    header = header.push(
                        button(text("＋ CREATE NEW").size(12))
                            .padding([6, 10])
                            .on_press(Message::PromptNew(surface_id.clone())),
                    );
                } else if active_task.is_some() {
                    header = header.push(
                        button(text("＋ FOLLOW UP").size(12))
                            .padding([6, 10])
                            .on_press(Message::PromptNew(surface_id.clone())),
                    );
                }
                let mut column = iced::widget::Column::<Message>::new()
                    .spacing(10)
                    .push(header);

                if !tasks.is_empty() {
                    let mut tabs = row![].spacing(6);
                    for task in tasks.iter().rev().take(5) {
                        let selected = *active_task == Some(task.id);
                        tabs = tabs.push(
                            button(
                                text(format!(
                                    "{} {}",
                                    if selected { "●" } else { "○" },
                                    task.kind.to_ascii_uppercase()
                                ))
                                .size(11),
                            )
                            .padding([5, 8])
                            .on_press(Message::PromptTaskSelected {
                                surface_id: surface_id.clone(),
                                task_id: task.id,
                            }),
                        );
                    }
                    column = column.push(tabs);
                }

                if !task_kinds.is_empty() && selected_kind.is_none() && active_task.is_none() {
                    column = column.push(text("Create New…").size(24).color(color(palette.text)));
                    let mut kinds = row![].spacing(8);
                    for kind in task_kinds {
                        kinds = kinds.push(
                            button(text(kind.to_ascii_uppercase()).size(13))
                                .padding([9, 14])
                                .on_press(Message::PromptKindSelected {
                                    surface_id: surface_id.clone(),
                                    kind: kind.clone(),
                                }),
                        );
                    }
                    column = column.push(kinds).push(
                        text("Start a task here; the workspace becomes its live studio.")
                            .size(12)
                            .color(color(palette.text_muted)),
                    );
                } else if let Some(task) =
                    active_task.and_then(|task_id| tasks.iter().find(|task| task.id == task_id))
                {
                    let state = task.state.to_ascii_uppercase();
                    column = column
                        .push(
                            text(format!("{} · {state}", task.kind.to_ascii_uppercase()))
                                .size(13)
                                .color(color(palette.primary)),
                        )
                        .push(
                            text("FINAL PROMPT")
                                .size(10)
                                .color(color(palette.text_muted)),
                        )
                        .push(text(&task.prompt).size(19).color(color(palette.text)))
                        .push(
                            text(
                                task.detail
                                    .as_deref()
                                    .unwrap_or("Watching live task activity"),
                            )
                            .size(11)
                            .color(color(palette.text_muted)),
                        );
                    column = column.push(
                        text("LIVE ACTIVITY")
                            .size(10)
                            .color(color(palette.text_muted)),
                    );
                    if task.activity.is_empty() {
                        column = column.push(
                            text("Waiting for the tool to report progress…")
                                .size(11)
                                .color(color(palette.text_muted)),
                        );
                    } else {
                        for entry in task.activity.iter().rev().take(6).rev() {
                            column = column.push(
                                text(format!("› {entry}"))
                                    .size(11)
                                    .color(color(palette.text)),
                            );
                        }
                    }
                    if !matches!(task.state.as_str(), "complete" | "failed") {
                        column = column.push(canvas_widgets::live::loading_rails(
                            self.effective_visual_theme(&self.document),
                        ));
                    }
                } else {
                    if let Some(kind) = selected_kind {
                        column = column.push(
                            text(format!("NEW {}  ·  {addressed}", kind.to_ascii_uppercase()))
                                .size(13)
                                .color(color(palette.primary)),
                        );
                    }

                    let (line, line_color) = if draft.is_empty() {
                        (
                            placeholder
                                .clone()
                                .unwrap_or_else(|| "Describe the outcome…".into()),
                            palette.text_muted,
                        )
                    } else {
                        (draft.clone(), palette.text)
                    };
                    column = column.push(
                        text(format!("{line}{}", if focused { "▌" } else { "" }))
                            .size(22)
                            .color(color(line_color)),
                    );
                }

                if task_kinds.is_empty() {
                    for line in sent.iter().rev() {
                        column = column.push(
                            text(format!("sent · {line}"))
                                .size(12)
                                .color(color(palette.text_muted)),
                        );
                    }
                }

                container(scrollable(column).height(Length::Fill))
                    .padding(16)
                    .width(width)
                    .height(height)
                    .style(move |_| iced::widget::container::Style {
                        background: Some(color(palette.surface).into()),
                        border: iced::Border {
                            color: color(if focused {
                                palette.primary
                            } else {
                                palette.border
                            }),
                            width: 1.0,
                            radius: 6.0.into(),
                        },
                        ..Default::default()
                    })
                    .into()
            }
            ContentSource::Stream { url } => {
                // No decoder is wired for video/streams yet (see docs/widget-quality.md
                // rule 3: a surface must not claim a connection it cannot make).
                // A hollow "Connecting to stream..." is worse than a label — it
                // reads as in-progress when the capability does not exist.
                let palette = self.project_palette();
                container(
                    iced::widget::Column::<Message>::new()
                        .push(
                            text("Stream playback is not yet available")
                                .size(20)
                                .color(color(palette.text)),
                        )
                        .push(text(url.clone()).size(12).color(color(palette.text_muted)))
                        .push(
                            text("Video and continuous streams arrive via the native surface path,\nwhich is still under construction.")
                                .size(12)
                                .color(color(palette.text_muted)),
                        ),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            }
            ContentSource::Widget { spec } => {
                // An authored widget draws its retained scene. The static library
                // still answers for a module that has not resolved at all, so the
                // two failure shapes stay distinguishable on the wall: an
                // unresolved module says "unresolved", a resolved one that failed
                // says why.
                #[cfg(feature = "lua-scenes")]
                if let canvas_widgets::WidgetKind::Module { .. } = &spec.kind {
                    return match self.modules.get(&surface.snapshot.id) {
                        Some(ModuleSurface::Rendered {
                            scene,
                            buttons,
                            last_error,
                            ..
                        }) => {
                            let scene = container(stack![
                                self.scene_layer(scene, viewport),
                                self.module_button_layer(&surface.snapshot.id, buttons, viewport)
                            ])
                            .width(Length::Fill)
                            .height(Length::Fill);
                            if let Some(error) = last_error {
                                let visual = self.presented_visual_theme();
                                let palette = visual.palette;
                                stack![
                                    scene,
                                    container(
                                        text(format!("RELOAD REJECTED · {error}"))
                                            .size(visual.metrics.micro)
                                            .color(color(palette.danger)),
                                    )
                                    .padding(visual.metrics.space_2),
                                ]
                                .width(width)
                                .height(height)
                                .into()
                            } else {
                                scene.width(width).height(height).into()
                            }
                        }
                        Some(ModuleSurface::Failed { reason, .. }) => {
                            let metrics = self.presented_visual_theme().metrics;
                            let palette = self.project_palette();
                            container(
                                column![
                                    text(surface.snapshot.title.clone())
                                        .size(metrics.title)
                                        .color(color(palette.text)),
                                    text(reason.clone())
                                        .size(metrics.small)
                                        .color(color(palette.danger)),
                                ]
                                .spacing(metrics.space_2),
                            )
                            .width(width)
                            .height(height)
                            .padding(metrics.space_4)
                            .into()
                        }
                        None => canvas_widgets::live::element_at_with_artwork(
                            spec,
                            &self.presented_visual_theme(),
                            canvas_widgets::live::chrono_now_at_millis(
                                self.last_clock_millis as i64,
                            ),
                            None,
                        ),
                    };
                }
                let visual = self.presented_visual_theme();
                let artwork = match &spec.kind {
                    canvas_widgets::WidgetKind::MediaCard {
                        artwork_source: Some(source_id),
                        ..
                    } => self
                        .live_frame_cache
                        .get(source_id)
                        .map(|(_, frame)| frame.handle.clone()),
                    _ => None,
                };
                canvas_widgets::live::element_at_with_artwork(
                    spec,
                    &visual,
                    canvas_widgets::live::chrono_now_at_millis(self.last_clock_millis as i64),
                    artwork,
                )
            }
            ContentSource::Image { .. }
            | ContentSource::Pdf { .. }
            | ContentSource::LiveImage { .. }
            | ContentSource::LiveVideo { .. }
            | ContentSource::ExternalSurface { .. } => {
                let view = self.shared_views.get(&surface.snapshot.id);
                let mut native_alpha = false;
                let picture: Element<'_, Message> = match &surface.source {
                    ContentSource::LiveVideo {
                        path,
                        width: frame_width,
                        height: frame_height,
                        source_id,
                        orbitable,
                        ..
                    } => {
                        let stream = source_id.clone();
                        canvas_model_viewer::video::view(
                            path.clone(),
                            *frame_width,
                            *frame_height,
                            orbitable.then(|| {
                                std::sync::Arc::new(move |event| Message::ModelOrbit {
                                    stream_id: stream.clone(),
                                    event,
                                }) as _
                            }),
                        )
                    }
                    ContentSource::ExternalSurface { source_id } => {
                        match self.external_surfaces.frame(source_id) {
                            Some(frame) => {
                                native_alpha = frame.descriptor.alpha_mode
                                    != canvas_core::external_surface::AlphaMode::Opaque;
                                canvas_model_viewer::video::external_view(frame)
                            }
                            None => {
                                let visual = self.presented_visual_theme();
                                container(
                                    column![
                                        text("Waiting for external surface")
                                            .size(visual.metrics.body)
                                            .color(color(visual.palette.text)),
                                        text(source_id.clone())
                                            .size(visual.metrics.small)
                                            .color(color(visual.palette.text_muted)),
                                    ]
                                    .spacing(visual.metrics.space_2),
                                )
                                .center_x(Length::Fill)
                                .center_y(Length::Fill)
                                .into()
                            }
                        }
                    }
                    _ => {
                        let mut image = image_widget(surface.handle.clone())
                            .content_fit(ContentFit::Contain)
                            .width(width)
                            .height(height);
                        if let Some(view) = view {
                            image =
                                image.crop(view_crop_pixels(view.viewport, surface.source_pixels));
                        }
                        image.into()
                    }
                };
                let mut layers = vec![picture];
                if let ContentSource::LiveVideo { source_id, .. } = &surface.source
                    && let Some(detections) = self.camera_detections.get(source_id)
                    && !detections.detections.is_empty()
                {
                    let palette = self.presented_palette();
                    layers.push(
                        Canvas::new(DetectionOverlayProgram {
                            detections: detections.detections.clone(),
                            source_pixels: surface.source_pixels,
                            color: color(palette.primary),
                        })
                        .width(width)
                        .height(height)
                        .into(),
                    );
                }
                if let ContentSource::LiveVideo {
                    source_id,
                    orbitable: true,
                    ..
                } = &surface.source
                {
                    layers.push(self.model_view_cube(source_id));
                }
                if let Some(view) = view
                    && let Some(attention) = &view.attention
                {
                    let palette = self.presented_palette();
                    layers.push(
                        Canvas::new(AttentionProgram {
                            viewport: view.viewport,
                            attention: attention.region,
                            source_pixels: surface.source_pixels,
                            color: color(palette.primary),
                            label: attention.label.clone(),
                        })
                        .width(width)
                        .height(height)
                        .into(),
                    );
                }
                let visual: Element<'_, Message> = stack(layers).into();
                // Pixel streams are often letterboxed by their producer and do not carry an
                // alpha channel. Only opaque streams get an inner matte; native
                // alpha-bearing surfaces must compose over the desktop directly.
                // Give opaque streams a matte so the desktop/background
                // cannot bleed through their edges while preserving the stream's aspect ratio.
                let visual: Element<'_, Message> = if matches!(
                    &surface.source,
                    ContentSource::LiveVideo { .. } | ContentSource::ExternalSurface { .. }
                ) && !native_alpha
                {
                    container(visual)
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(stream_matte_padding(stream_source_pixels(surface)))
                        .style(|_| container::Style {
                            background: Some(Color::BLACK.into()),
                            ..Default::default()
                        })
                        .into()
                } else {
                    visual
                };
                match &surface.source {
                    ContentSource::Pdf {
                        page_index,
                        page_count,
                        ..
                    } => column![
                        container(visual).width(Length::Fill).height(Length::Fill),
                        self.pdf_toolbar(&surface.snapshot.id, *page_index, *page_count),
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                    ContentSource::LiveVideo {
                        source_id,
                        orbitable,
                        timeline,
                        ..
                    } if *orbitable || *timeline => column![
                        container(visual).width(Length::Fill).height(Length::Fill),
                        self.model_toolbar(source_id, *orbitable, *timeline),
                    ]
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .into(),
                    _ => visual,
                }
            }
        }
    }

    fn unloaded_media<'a>(
        &self,
        title: &'a str,
        source_id: &'a str,
        width: Length,
        height: Length,
    ) -> Element<'a, Message> {
        let visual = self.presented_visual_theme();
        let palette = visual.palette;
        let metrics = visual.metrics;
        let content = column![
            text("AWAITING LIVE ARTIFACT")
                .size(metrics.small)
                .color(color(palette.primary)),
            text(title).size(metrics.title).color(color(palette.text)),
            text(source_id)
                .size(metrics.small)
                .color(color(palette.text_muted)),
            space::vertical().height(Length::Fixed(metrics.space_3)),
            canvas_widgets::live::loading_rails(visual),
        ]
        .spacing(metrics.space_2);
        container(content)
            .padding(metrics.space_4)
            .width(width)
            .height(height)
            .style(move |_| container::Style {
                background: Some(
                    Color {
                        a: 0.96,
                        ..color(visual.surface_raised)
                    }
                    .into(),
                ),
                border: iced::Border {
                    color: Color {
                        a: 0.72,
                        ..color(palette.border_strong)
                    },
                    width: 1.0,
                    radius: metrics.radius_sm.into(),
                },
                ..Default::default()
            })
            .into()
    }

    /// Native controls adjacent to the video rather than burned into or laid
    /// over its pixels. This keeps the model readable and gives pointer capture
    /// an explicit, visible mode.
    fn model_toolbar<'a>(
        &self,
        stream_id: &'a str,
        orbitable: bool,
        timeline: bool,
    ) -> Element<'a, Message> {
        canvas_model_viewer::view::toolbar(
            self.presented_visual_theme(),
            stream_id,
            self.model_viewer.controller(stream_id),
            self.model_viewer.timeline(stream_id),
            orbitable,
            timeline,
            |stream_id, action| match action {
                canvas_model_viewer::view::Action::ToggleOrbit => {
                    Message::ModelOrbitToggle { stream_id }
                }
                canvas_model_viewer::view::Action::Camera(action) => {
                    Message::ModelOrbitAction { stream_id, action }
                }
                canvas_model_viewer::view::Action::Timeline(action) => {
                    Message::ModelTimeline { stream_id, action }
                }
            },
        )
    }

    /// Compact CAD view cube overlaid in the model's lower-right corner.
    /// Faces and chamfer-like corner buttons all resolve through the same
    /// named-view controller used by keys and drag gestures.
    fn model_view_cube<'a>(&self, stream_id: &'a str) -> Element<'a, Message> {
        canvas_model_viewer::view::view_cube(
            self.presented_visual_theme(),
            stream_id,
            self.model_viewer.controller(stream_id),
            |stream_id, action| match action {
                canvas_model_viewer::view::Action::Camera(action) => {
                    Message::ModelOrbitAction { stream_id, action }
                }
                _ => unreachable!("view cube only emits camera actions"),
            },
        )
    }

    fn pdf_toolbar<'a>(
        &self,
        surface_id: &'a str,
        page_index: u32,
        page_count: u32,
    ) -> Element<'a, Message> {
        use canvas_widgets::live::app_chrome::Item;
        let previous = if page_index == 0 {
            Item::disabled("◀")
        } else {
            Item::action(
                "◀",
                Message::PdfNavigate {
                    surface_id: surface_id.to_string(),
                    target: Navigation::Previous,
                },
            )
        };
        let next = if page_index + 1 >= page_count {
            Item::disabled("▶")
        } else {
            Item::action(
                "▶",
                Message::PdfNavigate {
                    surface_id: surface_id.to_string(),
                    target: Navigation::Next,
                },
            )
        };
        canvas_widgets::live::app_chrome::bar(
            self.presented_visual_theme(),
            [
                previous,
                Item::Status(format!("PAGE {} / {}", page_index + 1, page_count)),
                next,
            ],
        )
    }

    fn surface_controls<'a>(
        &self,
        title: &'a str,
        focused: bool,
        maximized: bool,
        widget_chrome: Option<canvas_widgets::WidgetChrome>,
    ) -> Element<'a, Message> {
        if self.document.chrome.title == SurfaceTitleMode::Hidden
            || matches!(
                widget_chrome,
                Some(
                    canvas_widgets::WidgetChrome::Frameless
                        | canvas_widgets::WidgetChrome::Integrated
                )
            )
        {
            return container(space::horizontal())
                .width(Length::Fill)
                .height(Length::Fixed(0.0))
                .into();
        }
        let visual_theme = self.presented_visual_theme();
        let palette = visual_theme.palette;
        let chrome = self.window_chrome();
        let bar_height = chrome.titlebar_height;
        let control_style = chrome.control_style.clone();
        let mark = move |rgb, symbol: &'static str| -> Element<'a, Message> {
            let fill = color(rgb);
            match control_style.as_str() {
                "glyphs" => container(text(symbol).size(11).color(fill))
                    .width(Length::Fixed(14.0))
                    .height(Length::Fixed(14.0))
                    .center(Length::Fill)
                    .into(),
                "segments" => container(text(symbol).size(10).color(fill))
                    .width(Length::Fixed(15.0))
                    .height(Length::Fixed(15.0))
                    .center(Length::Fill)
                    .style(move |_| container::Style {
                        background: Some(Color { a: 0.10, ..fill }.into()),
                        border: iced::Border {
                            color: Color { a: 0.72, ..fill },
                            width: 1.0,
                            radius: 1.0.into(),
                        },
                        ..Default::default()
                    })
                    .into(),
                _ => container(space::horizontal())
                    .width(Length::Fixed(9.0))
                    .height(Length::Fixed(9.0))
                    .style(move |_| container::Style {
                        background: Some(fill.into()),
                        border: iced::Border {
                            color: Color { a: 0.42, ..fill },
                            width: 1.0,
                            radius: 4.5.into(),
                        },
                        ..Default::default()
                    })
                    .into(),
            }
        };
        let control = |rgb, symbol: &'static str, label: &'static str| {
            tooltip(
                mouse_area(
                    container(mark(rgb, symbol))
                        .width(Length::Fixed(SURFACE_CONTROL_HIT_SIZE as f32))
                        .height(Length::Fixed(bar_height))
                        .align_x(iced::alignment::Horizontal::Center)
                        .align_y(iced::alignment::Vertical::Center),
                )
                .interaction(mouse::Interaction::Pointer),
                container(text(label).size(11)).padding(6),
                tooltip::Position::Bottom,
            )
        };
        if widget_chrome == Some(canvas_widgets::WidgetChrome::DragOnly) {
            let grip = container(space::horizontal())
                .width(Length::Fixed(34.0))
                .height(Length::Fixed(3.0))
                .style(move |_| container::Style {
                    background: Some(
                        Color::from_rgba8(
                            palette.border_strong[0],
                            palette.border_strong[1],
                            palette.border_strong[2],
                            if focused { 0.72 } else { 0.42 },
                        )
                        .into(),
                    ),
                    border: iced::Border {
                        radius: 2.0.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                });
            return container(grip)
                .width(Length::Fill)
                .height(Length::Fixed(bar_height))
                .center_x(Length::Fill)
                .align_y(iced::alignment::Vertical::Center)
                .into();
        }
        let controls: Element<'_, Message> =
            if self.document.chrome.title == SurfaceTitleMode::Presentation {
                space::horizontal().width(Length::Fixed(0.0)).into()
            } else {
                row![
                    control(
                        palette.danger,
                        "×",
                        window_action_label(WindowAction::Close, maximized)
                    ),
                    control(
                        palette.warning,
                        "−",
                        window_action_label(WindowAction::Minimize, maximized)
                    ),
                    control(
                        palette.success,
                        if maximized { "◇" } else { "□" },
                        window_action_label(WindowAction::Maximize, maximized)
                    ),
                ]
                .spacing(0)
                .align_y(iced::alignment::Vertical::Center)
                .into()
            };
        let show_title = surface_title_visible(self.document.chrome.title, focused);
        let title = if chrome.uppercase_titles {
            title.to_uppercase()
        } else {
            title.to_owned()
        };
        let bar = if show_title {
            row![
                controls,
                container(text(title).size(11).color(color(palette.text_muted)))
                    .padding(iced::Padding {
                        top: 0.0,
                        right: 8.0,
                        bottom: 0.0,
                        left: 8.0
                    })
                    .width(Length::Fill)
                    .align_x(if chrome.title_alignment == "center" {
                        iced::alignment::Horizontal::Center
                    } else {
                        iced::alignment::Horizontal::Left
                    })
            ]
            .align_y(iced::alignment::Vertical::Center)
        } else {
            row![controls].align_y(iced::alignment::Vertical::Center)
        };
        container(bar)
            .padding(iced::Padding {
                top: 0.0,
                right: 0.0,
                bottom: 0.0,
                left: 8.0,
            })
            .width(Length::Fill)
            .height(Length::Fixed(bar_height))
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Top)
            .style(move |_| container::Style {
                background: Some(
                    Color::from_rgba8(
                        palette.surface[0],
                        palette.surface[1],
                        palette.surface[2],
                        chrome.header_opacity,
                    )
                    .into(),
                ),
                border: iced::Border {
                    // The outer surface is the sole owner of window shape.
                    // Giving the header its own rounded border makes it read
                    // as a detached card floating above the content.
                    color: Color::TRANSPARENT,
                    width: 0.0,
                    radius: 0.0.into(),
                },
                ..Default::default()
            })
            .into()
    }

    fn minimized_dock(&self) -> Option<Element<'_, Message>> {
        if self.minimized_surfaces.is_empty() {
            return None;
        }
        let palette = self.presented_palette();
        let items = self
            .document
            .surfaces
            .iter()
            .filter(|surface| self.minimized_surfaces.contains(&surface.id))
            .map(|surface| {
                let initial = surface
                    .title
                    .chars()
                    .find(|character| character.is_alphanumeric())
                    .unwrap_or('•')
                    .to_uppercase()
                    .to_string();
                tooltip(
                    button(text(initial).size(14))
                        .width(Length::Fixed(36.0))
                        .height(Length::Fixed(36.0))
                        .padding(0)
                        .on_press(Message::RestoreSurface(surface.id.clone()))
                        .style(button::secondary),
                    text(surface.title.clone()).size(12),
                    tooltip::Position::Top,
                )
                .into()
            })
            .collect::<Vec<Element<'_, Message>>>();
        let dock = container(row(items).spacing(8))
            .padding(8)
            .style(move |_| container::Style {
                background: Some(
                    Color::from_rgba8(
                        palette.surface[0],
                        palette.surface[1],
                        palette.surface[2],
                        0.82,
                    )
                    .into(),
                ),
                border: iced::Border {
                    color: Color::from_rgba8(
                        palette.border[0],
                        palette.border[1],
                        palette.border[2],
                        0.72,
                    ),
                    width: 1.0,
                    radius: 14.0.into(),
                },
                shadow: iced::Shadow {
                    color: Color::from_rgba8(0, 0, 0, 0.32),
                    offset: iced::Vector::new(0.0, 8.0),
                    blur_radius: 24.0,
                },
                ..Default::default()
            });
        Some(
            container(dock)
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(18)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Bottom)
                .into(),
        )
    }

    fn replan_rasters(&mut self) {
        let viewport = self.media_viewport();
        let scale_factor = self.scale_factor;
        for surface in self.decoded.values_mut() {
            if !matches!(surface.source, ContentSource::Image { .. }) {
                continue;
            }
            if let Ok(plan) = plan_raster(
                surface
                    .snapshot
                    .asset_sha256
                    .clone()
                    .unwrap_or_else(|| surface.snapshot.id.clone()),
                PixelSize {
                    width: surface.source_pixels[0],
                    height: surface.source_pixels[1],
                },
                viewport,
                scale_factor,
                MediaLimits::default(),
            ) {
                if plan.target_px.width > surface.media_plan.target_px.width
                    || plan.target_px.height > surface.media_plan.target_px.height
                {
                    continue;
                }
                apply_plan_diagnostics(&mut surface.snapshot, &plan, surface.upload_pixels);
                surface.media_plan = plan;
            }
        }
    }

    fn has_live_frames(&self) -> bool {
        self.decoded.values().any(|surface| {
            matches!(
                surface.source,
                ContentSource::LiveImage { .. }
                    | ContentSource::LiveVideo { .. }
                    | ContentSource::ExternalSurface { .. }
            )
        })
    }

    fn widget_animation_interval(&self) -> Option<Duration> {
        // Bindings mutate the retained runtime spec, not the persisted
        // workspace template. Cadence must therefore follow `decoded`, or a
        // paused template that Lua turns into playing media never gets frames.
        self.decoded
            .values()
            .filter_map(|surface| match &surface.source {
                ContentSource::Widget { spec } => spec.animation_interval_ms(),
                _ => None,
            })
            .min()
            .map(Duration::from_millis)
    }

    fn refresh_camera_detections(&mut self) {
        let Some(root) = ::canvas::runtime_config::root() else {
            return;
        };
        let sources = self
            .decoded
            .values()
            .filter_map(|surface| match &surface.source {
                ContentSource::LiveVideo { source_id, .. } => Some(source_id.clone()),
                _ => None,
            })
            .collect::<BTreeSet<_>>();
        let now = canvas_media_edge_time_ms();
        for source_id in sources {
            let Some(path) = canvas_core::detection_file(&root, &source_id) else {
                continue;
            };
            let Ok(metadata) = std::fs::metadata(&path) else {
                self.camera_detections.remove(&source_id);
                self.camera_detection_stamps.remove(&source_id);
                continue;
            };
            let stamp = (
                metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                metadata.len(),
            );
            if self.camera_detection_stamps.get(&source_id) == Some(&stamp) {
                if self
                    .camera_detections
                    .get(&source_id)
                    .is_some_and(|value| value.valid_until_unix_ms <= now)
                {
                    self.camera_detections.remove(&source_id);
                    self.bump();
                }
                continue;
            }
            self.camera_detection_stamps
                .insert(source_id.clone(), stamp);
            let parsed = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<CameraDetections>(&bytes).ok())
                .filter(|value| value.camera_id == source_id && value.valid_until_unix_ms > now);
            match parsed {
                Some(value) => {
                    self.camera_detections.insert(source_id, value);
                    self.bump();
                }
                None => {
                    self.camera_detections.remove(&source_id);
                }
            }
        }
    }

    /// Notice changed `frames/<source_id>.jpg` files and dispatch their decode
    /// off the main thread. Transport, decoding, and retry policy belong to the
    /// producer; this screen only notices new files and then hands the JPEG to
    /// iced's runtime, applying the result in `Message::LiveFrameLoaded`. A
    /// missing or rejected frame keeps the last good image on the wall rather
    /// than blinking to black; the miss is emitted so the gap is visible.
    ///
    /// `seen` advances at claim time, so a frame that fails to decode is not
    /// re-decoded every tick until its bytes change — the retry-storm guard a
    /// Pi 4 needs. `live_pending` blocks a duplicate spawn for a surface that
    /// is already decoding, and a result is dropped if the file moved on.
    fn reload_live_images(&mut self) -> Task<Message> {
        let viewport = self.media_viewport();
        let scale_factor = self.scale_factor;
        let mut notes: Vec<String> = Vec::new();
        let mut tasks: Vec<Task<Message>> = Vec::new();
        let mut immediate: Vec<(String, (std::time::SystemTime, u64), LiveFrameResult)> =
            Vec::new();
        for (surface_id, surface) in self.decoded.iter_mut() {
            let (source_id, path, seen, raw_size) = match &mut surface.source {
                ContentSource::LiveImage {
                    source_id,
                    path,
                    seen,
                } => (source_id, path, seen, None),
                // Raw video is uploaded by `canvas-model-viewer` directly into a
                // stable WGPU texture. It must never enter iced's immutable
                // image-handle cache, which allocates a new texture per frame.
                ContentSource::LiveVideo { .. } => continue,
                _ => continue,
            };
            let metadata = match std::fs::metadata(&*path) {
                Ok(metadata) => metadata,
                Err(error) => {
                    if seen.take().is_some() {
                        notes.push(format!("live_image_gone {source_id}: {error}"));
                    }
                    continue;
                }
            };
            let stamp = (
                metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                metadata.len(),
            );
            let surface_key = surface_id.clone();
            if *seen == Some(stamp) || self.live_pending.contains_key(&surface_key) {
                continue;
            }
            *seen = Some(stamp);
            if let Some((width, height)) = raw_size {
                // Raw video is already decoded and lives on tmpfs. Scheduling
                // this tiny read through iced's async task executor held the
                // surface behind one `live_pending` job for ~150 ms, reducing a
                // measured 24 fps producer to 6-7 visible fps. Claim the latest
                // atomic file and prepare its handle in this tick; JPEG/PDF
                // decode remains asynchronous below.
                let expected = usize::try_from(width)
                    .ok()
                    .and_then(|value| value.checked_mul(height as usize))
                    .and_then(|pixels| pixels.checked_mul(4));
                let result = expected
                    .ok_or_else(|| format!("invalid raw frame dimensions {width}x{height}"))
                    .and_then(|expected| {
                        let bytes = std::fs::read(&*path)
                            .map_err(|error| format!("read {}: {error}", path.display()))?;
                        if bytes.len() != expected {
                            return Err(format!(
                                "raw frame length {} does not match {width}x{height} RGBA ({expected})",
                                bytes.len()
                            ));
                        }
                        let source_pixels = [width, height];
                        let media_plan = plan_raster(
                            format!("live:{source_id}"),
                            PixelSize { width, height },
                            viewport,
                            scale_factor,
                            MediaLimits::default(),
                        )
                        .map_err(|error| error.to_string())?;
                        Ok(PreparedFrame {
                            handle: iced::widget::image::Handle::from_rgba(width, height, bytes),
                            source_pixels,
                            media_plan,
                            upload_pixels: source_pixels,
                            sha256: None,
                        })
                    });
                immediate.push((surface_key, stamp, result));
                continue;
            }
            self.live_pending.insert(surface_key.clone(), stamp);
            let path = path.clone();
            let source = format!("live:{source_id}");
            tasks.push(Task::perform(
                async move {
                    let (handle, source_pixels, media_plan, upload_pixels, sha) =
                        if let Some((width, height)) = raw_size {
                            let expected = usize::try_from(width)
                                .ok()
                                .and_then(|value| value.checked_mul(height as usize))
                                .and_then(|pixels| pixels.checked_mul(4))
                                .ok_or_else(|| format!("invalid raw frame dimensions {width}x{height}"))?;
                            let bytes = std::fs::read(&path)
                                .map_err(|error| format!("read {}: {error}", path.display()))?;
                            if bytes.len() != expected {
                                return Err(format!(
                                    "raw frame length {} does not match {width}x{height} RGBA ({expected})",
                                    bytes.len()
                                ));
                            }
                            let source_pixels = [width, height];
                            let media_plan = plan_raster(
                                source,
                                PixelSize { width, height },
                                viewport,
                                scale_factor,
                                MediaLimits::default(),
                            )
                            .map_err(|error| error.to_string())?;
                            (
                                iced::widget::image::Handle::from_rgba(width, height, bytes),
                                source_pixels,
                                media_plan,
                                source_pixels,
                                None,
                            )
                        } else {
                            let sha = hash_file(&path).ok();
                            let (handle, _media_type, source_pixels, media_plan) =
                                prepare_raster(&path, source, viewport, scale_factor)
                                    .map_err(|failure| failure.message)?;
                            let upload_pixels =
                                [media_plan.target_px.width, media_plan.target_px.height];
                            (handle, source_pixels, media_plan, upload_pixels, sha)
                        };
                    Ok(PreparedFrame {
                        handle,
                        source_pixels,
                        media_plan,
                        upload_pixels,
                        sha256: sha,
                    })
                },
                move |result| Message::LiveFrameLoaded {
                    surface_id: surface_key,
                    stamp,
                    result: Box::new(result),
                },
            ));
        }
        for (surface_id, stamp, result) in immediate {
            self.apply_live_frame(&surface_id, stamp, result);
        }
        for note in notes {
            self.emit_event(note);
        }
        if tasks.is_empty() {
            return Task::none();
        }
        Task::batch(tasks)
    }

    /// Swap a decoded frame into its surface, unless the file moved on (a newer
    /// claim superseded this one) — then the stale result is dropped and the
    /// newer task applies in its place.
    fn apply_live_frame(
        &mut self,
        surface_id: &str,
        stamp: (std::time::SystemTime, u64),
        result: LiveFrameResult,
    ) {
        self.live_pending.remove(surface_id);
        let Some(surface) = self.decoded.get_mut(surface_id) else {
            return;
        };
        let seen = match &mut surface.source {
            ContentSource::LiveImage { seen, .. } | ContentSource::LiveVideo { seen, .. } => seen,
            _ => return,
        };
        if *seen != Some(stamp) {
            return;
        }
        match result {
            Ok(frame) => {
                let cached_frame = frame.clone();
                let source_id = match &surface.source {
                    ContentSource::LiveImage { source_id, .. }
                    | ContentSource::LiveVideo { source_id, .. } => Some(source_id.clone()),
                    _ => None,
                };
                surface.handle = frame.handle;
                surface.source_pixels = frame.source_pixels;
                surface.upload_pixels = frame.upload_pixels;
                apply_plan_diagnostics(
                    &mut surface.snapshot,
                    &frame.media_plan,
                    frame.upload_pixels,
                );
                surface.media_plan = frame.media_plan;
                surface.snapshot.asset_sha256 = frame.sha256;
                if let Some(source_id) = source_id {
                    if self.live_frame_cache.len() >= 32
                        && !self.live_frame_cache.contains_key(&source_id)
                        && let Some(oldest) = self.live_frame_cache.keys().next().cloned()
                    {
                        self.live_frame_cache.remove(&oldest);
                    }
                    self.live_frame_cache
                        .insert(source_id, (stamp, cached_frame));
                }
                self.bump();
            }
            Err(error) => {
                self.emit_event(format!("live_image_rejected {surface_id}: {error}"));
            }
        }
    }

    fn theme(&self) -> Theme {
        let palette = self.presented_palette();
        Theme::custom(
            format!("Canvas · {}", self.document.project_id),
            iced::theme::Palette {
                background: color(palette.background),
                text: color(palette.text),
                primary: color(palette.primary),
                success: color(palette.success),
                danger: color(palette.danger),
                warning: color(palette.warning),
            },
        )
    }

    fn view(&self, window: iced::window::Id) -> Element<'_, Message> {
        #[cfg(all(unix, feature = "lua-scenes"))]
        if let Some(host) = self
            .native_switcher
            .as_ref()
            .filter(|host| host.window == Some(window))
        {
            if let (Some(snapshot), Some(retained)) = (&host.snapshot, host.presentation.retained())
            {
                let frame = snapshot.switcher.frame;
                return self.shell_scene_layer(
                    &retained.scene,
                    LogicalSize {
                        width: f64::from(frame.width),
                        height: f64::from(frame.height),
                    },
                );
            }
            return container(space::horizontal()).into();
        }
        let viewport = self.windows.get(&window).copied().unwrap_or(self.frame);
        let viewport_x = f64::from(viewport.x - self.frame.x);
        let viewport_y = f64::from(viewport.y - self.frame.y);
        let visual_theme = self.presented_visual_theme();
        let palette = visual_theme.palette;
        let window_chrome = self.window_chrome();
        let background: iced::Background = if self.screensaver_settings.enabled {
            Color::TRANSPARENT.into()
        } else {
            match &self.document.background {
                // The theme ground is a retained GPU underlay.
                Background::Theme => Color::TRANSPARENT.into(),
                Background::Solid { color } => rgba(*color).into(),
                Background::Gradient {
                    angle_degrees,
                    stops,
                } => gradient(*angle_degrees, stops).into(),
                Background::Image { .. } | Background::Shader { .. } => Color::TRANSPARENT.into(),
                // A background written by a newer Canvas. It is retained
                // verbatim, but this build cannot draw it, so fall back to the
                // theme ground rather than leaving the desktop black.
                Background::Unsupported(_) => Color::TRANSPARENT.into(),
            }
        };
        let surface_color = color(palette.surface);
        let border_color = color(palette.border);
        let muted = color(palette.text_muted);
        let text_color = color(palette.text);
        let shell_bar_height = self
            .overlay_on_viewport(self.overlays.fpl_mark, viewport)
            .then_some(::canvas::DESKTOP_BAR_HEIGHT)
            .unwrap_or_default();
        let content_frame = Rect {
            height: self.frame.height.saturating_sub(shell_bar_height).max(1),
            ..self.frame
        };

        let content: Element<'_, Message> = match self.document.active_layout() {
            Layout::Canvas { placements } => {
                let mut placements = placements.iter().collect::<Vec<_>>();
                placements.sort_by_key(|placement| {
                    (
                        self.maximized_surfaces.contains(&placement.surface_id),
                        placement.layer,
                    )
                });
                let mut surfaces = placements
                    .into_iter()
                    .filter_map(|placement| {
                        if self.minimized_surfaces.contains(&placement.surface_id) {
                            return None;
                        }
                        let surface = self.decoded.get(&placement.surface_id)?;
                        let bounds = {
                            let target = if self.maximized_surfaces.contains(&placement.surface_id)
                            {
                                NormalizedRect {
                                    x: 0.0,
                                    y: 0.0,
                                    width: 1.0,
                                    height: 1.0,
                                }
                            } else {
                                placement.bounds
                            };
                            self.presented_surface_geometry(
                                &placement.surface_id,
                                target,
                                content_frame,
                                shell_bar_height,
                            )
                        };
                        let bounds = LogicalRect {
                            x: bounds.x - viewport_x,
                            y: bounds.y - viewport_y,
                            ..bounds
                        };
                        if bounds.right() <= 0.0
                            || bounds.bottom() <= 0.0
                            || bounds.x >= f64::from(viewport.width)
                            || bounds.y >= f64::from(viewport.height)
                        {
                            return None;
                        }
                        let widget_chrome = self.widget_chrome(&placement.surface_id);
                        let frameless = matches!(
                            widget_chrome,
                            Some(
                                canvas_widgets::WidgetChrome::Frameless
                                    | canvas_widgets::WidgetChrome::Integrated
                            )
                        );
                        let control_height = if self.document.chrome.title
                            == SurfaceTitleMode::Hidden
                            || frameless
                        {
                            0.0
                        } else {
                            f64::from(window_chrome.titlebar_height)
                        };
                        let content = self.surface_widget(
                            surface,
                            Length::Fill,
                            Length::Fill,
                            LogicalSize {
                                width: bounds.width.max(1.0),
                                height: (bounds.height - control_height).max(1.0),
                            },
                        );
                        let focused = self.document.focused_surface_id.as_deref()
                            == Some(placement.surface_id.as_str());
                        let hovered = self.hovered_surface_id.as_deref()
                            == Some(placement.surface_id.as_str());
                        let emphasis = self.focus_emphasis(&placement.surface_id);
                        let maximized = self.maximized_surfaces.contains(&placement.surface_id);
                        let chrome_hidden = self.document.chrome.title == SurfaceTitleMode::Hidden;
                        let edge = if (focused || hovered) && (!chrome_hidden || emphasis > 0.0) {
                            if window_chrome.focus_edge == "left" {
                                color(palette.danger)
                            } else {
                                color(palette.primary)
                            }
                        } else {
                            border_color
                        };
                        let chrome_style = window_chrome.clone();
                        let plane = container(column![
                            self.surface_controls(
                                &surface.snapshot.title,
                                focused,
                                maximized,
                                widget_chrome,
                            ),
                            content,
                        ])
                        .width(Length::Fixed(bounds.width as f32))
                        .height(Length::Fixed(bounds.height as f32))
                        .clip(true)
                        .style(move |_| container::Style {
                            border: iced::Border {
                                color: Color {
                                    a: if emphasis > 0.0 {
                                        0.72 + emphasis * 0.28
                                    } else if frameless && hovered {
                                        0.46
                                    } else if frameless {
                                        0.0
                                    } else if chrome_hidden {
                                        if hovered { 0.78 } else { 0.52 }
                                    } else if focused {
                                        0.88
                                    } else {
                                        0.62
                                    },
                                    ..edge
                                },
                                width: if emphasis > 0.0 {
                                    chrome_style.border_width + 0.75 + emphasis * 1.25
                                } else if frameless && hovered {
                                    1.0
                                } else if frameless {
                                    0.0
                                } else if chrome_hidden {
                                    1.0
                                } else if focused {
                                    chrome_style.border_width + 0.5
                                } else {
                                    chrome_style.border_width
                                },
                                // The shell owns one corner language. Hidden
                                // title bars used to silently force 4px while
                                // visible chrome followed the selected theme,
                                // producing a visibly unrelated set of tiles.
                                radius: if frameless {
                                    0.0
                                } else {
                                    chrome_style.border_radius
                                }
                                .into(),
                            },
                            shadow: iced::Shadow {
                                color: Color::from_rgba8(
                                    0,
                                    0,
                                    0,
                                    if emphasis > 0.0 {
                                        (chrome_style.shadow_opacity + emphasis * 0.22).min(0.92)
                                    } else if frameless {
                                        0.0
                                    } else if chrome_hidden {
                                        0.10
                                    } else if focused {
                                        chrome_style.shadow_opacity
                                    } else {
                                        chrome_style.shadow_opacity * 0.66
                                    },
                                ),
                                offset: iced::Vector::new(
                                    0.0,
                                    if frameless {
                                        0.0
                                    } else if chrome_hidden {
                                        2.0
                                    } else {
                                        chrome_style.shadow_offset
                                    },
                                ),
                                blur_radius: if emphasis > 0.0 {
                                    chrome_style.shadow_blur + emphasis * 14.0
                                } else if frameless {
                                    0.0
                                } else if chrome_hidden {
                                    8.0
                                } else {
                                    chrome_style.shadow_blur
                                },
                            },
                            ..Default::default()
                        });
                        let surface_id = placement.surface_id.clone();
                        Some(
                            container(
                                mouse_area(plane)
                                    .on_enter(Message::SurfaceHover(Some(surface_id)))
                                    .on_exit(Message::SurfaceHover(None)),
                            )
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .padding(iced::Padding {
                                top: bounds.y as f32,
                                right: 0.0,
                                bottom: 0.0,
                                left: bounds.x as f32,
                            })
                            .align_x(iced::alignment::Horizontal::Left)
                            .align_y(iced::alignment::Vertical::Top)
                            .into(),
                        )
                    })
                    .collect::<Vec<Element<'_, Message>>>();
                if let Some(zone) = self.snap_preview {
                    // Desktop-manager style dock preview: the ghost shows the
                    // half or quarter the surface will occupy on release.
                    let ghost = container(space::vertical())
                        .width(Length::Fixed(zone.width.max(1.0) as f32))
                        .height(Length::Fixed(zone.height.max(1.0) as f32))
                        .style(move |_| container::Style {
                            background: Some(iced::Background::Color(Color {
                                a: 0.18,
                                ..color(palette.primary)
                            })),
                            border: iced::Border {
                                color: Color {
                                    a: 0.9,
                                    ..color(palette.primary)
                                },
                                width: 2.0,
                                radius: 9.0.into(),
                            },
                            ..Default::default()
                        });
                    surfaces.push(
                        container(ghost)
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .padding(iced::Padding {
                                top: (zone.y - viewport_y) as f32,
                                right: 0.0,
                                bottom: 0.0,
                                left: (zone.x - viewport_x) as f32,
                            })
                            .align_x(iced::alignment::Horizontal::Left)
                            .align_y(iced::alignment::Vertical::Top)
                            .into(),
                    );
                }
                if surfaces.is_empty() {
                    container(space::vertical())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .into()
                } else {
                    stack(surfaces).into()
                }
            }
            layout => single_surface_id(layout, self.document.focused_surface_id.as_deref())
                .and_then(|id| self.decoded.get(id))
                .map_or_else(
                    || {
                        container(space::vertical())
                            .width(Length::Fill)
                            .height(Length::Fill)
                            .into()
                    },
                    |surface| {
                        if self.minimized_surfaces.contains(&surface.snapshot.id) {
                            return container(space::vertical())
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .into();
                        }
                        let mut viewport = self.media_viewport();
                        let widget_chrome = self.widget_chrome(&surface.snapshot.id);
                        if self.document.chrome.title != SurfaceTitleMode::Hidden
                            && !matches!(
                                widget_chrome,
                                Some(
                                    canvas_widgets::WidgetChrome::Frameless
                                        | canvas_widgets::WidgetChrome::Integrated
                                )
                            )
                        {
                            viewport.height = (viewport.height
                                - f64::from(window_chrome.titlebar_height))
                            .max(1.0);
                        }
                        let content =
                            self.surface_widget(surface, Length::Fill, Length::Fill, viewport);
                        container(column![
                            self.surface_controls(
                                &surface.snapshot.title,
                                true,
                                true,
                                widget_chrome,
                            ),
                            content,
                        ])
                        .center(Length::Fill)
                        .into()
                    },
                ),
        };

        // One field across the desk rather than one copy per display: this
        // window shows the slice of the shader that starts where it sits.
        let desk = screensaver::Field::Desk {
            origin: [viewport_x as f32, viewport_y as f32],
            extent: [self.frame.width as f32, self.frame.height as f32],
        };

        let mut layers: Vec<Element<'_, Message>> = Vec::new();
        if self.screensaver_settings.enabled {
            layers.push(screensaver::view_in(
                self.screensaver_index,
                self.screensaver_elapsed,
                desk,
            ));
        } else {
            match &self.document.background {
                Background::Image { asset, fit } => {
                    if let Some(handle) = self.background_assets.get(&asset.sha256) {
                        layers.push(
                            image_widget(handle.clone())
                                .width(Length::Fill)
                                .height(Length::Fill)
                                .content_fit(background_fit(*fit))
                                .into(),
                        );
                    }
                }
                Background::Shader { shader_id, .. } => layers.push(screensaver::view_in(
                    screensaver::index_for(*shader_id),
                    self.screensaver_elapsed,
                    desk,
                )),
                Background::Theme => layers.push(
                    image_widget(default_background::handle())
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .content_fit(ContentFit::Cover)
                        .into(),
                ),
                Background::Solid { .. } | Background::Gradient { .. } => {}
                // Nothing to layer: the ground above already fell back to the
                // theme gradient for a background this build cannot draw.
                Background::Unsupported(_) => {}
            }
        }
        layers.push(content);
        if let Some(picker) = self.background_picker.as_ref() {
            // Above the content, because it is a modal choice, but still below
            // the minimized dock so the desktop's own furniture stays reachable.
            layers.push(
                picker.view(self.picker_elapsed, &self.presented_palette(), &|index| {
                    Message::BackgroundTileClicked(index)
                }),
            );
        }
        if let Some(picker) = self.theme_picker.as_ref() {
            layers.push(picker.view::<Message>(&|index| Message::ThemePickerSelect(index)));
        }
        if let Some(picker) = self.project_switcher.as_ref() {
            #[cfg(feature = "lua-scenes")]
            if let Some(scene) = &picker.scene {
                let [width, height] = picker.viewport;
                let size = LogicalSize {
                    width: width as f64,
                    height: height as f64,
                };
                let mut sheet_layers = vec![self.shell_scene_layer(scene, size)];
                let rows = picker.matches();
                let start = picker.row_start();
                sheet_layers.extend(self.native_module_buttons_selected(
                    &picker.buttons,
                    size,
                    |control| {
                        if !control.enabled {
                            return None;
                        }
                        if control.id == "native-apps" || control.id == "canvas-apps" {
                            return (!picker.window_cycle).then_some(Message::ShellLauncherScope {
                                generation: picker.generation,
                                scope: if control.id == "native-apps" {
                                    ::canvas::shell_launcher::Scope::Native
                                } else {
                                    ::canvas::shell_launcher::Scope::Canvas
                                },
                            });
                        }
                        let slot = control.id.strip_prefix("row")?.parse::<usize>().ok()?;
                        if slot >= 6 {
                            return None;
                        }
                        let entry = rows.get(start + slot)?;
                        Some(Message::ShellLauncherPick {
                            generation: picker.generation,
                            revision: picker.catalog.as_ref()?.revision.clone(),
                            entry_id: entry.id.clone(),
                        })
                    },
                    |control| {
                        control
                            .id
                            .strip_prefix("row")
                            .and_then(|n| n.parse::<usize>().ok())
                            .is_some_and(|slot| start + slot == picker.selected)
                    },
                ));
                let sheet = stack(sheet_layers);
                layers.push(
                    container(container(sheet).width(width).height(height))
                        .center(Length::Fill)
                        .into(),
                );
            } else {
                layers.push(
                    container(text(&picker.status).color(color(self.presented_palette().warning)))
                        .center(Length::Fill)
                        .into(),
                );
            }
            #[cfg(not(feature = "lua-scenes"))]
            layers.push(
                container(text("Launcher requires Lua scenes"))
                    .center(Length::Fill)
                    .into(),
            );
        }
        if let Some(picker) = self.project_picker.as_ref() {
            layers.push(picker.view(&self.catalog, &self.document.project_id));
        }
        if let Some(dock) = self.minimized_dock() {
            layers.push(dock);
        }

        if self.jarvis_visible && self.overlay_on_viewport(self.overlays.presence_orb, viewport) {
            let palette = self.presented_palette();
            let orb_color = match &self.presence {
                Some(p) if p.occupied => Color::from_rgba8(34, 197, 94, 0.85),
                Some(_) => Color::from_rgba8(239, 68, 68, 0.85),
                None => Color::from_rgba8(250, 204, 21, 0.85),
            };
            let vu_height: f32 = 6.0;
            let vu_bars = 8;
            let t = self.screensaver_elapsed.as_secs_f32();
            let level = ((t * 0.7).sin() * 0.5 + 0.5) * (vu_bars - 2) as f32 + 2.0;
            let bar_elements: Vec<Element<'_, Message>> = (0..vu_bars)
                .map(|i| {
                    let active = i < level as usize;
                    let h = if active { vu_height * 2.0 } else { vu_height };
                    let c = if active {
                        color(palette.success)
                    } else {
                        Color {
                            a: 0.2,
                            ..color(palette.text_muted)
                        }
                    };
                    container(space::horizontal())
                        .width(Length::Fixed(3.0))
                        .height(Length::Fixed(h))
                        .style(move |_| container::Style {
                            background: Some(c.into()),
                            ..Default::default()
                        })
                        .into()
                })
                .collect();
            let vu = row(bar_elements).spacing(2).padding(iced::Padding {
                top: 20.0,
                right: 0.0,
                bottom: 0.0,
                left: 40.0,
            });
            layers.push(
                container(vu)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Left)
                    .align_y(iced::alignment::Vertical::Top)
                    .into(),
            );
            layers.push(
                container(space::horizontal())
                    .width(Length::Fixed(16.0))
                    .height(Length::Fixed(16.0))
                    .style(move |_| container::Style {
                        background: Some(orb_color.into()),
                        border: iced::Border {
                            color: Color {
                                a: 0.4,
                                ..orb_color
                            },
                            width: 1.0,
                            radius: 8.0.into(),
                        },
                        ..Default::default()
                    })
                    .padding(iced::Padding {
                        top: 12.0,
                        right: 0.0,
                        bottom: 0.0,
                        left: 12.0,
                    })
                    .align_x(iced::alignment::Horizontal::Left)
                    .align_y(iced::alignment::Vertical::Top)
                    .into(),
            );
        }

        if self.overlay_on_viewport(self.overlays.fpl_mark, viewport) {
            let workspace_name = workspace_identity_name(&self.catalog, &self.document.project_id);
            let primary = color(palette.primary);
            // A wall is an appliance, not a tiny Mac. Present stable shell
            // identity and health instead of advertising a keyboard shortcut
            // that does not exist on the target device.
            let project_label = format!("{workspace_name} ▾");
            let live_sources = self
                .decoded
                .values()
                .filter(|surface| matches!(surface.source, ContentSource::LiveVideo { .. }))
                .count();
            let now = canvas_widgets::live::chrono_now();
            let clock = format!("{:02}:{:02}:{:02}", now.hour, now.minute, now.second);
            let back = button(text("‹").size(18).center())
                .padding([2, 9])
                .on_press_maybe(
                    self.jump_history
                        .can_go_back()
                        .then_some(Message::ShellBack),
                )
                .style(move |_, status| {
                    canvas_widgets::live::app_chrome::cube_button_style(visual_theme, false, status)
                });
            let forward = button(text("›").size(18).center())
                .padding([2, 9])
                .on_press_maybe(
                    self.jump_history
                        .can_go_forward()
                        .then_some(Message::ShellForward),
                )
                .style(move |_, status| {
                    canvas_widgets::live::app_chrome::cube_button_style(visual_theme, false, status)
                });
            let project = button(text(project_label).size(fpl_gfx::chrome::BAR_TEXT_SIZE))
                .height(Length::Fixed(fpl_gfx::chrome::BAR_CONTROL_HEIGHT as f32))
                .padding([0, fpl_gfx::chrome::BAR_PADDING as u16])
                .on_press(Message::ShellProjects)
                .style(move |_, status| {
                    canvas_widgets::live::app_chrome::quiet_button_style(visual_theme, true, status)
                });
            let has_creator = self.decoded.values().any(|surface| {
                matches!(
                    &surface.source,
                    ContentSource::Prompt { task_kinds, .. } if !task_kinds.is_empty()
                )
            });
            let create = button(text("＋ NEW").size(10))
                .padding([5, 10])
                .on_press_maybe(has_creator.then_some(Message::ShellNewTask))
                .style(move |_, status| {
                    canvas_widgets::live::app_chrome::cube_button_style(visual_theme, false, status)
                });
            let identity = row![
                text("fungOS")
                    .size(fpl_gfx::chrome::BAR_TEXT_SIZE)
                    .color(primary),
                back,
                forward,
                project,
                text("/").size(11).color(Color { a: 0.40, ..muted }),
                text(self.document.active_page_name().to_owned())
                    .size(fpl_gfx::chrome::BAR_TEXT_SIZE)
                    .color(Color { a: 0.72, ..muted }),
                create,
            ]
            .spacing(fpl_gfx::chrome::BAR_PADDING)
            .align_y(iced::Alignment::Center);
            let health = row![
                text(format!("{live_sources} SOURCES"))
                    .size(10)
                    .color(Color { a: 0.72, ..primary }),
                text(clock.chars().take(5).collect::<String>())
                    .size(fpl_gfx::chrome::BAR_TEXT_SIZE)
                    .color(Color {
                        a: 0.94,
                        ..text_color
                    }),
            ]
            .spacing(10)
            .align_y(iced::Alignment::Center);
            let bar = row![identity, space::horizontal(), health].align_y(iced::Alignment::Center);
            let signature = container(bar)
                .width(Length::Fill)
                .height(Length::Fixed(shell_bar_height as f32))
                .padding([0, fpl_gfx::chrome::BAR_PADDING as u16])
                .align_y(iced::alignment::Vertical::Center)
                .style(move |_| container::Style {
                    background: Some(color(visual_theme.palette.surface).into()),
                    border: iced::Border {
                        color: Color {
                            a: 0.64,
                            ..border_color
                        },
                        width: 0.0,
                        radius: 0.0.into(),
                    },
                    shadow: iced::Shadow::default(),
                    ..Default::default()
                });
            let signature: Element<'static, Message> = signature.into();
            #[cfg(feature = "lua-scenes")]
            let signature = if let Some(presentation) = self.desktop_bar.retained() {
                let size = LogicalSize {
                    width: f64::from(viewport.width),
                    height: f64::from(shell_bar_height),
                };
                let controls = self.native_module_buttons_selected(
                    &presentation.buttons,
                    size,
                    |control| {
                        if !control.enabled {
                            return None;
                        }
                        match control.id.as_str() {
                            "back" => self
                                .jump_history
                                .can_go_back()
                                .then_some(Message::ShellBack),
                            "forward" => self
                                .jump_history
                                .can_go_forward()
                                .then_some(Message::ShellForward),
                            "launcher" => Some(Message::ShellProjects),
                            "create" => has_creator.then_some(Message::ShellNewTask),
                            _ => None,
                        }
                    },
                    |control| control.id == "launcher",
                );
                let mut children = vec![self.scene_layer(&presentation.scene, size)];
                children.extend(controls);
                container(stack(children))
                    .width(Length::Fill)
                    .height(Length::Fixed(shell_bar_height as f32))
                    .clip(true)
                    .into()
            } else {
                signature
            };
            layers.push(
                container(signature)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(iced::alignment::Horizontal::Center)
                    .align_y(iced::alignment::Vertical::Top)
                    .into(),
            );
        }

        #[cfg(feature = "lua-scenes")]
        if shell_bar_height > 0
            && self
                .project_switcher
                .as_ref()
                .is_none_or(|picker| picker.window_cycle)
        {
            if let Some(presentation) = self.desktop_dock.retained() {
                let dock_height = ::canvas::shell_geometry::DOCK_HEIGHT;
                let size = LogicalSize {
                    width: f64::from(viewport.width),
                    height: f64::from(dock_height),
                };
                let entries = self.dock_entries();
                let controls = self.native_module_buttons_selected(
                    &presentation.buttons,
                    size,
                    |control| match control.id.as_str() {
                        "launcher" => Some(Message::ShellCanvasApps),
                        id => id
                            .strip_prefix("item")
                            .and_then(|n| n.parse::<usize>().ok())
                            .filter(|n| *n < 6)
                            .and_then(|n| entries.get(n).cloned())
                            .map(|entry| match entry.choice {
                                ::canvas::shell_launcher::Choice::Surface { id } => {
                                    Message::ShellFocusSurface(id)
                                }
                                _ => Message::ShellDockActivate(entry.id),
                            }),
                    },
                    |control| {
                        control
                            .id
                            .strip_prefix("item")
                            .and_then(|n| n.parse::<usize>().ok())
                            .and_then(|n| entries.get(n))
                            .is_some_and(|entry| {
                                !self.desktop_focused
                                    && entry.id.strip_prefix("surface:").is_some_and(|id| {
                                        self.document.focused_surface_id.as_deref() == Some(id)
                                    })
                            })
                    },
                );
                let mut children = vec![self.shell_scene_layer(&presentation.scene, size)];
                children.extend(controls);
                layers.push(
                    container(container(stack(children)).height(Length::Fixed(dock_height)))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(iced::Padding {
                            bottom: 12.0,
                            ..Default::default()
                        })
                        .align_y(iced::alignment::Vertical::Bottom)
                        .into(),
                );
            }
        }

        let global_tasks = self
            .decoded
            .iter()
            .flat_map(|(surface_id, surface)| match &surface.source {
                ContentSource::Prompt {
                    tasks, active_task, ..
                } => tasks
                    .iter()
                    .map(|task| (surface_id.clone(), task, *active_task == Some(task.id)))
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect::<Vec<_>>();
        if !global_tasks.is_empty() {
            let mut task_switcher = row![
                text("ACTIVE TASKS")
                    .size(10)
                    .color(color(palette.text_muted))
            ]
            .spacing(6)
            .align_y(iced::Alignment::Center);
            for (surface_id, task, selected) in global_tasks.iter().rev().take(6) {
                let selected = *selected;
                let state_glyph = match task.state.as_str() {
                    "complete" => "✓",
                    "failed" => "!",
                    "working" | "delivered" => "●",
                    _ => "○",
                };
                let summary: String = task.prompt.chars().take(28).collect();
                task_switcher = task_switcher.push(
                    button(
                        text(format!(
                            "{state_glyph} {} · {summary}",
                            task.kind.to_uppercase()
                        ))
                        .size(11),
                    )
                    .padding([6, 10])
                    .on_press(Message::PromptTaskSelected {
                        surface_id: surface_id.clone(),
                        task_id: task.id,
                    })
                    .style(move |_, status| {
                        canvas_widgets::live::app_chrome::cube_button_style(
                            visual_theme,
                            selected,
                            status,
                        )
                    }),
                );
            }
            layers.push(
                container(container(task_switcher).padding([6, 8]).style(move |_| {
                    container::Style {
                        background: Some(
                            Color {
                                a: 0.94,
                                ..surface_color
                            }
                            .into(),
                        ),
                        border: iced::Border {
                            color: border_color,
                            width: 1.0,
                            radius: 6.0.into(),
                        },
                        ..Default::default()
                    }
                }))
                .width(Length::Fill)
                .height(Length::Fill)
                .align_x(iced::alignment::Horizontal::Center)
                .align_y(iced::alignment::Vertical::Bottom)
                .padding(14)
                .into(),
            );
        }

        // Voice presentation needs the gfx `voice_popup` module; it is absent
        // from the upstream gfx refs, so the whole overlay is feature-gated
        // rather than faked. See `voice-popup` in Cargo.toml.
        #[cfg(feature = "voice-popup")]
        if let Some(active) = self
            .visible_indicator_id
            .as_ref()
            .and_then(|id| self.indicators.get(id))
        {
            layers.push(indicator::view(
                active,
                self.indicator_started.elapsed(),
                (self.indicator_started.elapsed().as_secs_f32()
                    / fpl_gfx::voice_popup::REVEAL_SECONDS)
                    .min(1.0),
                indicator::IndicatorPalette {
                    panel: Color {
                        a: 0.94,
                        ..surface_color
                    },
                    border: border_color,
                    text: text_color,
                    muted,
                },
            ));
        } else if let Some((active, since)) = &self.indicator_dismissing {
            layers.push(indicator::view(
                active,
                self.indicator_started.elapsed(),
                (1.0 - since.elapsed().as_secs_f32() / fpl_gfx::voice_popup::DISMISS_SECONDS)
                    .max(0.0),
                indicator::IndicatorPalette {
                    panel: surface_color,
                    border: border_color,
                    text: text_color,
                    muted,
                },
            ));
        }

        if !self.notifications.is_empty() {
            layers.push(notification::view(
                &self.notifications,
                NOTIFICATION_LIFETIME,
                notification::NotificationPalette {
                    panel: surface_color,
                    border: border_color,
                    text: text_color,
                    muted,
                },
            ));
        }

        let presence_palette = presence::PresencePalette {
            panel: Color {
                a: 0.94,
                ..surface_color
            },
            border: border_color,
            text: text_color,
            muted,
            presence: color(palette.primary),
            face: color(palette.success),
            voice: color(palette.warning),
            warning: color(palette.danger),
        };
        if self.jarvis_visible
            && self.overlay_on_viewport(self.overlays.presence_rail, viewport)
            && let Some(snapshot) = self.presence.as_ref()
        {
            let stale = self.presence_received.is_none_or(|received| {
                received.elapsed() >= Duration::from_millis(snapshot.expires_after_ms)
            });
            layers.push(presence::rail(snapshot, stale, presence_palette));
        }
        if self.jarvis_visible
            && self.overlay_on_viewport(self.overlays.presence_preview, viewport)
            && let Some(preview) = self.presence_preview.as_ref()
        {
            layers.push(presence::preview(
                presence::Preview {
                    value: &preview.value,
                    image: preview.handle.clone(),
                    elapsed: preview.started.elapsed(),
                },
                presence_palette,
            ));
        }

        let mesh_nodes = self
            .mesh
            .as_ref()
            .map(|mesh| mesh.nodes.as_slice())
            .unwrap_or_default();
        if self.hud_visible && self.overlay_on_viewport(self.overlays.system_hud, viewport) {
            layers.push(hud::view(
                &self.metrics,
                mesh_nodes,
                hud::HudPalette {
                    panel: Color {
                        a: 0.94,
                        ..surface_color
                    },
                    border: border_color,
                    text: text_color,
                    muted,
                    primary: color(palette.primary),
                    success: color(palette.success),
                    warning: color(palette.warning),
                    danger: color(palette.danger),
                },
            ));
        }

        if self.shortcuts_visible && self.overlay_on_viewport(self.overlays.shortcut_help, viewport)
        {
            layers.push(shortcut_help(
                surface_color,
                border_color,
                text_color,
                muted,
            ));
        }

        // Canvas-native text input overlay for JARVIS. Appears as a text field
        // on the wall, not as a macOS dialog. Submit with Enter.
        if self.jarvis_text_visible {
            layers.push(jarvis_text_input(
                &self.jarvis_text,
                self.jarvis_text_reply.as_deref(),
                surface_color,
                border_color,
                text_color,
                muted,
            ));
        }

        if self.screensaver_settings.enabled
            || matches!(self.document.background, Background::Shader { .. })
        {
            layers.push(
                container(
                    text(screensaver::attribution(self.screensaver_index))
                        .size(10)
                        .color(Color { a: 0.45, ..muted }),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(iced::Padding {
                    top: 0.0,
                    right: 18.0,
                    bottom: 14.0,
                    left: 0.0,
                })
                .align_x(iced::alignment::Horizontal::Right)
                .align_y(iced::alignment::Vertical::Bottom)
                .into(),
            );
            if self.screensaver_settings.show_fps {
                let fps = if self.screensaver_fps > 0.0 {
                    format!(
                        "{:.1} / {} FPS",
                        self.screensaver_fps, self.screensaver_settings.max_fps
                    )
                } else {
                    format!("measuring / {} FPS", self.screensaver_settings.max_fps)
                };
                layers.push(
                    container(text(fps).size(12).color(Color { a: 0.45, ..muted }))
                        .width(Length::Fill)
                        .height(Length::Fill)
                        .padding(iced::Padding {
                            top: 0.0,
                            right: 20.0,
                            bottom: 18.0,
                            left: 0.0,
                        })
                        .align_x(iced::alignment::Horizontal::Right)
                        .align_y(iced::alignment::Vertical::Bottom)
                        .into(),
                );
            }
            if self.screensaver_settings.show_time {
                let (clock, _date) = hud::now_local();
                layers.push(
                    container(text(clock).size(72).color(Color {
                        a: 0.85,
                        ..text_color
                    }))
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .center(Length::Fill)
                    .into(),
                );
            }
        }

        let ground = container(stack(layers))
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| container::Style {
                background: Some(background),
                ..Default::default()
            });
        // An underlay must not become the content stack's sizing base.
        let ground: Element<'_, Message> = if !self.screensaver_settings.enabled
            && matches!(self.document.background, Background::Unsupported(_))
        {
            stack([ground.into()])
                .push_under(canvas_widgets::live::retained_gradient::view(
                    color(palette.background),
                    color(palette.background_strong),
                ))
                .into()
        } else {
            ground.into()
        };
        mouse_area(ground)
            .interaction(self.pointer_interaction(window))
            .into()
    }

    fn overlay_on_viewport(&self, target: OverlayTarget, viewport: Rect) -> bool {
        let mut selected = self
            .selected_display_ids
            .iter()
            .filter_map(|id| self.displays.iter().find(|display| display.id == *id))
            .map(|display| display.frame);
        match target {
            OverlayTarget::Hidden => false,
            OverlayTarget::All => true,
            OverlayTarget::Leftmost => selected
                .by_ref()
                .map(|frame| frame.x)
                .min()
                .is_some_and(|x| viewport.x == x),
            OverlayTarget::Center => {
                let frames = selected.collect::<Vec<_>>();
                if frames.is_empty() {
                    false
                } else {
                    let left = frames
                        .iter()
                        .map(|frame| frame.x)
                        .min()
                        .unwrap_or(viewport.x);
                    let right = frames
                        .iter()
                        .map(|frame| frame.x + frame.width as i32)
                        .max()
                        .unwrap_or(viewport.x + viewport.width as i32);
                    let center = f64::from(left + right) / 2.0;
                    frames
                        .into_iter()
                        .min_by(|first, second| {
                            let first_center = f64::from(first.x) + f64::from(first.width) / 2.0;
                            let second_center = f64::from(second.x) + f64::from(second.width) / 2.0;
                            (first_center - center)
                                .abs()
                                .total_cmp(&(second_center - center).abs())
                                .then_with(|| first.x.cmp(&second.x))
                        })
                        .is_none_or(|frame| frame == viewport)
                }
            }
            OverlayTarget::Rightmost => selected
                .map(|frame| frame.x)
                .max()
                .is_some_and(|x| viewport.x == x),
        }
    }
}

fn jarvis_text_input<'a, Message: 'a>(
    input_text: &'a str,
    reply: Option<&'a str>,
    surface: Color,
    border: Color,
    text_color: Color,
    muted: Color,
) -> Element<'a, Message> {
    let display = if let Some(reply) = reply {
        column![
            text("JARVIS:").size(14).color(muted),
            text(reply).size(16).color(text_color),
        ]
        .spacing(4)
    } else {
        let placeholder = text("Type message to JARVIS (Enter to send, Esc to close)")
            .size(14)
            .color(muted);
        let label = if input_text.is_empty() {
            placeholder
        } else {
            text(input_text).size(16).color(text_color)
        };
        column![
            text("To JARVIS:").size(14).color(muted),
            label,
            text("▌").size(16).color(text_color),
        ]
        .spacing(4)
    };

    container(display)
        .padding(18)
        .width(Length::Fill)
        .height(Length::Shrink)
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Bottom)
        .style(move |_| container::Style {
            background: Some(Color { a: 0.94, ..surface }.into()),
            border: iced::Border {
                color: border,
                width: 1.0,
                radius: 12.0.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba8(0, 0, 0, 0.42),
                offset: iced::Vector::new(0.0, 14.0),
                blur_radius: 44.0,
            },
            ..Default::default()
        })
        .into()
}

fn shortcut_help<'a>(
    surface: Color,
    border: Color,
    text_color: Color,
    muted: Color,
) -> Element<'a, Message> {
    let title = text("KEYBOARD").size(13).color(muted);
    let shortcuts = text(format!(
        "Clipboard\n\
         Ctrl-X                 Cut\n\
         Ctrl-C                 Copy\n\
         Ctrl-V                 Paste\n\
         \n\
         Workspaces\n\
         Ctrl-Shift-↑ / ↓        Swap project workspace\n\
         Ctrl-← / Ctrl-→        Saved views (reserved)\n\
         \n\
         Canvas\n\
         F2                     Choose background\n\
         F3                     Toggle diagnostics\n\
         F4                     Choose theme / palette\n\
         Ctrl-Space / Ctrl-P   Canvas launcher\n\
         Ctrl-1 / 2 / 3         Toggle left / center / right display\n\
         Ctrl-0                 Show Canvas on every display\n\
         {SNAP_LAYOUT_KEYS:<22} Snap layout to saved view (again to restore)\n\
         {FIT_LAYOUT_KEYS:<22} Fit layout to the displays Canvas is on\n\
         {CANVAS_TOGGLE_KEYS:<22} Toggle Canvas\n\
         {JARVIS_TOGGLE_KEYS:<22} Toggle JARVIS presence\n\
          {JARVIS_TEXT_KEYS:<22} Text to JARVIS\n\
          {JUMP_BACK_KEYS:<22} Jump back (vim Ctrl-O)\n\
          {JUMP_FORWARD_KEYS:<22} Jump forward\n\
          {FOCUSED_HELP_KEYS:<22} Toggle Help (Canvas focused)\n\
          {SHORTCUT_HELP_KEYS:<22} Toggle Help globally"
    ))
    .font(iced::Font::MONOSPACE)
    .size(15)
    .line_height(iced::widget::text::LineHeight::Relative(1.55))
    .color(text_color);
    let panel = container(column![title, shortcuts].spacing(14))
        .width(Length::Fixed(620.0))
        .padding(24)
        .style(move |_| container::Style {
            background: Some(Color { a: 0.97, ..surface }.into()),
            border: iced::Border {
                color: border,
                width: 1.0,
                radius: 14.0.into(),
            },
            shadow: iced::Shadow {
                color: Color::from_rgba8(0, 0, 0, 0.42),
                offset: iced::Vector::new(0.0, 14.0),
                blur_radius: 44.0,
            },
            ..Default::default()
        });
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center(Length::Fill)
        .into()
}

fn render_mermaid(source: &Path, surface_id: &str) -> Result<PathBuf, Failure> {
    let temp = std::env::temp_dir().join(format!(
        "canvas-mermaid-{}-{}.png",
        std::process::id(),
        surface_id
    ));
    let status = Process::new("mmdc")
        .arg("-i")
        .arg(source)
        .arg("-o")
        .arg(&temp)
        .arg("--quiet")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|e| Failure::resource(format!("mmdc not available: {e}")))?;
    if !status.success() {
        return Err(Failure::resource("mmdc failed to render Mermaid diagram"));
    }
    if !temp.exists() {
        return Err(Failure::resource("mmdc produced no output"));
    }
    Ok(temp)
}

fn load_terminal(
    surface_id: String,
    shell: &str,
    title: Option<String>,
) -> Result<LoadedContent, Failure> {
    let (master_fd, stdin_fd, pid) = terminal::spawn_terminal(shell).map_err(Failure::resource)?;
    let display_title = title.unwrap_or_else(|| format!("Terminal: {shell}"));
    let snapshot = SurfaceSnapshot {
        id: surface_id.clone(),
        title: display_title,
        content: ContentKind::Terminal,
        asset_sha256: None,
        page_index: None,
        page_count: None,
        source_pixels: None,
        rendered_pixels: None,
        display_logical: None,
        scale_factor: None,
    };
    Ok(LoadedContent {
        snapshot,
        source: ContentSource::Terminal {
            master_fd,
            stdin_fd,
            pid,
            buffer: String::new(),
        },
        handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
        asset: AssetRef {
            sha256: format!("{:x}", Sha256::digest(surface_id.as_bytes())),
            media_type: "application/x-terminal".into(),
            byte_length: 1,
        },
        source_pixels: [1, 1],
        media_plan: MediaPlan {
            viewport_logical: LogicalSize {
                width: 1.0,
                height: 1.0,
            },
            source_dimensions: SourceDimensions::RasterPixels(PixelSize {
                width: 1,
                height: 1,
            }),
            target_px: PixelSize {
                width: 1,
                height: 1,
            },
            display_logical: LogicalSize {
                width: 1.0,
                height: 1.0,
            },
            scale_factor: 1.0,
            cache_key: MediaCacheKey {
                source_id: "terminal".to_string(),
                variant: MediaVariant::Raster,
                pixel_size: PixelSize {
                    width: 1,
                    height: 1,
                },
            },
        },
        upload_pixels: [1, 1],
    })
}

fn color(rgb: [u8; 3]) -> Color {
    Color::from_rgb8(rgb[0], rgb[1], rgb[2])
}

fn stream_matte_padding(source_pixels: [u32; 2]) -> f32 {
    ::canvas::stream_geometry::matte_padding(source_pixels)
}

fn stream_source_pixels(surface: &RuntimeSurface) -> [u32; 2] {
    match &surface.source {
        ContentSource::LiveVideo {
            path,
            width,
            height,
            ..
        } => canvas_core::video_frame_size_file_for(path)
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| canvas_core::parse_frame_size(&text))
            .map(|(width, height)| [width, height])
            .unwrap_or([*width, *height]),
        _ => surface.source_pixels,
    }
}

fn workspace_identity_name<'a>(catalog: &'a Catalog, project_id: &'a str) -> &'a str {
    catalog
        .project(project_id)
        .map(|project| project.name.as_str())
        .unwrap_or(project_id)
}

fn rgba(value: [u8; 4]) -> Color {
    Color::from_rgba8(value[0], value[1], value[2], f32::from(value[3]) / 255.0)
}

fn gradient(angle_degrees: f32, stops: &[GradientStop]) -> iced::gradient::Linear {
    stops.iter().fold(
        iced::gradient::Linear::new(angle_degrees.to_radians()),
        |gradient, stop| gradient.add_stop(stop.offset, rgba(stop.color)),
    )
}

fn background_fit(fit: BackgroundFit) -> ContentFit {
    match fit {
        BackgroundFit::Cover => ContentFit::Cover,
        BackgroundFit::Contain => ContentFit::Contain,
        BackgroundFit::Stretch => ContentFit::Fill,
        BackgroundFit::Center => ContentFit::None,
    }
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    if unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) } == 0 {
        let len = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        let name = String::from_utf8_lossy(&buffer[..len]).into_owned();
        if !name.trim().is_empty() && name.len() <= 64 {
            return name;
        }
    }
    "local".to_string()
}

/// One owner-local HTTP GET for Alfalfa discovery: fresh connection, bounded
/// by both connect and read timeouts, because this runs on the daemon's main
/// thread at startup and on picker open — an untimed read there wedges the
/// whole wall the moment Alfalfa answers slowly.
fn alfalfa_get(addr: &str, path: &str, timeout: std::time::Duration) -> Option<String> {
    let mut stream = std::net::TcpStream::connect_timeout(&addr.parse().ok()?, timeout).ok()?;
    stream.set_read_timeout(Some(timeout)).ok()?;
    use std::io::Write;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = String::new();
    use std::io::Read;
    let _ = stream.read_to_string(&mut response);
    Some(response.split("\r\n\r\n").nth(1).unwrap_or("").to_owned())
}

fn push_catalog_project(catalog: &mut Catalog, id: String, name: String) {
    if id.is_empty() || catalog.projects.iter().any(|p| p.id == id) {
        return;
    }
    catalog.projects.push(canvas_core::Project {
        id,
        name,
        theme: canvas_core::ThemeSelection::Coherence {
            mode: canvas_core::ThemeMode::Dark,
            accent: canvas_core::Accent::Brand,
        },
        resources: Vec::new(),
    });
}

/// Fold `/api/v1/projects` into the catalog; returns whether any were found.
fn merge_projects_body(catalog: &mut Catalog, body: &str) -> bool {
    let Ok(projects) = serde_json::from_str::<Vec<serde_json::Value>>(body) else {
        return false;
    };
    let found = !projects.is_empty();
    for project in projects {
        let Some(p) = project.get("project").and_then(|p| p.as_object()) else {
            continue;
        };
        let id = p.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
        push_catalog_project(catalog, id.to_string(), name.to_string());
    }
    found
}

/// Fold `/api/v1/repos` into the catalog (fallback roster).
fn merge_repos_body(catalog: &mut Catalog, body: &str) {
    let Ok(repos) = serde_json::from_str::<Vec<serde_json::Value>>(body) else {
        return;
    };
    for repo in repos {
        let id = repo.get("id").and_then(|v| v.as_str()).unwrap_or("");
        push_catalog_project(catalog, id.to_string(), id.to_string());
    }
}

fn theme_chrome_for(
    packs: &[::canvas::theme_pack::ThemePack],
    selections: &BTreeMap<String, String>,
    project_id: &str,
) -> ::canvas::theme_pack::ThemeChrome {
    selections
        .get(project_id)
        .and_then(|id| packs.iter().find(|pack| &pack.id == id))
        .map(|pack| pack.chrome.clone())
        .unwrap_or_default()
}

fn interaction_engine(
    frame: Rect,
    chrome: SurfaceChrome,
    titlebar_height: f32,
) -> InteractionEngine {
    interaction_engine_with_shell(frame, chrome, titlebar_height, 0)
}

fn interaction_engine_with_shell(
    frame: Rect,
    chrome: SurfaceChrome,
    titlebar_height: f32,
    shell_height: u32,
) -> InteractionEngine {
    let workspace = interaction_workspace_bounds_with_shell(frame, chrome, shell_height);
    InteractionEngine::new(InteractionConfig {
        workspace_bounds: workspace,
        minimum_surface_size: LogicalPoint { x: 160.0, y: 120.0 },
        resize_handle_size: SURFACE_RESIZE_HANDLE_SIZE,
        top_bar_height: f64::from(titlebar_height),
        snap_distance: 12.0,
        press_deadzone: 4.0,
        snap_zone_trigger: 44.0,
        grid_size: Some(8.0),
    })
    .expect("Canvas interaction defaults must remain valid")
}

fn interaction_workspace_bounds(frame: Rect, chrome: SurfaceChrome) -> LogicalRect {
    interaction_workspace_bounds_with_shell(frame, chrome, 0)
}

/// The rectangle a surface may occupy: the content frame (the display minus the
/// desktop bar) projected the same way drawing projects it, then moved down by
/// the bar. Drag clamping and pointer positions have to agree with that, or the
/// window snaps away from the point it was grabbed by.
fn interaction_workspace_bounds_with_shell(
    frame: Rect,
    chrome: SurfaceChrome,
    shell_height: u32,
) -> LogicalRect {
    let content = Rect {
        height: frame.height.saturating_sub(shell_height).max(1),
        ..frame
    };
    let mut bounds = visible_surface_bounds(
        NormalizedRect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        content,
        chrome,
    );
    bounds.y += f64::from(shell_height);
    bounds
}

fn resize_cursor(handle: ResizeHandle) -> mouse::Interaction {
    match handle {
        ResizeHandle::East | ResizeHandle::West => mouse::Interaction::ResizingHorizontally,
        ResizeHandle::North | ResizeHandle::South => mouse::Interaction::ResizingVertically,
        ResizeHandle::NorthEast | ResizeHandle::SouthWest => {
            mouse::Interaction::ResizingDiagonallyUp
        }
        ResizeHandle::NorthWest | ResizeHandle::SouthEast => {
            mouse::Interaction::ResizingDiagonallyDown
        }
    }
}

fn parse_dataviz_file(file: &Path, chart_type: &str) -> Result<ChartDocument, String> {
    let content = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
    let kind = ChartKind::parse(chart_type).map_err(|error| error.to_string())?;
    let title = file
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Chart");
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext {
        ref extension if extension == "csv" => {
            ::canvas::chart::parse_delimited(&content, b',', title, kind)
        }
        ref extension if extension == "tsv" => {
            ::canvas::chart::parse_delimited(&content, b'\t', title, kind)
        }
        ref extension if extension == "json" => ::canvas::chart::parse_json(&content, title, kind),
        _ => return Err("chart file must use .csv, .tsv, or .json".into()),
    }
    .map_err(|error| error.to_string())
}

fn surface_layout_metrics(frame: Rect, chrome: SurfaceChrome) -> (f64, f64, f64, f64) {
    let frame_width = f64::from(frame.width.max(1));
    let frame_height = f64::from(frame.height.max(1));
    let padding = f64::from(chrome.canvas_padding).min((frame_width / 4.0).min(frame_height / 4.0));
    let inner_width = (frame_width - padding * 2.0).max(1.0);
    let inner_height = (frame_height - padding * 2.0).max(1.0);
    (
        padding,
        inner_width,
        inner_height,
        f64::from(chrome.surface_gap) / 2.0,
    )
}

fn visible_surface_bounds(
    bounds: NormalizedRect,
    frame: Rect,
    chrome: SurfaceChrome,
) -> LogicalRect {
    let (padding, inner_width, inner_height, gap_inset) = surface_layout_metrics(frame, chrome);
    LogicalRect {
        x: padding + f64::from(bounds.x) * inner_width + gap_inset,
        y: padding + f64::from(bounds.y) * inner_height + gap_inset,
        width: (f64::from(bounds.width) * inner_width - gap_inset * 2.0).max(1.0),
        height: (f64::from(bounds.height) * inner_height - gap_inset * 2.0).max(1.0),
    }
}

fn flatten_layout(layout: &Layout) -> Vec<Placement> {
    ::canvas::layout_tree::placements(layout)
}

fn layout_surface_bounds(
    layout: &Layout,
    surface_id: &str,
    bounds: LogicalRect,
) -> Option<LogicalRect> {
    match layout {
        Layout::Empty | Layout::Canvas { .. } => None,
        Layout::Surface {
            surface_id: candidate,
        } if candidate == surface_id => Some(bounds),
        Layout::Surface { .. } => None,
        Layout::Stack {
            active_surface_id, ..
        } if active_surface_id == surface_id => Some(bounds),
        Layout::Stack { .. } => None,
        Layout::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let ratio = f64::from(*ratio);
            let (first_bounds, second_bounds) = match axis {
                Axis::Horizontal => {
                    let first_width = bounds.width * ratio;
                    (
                        LogicalRect {
                            width: first_width,
                            ..bounds
                        },
                        LogicalRect {
                            x: bounds.x + first_width,
                            width: bounds.width - first_width,
                            ..bounds
                        },
                    )
                }
                Axis::Vertical => {
                    let first_height = bounds.height * ratio;
                    (
                        LogicalRect {
                            height: first_height,
                            ..bounds
                        },
                        LogicalRect {
                            y: bounds.y + first_height,
                            height: bounds.height - first_height,
                            ..bounds
                        },
                    )
                }
            };
            layout_surface_bounds(first, surface_id, first_bounds)
                .or_else(|| layout_surface_bounds(second, surface_id, second_bounds))
        }
    }
}

fn logical_to_screen_rect(bounds: LogicalRect, frame: Rect) -> Option<Rect> {
    if ![bounds.x, bounds.y, bounds.width, bounds.height]
        .iter()
        .all(|value| value.is_finite())
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return None;
    }
    let local_x = bounds.x.round() as i32;
    let local_y = bounds.y.round() as i32;
    Some(Rect {
        x: frame.x.checked_add(local_x)?,
        y: frame.y.checked_add(local_y)?,
        width: bounds.width.ceil().max(1.0) as u32,
        height: bounds.height.ceil().max(1.0) as u32,
    })
}

fn normalize_visible_surface_bounds(
    bounds: LogicalRect,
    frame: Rect,
    chrome: SurfaceChrome,
) -> NormalizedRect {
    let (padding, inner_width, inner_height, gap_inset) = surface_layout_metrics(frame, chrome);
    // Horizontal positions may be negative when a surface is parked partly
    // offscreen. The interaction engine has already kept a usable section of
    // its top bar reachable, so clamping the origin here would turn the move
    // into a resize by shortening the stored width to the workspace edge.
    let x = (bounds.x - gap_inset - padding) / inner_width;
    let y = ((bounds.y - gap_inset - padding) / inner_height).max(0.0);
    let width = ((bounds.width + gap_inset * 2.0) / inner_width).clamp(f64::EPSILON, 1.0);
    let height = ((bounds.height + gap_inset * 2.0) / inner_height).clamp(f64::EPSILON, 1.0);
    NormalizedRect {
        x: x as f32,
        y: y as f32,
        width: width as f32,
        height: height as f32,
    }
}

/// Resolve a scene label's ink role against the presented palette.
///
/// A scene cannot ask for a colour, only for a meaning, so the same authored
/// grid stays readable on a light theme and a dark one. Absent means `Text`,
/// which is what every pre-`ink` scene drew.
/// The identity of everything an authored scene depends on.
///
/// The civil date is in the key and no finer time is: an authored calendar has
/// to change at midnight, and nothing an author legitimately draws changes
/// because a minute passed. Keying on minutes would start a VM for every module
/// on the wall, sixty times an hour, to redraw the same glyphs.
#[cfg(feature = "lua-scenes")]
fn module_key(
    module: &str,
    inputs: &BTreeMap<String, canvas_widgets::WidgetInput>,
    visual: &VisualTheme,
    year: i32,
    month: u32,
    day: u32,
    artifact_revision: u64,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    module.hash(&mut hasher);
    artifact_revision.hash(&mut hasher);
    // Serialized rather than structurally hashed: a spec is the artifact an agent
    // authors and a person inspects, so byte equality is the honest key.
    serde_json::to_string(inputs)
        .unwrap_or_default()
        .hash(&mut hasher);
    for size in [
        visual.metrics.micro,
        visual.metrics.small,
        visual.metrics.body,
        visual.metrics.large,
        visual.metrics.title,
        visual.metrics.display,
        visual.metrics.hero,
        visual.metrics.space_2,
        visual.metrics.radius,
    ] {
        f64::from(size).to_bits().hash(&mut hasher);
    }
    for role in [
        visual.palette.text,
        visual.palette.text_muted,
        visual.palette.primary,
        visual.palette.success,
        visual.palette.warning,
        visual.palette.danger,
        visual.palette.info,
        visual.palette.background,
    ] {
        role.hash(&mut hasher);
    }
    (year, month, day).hash(&mut hasher);
    hasher.finish()
}

/// Digest the two authored files that define a module. Missing/error states are
/// hashed too, so installing or repairing a file changes the retained key and
/// triggers a fresh candidate render on the next one-second poll.
#[cfg(feature = "lua-scenes")]
fn module_artifact_revision(name: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for extension in ["json", "lua"] {
        let relative = PathBuf::from("modules").join(format!("{name}.{extension}"));
        match ::canvas::runtime_config::read(
            &relative,
            ::canvas::stream_binding::MAX_ADAPTER_BYTES as u64,
        ) {
            Ok(Some(bytes)) => bytes.hash(&mut hasher),
            Ok(None) => "missing".hash(&mut hasher),
            Err(error) => error.to_string().hash(&mut hasher),
        }
    }
    hasher.finish()
}

/// Run an authored module against its declared inputs and return its scene.
///
/// Source and manifest are read from the Canvas runtime directory, disk-first,
/// the same rule stream binders already follow: adding a wall widget is writing
/// two files, not rebuilding Canvas. A missing file is an error naming the path,
/// never an empty scene — an uninstalled module and a widget with nothing in it
/// are otherwise indistinguishable from across the room.
#[cfg(feature = "lua-scenes")]
fn resolve_module(
    name: &str,
    inputs: &BTreeMap<String, canvas_widgets::WidgetInput>,
    theme: ::canvas::lua_module::ModuleThemeTokens,
    now_ms: u64,
) -> std::result::Result<::canvas::lua_module::RetainedModuleScene, String> {
    resolve_module_event(name, inputs, theme, now_ms, None)
}

fn launcher_task(
    target: ::canvas::shell_launcher::Target,
    command: Command,
    generation: u64,
    activating: bool,
) -> Task<Message> {
    let target_id = target.id.clone();
    let (send, receive) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = send.send(::canvas::shell_launcher::request(&target, command));
    });
    Task::perform(
        async move {
            receive
                .await
                .unwrap_or_else(|_| Err("Launcher worker stopped".into()))
        },
        move |result| Message::LauncherReply {
            generation,
            target_id: target_id.clone(),
            activating,
            result,
        },
    )
}

#[cfg(feature = "lua-scenes")]
fn resolve_module_event(
    name: &str,
    inputs: &BTreeMap<String, canvas_widgets::WidgetInput>,
    theme: ::canvas::lua_module::ModuleThemeTokens,
    now_ms: u64,
    event: Option<::canvas::lua_module::ModuleEvent>,
) -> std::result::Result<::canvas::lua_module::RetainedModuleScene, String> {
    use ::canvas::lua_module::{LuaModule, ModuleInputSnapshot, ModuleManifest};
    use ::canvas::lua_scene::{LuaSceneCapabilities, LuaSceneLimits};

    let limit = ::canvas::stream_binding::MAX_ADAPTER_BYTES as u64;
    let read = |file: &str| -> std::result::Result<Option<String>, String> {
        let relative = PathBuf::from("modules").join(file);
        ::canvas::runtime_config::read(&relative, limit)
            .map(|bytes| bytes.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()))
            .map_err(|error| error.to_string())
    };
    let manifest_name = format!("{name}.json");
    let Some(manifest_text) = read(&manifest_name)? else {
        return Err(format!(
            "no manifest at modules/{name}.json in the Canvas runtime directory"
        ));
    };
    let manifest = ModuleManifest::parse(&manifest_text).map_err(|error| error.to_string())?;
    let Some(source) = read(&format!("{name}.lua"))? else {
        return Err(format!(
            "no source at modules/{name}.lua in the Canvas runtime directory"
        ));
    };
    let snapshot = ModuleInputSnapshot {
        revision: 1,
        now_ms,
        values: inputs
            .iter()
            .map(|(id, value)| (id.clone(), module_input(value)))
            .collect(),
    };
    // Assets and host resources are not granted to document-authored modules yet:
    // a surface reference has to come from a trusted caller, and a spec written
    // by an agent is not one. A module that asks for one fails loudly here
    // rather than drawing a blank where a video should be.
    let mut module = LuaModule::load(
        manifest,
        source,
        snapshot.clone(),
        theme,
        BTreeMap::new(),
        LuaSceneCapabilities::default(),
        LuaSceneLimits::default(),
    )
    .map_err(|error| error.to_string())?;
    if let Some(event) = event {
        let snapshot = ModuleInputSnapshot {
            revision: 2,
            ..snapshot
        };
        module
            .update(snapshot, theme, event)
            .map_err(|error| error.to_string())?;
    }
    Ok(module.retained().clone())
}

/// One declared input, in the vocabulary the module host accepts.
#[cfg(feature = "lua-scenes")]
fn module_input(value: &canvas_widgets::WidgetInput) -> ::canvas::lua_module::ModuleInputValue {
    use ::canvas::lua_module::{ModuleInputValue, SeriesPoint, StatusLevel};
    match value {
        canvas_widgets::WidgetInput::Number { value } => ModuleInputValue::Number(*value),
        canvas_widgets::WidgetInput::Text { value } => ModuleInputValue::Text(value.clone()),
        canvas_widgets::WidgetInput::Boolean { value } => ModuleInputValue::Boolean(*value),
        canvas_widgets::WidgetInput::Series { points } => ModuleInputValue::Series(
            points
                .iter()
                .map(|point| SeriesPoint {
                    x: point.x,
                    y: point.y,
                })
                .collect(),
        ),
        canvas_widgets::WidgetInput::Status { level, label } => ModuleInputValue::Status {
            level: match level {
                canvas_widgets::StatusLevel::Neutral => StatusLevel::Neutral,
                canvas_widgets::StatusLevel::Good => StatusLevel::Good,
                canvas_widgets::StatusLevel::Warning => StatusLevel::Warning,
                canvas_widgets::StatusLevel::Critical => StatusLevel::Critical,
            },
            label: label.clone(),
        },
    }
}

fn scene_ink(ink: Option<SemanticInk>, palette: &ThemePalette) -> [u8; 3] {
    match ink.unwrap_or(SemanticInk::Text) {
        SemanticInk::Text => palette.text,
        SemanticInk::Surface => palette.surface,
        SemanticInk::Background => palette.background,
        SemanticInk::Muted => palette.text_muted,
        SemanticInk::Primary => palette.primary,
        SemanticInk::Success => palette.success,
        SemanticInk::Warning => palette.warning,
        SemanticInk::Danger => palette.danger,
        SemanticInk::Info => palette.info,
    }
}

fn scene_node_position(translation: [f32; 3], viewport: LogicalSize) -> [f32; 2] {
    let width = viewport.width.max(1.0) as f32;
    let height = viewport.height.max(1.0) as f32;
    let x_fraction = (translation[0] + 4.0) / 8.0;
    let y_fraction = 1.0 - (translation[1] + 4.0) / 8.0;
    [
        (x_fraction * width).max(0.0),
        (y_fraction * height).max(0.0),
    ]
}

/// Z-based scale factor: nodes at higher Z (closer to the camera) appear
/// larger, giving a basic perspective effect on what is otherwise a 2D
/// projection. Range: 0.5× at Z=-4 (far) to 2× at Z=+4 (near).
fn scene_node_scale(translation_z: f32) -> f32 {
    let clamped = translation_z.clamp(-4.0, 4.0);
    0.5 + ((clamped + 4.0) / 8.0) * 1.5
}

fn scene_text_size(scale_y: f32, body: f32, depth: f32, minimum: f32) -> f32 {
    (scale_y.abs() * body * depth).max(minimum)
}

/// Extract the 2D rotation angle (radians) from a quaternion.
/// Only the Z-axis component is meaningful for flat 2D text rendering;
/// other rotations are noted but not applied.
#[allow(dead_code, unused_variables)]
fn scene_node_rotation(rotation: [f32; 4]) -> f32 {
    let [x, y, z, w] = rotation;
    let norm = (x * x + y * y + z * z + w * w).sqrt().max(f32::EPSILON);
    let (x, y, z, w) = (x / norm, y / norm, z / norm, w / norm);
    // Rotation around Z: angle = 2 * atan2(z, w)
    2.0 * z.atan2(w)
}

/// Build an effective transform for a node by accumulating parent transforms.
fn scene_node_effective(
    node_index: usize,
    nodes: &[SceneNode],
    parents: &[usize],
) -> ([f32; 3], [f32; 4], [f32; 3]) {
    let node = &nodes[node_index];
    let mut translation = node.transform.translation;
    let mut rotation = node.transform.rotation;
    let mut scale = node.transform.scale;
    // Apply parent transforms in order: root → … → parent → self
    for &parent_idx in parents {
        let parent = &nodes[parent_idx];
        // Accumulate translation
        translation[0] += parent.transform.translation[0];
        translation[1] += parent.transform.translation[1];
        translation[2] += parent.transform.translation[2];
        // Combine scales (component-wise)
        scale[0] *= parent.transform.scale[0];
        scale[1] *= parent.transform.scale[1];
        scale[2] *= parent.transform.scale[2];
        // Quaternion multiplication: parent_rot * self_rot
        let (px, py, pz, pw) = (
            parent.transform.rotation[0],
            parent.transform.rotation[1],
            parent.transform.rotation[2],
            parent.transform.rotation[3],
        );
        let (rx, ry, rz, rw) = (rotation[0], rotation[1], rotation[2], rotation[3]);
        rotation[0] = pw * rx + px * rw + py * rz - pz * ry;
        rotation[1] = pw * ry - px * rz + py * rw + pz * rx;
        rotation[2] = pw * rz + px * ry - py * rx + pz * rw;
        rotation[3] = pw * rw - px * rx - py * ry - pz * rz;
    }
    let norm = (rotation[0] * rotation[0]
        + rotation[1] * rotation[1]
        + rotation[2] * rotation[2]
        + rotation[3] * rotation[3])
        .sqrt()
        .max(f32::EPSILON);
    rotation[0] /= norm;
    rotation[1] /= norm;
    rotation[2] /= norm;
    rotation[3] /= norm;
    (translation, rotation, scale)
}

/// Build the parent chain for a node (root first).
fn scene_parent_chain(node_index: usize, nodes: &[SceneNode]) -> Vec<usize> {
    let mut chain = Vec::new();
    let mut current = node_index;
    while let Some(parent_id) = nodes[current].parent_id.as_ref() {
        let parent_idx = nodes.iter().position(|n| &n.id == parent_id);
        match parent_idx {
            Some(idx) => {
                chain.insert(0, idx);
                current = idx;
            }
            None => break,
        }
    }
    chain
}

fn apply_plan_diagnostics(
    snapshot: &mut SurfaceSnapshot,
    plan: &MediaPlan,
    rendered_pixels: [u32; 2],
) {
    snapshot.rendered_pixels = Some(rendered_pixels);
    snapshot.display_logical = Some([plan.display_logical.width, plan.display_logical.height]);
    snapshot.scale_factor = Some(plan.scale_factor);
}

fn load_content(
    surface_id: String,
    path: PathBuf,
    title: Option<String>,
    viewport: LogicalSize,
    scale_factor: f64,
) -> Result<LoadedContent, Failure> {
    let path_str = path.to_string_lossy();
    if path_str.starts_with("tty://") {
        let shell = path_str.strip_prefix("tty://").unwrap_or("sh");
        return load_terminal(surface_id, shell, title);
    }
    let path = canonical_regular_file(&path)?;
    let metadata = fs::metadata(&path).map_err(|error| Failure::invalid(error.to_string()))?;
    if metadata.len() == 0 || metadata.len() > MAX_FILE_BYTES {
        return Err(Failure::resource(format!(
            "file must contain 1–{MAX_FILE_BYTES} bytes"
        )));
    }
    let sha256 = hash_file(&path)?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let display_title = title.unwrap_or_else(|| {
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Untitled")
            .to_owned()
    });
    if extension == "md" || extension == "markdown" {
        let bytes = fs::read(&path).map_err(|error| Failure::invalid(error.to_string()))?;
        let content = String::from_utf8(bytes)
            .map_err(|error| Failure::invalid(format!("Markdown must be valid UTF-8: {error}")))?;
        let items: Vec<_> = markdown::parse(&content).collect();
        let snapshot = SurfaceSnapshot {
            id: surface_id,
            title: display_title,
            content: ContentKind::Markdown,
            asset_sha256: Some(sha256.clone()),
            page_index: None,
            page_count: None,
            source_pixels: None,
            rendered_pixels: None,
            display_logical: None,
            scale_factor: None,
        };
        let media_plan = plan_raster(
            sha256.clone(),
            PixelSize {
                width: 1,
                height: 1,
            },
            viewport,
            scale_factor,
            MediaLimits::default(),
        )
        .map_err(|error| Failure::resource(error.to_string()))?;
        return Ok(LoadedContent {
            snapshot,
            source: ContentSource::Markdown { path, items },
            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
            asset: AssetRef {
                sha256,
                media_type: "text/markdown".into(),
                byte_length: metadata.len(),
            },
            source_pixels: [1, 1],
            media_plan,
            upload_pixels: [1, 1],
        });
    }
    if extension == "mmd" || extension == "mermaid" {
        let rendered = render_mermaid(&path, &surface_id)?;
        return load_content(
            surface_id,
            rendered,
            Some(display_title.clone()),
            viewport,
            scale_factor,
        );
    }
    if extension == "pdf" {
        let page_count = pdf_page_count(&path)?;
        let (handle, source_pixels) = render_pdf_page(&path, 0)?;
        let media_plan = plan_pdf_page(
            sha256.clone(),
            0,
            LogicalSize {
                width: f64::from(source_pixels[0]),
                height: f64::from(source_pixels[1]),
            },
            viewport,
            scale_factor,
            MediaLimits::default(),
        )
        .map_err(|error| Failure::resource(error.to_string()))?;
        let mut snapshot = SurfaceSnapshot {
            id: surface_id,
            title: display_title,
            content: ContentKind::Pdf,
            asset_sha256: Some(sha256.clone()),
            page_index: Some(0),
            page_count: Some(page_count),
            source_pixels: Some(source_pixels),
            rendered_pixels: None,
            display_logical: None,
            scale_factor: None,
        };
        apply_plan_diagnostics(&mut snapshot, &media_plan, source_pixels);
        return Ok(LoadedContent {
            snapshot,
            source: ContentSource::Pdf {
                path,
                page_index: 0,
                page_count,
            },
            handle,
            source_pixels,
            media_plan,
            upload_pixels: source_pixels,
            asset: AssetRef {
                sha256,
                media_type: "application/pdf".into(),
                byte_length: metadata.len(),
            },
        });
    }
    if metadata.len() > MAX_IMAGE_BYTES {
        return Err(Failure::resource(format!(
            "image file exceeds {MAX_IMAGE_BYTES} bytes"
        )));
    }
    let (handle, media_type, source_pixels, media_plan) =
        prepare_raster(&path, sha256.clone(), viewport, scale_factor)?;
    let mut snapshot = SurfaceSnapshot {
        id: surface_id,
        title: display_title,
        content: ContentKind::Image,
        asset_sha256: Some(sha256.clone()),
        page_index: None,
        page_count: None,
        source_pixels: Some(source_pixels),
        rendered_pixels: None,
        display_logical: None,
        scale_factor: None,
    };
    let upload_pixels = [media_plan.target_px.width, media_plan.target_px.height];
    apply_plan_diagnostics(&mut snapshot, &media_plan, upload_pixels);
    Ok(LoadedContent {
        snapshot,
        source: ContentSource::Image { _path: path },
        handle,
        source_pixels,
        media_plan,
        upload_pixels,
        asset: AssetRef {
            sha256,
            media_type,
            byte_length: metadata.len(),
        },
    })
}

fn shared_view_for_loaded(loaded: &LoadedContent) -> Option<SharedViewDocument> {
    let (contract, navigation) = match loaded.snapshot.content {
        ContentKind::Image => ("image.rgba.v1", ViewNavigation::None),
        ContentKind::Pdf => (
            "document.pdf.v1",
            ViewNavigation::Page {
                index: loaded.snapshot.page_index.unwrap_or(0),
            },
        ),
        _ => return None,
    };
    let source_id = format!("asset-{}", loaded.asset.sha256);
    let view = SharedViewDocument {
        layout_policy: Default::default(),
        schema_version: SCHEMA_VERSION,
        id: loaded.snapshot.id.clone(),
        revision: 1,
        sources: BTreeMap::from([(
            "content".into(),
            ViewSource {
                source: VisualSourceRef {
                    owner: "dock".into(),
                    id: source_id,
                    contract: contract.into(),
                    kind: VisualSourceKind::Artifact,
                },
                position: SourcePosition::Revision { revision: 1 },
            },
        )]),
        viewport: ViewRect::default(),
        attention: None,
        navigation,
        parameters: BTreeMap::new(),
        // An opened artifact is the whole view; widgets are placed onto it later.
        widgets: BTreeMap::new(),
    };
    view.validate()
        .expect("loaded image/PDF must produce a valid shared view");
    Some(view)
}

fn load_background(path: PathBuf) -> Result<LoadedBackground, Failure> {
    let path = canonical_regular_file(&path)?;
    let metadata = fs::metadata(&path).map_err(|error| Failure::invalid(error.to_string()))?;
    if metadata.len() == 0 || metadata.len() > MAX_IMAGE_BYTES {
        return Err(Failure::resource(format!(
            "background image must contain 1 to {MAX_IMAGE_BYTES} bytes"
        )));
    }
    let sha256 = hash_file(&path)?;
    let (handle, media_type, _) = decode_image(&path)?;
    Ok(LoadedBackground {
        asset: AssetRef {
            sha256,
            media_type,
            byte_length: metadata.len(),
        },
        handle,
    })
}

fn canonical_regular_file(path: &Path) -> Result<PathBuf, Failure> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| Failure::invalid(format!("cannot inspect {}: {error}", path.display())))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(Failure::invalid(
            "Canvas content must be a regular non-symlink file",
        ));
    }
    fs::canonicalize(path).map_err(|error| Failure::invalid(error.to_string()))
}

fn hash_file(path: &Path) -> Result<String, Failure> {
    let mut file = File::open(path).map_err(|error| Failure::invalid(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| Failure::invalid(error.to_string()))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn decode_rgba(path: &Path) -> Result<(image::RgbaImage, String), Failure> {
    let file = File::open(path).map_err(|error| Failure::invalid(error.to_string()))?;
    let mut reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|error| Failure::unsupported(error.to_string()))?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    let format = reader
        .format()
        .ok_or_else(|| Failure::unsupported("unsupported image format"))?;
    let media_type = match format {
        image::ImageFormat::Png => "image/png",
        image::ImageFormat::Jpeg => "image/jpeg",
        image::ImageFormat::WebP => "image/webp",
        _ => {
            return Err(Failure::unsupported(
                "Canvas supports PNG, JPEG, WebP, and PDF",
            ));
        }
    };
    let decoded = reader
        .decode()
        .map_err(|error| Failure::unsupported(format!("image decode failed: {error}")))?
        .into_rgba8();
    Ok((decoded, media_type.into()))
}

fn decode_presence_preview(preview: PresencePreview) -> Result<PresencePreviewVisual, Failure> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&preview.jpeg_base64)
        .map_err(|error| Failure::invalid(format!("invalid presence preview base64: {error}")))?;
    let decoded = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
        .map_err(|error| Failure::invalid(format!("invalid presence preview JPEG: {error}")))?
        .into_rgba8();
    if decoded.width() != preview.width || decoded.height() != preview.height {
        return Err(Failure::invalid(
            "presence preview dimensions do not match its JPEG",
        ));
    }
    let handle = iced::widget::image::Handle::from_rgba(
        decoded.width(),
        decoded.height(),
        decoded.into_raw(),
    );
    Ok(PresencePreviewVisual {
        value: preview,
        handle,
        started: Instant::now(),
        last_frame: Instant::now(),
    })
}

fn decode_image(path: &Path) -> Result<(iced::widget::image::Handle, String, [u32; 2]), Failure> {
    let (decoded, media_type) = decode_rgba(path)?;
    let (width, height) = decoded.dimensions();
    Ok((
        iced::widget::image::Handle::from_rgba(width, height, decoded.into_raw()),
        media_type,
        [width, height],
    ))
}

fn prepare_raster(
    path: &Path,
    source_id: String,
    viewport: LogicalSize,
    scale_factor: f64,
) -> Result<(iced::widget::image::Handle, String, [u32; 2], MediaPlan), Failure> {
    let (decoded, media_type) = decode_rgba(path)?;
    let (source_width, source_height) = decoded.dimensions();
    let plan = plan_raster(
        source_id,
        PixelSize {
            width: source_width,
            height: source_height,
        },
        viewport,
        scale_factor,
        MediaLimits::default(),
    )
    .map_err(|error| Failure::resource(error.to_string()))?;
    let upload = if plan.target_px.width == source_width && plan.target_px.height == source_height {
        decoded
    } else {
        image::imageops::resize(
            &decoded,
            plan.target_px.width,
            plan.target_px.height,
            image::imageops::FilterType::Lanczos3,
        )
    };
    Ok((
        iced::widget::image::Handle::from_rgba(
            plan.target_px.width,
            plan.target_px.height,
            upload.into_raw(),
        ),
        media_type,
        [source_width, source_height],
        plan,
    ))
}

fn pdf_page_count(path: &Path) -> Result<u32, Failure> {
    let tool = pdf_tool("pdfinfo")?;
    let temp = TempRender::new()?;
    let output_path = temp.root.join("pdfinfo.txt");
    let output_file = File::create(&output_path)
        .map_err(|error| Failure::platform(format!("pdfinfo output failed: {error}")))?;
    let mut child = Process::new(tool)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(output_file))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| Failure::platform(format!("pdfinfo failed: {error}")))?;
    if !wait_for_tool(&mut child, "PDF metadata")? {
        return Err(Failure::unsupported("PDF metadata could not be read"));
    }
    let mut output = Vec::new();
    File::open(output_path)
        .map_err(|error| Failure::platform(error.to_string()))?
        .take(128 * 1024 + 1)
        .read_to_end(&mut output)
        .map_err(|error| Failure::platform(error.to_string()))?;
    if output.len() > 128 * 1024 {
        return Err(Failure::resource("PDF metadata exceeds limit"));
    }
    let text = String::from_utf8_lossy(&output);
    let pages = text
        .lines()
        .find_map(|line| line.strip_prefix("Pages:"))
        .and_then(|value| value.trim().parse::<u32>().ok())
        .ok_or_else(|| Failure::unsupported("PDF page count is unavailable"))?;
    if pages == 0 || pages > MAX_PDF_PAGES {
        return Err(Failure::resource(format!(
            "PDF must contain 1–{MAX_PDF_PAGES} pages"
        )));
    }
    Ok(pages)
}

fn render_pdf_page(
    path: &Path,
    page_index: u32,
) -> Result<(iced::widget::image::Handle, [u32; 2]), Failure> {
    let tool = pdf_tool("pdftoppm")?;
    let temp = TempRender::new()?;
    let page_number = page_index + 1;
    let mut child = Process::new(tool)
        .args([
            "-f",
            &page_number.to_string(),
            "-l",
            &page_number.to_string(),
            "-singlefile",
            "-scale-to",
            "2400",
            "-png",
        ])
        .arg(path)
        .arg(temp.root.join("page"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| Failure::platform(format!("pdftoppm failed: {error}")))?;
    if !wait_for_tool(&mut child, "PDF page render")? {
        return Err(Failure::unsupported("PDF page could not be rendered"));
    }
    decode_image(&temp.root.join("page.png")).map(|(handle, _, pixels)| (handle, pixels))
}

fn wait_for_tool(child: &mut std::process::Child, operation: &str) -> Result<bool, Failure> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match child
            .try_wait()
            .map_err(|error| Failure::platform(error.to_string()))?
        {
            Some(status) => return Ok(status.success()),
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Failure::resource(format!(
                    "{operation} exceeded 15 seconds"
                )));
            }
        }
    }
}

fn pdf_tool(name: &str) -> Result<PathBuf, Failure> {
    [
        PathBuf::from("/opt/homebrew/bin").join(name),
        PathBuf::from("/usr/local/bin").join(name),
        PathBuf::from("/usr/bin").join(name),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .ok_or_else(|| Failure::unsupported(format!("{name} is required for PDF surfaces")))
}

struct TempRender {
    root: PathBuf,
}

impl TempRender {
    fn new() -> Result<Self, Failure> {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let root = std::env::temp_dir().join(format!(
            "canvas-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder
            .create(&root)
            .map_err(|error| Failure::platform(error.to_string()))?;
        Ok(Self { root })
    }
}

impl Drop for TempRender {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn set_wallpaper(
    request: WallpaperRequest,
) -> Result<canvas_core::desktop::WallpaperReceipt, Failure> {
    platform::set_wallpaper(&request).map_err(Failure::platform)?;
    Ok(canvas_core::desktop::WallpaperReceipt {
        project: request.project,
        display_id: request.display_id,
        wallpaper: request.wallpaper,
    })
}

mod platform {
    use ::canvas::display::DisplayInfo;
    use canvas_core::desktop::WallpaperRequest;
    use canvas_core::layout::Rect;

    #[cfg(target_os = "macos")]
    pub(super) fn canvas_frame(display: &canvas_control::Display) -> Rect {
        Rect {
            x: display.frame.x,
            y: display.frame.y,
            width: display.frame.width,
            height: display.frame.height,
        }
    }

    #[cfg(target_os = "macos")]
    pub(super) fn canvas_presentation_options(
        kiosk: bool,
    ) -> objc2_app_kit::NSApplicationPresentationOptions {
        use objc2_app_kit::NSApplicationPresentationOptions;

        if !kiosk {
            return NSApplicationPresentationOptions::empty();
        }
        NSApplicationPresentationOptions::NSApplicationPresentationAutoHideDock
            | NSApplicationPresentationOptions::NSApplicationPresentationAutoHideMenuBar
    }

    /// Canvas reads its own screens. Routing this through Dock Control made a
    /// second process a hard dependency of Canvas knowing where it can draw:
    /// with nothing listening on the control socket the inventory came back
    /// empty, so the display shortcuts reported every monitor as disconnected
    /// and Canvas could not be targeted at one.
    #[cfg(target_os = "macos")]
    pub fn displays() -> Result<Vec<DisplayInfo>, String> {
        Ok(canvas_control::displays::enumerate()?
            .iter()
            .map(|display| DisplayInfo {
                id: display.id.clone(),
                name: display.name.clone(),
                frame: canvas_frame(display),
                primary: display.primary,
            })
            .collect())
    }

    #[cfg(target_os = "macos")]
    pub fn display_frames() -> Result<Vec<Rect>, String> {
        Ok(displays()?
            .into_iter()
            .map(|display| display.frame)
            .collect())
    }

    #[cfg(target_os = "macos")]
    pub(super) fn canvas_collection_behavior(
        kiosk: bool,
    ) -> objc2_app_kit::NSWindowCollectionBehavior {
        use objc2_app_kit::NSWindowCollectionBehavior;

        if !kiosk {
            // A normal window belongs to the active Space and moves with it.
            return NSWindowCollectionBehavior::Default;
        }
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
    }

    #[cfg(target_os = "macos")]
    pub fn make_windows_shareable(kiosk: bool) -> Result<(), String> {
        use objc2_app_kit::{NSApplication, NSWindowSharingType};
        use objc2_foundation::MainThreadMarker;

        let marker = MainThreadMarker::new().ok_or("AppKit window setup left the main thread")?;
        let application = NSApplication::sharedApplication(marker);
        application.setPresentationOptions(canvas_presentation_options(kiosk));
        for window in application.windows().iter() {
            window.setSharingType(NSWindowSharingType::NSWindowSharingReadOnly);
            unsafe { window.setCollectionBehavior(canvas_collection_behavior(kiosk)) };
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn make_windows_shareable(_kiosk: bool) -> Result<(), String> {
        Ok(())
    }

    #[cfg(target_os = "macos")]
    // This stable API also supports older macOS deployments; the newer activate
    // selector is macOS 14+. Both calls stay inside our own GUI process.
    #[allow(deprecated)]
    pub fn activate_for_focus() {
        if let Some(marker) = objc2_foundation::MainThreadMarker::new() {
            objc2_app_kit::NSApplication::sharedApplication(marker).activateIgnoringOtherApps(true);
        }
    }

    #[cfg(not(target_os = "macos"))]
    pub fn activate_for_focus() {}

    #[cfg(not(target_os = "macos"))]
    pub fn displays() -> Result<Vec<DisplayInfo>, String> {
        Ok(Vec::new())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn display_frames() -> Result<Vec<Rect>, String> {
        Ok(Vec::new())
    }

    pub fn preferred_canvas_frame(fallback: Rect) -> Rect {
        display_frames()
            .ok()
            .and_then(|frames| {
                frames
                    .into_iter()
                    .max_by_key(|frame| i64::from(frame.x) + i64::from(frame.width))
            })
            .unwrap_or(fallback)
    }

    pub fn frame_within_displays(frame: Rect, displays: &[Rect]) -> bool {
        let Some(right) = frame.x.checked_add_unsigned(frame.width.saturating_sub(1)) else {
            return false;
        };
        let Some(bottom) = frame.y.checked_add_unsigned(frame.height.saturating_sub(1)) else {
            return false;
        };
        [
            (frame.x, frame.y),
            (right, frame.y),
            (frame.x, bottom),
            (right, bottom),
        ]
        .into_iter()
        .all(|(x, y)| {
            displays.iter().any(|display| {
                let display_right = i64::from(display.x) + i64::from(display.width);
                let display_bottom = i64::from(display.y) + i64::from(display.height);
                i64::from(x) >= i64::from(display.x)
                    && i64::from(x) < display_right
                    && i64::from(y) >= i64::from(display.y)
                    && i64::from(y) < display_bottom
            })
        })
    }

    pub fn open_url(url: &str) -> Result<(), String> {
        if url.len() > 4_096
            || url.chars().any(char::is_control)
            || !(url.starts_with("https://") || url.starts_with("http://"))
        {
            return Err("Canvas links must be bounded HTTP(S) URLs".into());
        }
        #[cfg(target_os = "macos")]
        let mut command = std::process::Command::new("/usr/bin/open");
        #[cfg(not(target_os = "macos"))]
        let mut command = std::process::Command::new("xdg-open");
        let status = command
            .arg(url)
            .status()
            .map_err(|error| error.to_string())?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("URL opener exited with {status}"))
        }
    }

    #[cfg(target_os = "macos")]
    pub fn set_wallpaper(_request: &WallpaperRequest) -> Result<(), String> {
        Err("macOS Canvas wallpaper adapter is not installed yet".into())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn set_wallpaper(_request: &WallpaperRequest) -> Result<(), String> {
        Err("Canvas wallpaper adapter is unavailable on this platform".into())
    }
}

struct SocketGuard {
    path: PathBuf,
}

impl Drop for SocketGuard {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path).is_ok_and(|metadata| {
            metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::geteuid() }
        }) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn start_server() -> Result<(PendingReceiver, SocketGuard), String> {
    unsafe {
        libc::umask(0o077);
    }
    let socket = ::canvas::control::socket_path()?;
    let directory = socket.parent().ok_or("Canvas socket has no parent")?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(directory)
        .map_err(|error| error.to_string())?;
    let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err("Canvas control directory must be owner-only mode 0700".into());
    }
    let lock = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("runtime.lock"))
        .map_err(|error| error.to_string())?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Canvas runtime is already running".into());
    }
    if let Ok(metadata) = fs::symlink_metadata(&socket) {
        if !metadata.file_type().is_socket() || metadata.uid() != unsafe { libc::geteuid() } {
            return Err("refusing to replace unsafe Canvas socket path".into());
        }
        fs::remove_file(&socket).map_err(|error| error.to_string())?;
    }
    let listener = UnixListener::bind(&socket).map_err(|error| error.to_string())?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .map_err(|error| error.to_string())?;
    let (sender, receiver) = mpsc::sync_channel::<Pending>(64);
    std::thread::Builder::new()
        .name("canvas-control".into())
        .spawn(move || serve(listener, sender, lock))
        .map_err(|error| error.to_string())?;
    Ok((Arc::new(Mutex::new(receiver)), SocketGuard { path: socket }))
}

fn serve(listener: UnixListener, sender: SyncSender<Pending>, _lock: File) {
    for accepted in listener.incoming() {
        let mut stream = match accepted {
            Ok(stream) => stream,
            Err(error) => {
                eprintln!("Canvas control accept failed: {error}");
                continue;
            }
        };
        if let Err(error) = canvas_control::verify_peer(&stream) {
            eprintln!("Canvas rejected peer: {error}");
            continue;
        }
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
        let bytes = match ::canvas::control::read_frame(&stream) {
            Ok(bytes) => bytes,
            Err(message) => {
                write_error_reply(&mut stream, false, ErrorCode::InvalidRequest, &message);
                continue;
            }
        };
        let trimmed = std::str::from_utf8(&bytes).unwrap_or("").trim().to_string();
        let is_text = !trimmed.starts_with('{');
        let request = if is_text {
            ::canvas::control::text::parse_line(&trimmed)
        } else {
            serde_json::from_str::<ControlRequest>(&trimmed).map_err(|e| e.to_string())
        };
        let request = match request {
            Ok(request) => request,
            Err(message) => {
                write_error_reply(&mut stream, is_text, ErrorCode::InvalidRequest, &message);
                continue;
            }
        };
        let (reply_sender, reply_receiver) = mpsc::sync_channel(1);
        if sender
            .send(Pending {
                request,
                reply: reply_sender,
            })
            .is_err()
        {
            break;
        }
        match reply_receiver.recv_timeout(Duration::from_secs(30)) {
            Ok(reply) => {
                if is_text {
                    write_text_reply(&mut stream, &reply);
                } else {
                    write_reply(&mut stream, &reply);
                }
            }
            Err(_) => {
                write_error_reply(
                    &mut stream,
                    is_text,
                    ErrorCode::Internal,
                    "Canvas command timed out",
                );
            }
        }
    }
}

fn write_text_reply(stream: &mut impl Write, reply: &ControlReply) {
    let text = ::canvas::control::text::format_reply(reply);
    let result = {
        let mut bytes = text.into_bytes();
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_CONTROL_MESSAGE {
            Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Canvas text reply exceeds protocol limit",
            ))
        } else {
            stream.write_all(&bytes)
        }
    };
    if let Err(error) = result {
        eprintln!("Canvas control text reply failed: {error}");
    }
}

fn write_error_reply(stream: &mut impl Write, is_text: bool, code: ErrorCode, message: &str) {
    let reply = ControlReply {
        schema_version: CONTROL_VERSION,
        request_id: ::canvas::control::UNCORRELATED_REQUEST_ID.into(),
        revision: 0,
        outcome: Outcome::Error {
            code,
            message: message.to_string(),
        },
    };
    if is_text {
        write_text_reply(stream, &reply);
    } else {
        write_reply(stream, &reply);
    }
}

fn write_reply(stream: &mut impl Write, reply: &ControlReply) {
    let result = serde_json::to_vec(reply).and_then(|mut bytes| {
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_CONTROL_MESSAGE {
            return Err(serde_json::Error::io(std::io::Error::other(
                "Canvas reply exceeds protocol limit",
            )));
        }
        stream.write_all(&bytes).map_err(serde_json::Error::io)
    });
    if let Err(error) = result {
        eprintln!("Canvas control reply failed: {error}");
    }
}

fn daemon_view(app: &App, window: iced::window::Id) -> Element<'_, Message> {
    app.view(window)
}

fn daemon_theme(app: &App, _window: iced::window::Id) -> Theme {
    app.theme()
}

/// Host window geometry for one display.
///
/// The default Canvas desktop is a normal decorated window that the compositor
/// maximizes, so window management stays native: drag, resize, and title-bar
/// double-click zoom all behave like any other app. Only an explicit kiosk
/// launch (`CANVAS_KIOSK=1`) drops the chrome and takes the whole output.
fn canvas_window_settings(frame: Rect, kiosk: bool) -> iced::window::Settings {
    iced::window::Settings {
        size: Size::new(frame.width as f32, frame.height as f32),
        position: iced::window::Position::Specific(Point::new(frame.x as f32, frame.y as f32)),
        maximized: !kiosk,
        fullscreen: kiosk,
        decorations: !kiosk,
        exit_on_close_request: false,
        ..iced::window::Settings::default()
    }
}

/// The native project switcher is a transparent presentation overlay, not a
/// desktop window: it is always borderless and never maximized.
fn switcher_window_settings(frame: Rect) -> iced::window::Settings {
    iced::window::Settings {
        decorations: false,
        maximized: false,
        ..canvas_window_settings(frame, false)
    }
}

fn canvas_kiosk_requested() -> bool {
    kiosk_from_env(std::env::var(KIOSK_ENV).ok().as_deref())
}

fn kiosk_from_env(value: Option<&str>) -> bool {
    value.is_some_and(|value| value.trim() == "1")
}

/// Report whether a press completes a double-click at the same spot.
fn double_click(
    previous: Option<(Instant, LogicalPoint)>,
    now: Instant,
    position: LogicalPoint,
) -> bool {
    previous.is_some_and(|(at, at_position)| {
        now.saturating_duration_since(at) <= DOUBLE_CLICK_INTERVAL
            && (at_position.x - position.x).abs() <= DOUBLE_CLICK_SLOP
            && (at_position.y - position.y).abs() <= DOUBLE_CLICK_SLOP
    })
}

fn render_cadence(
    screensaver_enabled: bool,
    background_animated: bool,
    picker_open: bool,
    transient_animated: bool,
    screensaver_interval: Duration,
    background_interval: Duration,
    idle_interval: Duration,
) -> Duration {
    if screensaver_enabled || picker_open {
        screensaver_interval
    } else if background_animated || transient_animated {
        background_interval
    } else {
        idle_interval
    }
}

fn background_redraw_interval() -> Duration {
    Duration::from_secs_f64(
        1.0 / f64::from(
            canvas_core::appearance::current()
                .motion
                .background_max_fps
                .max(1),
        ),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if matches!(
        std::env::args().nth(1).as_deref(),
        Some("--version" | "_self-check")
    ) {
        println!("canvas {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if std::env::args_os().len() != 1 {
        return Err("canvas takes no commands; control it through its owner-local socket".into());
    }
    // Before anything renders, so the first frame is already at the configured
    // scale rather than resizing under the viewer a moment later.
    match ::canvas::runtime_config::load_appearance() {
        Ok(appearance) => {
            if appearance != canvas_core::appearance::Appearance::DEFAULT {
                eprintln!(
                    "Canvas appearance: scale {:.2}, panel opacity {:.2}, transition {}ms, idle tick {}ms",
                    appearance.density.scale,
                    appearance.panels.opacity,
                    appearance.motion.transition_ms,
                    appearance.motion.idle_tick_ms
                );
            }
        }
        // Loud, and not fatal: a bad config should not stop the wall coming up.
        Err(error) => eprintln!("Canvas appearance config ignored: {error}"),
    }
    let (receiver, socket) = start_server()?;
    let kiosk = canvas_kiosk_requested();
    let initial_frame = std::env::var("CANVAS_CAPTURE_FRAME")
        .ok()
        .and_then(|value| {
            let (width, height) = value.split_once('x')?;
            Some((width.parse().ok()?, height.parse().ok()?))
        })
        .map_or_else(
            || platform::preferred_canvas_frame(INITIAL_FRAME),
            |(width, height)| Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
        );
    let workspace_store_path = std::env::var_os("CANVAS_WORKSPACE_STORE_PATH")
        .map(PathBuf::from)
        .map_or_else(workspace_store::default_path, Ok)?;
    let boot_receiver = Arc::clone(&receiver);
    let boot_socket = Arc::new(Mutex::new(Some(socket)));
    iced::daemon(
        move || {
            let socket = boot_socket
                .lock()
                .expect("Canvas boot lock poisoned")
                .take()
                .expect("Canvas can only boot once");
            let mut app = App::new(
                Arc::clone(&boot_receiver),
                socket,
                initial_frame,
                workspace_store_path.clone(),
                kiosk,
            )
            .expect("Canvas project workspace store must remain valid");
            let (window, open) = iced::window::open(canvas_window_settings(initial_frame, kiosk));
            app.windows.insert(window, initial_frame);
            (app, open.map(Message::WindowReady))
        },
        App::update,
        daemon_view,
    )
    .subscription(|app| {
        let animated = app.visible_indicator_id.is_some()
            || app.indicator_dismissing.is_some()
            || !app.notifications.is_empty()
            || app.presence_preview.is_some()
            || app.transition.is_some()
            || app.focus_emphasis_started.is_some();
        let background_animated = app.document.background.is_animated();
        // Background-only animation uses the configured background_max_fps, not
        // a hardcoded 60 Hz: shaders like Star Nest and Smoke Rings are
        // low-frequency motion where 20-24 fps is indistinguishable from 60.
        let background_interval = background_redraw_interval();
        // An open picker needs frames whatever the workspace background is doing.
        let cadence = render_cadence(
            app.screensaver_settings.enabled,
            background_animated,
            app.background_picker.is_some(),
            animated,
            app.screensaver_clock.interval(),
            background_interval,
            Duration::from_millis(canvas_core::appearance::current().motion.idle_tick_ms as u64),
        );
        // Live camera frames deserve sub-second pickup without becoming a
        // steady redraw: the reload itself only fires when bytes change.
        let cadence = if app.has_live_frames() {
            cadence.min(Duration::from_millis(40))
        } else {
            cadence
        };
        let cadence = if let Some(widget_interval) = app.widget_animation_interval() {
            cadence.min(widget_interval)
        } else {
            cadence
        };
        Subscription::batch([
            #[cfg(all(unix, feature = "lua-scenes"))]
            if app.native_switcher.is_some() {
                iced::time::every(Duration::from_millis(40)).map(|_| Message::NativeSwitcherTick)
            } else {
                Subscription::none()
            },
            iced::time::every(cadence).map(|_| Message::Tick),
            iced::event::listen_with(|event, status, window| match event {
                iced::Event::Window(event) => Some(Message::WindowEvent(window, event)),
                iced::Event::Mouse(
                    event @ (iced::mouse::Event::ButtonPressed(_)
                    | iced::mouse::Event::WheelScrolled { .. }),
                ) => Some(Message::MouseEvent(
                    window,
                    event,
                    matches!(status, iced::event::Status::Captured),
                )),
                iced::Event::Mouse(event) => Some(Message::MouseEvent(window, event, false)),
                iced::Event::Keyboard(event) => Some(Message::TerminalInput(event)),
                _ => None,
            }),
        ])
    })
    .title(|app: &App, _| format!("Canvas · {}", app.document.project_id))
    .theme(daemon_theme)
    .antialiasing(true)
    .run()?;
    Ok(())
}

/// Wall-clock milliseconds. Local rather than `stream_binding::now_ms`, which
/// lives behind the `lua-scenes` feature; presence narrowing must not be
/// feature-gated.
fn read_theme_pack_id(project_id: &str) -> Option<String> {
    let relative = PathBuf::from("themes/selections").join(format!("{project_id}.txt"));
    let bytes = ::canvas::runtime_config::read(&relative, 48).ok()??;
    let id = std::str::from_utf8(&bytes).ok()?.trim();
    (!id.is_empty()).then(|| id.to_string())
}

fn now_unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(feature = "lua-scenes")]
/// The sequence to filter a binding's incoming events against.
///
/// A sequence counts within one producer run, so a producer that restarts
/// begins again below the mark we kept. Filtering on the old mark then wedges
/// the surface until Canvas itself restarts — the Speedy decision widgets sat
/// on their placeholder for hours because every agent restart reset the count
/// to 1. A run that has plainly gone backwards is a new run, so re-baseline.
/// Replays and duplicates within one run are still dropped.
fn effective_stream_watermark(marked: u64, newest: u64) -> u64 {
    if newest < marked { 0 } else { marked }
}

fn stream_health_transition(
    current: canvas_widgets::WidgetDataState,
    since_start: Duration,
    last_age: Option<Duration>,
) -> Option<canvas_widgets::WidgetDataState> {
    match last_age {
        Some(age)
            if age >= STREAM_STALE_AFTER
                && !matches!(
                    current,
                    canvas_widgets::WidgetDataState::Stale | canvas_widgets::WidgetDataState::Error
                ) =>
        {
            Some(canvas_widgets::WidgetDataState::Stale)
        }
        None if since_start >= STREAM_FIRST_EVENT_TIMEOUT
            && current == canvas_widgets::WidgetDataState::Loading =>
        {
            Some(canvas_widgets::WidgetDataState::Disconnected)
        }
        _ => None,
    }
}

/// Builds the presence a caller may see, rather than handing out the roster.
///
/// Canvas is not an identity authority: it renders what a publisher observed.
/// What it must not do is repeat an observation more widely, or more
/// confidently, than the observation itself supports. Two narrowings, both of
/// which only ever remove:
///
/// 1. An expired observation empties rather than being marked stale. A sensor
///    that stopped reporting cannot keep asserting who is in a room, and a
///    disconnected camera must not hold a room open indefinitely. The same
///    semantics are already proven on the JARVIS side, at
///    `jarvis-presence/src/publication.rs:96-104`, which additionally assumes
///    others are present once an observation expires.
/// 2. An occupant whose identity is not `Verified` is omitted entirely rather
///    than published as an unknown row. A detection is evidence that *someone*
///    is there; it is not evidence of *who*, and a row carrying a display name
///    on `Detected` alone is exactly the claim
///    `jarvis/distributed-topology.md` forbids: "a face observation alone must
///    not silently give an unknown speaker another person's private history".
///
/// `occupied` survives both narrowings, because whether a room is occupied is
/// what the sensor observed; only the identities are withheld.
fn assemble_presence(
    observation: Option<&::canvas::control::PresenceSnapshot>,
    now_unix_ms: u64,
) -> Option<::canvas::control::PresenceSnapshot> {
    use ::canvas::control::PresenceSignalState;
    let observation = observation?;
    let mut assembled = observation.clone();
    let age = now_unix_ms.saturating_sub(observation.observed_at_unix_ms);
    if age >= observation.expires_after_ms {
        // Expired: the room's occupancy is unknown, not empty and not held.
        assembled.occupants.clear();
        assembled.occupied = false;
        return Some(assembled);
    }
    assembled
        .occupants
        .retain(|occupant| occupant.face.state == PresenceSignalState::Verified);
    Some(assembled)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn launcher_catalog_is_revision_bound_and_rejects_unknown_choices() {
        let mut app = test_app();
        let reply = execute(&mut app, "shell-read", Command::ShellCatalog);
        let Outcome::Ok {
            receipt: Receipt::ShellCatalog { catalog },
        } = reply.outcome
        else {
            panic!("shell catalog")
        };
        assert!(!catalog.entries.is_empty());
        let reply = execute(
            &mut app,
            "shell-stale",
            Command::ShellActivate {
                catalog_revision: "0".repeat(64),
                entry_id: catalog.entries[0].id.clone(),
            },
        );
        assert!(matches!(
            reply.outcome,
            Outcome::Error {
                code: ErrorCode::Conflict,
                ..
            }
        ));
        let reply = execute(
            &mut app,
            "shell-unknown",
            Command::ShellActivate {
                catalog_revision: catalog.revision,
                entry_id: "app:/not/enrolled".into(),
            },
        );
        assert!(matches!(
            reply.outcome,
            Outcome::Error {
                code: ErrorCode::Conflict,
                ..
            }
        ));
    }
    #[test]
    fn launcher_focus_captures_input_instead_of_the_remote_surface() {
        let mut app = test_app();
        app.project_switcher = Some(::canvas::shell_launcher::Launcher::new().unwrap());
        let mut launcher = app.project_switcher.take().unwrap();
        let generation = launcher.generation;
        launcher.accept(generation, Ok(app.launcher_catalog()), now_unix_ms());
        app.project_switcher = Some(launcher);
        app.desktop_focused = false;
        let focused = app.document.focused_surface_id.clone();
        app.handle_browser_pointer("press", 0.5, 0.5, 0.0);
        assert_eq!(app.document.focused_surface_id, focused);
        assert_eq!(app.focused_hid_target(), None);
        assert!(switcher_requested(
            &browser_key_event(" ", false, true, false, false).unwrap()
        ));
    }

    #[test]
    fn shell_navigation_separates_workspaces_and_canvas_windows() {
        use ::canvas::shell_launcher::{Choice, Scope};
        let mut app = test_app();
        let original_pins = app.dock_pins.0.clone();
        let focused = app.document.focused_surface_id.clone();
        app.window_cycle.cycle(&["sample".into()], false);
        assert!(app.window_cycle.active());
        drop(app.toggle_project_switcher());
        assert!(!app.window_cycle.active());
        assert_eq!(app.document.focused_surface_id, focused);
        assert_eq!(
            app.project_switcher.as_ref().unwrap().scope,
            Scope::Workspaces
        );
        drop(app.toggle_shell_launcher(Scope::Canvas));
        let launcher = app.project_switcher.as_ref().unwrap();
        assert_eq!(launcher.scope, Scope::Canvas);
        let expected = ::canvas::shell_launcher::dock_surfaces(&app.document)
            .map(|s| s.id.clone())
            .collect::<BTreeSet<_>>();
        assert_eq!(launcher.canvas_surface_ids.as_ref(), Some(&expected));
        assert!(
            app.dock_entries()
                .iter()
                .all(|e| matches!(e.choice, Choice::Surface { .. }))
        );
        assert_eq!(app.dock_pins.0, original_pins);
    }

    #[test]
    fn desktop_takeover_disables_remote_keyboard_target() {
        let mut app = test_app();
        app.document.surfaces.push(Surface {
            id: "hid-test".into(),
            title: "Input test".into(),
            content: SurfaceContent::LiveVideo {
                source_id: "hid-test".into(),
                width: 1280,
                height: 720,
                orbitable: false,
                timeline: false,
            },
            inputs: vec![InputPort {
                id: "guest".into(),
                contract: "input.desktop_state.v1".into(),
            }],
        });
        app.document.focused_surface_id = Some("hid-test".into());
        app.desktop_focused = false;
        assert_eq!(app.focused_hid_target().as_deref(), Some("guest"));
        app.desktop_focused = true;
        assert_eq!(app.focused_hid_target(), None);
    }

    #[test]
    fn visibility_includes_surface_layout() {
        let layout = Layout::Surface {
            surface_id: "qemu".into(),
        };
        assert_eq!(placement_surface_ids(&layout), ["qemu"]);
        assert_eq!(visible_surface_ids(&layout), ["qemu"]);
    }

    #[test]
    fn visibility_keeps_only_the_active_stack_member() {
        let layout = Layout::Stack {
            surface_ids: vec!["camera".into(), "qemu".into()],
            active_surface_id: "qemu".into(),
        };
        assert_eq!(placement_surface_ids(&layout), ["camera", "qemu"]);
        assert_eq!(visible_surface_ids(&layout), ["qemu"]);
    }

    #[test]
    fn browser_keys_preserve_named_keys_text_and_modifiers() {
        use iced::keyboard::{Event, Key, Modifiers, key};

        let Event::KeyPressed { key, text, .. } =
            browser_key_event("ArrowDown", false, false, false, false).unwrap()
        else {
            panic!("browser key must produce a key-pressed event");
        };
        assert_eq!(key, Key::Named(key::Named::ArrowDown));
        assert!(text.is_none());

        let Event::KeyPressed {
            key,
            text,
            modifiers,
            ..
        } = browser_key_event("k", true, true, false, false).unwrap()
        else {
            panic!("browser character must produce a key-pressed event");
        };
        assert_eq!(key, Key::Character("k".into()));
        assert_eq!(text.as_deref(), Some("k"));
        assert!(modifiers.contains(Modifiers::SHIFT));
        assert!(modifiers.contains(Modifiers::CTRL));
    }

    #[cfg(feature = "lua-scenes")]
    #[test]
    /// The Speedy failure: an agent restarts, its decision counter returns to
    /// 1, and the binding's mark is far above that. Filtering on the stale mark
    /// discards every later decision, so the widget keeps its placeholder until
    /// Canvas restarts.
    fn a_restarted_producer_rebaselines_instead_of_being_ignored() {
        assert_eq!(effective_stream_watermark(5000, 1), 0);
    }

    #[cfg(feature = "lua-scenes")]
    #[test]
    fn a_running_producer_keeps_its_mark() {
        assert_eq!(effective_stream_watermark(41, 42), 41);
    }

    /// A duplicate or replay inside one run is not a restart.
    #[cfg(feature = "lua-scenes")]
    #[test]
    fn an_equal_sequence_is_not_treated_as_a_restart() {
        assert_eq!(effective_stream_watermark(42, 42), 42);
    }

    #[cfg(feature = "lua-scenes")]
    #[test]
    fn a_first_event_starts_from_zero() {
        assert_eq!(effective_stream_watermark(0, 1), 0);
    }

    #[test]
    fn stream_health_names_missing_first_events_and_preserves_errors() {
        use canvas_widgets::WidgetDataState;
        assert_eq!(
            stream_health_transition(WidgetDataState::Loading, STREAM_FIRST_EVENT_TIMEOUT, None),
            Some(WidgetDataState::Disconnected)
        );
        assert_eq!(
            stream_health_transition(
                WidgetDataState::Ready,
                Duration::ZERO,
                Some(STREAM_STALE_AFTER)
            ),
            Some(WidgetDataState::Stale)
        );
        assert_eq!(
            stream_health_transition(
                WidgetDataState::Error,
                Duration::from_secs(600),
                Some(Duration::from_secs(600))
            ),
            None
        );
    }
    use ::canvas::{BackgroundShader, control::MeshNode};

    fn test_shared_view(id: &str, revision: u64) -> SharedViewDocument {
        SharedViewDocument {
            layout_policy: Default::default(),
            schema_version: SCHEMA_VERSION,
            id: id.into(),
            revision,
            sources: BTreeMap::from([(
                "content".into(),
                ViewSource {
                    source: VisualSourceRef {
                        owner: "dock".into(),
                        id: "asset-ab".into(),
                        contract: "image.rgba.v1".into(),
                        kind: VisualSourceKind::Artifact,
                    },
                    position: SourcePosition::Revision { revision: 1 },
                },
            )]),
            viewport: ViewRect::default(),
            attention: None,
            navigation: ViewNavigation::None,
            parameters: BTreeMap::new(),
            widgets: BTreeMap::new(),
        }
    }

    fn test_app() -> App {
        static NEXT_STORE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let store_id = NEXT_STORE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let (_sender, receiver) = mpsc::sync_channel(1);
        let mut app = App::new(
            Arc::new(Mutex::new(receiver)),
            SocketGuard {
                path: std::env::temp_dir()
                    .join(format!("canvas-missing-test-socket-{}", std::process::id())),
            },
            INITIAL_FRAME,
            std::env::temp_dir()
                .join(format!(
                    "canvas-test-workspaces-{}-{store_id}",
                    std::process::id(),
                ))
                .join("workspaces.json"),
            false,
        )
        .unwrap();
        app.frame = INITIAL_FRAME;
        // Geometry tests assert exact pixel<->normalized mapping; pin chrome
        // to zero so the humane edge-padding default (covered separately in
        // the lib tests) does not shift every expected coordinate by 24px.
        app.document.chrome = SurfaceChrome {
            canvas_padding: 0,
            surface_gap: 0,
            ..SurfaceChrome::default()
        };
        app.interaction = interaction_engine(
            INITIAL_FRAME,
            app.document.chrome,
            ::canvas::theme_pack::ThemeChrome::default().titlebar_height,
        );
        app.hud_visible = true;
        app
    }

    #[test]
    fn boot_restores_the_active_workspace_and_reports_every_salvage_drop() {
        // dock-8gr end to end: a store saved while Alfalfa was merged (so it
        // holds a workspace for a project the bare bundled catalog does not
        // know) plus one structurally broken document must come back as the
        // active arrangement, keep the unknown-project workspace for later,
        // and say out loud what it dropped.
        let root = std::env::temp_dir().join(format!("canvas-boot-store-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let store_path = root.join("workspaces.json");
        let base = Catalog::bundled().unwrap();
        let dock = base.projects[0].id.clone();
        let doc = |project: &str, id: &str| WorkspaceDocument {
            pages: Vec::new(),
            active_page: 0,
            schema_version: SCHEMA_VERSION,
            id: id.into(),
            project_id: project.into(),
            revision: 3,
            theme: ThemeRef::Project,
            background: ::canvas::Background::default(),
            chrome: SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: Layout::Empty,
            bindings: Vec::new(),
            focused_surface_id: None,
        };
        let mut store = workspace_store::WorkspaceStore::default();
        store.upsert(doc(&dock, "restored-wall"));
        store
            .projects
            .insert("ghost".into(), doc("ghost", "ghost-workspace"));
        store
            .projects
            .insert("broken".into(), doc("broken", "Bad Id!"));
        store.active_project_id = Some(dock.clone());
        fs::write(&store_path, serde_json::to_vec(&store).unwrap()).unwrap();
        fs::set_permissions(&store_path, fs::Permissions::from_mode(0o600)).unwrap();
        let (_sender, receiver) = mpsc::sync_channel(1);
        let app = App::new(
            Arc::new(Mutex::new(receiver)),
            SocketGuard {
                path: root.join("control.sock"),
            },
            INITIAL_FRAME,
            store_path,
            false,
        )
        .unwrap();
        assert_eq!(app.document.id, "restored-wall");
        assert_eq!(app.active_project_workspace.as_deref(), Some(dock.as_str()));
        assert!(
            app.workspaces.get("ghost").is_some(),
            "unknown-project workspaces survive for later switches"
        );
        assert!(
            app.events.iter().any(|event| event.contains("broken")),
            "drops must be visible in boot events: {:?}",
            app.events
        );
        let _ = fs::remove_dir_all(root);
    }

    fn execute(app: &mut App, request_id: &str, command: Command) -> ControlReply {
        let (reply, replies) = mpsc::sync_channel(1);
        drop(app.execute(Pending {
            request: ControlRequest {
                schema_version: CONTROL_VERSION,
                request_id: request_id.into(),
                expected_revision: None,
                actor: None,
                command,
            },
            reply,
        }));
        replies.recv().unwrap()
    }

    #[test]
    fn nested_scene_coordinates_reflow_with_surface_aspect_ratio() {
        let translation = [2.0, -2.0, 0.0];
        assert_eq!(
            scene_node_position(
                translation,
                LogicalSize {
                    width: 800.0,
                    height: 400.0,
                },
            ),
            [600.0, 300.0]
        );
        assert_eq!(
            scene_node_position(
                translation,
                LogicalSize {
                    width: 400.0,
                    height: 800.0,
                },
            ),
            [300.0, 600.0]
        );
    }

    #[test]
    fn scene_node_scale_responds_to_z_depth() {
        // Z at range extremes
        assert!((scene_node_scale(-4.0) - 0.5).abs() < 0.01);
        assert!((scene_node_scale(4.0) - 2.0).abs() < 0.01);
        // Z in the middle is 1.25
        assert!((scene_node_scale(0.0) - 1.25).abs() < 0.01);
        // Values outside the range are clamped
        assert_eq!(scene_node_scale(-100.0), scene_node_scale(-4.0));
        assert_eq!(scene_node_scale(100.0), scene_node_scale(4.0));
    }

    #[test]
    fn lua_text_size_scales_linearly_not_quadratically() {
        let base = scene_text_size(1.0, 16.0, 1.25, 8.0);
        assert_eq!(scene_text_size(2.0, 16.0, 1.25, 8.0), base * 2.0);
        assert_eq!(scene_text_size(0.01, 16.0, 1.25, 8.0), 8.0);
    }

    #[test]
    fn scene_node_rotation_extracts_z_angle() {
        use std::f32::consts::PI;
        // No rotation: identity quaternion [0,0,0,1]
        assert!(scene_node_rotation([0.0, 0.0, 0.0, 1.0]).abs() < 0.001);
        // 90 degrees around Z: quaternion [0, 0, sin(45°), cos(45°)]
        let angle = PI / 2.0;
        let half = angle / 2.0;
        let quat = [0.0, 0.0, half.sin(), half.cos()];
        let extracted = scene_node_rotation(quat);
        assert!(
            (extracted - angle).abs() < 0.001,
            "expected {angle}, got {extracted}"
        );
    }

    #[test]
    fn shared_view_crop_maps_normalized_source_coordinates_to_pixels() {
        let crop = view_crop_pixels(
            ViewRect {
                x: 0.25,
                y: 0.1,
                width: 0.5,
                height: 0.75,
            },
            [1200, 800],
        );
        assert_eq!(
            crop,
            iced::Rectangle {
                x: 300,
                y: 80,
                width: 600,
                height: 600,
            }
        );
    }

    #[test]
    fn scene_parent_chain_resolves_node_hierarchy() {
        // node_a (root) -> node_b -> node_c
        let nodes = vec![
            SceneNode {
                id: "a".into(),
                parent_id: None,
                transform: ::canvas::Transform3::default(),
                content: NodeContent::Group,
            },
            SceneNode {
                id: "b".into(),
                parent_id: Some("a".into()),
                transform: ::canvas::Transform3::default(),
                content: NodeContent::Group,
            },
            SceneNode {
                id: "c".into(),
                parent_id: Some("b".into()),
                transform: ::canvas::Transform3::default(),
                content: NodeContent::Text {
                    text: "hi".into(),
                    ink: None,
                },
            },
        ];
        let chain = scene_parent_chain(2, &nodes);
        assert_eq!(chain, vec![0, 1]); // root first, then parent
    }

    #[test]
    fn shared_attention_is_clipped_and_mapped_inside_the_visible_crop() {
        let geometry = attention_geometry(
            Size::new(600.0, 400.0),
            [1200, 800],
            ViewRect {
                x: 0.25,
                y: 0.0,
                width: 0.5,
                height: 1.0,
            },
            ViewRect {
                x: 0.1,
                y: 0.25,
                width: 0.3,
                height: 0.5,
            },
        )
        .unwrap();
        assert_eq!(
            geometry,
            Rectangle::new(Point::new(150.0, 100.0), Size::new(90.0, 200.0))
        );
        assert!(
            attention_geometry(
                Size::new(600.0, 400.0),
                [1200, 800],
                ViewRect {
                    x: 0.5,
                    y: 0.5,
                    width: 0.5,
                    height: 0.5,
                },
                ViewRect {
                    x: 0.0,
                    y: 0.0,
                    width: 0.25,
                    height: 0.25,
                },
            )
            .is_none()
        );
    }

    #[test]
    fn shared_view_patch_changes_the_visible_runtime_state_atomically() {
        let mut app = test_app();
        app.document.surfaces = vec![Surface {
            id: "drawing".into(),
            title: "Drawing".into(),
            content: SurfaceContent::Scene {
                scene: Scene { nodes: Vec::new() },
            },
            inputs: Vec::new(),
        }];
        app.document.layout = Layout::Canvas {
            placements: vec![Placement {
                surface_id: "drawing".into(),
                bounds: NormalizedRect {
                    x: 0.1,
                    y: 0.1,
                    width: 0.8,
                    height: 0.8,
                },
                layer: 1,
            }],
        };
        app.shared_views
            .insert("drawing".into(), test_shared_view("drawing", 2));

        let viewport = ViewRect {
            x: 0.25,
            y: 0.2,
            width: 0.5,
            height: 0.6,
        };
        let reply = execute(
            &mut app,
            "focus-drawing-region",
            Command::PatchView {
                surface_id: "drawing".into(),
                patch: ::canvas::view::SharedViewPatch {
                    schema_version: SCHEMA_VERSION,
                    view_id: "drawing".into(),
                    base_revision: 2,
                    revision: 3,
                    operations: vec![ViewOperation::SetViewport { viewport }],
                },
            },
        );
        assert!(matches!(
            reply.outcome,
            Outcome::Ok {
                receipt: Receipt::ViewPatched { .. }
            }
        ));
        let snapshot = app.snapshot();
        assert_eq!(snapshot.views.len(), 1);
        assert_eq!(snapshot.views[0].view.revision, 3);
        assert_eq!(snapshot.views[0].view.viewport, viewport);
        assert_eq!(snapshot.views[0].bounds, snapshot.surface_bounds["drawing"]);
    }

    #[test]
    fn screensaver_shader_selection_is_bounded_and_resets_elapsed_time() {
        let mut app = test_app();
        app.screensaver_piece_elapsed = Duration::from_secs(12);
        app.screensaver_elapsed = Duration::from_secs(12);

        let reply = execute(
            &mut app,
            "shader-valid",
            Command::ScreensaverShader {
                index: screensaver::SHADERS.len() - 1,
            },
        );
        assert!(matches!(reply.outcome, Outcome::Ok { .. }));
        assert_eq!(app.screensaver_index, screensaver::SHADERS.len() - 1);
        assert_eq!(app.screensaver_elapsed, Duration::ZERO);

        let reply = execute(
            &mut app,
            "shader-invalid",
            Command::ScreensaverShader {
                index: screensaver::SHADERS.len(),
            },
        );
        assert!(matches!(
            reply.outcome,
            Outcome::Error {
                code: ErrorCode::InvalidRequest,
                ..
            }
        ));
    }

    #[test]
    fn screensaver_shader_receipt_reports_what_is_actually_drawn() {
        let mut app = test_app();
        let reply = execute(
            &mut app,
            "shader-receipt",
            Command::ScreensaverShader { index: 0 },
        );
        let Outcome::Ok {
            receipt: Receipt::ScreensaverShaderSet { visible, .. },
        } = reply.outcome
        else {
            panic!("expected a screensaver shader receipt");
        };
        let visible = visible.expect("a mutation receipt must report visible state");
        assert_eq!(visible.project_id, app.document.project_id);
        assert_eq!(visible.active_project_id, app.document.project_id);
        assert_eq!(
            visible.screensaver_enabled,
            app.screensaver_settings.enabled
        );
        assert_eq!(visible.background, app.document.background);
        // The screensaver is off, so its shader index is not what the desk is
        // drawing — which is exactly what three identical `ok` replies failed
        // to say.
        assert_eq!(
            visible.drawn_shader.is_some(),
            matches!(app.document.background, Background::Shader { .. })
        );
    }

    #[test]
    fn background_shader_command_sets_what_the_presented_document_draws() {
        let mut app = test_app();
        let project = app.document.project_id.clone();
        let reply = execute(
            &mut app,
            "bg-shader-active",
            Command::SetBackgroundShader {
                project_id: project.clone(),
                shader_id: BackgroundShader::SmokeRings,
                speed: 0.5,
            },
        );
        let Outcome::Ok {
            receipt: Receipt::BackgroundShaderSet { visible },
        } = reply.outcome
        else {
            panic!("expected a background shader receipt");
        };
        assert_eq!(
            app.document.background,
            Background::Shader {
                shader_id: BackgroundShader::SmokeRings,
                speed: 0.5,
            }
        );
        assert!(visible.on_screen());
        assert_eq!(visible.project_id, project);
        assert_eq!(visible.background, app.document.background);
        assert_eq!(visible.drawn_shader, Some(BackgroundShader::SmokeRings));
        // The renderer follows the document rather than being aimed separately.
        assert_eq!(
            app.screensaver_index,
            screensaver::index_for(BackgroundShader::SmokeRings)
        );
    }

    #[test]
    fn background_shader_receipt_names_a_project_that_is_not_on_screen() {
        let mut app = test_app();
        let active = app.document.project_id.clone();
        app.workspaces.upsert(WorkspaceDocument {
            pages: Vec::new(),
            active_page: 0,
            schema_version: SCHEMA_VERSION,
            id: "studio-workspace".into(),
            project_id: "studio".into(),
            revision: 1,
            theme: ThemeRef::Project,
            background: Background::default(),
            chrome: SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: Layout::Empty,
            bindings: Vec::new(),
            focused_surface_id: None,
        });
        let unchanged = app.document.background.clone();

        let reply = execute(
            &mut app,
            "bg-shader-elsewhere",
            Command::SetBackgroundShader {
                project_id: "studio".into(),
                shader_id: BackgroundShader::Tidepool,
                speed: 1.0,
            },
        );
        let Outcome::Ok {
            receipt: Receipt::BackgroundShaderSet { visible },
        } = reply.outcome
        else {
            panic!("expected a background shader receipt");
        };
        // The reply that would have caught the failure on the first command
        // rather than the third: it names a project nobody was looking at.
        assert_eq!(visible.project_id, "studio");
        assert_eq!(visible.active_project_id, active);
        assert!(!visible.on_screen());
        assert_eq!(app.document.background, unchanged);
        assert_eq!(
            app.workspaces.get("studio").unwrap().background,
            Background::Shader {
                shader_id: BackgroundShader::Tidepool,
                speed: 1.0,
            }
        );

        let missing = execute(
            &mut app,
            "bg-shader-missing",
            Command::SetBackgroundShader {
                project_id: "nowhere".into(),
                shader_id: BackgroundShader::Tidepool,
                speed: 1.0,
            },
        );
        assert!(matches!(
            missing.outcome,
            Outcome::Error {
                code: ErrorCode::NotFound,
                ..
            }
        ));
    }

    #[test]
    fn background_updates_are_document_patches_and_unknown_assets_fail() {
        let mut app = test_app();
        let document_revision = app.document.revision;
        let runtime_revision = app.runtime_revision;
        app.apply_background(Background::Solid {
            color: [8, 16, 32, 255],
        })
        .unwrap();
        assert_eq!(app.document.revision, document_revision + 1);
        assert_eq!(app.runtime_revision, runtime_revision + 1);
        assert_eq!(
            app.document.background,
            Background::Solid {
                color: [8, 16, 32, 255]
            }
        );

        let before = app.document.clone();
        let error = app
            .apply_background(Background::Image {
                asset: AssetRef {
                    sha256: "ab".repeat(32),
                    media_type: "image/png".into(),
                    byte_length: 42,
                },
                fit: BackgroundFit::Cover,
            })
            .unwrap_err();
        assert!(error.message.contains("has not been imported"));
        assert_eq!(app.document, before);
    }

    #[test]
    fn named_shader_background_uses_bounded_clock_and_static_background_does_not_advance() {
        let mut app = test_app();
        let started = Instant::now();
        app.screensaver_clock = screensaver::FrameClock::new(15, started);
        app.screensaver_last_frame = Some(started);
        app.refresh_screensaver(started + Duration::from_millis(100));
        assert_eq!(app.screensaver_elapsed, Duration::ZERO);

        app.apply_background(Background::Shader {
            shader_id: BackgroundShader::AuroraDrift,
            speed: 0.5,
        })
        .unwrap();
        app.screensaver_clock = screensaver::FrameClock::new(15, started);
        app.screensaver_last_frame = Some(started);
        app.refresh_screensaver(started + Duration::from_millis(100));
        assert_eq!(
            app.screensaver_index,
            screensaver::index_for(BackgroundShader::AuroraDrift)
        );
        assert!(
            app.screensaver_elapsed >= Duration::from_millis(49)
                && app.screensaver_elapsed <= Duration::from_millis(51)
        );
    }

    #[test]
    fn background_image_fit_maps_to_iced_without_guessing() {
        assert_eq!(background_fit(BackgroundFit::Cover), ContentFit::Cover);
        assert_eq!(background_fit(BackgroundFit::Contain), ContentFit::Contain);
        assert_eq!(background_fit(BackgroundFit::Stretch), ContentFit::Fill);
        assert_eq!(background_fit(BackgroundFit::Center), ContentFit::None);
    }

    #[test]
    fn stream_matte_padding_scales_with_source_and_stays_bounded() {
        assert_eq!(stream_matte_padding([640, 480]), 6.0);
        assert_eq!(stream_matte_padding([1920, 1080]), 8.64);
        assert_eq!(stream_matte_padding([7680, 4320]), 12.0);
    }

    #[test]
    fn document_inspection_is_atomic_and_does_not_advance_revisions() {
        let mut app = test_app();
        let before = app.runtime_revision;
        let document = app.document.clone();
        let reply = execute(&mut app, "inspect-document", Command::ReadDocument);
        let Outcome::Ok {
            receipt:
                Receipt::DocumentSnapshot {
                    document: actual,
                    snapshot,
                },
        } = reply.outcome
        else {
            panic!("expected a complete document snapshot");
        };
        assert_eq!(*actual, document);
        assert_eq!(snapshot.revision, before);
        assert_eq!(reply.revision, before);
        assert_eq!(app.runtime_revision, before);
    }

    #[test]
    fn canvas_consumes_imported_palettes_through_the_canonical_theme_boundary() {
        let mut app = test_app();
        let palette = ThemePalette {
            background: [1, 2, 3],
            background_strong: [4, 5, 6],
            surface: [7, 8, 9],
            surface_muted: [10, 11, 12],
            text: [13, 14, 15],
            text_muted: [16, 17, 18],
            primary: [19, 20, 21],
            success: [22, 23, 24],
            warning: [25, 26, 27],
            danger: [28, 29, 30],
            info: [31, 32, 33],
            border: [34, 35, 36],
            border_strong: [37, 38, 39],
        };
        app.catalog.projects[0].theme = canvas_core::ThemeSelection::Palette { palette };

        assert_eq!(app.project_palette(), palette);
        assert_eq!(app.presented_palette(), palette);
    }

    #[test]
    fn stale_mcp_document_edit_preserves_the_visible_workspace() {
        let mut app = test_app();
        let document = app.document.clone();
        let (reply, replies) = mpsc::sync_channel(1);
        drop(app.execute(Pending {
            request: ControlRequest {
                schema_version: CONTROL_VERSION,
                request_id: "stale-edit".into(),
                expected_revision: Some(app.runtime_revision + 1),
                actor: None,
                command: Command::ApplyDocument {
                    document: document.clone(),
                },
            },
            reply,
        }));
        assert!(matches!(
            replies.recv().unwrap().outcome,
            Outcome::Error {
                code: ErrorCode::Conflict,
                ..
            }
        ));
        assert_eq!(app.document, document);
    }

    fn load_test_content(path: PathBuf) -> Result<LoadedContent, Failure> {
        load_content(
            "sample".into(),
            path,
            None,
            LogicalSize {
                width: 800.0,
                height: 600.0,
            },
            1.0,
        )
    }

    fn indicator(id: &str, state: ::canvas::control::IndicatorState) -> StatusIndicator {
        StatusIndicator {
            id: id.into(),
            title: "Assistant".into(),
            state,
            caption: Some("Working".into()),
            detail: None,
            diagnostic: None,
        }
    }

    #[test]
    fn voice_indicator_lease_renews_without_reveal_reset_and_expires_at_the_owner() {
        use ::canvas::control::IndicatorState;
        let mut app = test_app();
        let document = app.document.clone();
        execute(
            &mut app,
            "generic",
            Command::UpsertIndicator {
                indicator: indicator("other", IndicatorState::Thinking),
            },
        );
        let voice = canvas_workspace::observer::VOICE_INDICATOR_ID;
        execute(
            &mut app,
            "voice",
            Command::UpsertIndicator {
                indicator: indicator(voice, IndicatorState::Listening),
            },
        );
        let reveal = app.indicator_started;
        let revision = app.runtime_revision;
        execute(
            &mut app,
            "heartbeat",
            Command::UpsertIndicator {
                indicator: indicator(voice, IndicatorState::Listening),
            },
        );
        assert_eq!(app.indicator_started, reveal);
        assert_eq!(app.runtime_revision, revision);
        let seen = app.voice_indicator_seen.unwrap();
        assert!(!app.expire_voice_indicator(seen + Duration::from_millis(4999)));
        assert!(app.expire_voice_indicator(seen + Duration::from_millis(5000)));
        assert!(!app.indicators.contains_key(voice));
        assert!(app.indicators.contains_key("other"));
        assert_eq!(app.visible_indicator_id.as_deref(), Some("other"));
        assert_eq!(app.document, document);
    }

    #[test]
    fn image_decode_is_bounded_and_hashes_content() {
        let root = std::env::temp_dir().join(format!("canvas-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let path = root.join("pixel.png");
        image::RgbaImage::from_pixel(2, 2, image::Rgba([20, 40, 60, 255]))
            .save(&path)
            .unwrap();
        let loaded = load_test_content(path).unwrap();
        assert_eq!(loaded.snapshot.content, ContentKind::Image);
        assert_eq!(loaded.asset.sha256.len(), 64);
        assert_eq!(loaded.asset.media_type, "image/png");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn symlinks_and_unsupported_content_fail() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("canvas-link-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        let file = root.join("note.txt");
        fs::write(&file, "hello").unwrap();
        let link = root.join("note.png");
        symlink(&file, &link).unwrap();
        assert!(load_test_content(link).is_err());
        assert!(load_test_content(file).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn indicator_lifecycle_replaces_by_id_and_preserves_workspace() {
        use ::canvas::control::IndicatorState;

        let mut app = test_app();
        let durable_document = app.document.clone();
        let upsert = execute(
            &mut app,
            "upsert-ready",
            Command::UpsertIndicator {
                indicator: indicator("assistant-status", IndicatorState::Ready),
            },
        );
        assert!(matches!(
            upsert.outcome,
            Outcome::Ok {
                receipt: Receipt::IndicatorUpserted { ref indicator_id }
            } if indicator_id == "assistant-status"
        ));

        let first_revision = app.runtime_revision;
        execute(
            &mut app,
            "upsert-listening",
            Command::UpsertIndicator {
                indicator: indicator("assistant-status", IndicatorState::Listening),
            },
        );
        assert_eq!(app.indicators.len(), 1);
        assert_eq!(
            app.indicators["assistant-status"].state,
            IndicatorState::Listening
        );
        assert!(app.runtime_revision > first_revision);

        let replacement_revision = app.runtime_revision;
        execute(
            &mut app,
            "upsert-identical",
            Command::UpsertIndicator {
                indicator: indicator("assistant-status", IndicatorState::Listening),
            },
        );
        assert_eq!(app.runtime_revision, replacement_revision);

        let inspect = execute(&mut app, "inspect", Command::Inspect);
        let Outcome::Ok {
            receipt: Receipt::Snapshot { snapshot },
        } = inspect.outcome
        else {
            panic!("inspect did not return a snapshot");
        };
        assert_eq!(
            snapshot.active_indicators,
            vec![indicator("assistant-status", IndicatorState::Listening)]
        );
        assert_eq!(app.document, durable_document);

        let before_missing_dismiss = app.runtime_revision;
        let dismiss = execute(
            &mut app,
            "dismiss-missing",
            Command::DismissIndicator {
                indicator_id: "missing-status".into(),
            },
        );
        assert!(matches!(
            dismiss.outcome,
            Outcome::Ok {
                receipt: Receipt::IndicatorDismissed { ref indicator_id }
            } if indicator_id == "missing-status"
        ));
        assert_eq!(app.runtime_revision, before_missing_dismiss);

        execute(
            &mut app,
            "dismiss-active",
            Command::DismissIndicator {
                indicator_id: "assistant-status".into(),
            },
        );
        assert!(app.snapshot().active_indicators.is_empty());
        assert_eq!(app.document, durable_document);
    }

    #[test]
    fn presence_updates_preserve_workspace_and_are_inspectable() {
        use ::canvas::control::{
            PresenceOccupant, PresenceSignal, PresenceSignalState, PresenceSnapshot,
        };

        let mut app = test_app();
        let durable_document = app.document.clone();
        let detected = PresenceSignal {
            state: PresenceSignalState::Detected,
            confidence: Some(0.92),
            age_ms: 50,
        };
        let snapshot = PresenceSnapshot {
            zone: "bedroom".into(),
            observed_at_unix_ms: 1000,
            expires_after_ms: 5000,
            occupied: true,
            occupants: vec![PresenceOccupant {
                id: "ajmwagar".into(),
                display_name: "Avery".into(),
                presence: detected.clone(),
                face: PresenceSignal {
                    state: PresenceSignalState::Verified,
                    ..detected.clone()
                },
                voice: PresenceSignal {
                    state: PresenceSignalState::Unavailable,
                    confidence: None,
                    age_ms: 50,
                },
            }],
        };
        let reply = execute(
            &mut app,
            "presence-bedroom",
            Command::SetPresence {
                presence: snapshot.clone(),
            },
        );
        assert!(matches!(
            reply.outcome,
            Outcome::Ok {
                receipt: Receipt::PresenceSet {
                    ref zone,
                    occupant_count: 1
                }
            } if zone == "bedroom"
        ));
        // Deliberately NOT a round trip. What a caller sees is assembled from the
        // observation rather than echoed, so this asserts the assembled value.
        // The fixture is stamped at 1000ms past the epoch, which against a real
        // clock is long expired — so the correct result is a zone with nobody
        // named in it, not the roster that was published.
        let seen = app
            .snapshot()
            .presence
            .expect("an expired observation still reports its zone");
        assert_eq!(seen.zone, snapshot.zone);
        assert!(
            seen.occupants.is_empty(),
            "an expired observation names nobody"
        );
        assert!(!seen.occupied, "and stops claiming the room is occupied");
        // The publisher's own record is untouched; only disclosure narrows.
        assert_eq!(app.presence, Some(snapshot));
        assert_eq!(app.document, durable_document);
    }

    fn occupant(
        id: &str,
        face: ::canvas::control::PresenceSignalState,
    ) -> ::canvas::control::PresenceOccupant {
        use ::canvas::control::{PresenceOccupant, PresenceSignal, PresenceSignalState};
        let sig = |state| PresenceSignal {
            state,
            confidence: Some(0.9),
            age_ms: 10,
        };
        PresenceOccupant {
            id: id.into(),
            display_name: format!("Name of {id}"),
            presence: sig(PresenceSignalState::Detected),
            face: sig(face),
            voice: sig(PresenceSignalState::Unknown),
        }
    }

    fn observation(
        occupants: Vec<::canvas::control::PresenceOccupant>,
    ) -> ::canvas::control::PresenceSnapshot {
        ::canvas::control::PresenceSnapshot {
            zone: "bedroom".into(),
            observed_at_unix_ms: 1_000,
            expires_after_ms: 5_000,
            occupied: true,
            occupants,
        }
    }

    #[test]
    fn stale_presence_draws_no_name() {
        // A sensor that stopped reporting cannot keep asserting who is in a
        // room. Expiry empties rather than marking, so a disconnected camera
        // cannot hold a room open indefinitely.
        use ::canvas::control::PresenceSignalState;
        let obs = observation(vec![occupant("avery", PresenceSignalState::Verified)]);
        let fresh = assemble_presence(Some(&obs), 1_000 + 4_999).expect("still fresh");
        assert_eq!(
            fresh.occupants.len(),
            1,
            "an unexpired verified occupant is kept"
        );

        let expired =
            assemble_presence(Some(&obs), 1_000 + 5_000).expect("expiry still reports a zone");
        assert!(
            expired.occupants.is_empty(),
            "an expired observation names nobody"
        );
        assert!(
            !expired.occupied,
            "and does not keep claiming the room is occupied"
        );
    }

    #[test]
    fn unidentified_occupant_draws_no_name() {
        // Detection is evidence that someone is there; it is not evidence of
        // who. Publishing a display name on Detected alone is the claim
        // distributed-topology.md forbids.
        use ::canvas::control::PresenceSignalState;
        for unverified in [
            PresenceSignalState::Detected,
            PresenceSignalState::Unknown,
            PresenceSignalState::Unavailable,
        ] {
            let obs = observation(vec![occupant("stranger", unverified)]);
            let seen = assemble_presence(Some(&obs), 1_100).expect("assembled");
            assert!(
                seen.occupants.is_empty(),
                "{unverified:?} must not publish a name"
            );
            assert!(
                seen.occupied,
                "occupancy is what the sensor saw, and survives"
            );
        }
    }

    #[test]
    fn narrowing_only_ever_removes() {
        // Whatever else changes, assembly must never invent an occupant or
        // widen what the observation claimed.
        use ::canvas::control::PresenceSignalState;
        let obs = observation(vec![
            occupant("avery", PresenceSignalState::Verified),
            occupant("stranger", PresenceSignalState::Detected),
        ]);
        let seen = assemble_presence(Some(&obs), 1_100).expect("assembled");
        assert!(seen.occupants.len() <= obs.occupants.len());
        for shown in &seen.occupants {
            assert!(
                obs.occupants.iter().any(|o| o.id == shown.id),
                "invented an occupant"
            );
        }
        assert!(
            assemble_presence(None, 1_100).is_none(),
            "no observation means nothing to show"
        );
    }

    #[test]
    fn presence_preview_frames_refresh_without_restarting_or_reordering() {
        use ::canvas::control::{PresencePreview, PresencePreviewFace};

        fn preview(captured_at_unix_ms: u64) -> PresencePreview {
            let mut jpeg = std::io::Cursor::new(Vec::new());
            image::DynamicImage::new_rgb8(4, 3)
                .write_to(&mut jpeg, image::ImageFormat::Jpeg)
                .unwrap();
            PresencePreview {
                captured_at_unix_ms,
                width: 4,
                height: 3,
                linger_ms: 250,
                jpeg_base64: base64::engine::general_purpose::STANDARD.encode(jpeg.into_inner()),
                faces: vec![PresencePreviewFace {
                    occupant_id: Some("ajmwagar".into()),
                    display_name: Some("Avery".into()),
                    confidence: 0.92,
                    bounds: [0.2, 0.1, 0.8, 0.9],
                }],
            }
        }

        let mut app = test_app();
        execute(
            &mut app,
            "preview-first",
            Command::ShowPresencePreview {
                preview: preview(1000),
            },
        );
        let first_started = app.presence_preview.as_ref().unwrap().started;
        let first_frame = app.presence_preview.as_ref().unwrap().last_frame;
        std::thread::sleep(Duration::from_millis(2));
        execute(
            &mut app,
            "preview-next",
            Command::ShowPresencePreview {
                preview: preview(1001),
            },
        );
        let current = app.presence_preview.as_ref().unwrap();
        assert_eq!(current.started, first_started);
        assert!(current.last_frame > first_frame);
        assert_eq!(current.value.captured_at_unix_ms, 1001);

        execute(
            &mut app,
            "preview-old",
            Command::ShowPresencePreview {
                preview: preview(999),
            },
        );
        assert_eq!(
            app.presence_preview
                .as_ref()
                .unwrap()
                .value
                .captured_at_unix_ms,
            1001
        );
    }

    #[test]
    fn indicator_runtime_enforces_active_limit() {
        use ::canvas::control::IndicatorState;

        let mut app = test_app();
        for index in 0..MAX_ACTIVE_INDICATORS {
            let id = format!("indicator-{index}");
            let reply = execute(
                &mut app,
                &format!("upsert-{index}"),
                Command::UpsertIndicator {
                    indicator: indicator(&id, IndicatorState::Thinking),
                },
            );
            assert!(matches!(reply.outcome, Outcome::Ok { .. }));
        }

        let rejected = execute(
            &mut app,
            "upsert-over-limit",
            Command::UpsertIndicator {
                indicator: indicator("one-too-many", IndicatorState::Thinking),
            },
        );
        assert!(matches!(
            rejected.outcome,
            Outcome::Error {
                code: ErrorCode::ResourceLimit,
                ..
            }
        ));
        assert_eq!(
            app.snapshot().active_indicators.len(),
            MAX_ACTIVE_INDICATORS
        );
    }

    fn mesh_node(id: &str, label: &str, state: ::canvas::control::MeshNodeState) -> MeshNode {
        MeshNode {
            id: id.into(),
            label: label.into(),
            state,
            detail: None,
        }
    }

    #[test]
    fn set_mesh_replaces_snapshot_and_is_idempotent() {
        use ::canvas::control::MeshNodeState;

        let mut app = test_app();
        let mesh = MeshSnapshot {
            nodes: vec![
                mesh_node("agora-one", "Agora One", MeshNodeState::Active),
                mesh_node("mac", "Mac", MeshNodeState::Paused),
            ],
        };
        let reply = execute(&mut app, "mesh-1", Command::SetMesh { mesh: mesh.clone() });
        assert!(matches!(
            reply.outcome,
            Outcome::Ok {
                receipt: Receipt::MeshSet { node_count: 2 }
            }
        ));
        assert_eq!(app.mesh.as_ref(), Some(&mesh));
        let first_revision = app.runtime_revision;

        execute(&mut app, "mesh-2", Command::SetMesh { mesh: mesh.clone() });
        assert_eq!(app.runtime_revision, first_revision);

        let mut updated = mesh.clone();
        updated.nodes[1].state = MeshNodeState::Stopped;
        execute(
            &mut app,
            "mesh-3",
            Command::SetMesh {
                mesh: updated.clone(),
            },
        );
        assert!(app.runtime_revision > first_revision);
        assert_eq!(app.snapshot().mesh.as_ref(), Some(&updated));

        let cleared = MeshSnapshot { nodes: Vec::new() };
        execute(
            &mut app,
            "mesh-4",
            Command::SetMesh {
                mesh: cleared.clone(),
            },
        );
        assert_eq!(app.snapshot().mesh.as_ref(), Some(&cleared));
    }

    #[test]
    fn local_metrics_refresh_reports_bounded_values() {
        let mut app = test_app();
        app.refresh_metrics();
        assert!(app.metrics.cpu_percent >= 0.0 && app.metrics.cpu_percent <= 100.0);
        assert!(
            app.metrics
                .cpu_cores
                .iter()
                .all(|core| (0.0..=100.0).contains(core))
        );
        assert!(app.metrics.ram_used <= app.metrics.ram_total);
        assert!(app.metrics.disk_used <= app.metrics.disk_total);
        assert!(app.metrics.top_processes.len() <= hud::MAX_HUD_PROCESSES);
        assert!(!app.metrics.name.trim().is_empty());
        assert!(app.metrics.uptime > Duration::ZERO);
    }

    #[test]
    fn display_bounds_accept_connected_spans_and_reject_gaps() {
        let displays = [
            Rect {
                x: 0,
                y: 0,
                width: 1920,
                height: 1200,
            },
            Rect {
                x: 1920,
                y: 0,
                width: 1920,
                height: 1200,
            },
        ];
        assert!(platform::frame_within_displays(
            Rect {
                x: 1200,
                y: 100,
                width: 1600,
                height: 900,
            },
            &displays,
        ));
        assert!(!platform::frame_within_displays(
            Rect {
                x: 3800,
                y: 100,
                width: 400,
                height: 900,
            },
            &displays,
        ));
    }

    #[test]
    fn display_target_opens_one_host_window_per_display_over_one_shared_frame() {
        let mut app = test_app();
        app.displays = (0..3)
            .map(|index| DisplayInfo {
                id: format!("display-{index}"),
                name: format!("Display {index}"),
                frame: Rect {
                    x: index * 1_920,
                    y: 0,
                    width: 1_920,
                    height: 1_200,
                },
                primary: index == 0,
            })
            .collect();
        app.display_frames = app.displays.iter().map(|display| display.frame).collect();
        app.frame = Rect {
            x: 0,
            y: 0,
            width: 5_760,
            height: 1_200,
        };

        let reply = execute(
            &mut app,
            "span-all",
            Command::SetDisplayTarget {
                target: ::canvas::display::DisplayTarget::All,
            },
        );
        assert!(matches!(
            reply.outcome,
            Outcome::Ok {
                receipt: Receipt::DisplayTargetSet { .. }
            }
        ));
        assert_eq!(app.windows.len(), 3);
        assert_eq!(
            app.frame,
            Rect {
                x: 0,
                y: 0,
                width: 5_760,
                height: 1_200,
            }
        );
        assert_eq!(
            app.snapshot().selected_display_ids,
            ["display-0", "display-1", "display-2"]
        );

        let before = app.frame;
        let rejected = execute(
            &mut app,
            "unknown-display",
            Command::SetDisplayTarget {
                target: ::canvas::display::DisplayTarget::Exact {
                    display_ids: vec!["missing".into()],
                },
            },
        );
        assert!(matches!(rejected.outcome, Outcome::Error { .. }));
        assert_eq!(app.frame, before);

        drop(app.toggle_display_ordinal(2));
        assert_eq!(
            app.selected_display_ids,
            ["display-0", "display-2"],
            "non-contiguous displays remain independently selectable"
        );
        assert_eq!(app.windows.len(), 2);
        drop(app.toggle_display_ordinal(0));
        assert_eq!(app.selected_display_ids.len(), 3);
    }

    fn three_displays() -> Vec<DisplayInfo> {
        (0..3)
            .map(|index| DisplayInfo {
                id: format!("display-{index}"),
                name: format!("Display {index}"),
                frame: Rect {
                    x: index * 1_920,
                    y: 0,
                    width: 1_920,
                    height: 1_200,
                },
                primary: index == 0,
            })
            .collect()
    }

    #[test]
    fn a_reflow_view_fills_one_display_instead_of_showing_a_slice() {
        let mut app = test_app();
        app.displays = three_displays();
        app.frame = Rect {
            x: 0,
            y: 0,
            width: 5_760,
            height: 1_200,
        };
        app.layout_policy = ::canvas::view::LayoutPolicy::Reflow;

        let target = DisplayTarget::Exact {
            display_ids: vec!["display-1".into()],
        };
        drop(app.apply_display_target(&target).expect("target resolves"));

        // The frame is what normalized bounds are mapped across, so it must
        // become the one display; leaving it at the desktop union is exactly
        // what showed a third of the arrangement.
        assert_eq!(app.frame.width, 1_920, "frame should follow the display");
        assert_eq!(app.frame.x, 1_920);
    }

    #[test]
    fn a_fixed_view_keeps_its_geometry_when_displays_change() {
        let mut app = test_app();
        app.displays = three_displays();
        let desktop = Rect {
            x: 0,
            y: 0,
            width: 5_760,
            height: 1_200,
        };
        app.frame = desktop;
        app.layout_policy = ::canvas::view::LayoutPolicy::Fixed;

        let target = DisplayTarget::Exact {
            display_ids: vec!["display-1".into()],
        };
        drop(app.apply_display_target(&target).expect("target resolves"));

        assert_eq!(app.frame, desktop, "a fixed view must not be reflowed");
    }

    fn one_display(width: u32, height: u32) -> Vec<DisplayInfo> {
        vec![DisplayInfo {
            id: "display-0".into(),
            name: "Display 0".into(),
            frame: Rect {
                x: 0,
                y: 0,
                width,
                height,
            },
            primary: true,
        }]
    }

    /// An app already showing `bifrost/runtime` on one display of the given size.
    fn app_showing_a_view(width: u32, height: u32) -> App {
        let mut app = test_app();
        app.displays = one_display(width, height);
        app.frame = Rect {
            x: 0,
            y: 0,
            width,
            height,
        };
        app.selected_display_ids = vec!["display-0".into()];
        app.snapped_view = Some("bifrost/runtime".into());
        let fit = app.view_fit();
        app.document =
            ::canvas::operational_views::materialize("bifrost/runtime", &app.document, fit)
                .expect("built-in view materializes")
                .document;
        app
    }

    #[test]
    fn a_display_appearing_rescues_a_canvas_that_started_with_none() {
        // The trap this exists to fix: Canvas started before any display backend
        // answered, so it enumerated nothing and drew on nothing, forever.
        let mut app = test_app();
        app.displays = Vec::new();
        app.selected_display_ids = Vec::new();
        assert!(app.windows.is_empty());

        drop(app.apply_display_change(Ok(three_displays())));

        assert_eq!(app.displays.len(), 3, "the new topology must be adopted");
        assert_eq!(
            app.selected_display_ids.len(),
            3,
            "no surviving selection falls back to the whole desktop"
        );
        assert_eq!(
            app.frame.width, 5_760,
            "the frame must become the desktop that now exists"
        );
        assert_eq!(app.display_frames.len(), 3);
    }

    #[test]
    fn a_display_that_survives_a_change_keeps_the_operator_s_selection() {
        let mut app = test_app();
        app.displays = three_displays();
        app.selected_display_ids = vec!["display-1".into(), "display-2".into()];

        // display-2 is unplugged; display-1 is untouched and must stay chosen.
        let remaining = three_displays()
            .into_iter()
            .filter(|display| display.id != "display-2")
            .collect::<Vec<_>>();
        drop(app.apply_display_change(Ok(remaining)));

        assert_eq!(
            app.selected_display_ids,
            vec!["display-1".to_owned()],
            "a surviving selection must not be reshuffled"
        );
    }

    #[test]
    fn a_failed_re_enumeration_reports_rather_than_blanking_the_desktop() {
        let mut app = test_app();
        app.displays = three_displays();
        let before = app.displays.clone();

        drop(app.apply_display_change(Err("Dock Control is not running".into())));

        assert_eq!(app.displays, before, "a failure must not drop the topology");
        assert!(
            app.status.contains("Dock Control is not running"),
            "status was {:?}",
            app.status
        );
    }

    #[test]
    fn a_display_change_refits_the_active_view_to_the_room_it_now_has() {
        let mut app = app_showing_a_view(3_840, 2_160);
        let roomy = app.document.layout.clone();

        drop(app.apply_display_change(Ok(one_display(640, 560))));

        assert_eq!(app.frame.width, 640);
        assert_ne!(
            app.document.layout, roomy,
            "the same proportions on a much smaller frame leave widgets unreadable, \
             so the view must be re-fitted rather than merely rescaled"
        );
        let view = ::canvas::operational_views::load("bifrost/runtime").expect("view loads");
        let metrics = app.view_fit().metrics;
        for (placement, widget) in match &app.document.layout {
            Layout::Canvas { placements } => placements,
            _ => unreachable!("operational views materialize as a canvas"),
        }
        .iter()
        .zip(view.widgets.values())
        {
            let hint = canvas_widgets::size_hint(&widget.spec.kind, &metrics);
            assert!(
                placement.bounds.width * 640.0 + 0.5 >= hint.min_width,
                "{} is {}px wide, below its {}px minimum",
                placement.surface_id,
                placement.bounds.width * 640.0,
                hint.min_width
            );
        }
    }

    #[test]
    fn a_fixed_view_is_left_alone_by_a_display_change() {
        // The other half of the policy: a hand-arranged workspace is navigated
        // from spatial memory, so a monitor appearing must not move anything.
        // The frame it is drawn across stays put, and the fit therefore has
        // nothing to recompute.
        let mut app = app_showing_a_view(3_840, 2_160);
        app.layout_policy = ::canvas::view::LayoutPolicy::Fixed;
        let arranged = app.document.layout.clone();
        let frame = app.frame;

        drop(app.apply_display_change(Ok(one_display(1_280, 800))));

        assert_eq!(app.frame, frame, "a fixed view must not be reflowed");
        assert_eq!(app.document.layout, arranged, "nor re-fitted");
    }

    #[test]
    fn a_display_change_does_not_undo_an_arrangement_someone_made_by_hand() {
        let mut app = app_showing_a_view(3_840, 2_160);
        match &mut app.document.layout {
            Layout::Canvas { placements } => placements[0].bounds.x = 0.123,
            _ => unreachable!("operational views materialize as a canvas"),
        }
        let moved = app.document.layout.clone();

        drop(app.apply_display_change(Ok(one_display(1_920, 1_080))));

        assert_eq!(
            app.document.layout, moved,
            "a monitor waking up is not a reason to throw away someone's work"
        );
    }

    #[test]
    fn fitting_is_available_as_an_escape_hatch_for_a_fixed_view() {
        let mut app = test_app();
        app.displays = three_displays();
        app.frame = Rect {
            x: 0,
            y: 0,
            width: 5_760,
            height: 1_200,
        };
        app.layout_policy = ::canvas::view::LayoutPolicy::Fixed;
        app.selected_display_ids = vec!["display-2".into()];

        app.fit_layout_to_displays();
        assert_eq!(app.frame.width, 1_920, "fit applies regardless of policy");
        assert_eq!(app.frame.x, 3_840);

        // Fitting again changes nothing and says so rather than churning.
        app.fit_layout_to_displays();
        assert!(
            app.status.contains("already fits"),
            "status was {:?}",
            app.status
        );
    }

    #[test]
    fn views_reflow_unless_they_say_otherwise() {
        // A view written before this existed, and every operational view, should
        // fill the displays it is given without declaring anything.
        let view: ::canvas::view::SharedViewDocument =
            serde_json::from_str(include_str!("../../../config/views/fleet-runtime.json"))
                .expect("fleet view parses");
        assert_eq!(view.layout_policy, ::canvas::view::LayoutPolicy::Reflow);
    }

    #[test]
    fn snapping_is_a_peek_that_never_loses_a_deliberate_arrangement() {
        let mut app = test_app();
        app.snapped_view = Some("bifrost/runtime".into());
        // Open the view for real first: its layout references its own surfaces,
        // so the document must carry them before any layout can be valid.
        let fit = app.view_fit();
        app.document =
            ::canvas::operational_views::materialize("bifrost/runtime", &app.document, fit)
                .expect("built-in view materializes")
                .document;
        let saved = app.document.layout.clone();

        // A layout must hold at least one placement, so "moved" is the saved
        // arrangement with a surface dragged elsewhere rather than an empty one.
        let moved = match &saved {
            Layout::Canvas { placements } => {
                let mut placements = placements.clone();
                placements[0].bounds.x = 0.123;
                Layout::Canvas { placements }
            }
            _ => unreachable!("operational views materialize as a canvas"),
        };
        assert_ne!(moved, saved, "the test must actually move something");
        app.document.layout = moved.clone();

        // First press snaps to the saved arrangement and stashes mine.
        app.toggle_snapped_layout();
        assert_eq!(app.document.layout, saved, "should snap to the saved view");
        assert!(app.free_layout.is_some(), "my arrangement must be stashed");

        // Second press hands my arrangement back untouched.
        app.toggle_snapped_layout();
        assert_eq!(app.document.layout, moved, "should restore what I had");
        assert!(app.free_layout.is_none());
    }

    #[test]
    fn snapping_twice_from_the_saved_layout_is_not_a_trap() {
        // Already snapped: the layout matches, so there is nothing to stash. A
        // press must not record the saved layout as "my arrangement", or the
        // next press would silently do nothing.
        let mut app = test_app();
        app.snapped_view = Some("bifrost/runtime".into());
        // The whole document, not just its layout: a layout on its own names
        // surfaces the document does not have, and every path that reaches this
        // state in Canvas opens the view for real first.
        let fit = app.view_fit();
        app.document =
            ::canvas::operational_views::materialize("bifrost/runtime", &app.document, fit)
                .expect("built-in view materializes")
                .document;
        app.toggle_snapped_layout();
        assert!(
            app.free_layout.is_none(),
            "must not stash the saved layout as mine"
        );
        assert!(
            app.status.contains("already matches"),
            "status was {:?}",
            app.status
        );
    }

    #[test]
    fn restore_snapped_layout_is_idempotent_for_a_button() {
        let mut app = test_app();
        app.snapped_view = Some("bifrost/runtime".into());
        let fit = app.view_fit();
        app.document =
            ::canvas::operational_views::materialize("bifrost/runtime", &app.document, fit)
                .expect("built-in view materializes")
                .document;
        let saved = app.document.layout.clone();
        let moved = match &saved {
            Layout::Canvas { placements } => {
                let mut placements = placements.clone();
                placements[0].bounds.x = 0.123;
                Layout::Canvas { placements }
            }
            _ => unreachable!("operational views materialize as a canvas"),
        };
        app.document.layout = moved.clone();

        app.restore_snapped_layout();
        assert_eq!(app.document.layout, saved);
        assert_eq!(app.free_layout.as_ref(), Some(&moved));

        app.restore_snapped_layout();
        assert_eq!(
            app.document.layout, saved,
            "a second click must stay snapped"
        );
        assert_eq!(
            app.free_layout.as_ref(),
            Some(&moved),
            "the reversible keyboard stash must remain intact"
        );
    }

    #[test]
    fn saved_operational_view_is_inferred_after_restart() {
        let app = test_app();
        let document = ::canvas::operational_views::materialize(
            "bifrost/runtime",
            &app.document,
            app.view_fit(),
        )
        .expect("built-in view materializes")
        .document;

        assert_eq!(
            inferred_snapped_view(&document).as_deref(),
            Some("bifrost/runtime")
        );

        let mut mixed = document;
        let Layout::Canvas { placements } = &mut mixed.layout else {
            unreachable!("operational views materialize as a canvas")
        };
        placements[0].surface_id = "unrelated-surface".into();
        assert_eq!(inferred_snapped_view(&mixed), None);
    }

    #[test]
    fn snapping_with_no_active_view_reports_rather_than_panicking() {
        let mut app = test_app();
        let before = app.document.layout.clone();
        app.toggle_snapped_layout();
        assert_eq!(app.document.layout, before, "layout must not change");
        assert!(
            app.status.contains("No saved workspace"),
            "status was {:?}",
            app.status
        );
    }

    #[test]
    fn project_dashboard_snaps_to_its_persisted_workspace_layout() {
        let mut app = test_app();
        app.document.surfaces = vec![Surface {
            id: "dashboard".into(),
            title: "Dashboard".into(),
            content: SurfaceContent::Stream {
                url: "https://example.com".into(),
                matte: None,
            },
            inputs: Vec::new(),
        }];
        let saved_layout = Layout::Canvas {
            placements: vec![Placement {
                surface_id: "dashboard".into(),
                bounds: NormalizedRect {
                    x: 0.0,
                    y: 0.0,
                    width: 1.0,
                    height: 1.0,
                },
                layer: 0,
            }],
        };
        app.document.layout = saved_layout.clone();
        app.workspaces.upsert(app.document.clone());
        let moved = Layout::Canvas {
            placements: vec![Placement {
                surface_id: "dashboard".into(),
                bounds: NormalizedRect {
                    x: 0.2,
                    y: 0.2,
                    width: 0.6,
                    height: 0.6,
                },
                layer: 0,
            }],
        };
        app.document.layout = moved.clone();

        app.toggle_snapped_layout();
        assert_eq!(app.document.layout, saved_layout);
        assert_eq!(app.free_layout.as_ref(), Some(&moved));

        app.toggle_snapped_layout();
        assert_eq!(app.document.layout, moved);
    }

    #[test]
    fn placing_on_left_preserves_ops_on_right_and_shows_both_displays() {
        let mut app = test_app();
        app.displays = (0..3)
            .map(|index| DisplayInfo {
                id: format!("display-{index}"),
                name: format!("Display {index}"),
                frame: Rect {
                    x: index * 1_920,
                    y: 0,
                    width: 1_920,
                    height: 1_200,
                },
                primary: index == 0,
            })
            .collect();
        app.display_frames = app.displays.iter().map(|display| display.frame).collect();
        app.frame = Rect {
            x: 0,
            y: 0,
            width: 5_760,
            height: 1_200,
        };
        app.selected_display_ids = vec!["display-2".into()];
        app.document.surfaces = ["ops", "pdf"]
            .into_iter()
            .map(|id| Surface {
                id: id.into(),
                title: id.into(),
                content: SurfaceContent::Stream {
                    url: format!("https://example.com/{id}"),
                    matte: None,
                },
                inputs: Vec::new(),
            })
            .collect();
        let ops_bounds = NormalizedRect {
            x: 0.68,
            y: 0.08,
            width: 0.30,
            height: 0.84,
        };
        app.document.layout = Layout::Canvas {
            placements: vec![
                Placement {
                    surface_id: "ops".into(),
                    bounds: ops_bounds,
                    layer: 0,
                },
                Placement {
                    surface_id: "pdf".into(),
                    bounds: NormalizedRect {
                        x: 0.4,
                        y: 0.2,
                        width: 0.2,
                        height: 0.6,
                    },
                    layer: 1,
                },
            ],
        };

        let reply = execute(
            &mut app,
            "place-pdf-left",
            Command::PlaceSurface {
                surface_id: "pdf".into(),
                display_ordinal: 1,
                region: DisplayRegion::Full,
                margin: 24,
            },
        );
        assert!(matches!(
            reply.outcome,
            Outcome::Ok {
                receipt: Receipt::SurfacePlaced {
                    ref surface_id,
                    ref display_id,
                    ..
                }
            } if surface_id == "pdf" && display_id == "display-0"
        ));
        assert!(app.selected_display_ids.contains(&"display-0".into()));
        assert!(app.selected_display_ids.contains(&"display-2".into()));
        let Layout::Canvas { placements } = &app.document.layout else {
            panic!("placement converts the workspace to free-form layout")
        };
        assert_eq!(
            placements
                .iter()
                .find(|placement| placement.surface_id == "ops")
                .unwrap()
                .bounds,
            ops_bounds
        );
        let pdf = placements
            .iter()
            .find(|placement| placement.surface_id == "pdf")
            .unwrap();
        assert!(pdf.bounds.x < 1.0 / 3.0);
        assert!(pdf.bounds.width < 1.0 / 3.0);
    }

    fn key(named: iced::keyboard::key::Named) -> iced::keyboard::Event {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        Event::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::default(),
            text: None,
            repeat: false,
        }
    }

    fn typed(character: &'static str) -> iced::keyboard::Event {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        Event::KeyPressed {
            key: Key::Character(character.into()),
            modified_key: Key::Character(character.into()),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::default(),
            text: Some(character.into()),
            repeat: false,
        }
    }

    fn app_with_prompt() -> App {
        let mut app = test_app();
        app.document.surfaces = vec![Surface {
            id: "ask".into(),
            title: "Ask transmog".into(),
            content: SurfaceContent::Prompt {
                target: "transmog".into(),
                placeholder: None,
                session: None,
                task_kinds: Vec::new(),
                task_targets: BTreeMap::new(),
            },
            inputs: Vec::new(),
        }];
        *app.document.active_layout_mut() = Layout::Surface {
            surface_id: "ask".into(),
        };
        app.document.focused_surface_id = Some("ask".into());
        app.ensure_scene_decoded();
        app
    }

    fn draft_of(app: &App) -> String {
        match &app.decoded.get("ask").expect("prompt decoded").source {
            ContentSource::Prompt { draft, .. } => draft.clone(),
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    #[test]
    fn typing_into_a_prompt_builds_a_draft_and_backspace_removes_it() {
        let mut app = app_with_prompt();
        for character in ["h", "i"] {
            let _ = app.handle_prompt_key("ask", typed(character));
        }
        assert_eq!(draft_of(&app), "hi");

        let _ = app.handle_prompt_key("ask", key(iced::keyboard::key::Named::Backspace));
        assert_eq!(draft_of(&app), "h");

        let _ = app.handle_prompt_key("ask", key(iced::keyboard::key::Named::Escape));
        assert_eq!(draft_of(&app), "", "escape abandons the line");
    }

    #[test]
    fn task_launcher_requires_a_kind_and_retains_the_submitted_prompt() {
        let mut app = app_with_prompt();
        let ContentSource::Prompt { task_kinds, .. } =
            &mut app.decoded.get_mut("ask").unwrap().source
        else {
            panic!("expected prompt")
        };
        *task_kinds = vec!["PCB".into(), "Part".into(), "Code".into()];

        let _ = app.handle_prompt_key("ask", typed("x"));
        assert_eq!(draft_of(&app), "", "idle launcher does not steal typing");

        drop(app.update(Message::PromptKindSelected {
            surface_id: "ask".into(),
            kind: "PCB".into(),
        }));
        for character in ["f", "i", "l", "t", "e", "r"] {
            let _ = app.handle_prompt_key("ask", typed(character));
        }
        let _ = app.handle_prompt_key("ask", key(iced::keyboard::key::Named::Enter));

        let ContentSource::Prompt {
            selected_kind,
            tasks,
            active_task,
            ..
        } = &app.decoded.get("ask").unwrap().source
        else {
            panic!("expected prompt")
        };
        assert!(selected_kind.is_none());
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].kind, "PCB");
        assert_eq!(tasks[0].prompt, "filter");
        assert_eq!(tasks[0].state, "queued");
        assert_eq!(*active_task, Some(tasks[0].id));
    }

    #[test]
    fn creation_kind_routes_to_its_declared_tool_target() {
        let targets = BTreeMap::from([
            ("PCB".into(), "lob".into()),
            ("Model".into(), "transmog".into()),
        ]);
        assert_eq!(prompt_target_for_kind("transmog", &targets, "PCB"), "lob");
        assert_eq!(
            prompt_target_for_kind("transmog", &targets, "Model"),
            "transmog"
        );
        assert_eq!(
            prompt_target_for_kind("transmog", &targets, "Code"),
            "transmog",
            "undeclared kinds retain the prompt's default target"
        );
    }

    #[test]
    fn selecting_a_global_task_jumps_to_its_page_and_restores_its_surface() {
        let mut app = app_with_prompt();
        let ContentSource::Prompt { tasks, .. } = &mut app.decoded.get_mut("ask").unwrap().source
        else {
            panic!("expected prompt")
        };
        tasks.push(LocalTask {
            id: 42,
            kind: "Model".into(),
            prompt: "a bracket".into(),
            state: "queued".into(),
            detail: None,
            activity: Vec::new(),
        });
        app.document.layout = Layout::Empty;
        app.document.pages = vec![::canvas::Page {
            name: "Model task".into(),
            layout: Layout::Surface {
                surface_id: "ask".into(),
            },
        }];
        app.document.active_page = 0;
        app.document.focused_surface_id = None;
        app.minimized_surfaces.insert("ask".into());

        drop(app.update(Message::PromptTaskSelected {
            surface_id: "ask".into(),
            task_id: 42,
        }));

        assert_eq!(app.document.active_page_index(), 1);
        assert_eq!(app.document.focused_surface_id.as_deref(), Some("ask"));
        assert!(!app.minimized_surfaces.contains("ask"));
        let ContentSource::Prompt { active_task, .. } = &app.decoded["ask"].source else {
            panic!("expected prompt")
        };
        assert_eq!(*active_task, Some(42));
    }

    #[cfg(feature = "lua-scenes")]
    #[test]
    fn bridge_status_updates_the_exact_creation_task() {
        let mut app = app_with_prompt();
        let ContentSource::Prompt { tasks, .. } = &mut app.decoded.get_mut("ask").unwrap().source
        else {
            panic!("expected prompt")
        };
        tasks.push(LocalTask {
            id: 42,
            kind: "Model".into(),
            prompt: "a bracket".into(),
            state: "queued".into(),
            detail: None,
            activity: Vec::new(),
        });
        app.apply_task_status(canvas_core::task::TaskStatus {
            task_id: "local-42".into(),
            kind: "Model".into(),
            state: "delivered".into(),
            target: "transmog".into(),
            session: Some("build-session".into()),
            detail: None,
            activity: vec!["agent started".into()],
        });
        let ContentSource::Prompt { tasks, .. } = &app.decoded["ask"].source else {
            panic!("expected prompt")
        };
        assert_eq!(tasks[0].state, "delivered");
        assert_eq!(
            tasks[0].detail.as_deref(),
            Some("Delivered to transmog · build-session")
        );
    }

    /// Enter spools the line and clears the draft. The draft must not survive:
    /// the line has left, and showing it still would invite sending it twice.
    #[test]
    fn enter_spools_the_prompt_and_clears_the_draft() {
        let mut app = app_with_prompt();
        for character in ["g", "o"] {
            let _ = app.handle_prompt_key("ask", typed(character));
        }
        let _ = app.handle_prompt_key("ask", key(iced::keyboard::key::Named::Enter));

        assert_eq!(draft_of(&app), "");
        match &app.decoded.get("ask").unwrap().source {
            ContentSource::Prompt { sent, .. } => assert_eq!(sent, &["go".to_string()]),
            other => panic!("expected a prompt, got {other:?}"),
        }

        let spool = ::canvas::runtime_config::root()
            .expect("runtime root")
            .join("prompts");
        let found: Vec<serde_json::Value> = std::fs::read_dir(&spool)
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| std::fs::read(entry.path()).ok())
            .filter_map(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .filter(|body| body["text"] == "go")
            .collect();
        assert!(
            !found.is_empty(),
            "the line should be spooled for the bridge"
        );
        assert_eq!(found[0]["target"], "transmog");
    }

    #[test]
    fn an_empty_prompt_is_not_sent() {
        let mut app = app_with_prompt();
        let _ = app.handle_prompt_key("ask", typed(" "));
        let _ = app.handle_prompt_key("ask", key(iced::keyboard::key::Named::Enter));
        match &app.decoded.get("ask").unwrap().source {
            ContentSource::Prompt { sent, .. } => {
                assert!(sent.is_empty(), "whitespace is not a prompt")
            }
            other => panic!("expected a prompt, got {other:?}"),
        }
    }

    /// Re-applying the document must not wipe what is being typed: the draft is
    /// runtime state the document does not carry, so identity is the target.
    #[test]
    fn a_half_typed_prompt_survives_a_document_reapplication() {
        let mut app = app_with_prompt();
        for character in ["h", "a", "l", "f"] {
            let _ = app.handle_prompt_key("ask", typed(character));
        }
        app.prune_decoded();
        app.ensure_scene_decoded();
        assert_eq!(draft_of(&app), "half");
    }

    #[test]
    fn widget_reapplication_retains_live_data_but_adopts_document_chrome() {
        let mut app = test_app();
        let mut spec = canvas_widgets::WidgetSpec::from_json(
            br#"{"id":"run","kind":"status_list","title":"Live","nodes":[],"chrome":"frameless"}"#,
        )
        .unwrap();
        app.document.surfaces = vec![Surface {
            id: "run".into(),
            title: "Run".into(),
            content: SurfaceContent::Widget { spec: spec.clone() },
            inputs: Vec::new(),
        }];
        app.ensure_scene_decoded();

        let canvas_widgets::WidgetKind::StatusList { title, .. } = &mut spec.kind else {
            unreachable!()
        };
        *title = "Template title must not replace live data".into();
        spec.chrome = canvas_widgets::WidgetChrome::Integrated;
        app.document.surfaces[0].content = SurfaceContent::Widget { spec };
        app.prune_decoded();

        let ContentSource::Widget { spec } = &app.decoded["run"].source else {
            panic!("widget remains decoded")
        };
        assert_eq!(
            spec.effective_chrome(),
            canvas_widgets::WidgetChrome::Integrated
        );
        let canvas_widgets::WidgetKind::StatusList { title, .. } = &spec.kind else {
            unreachable!()
        };
        assert_eq!(title, "Live");
    }

    #[test]
    fn control_arrows_step_pages_and_nothing_else_does() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let pressed = |named, modifiers| Event::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        };

        assert_eq!(
            page_cycle_direction(&pressed(key::Named::ArrowRight, Modifiers::CTRL)),
            Some(1)
        );
        assert_eq!(
            page_cycle_direction(&pressed(key::Named::ArrowLeft, Modifiers::CTRL)),
            Some(-1)
        );

        // Ctrl+Shift+Up/Down cycles workspaces. Pages must not also answer to
        // it, or one gesture would move two different things at once.
        assert_eq!(
            page_cycle_direction(&pressed(
                key::Named::ArrowRight,
                Modifiers::CTRL | Modifiers::SHIFT
            )),
            None
        );
        assert_eq!(
            page_cycle_direction(&pressed(key::Named::ArrowUp, Modifiers::CTRL)),
            None,
            "vertical belongs to workspaces"
        );
        assert_eq!(
            page_cycle_direction(&pressed(key::Named::ArrowRight, Modifiers::default())),
            None,
            "a bare arrow belongs to whatever has focus"
        );
    }

    /// Holding the key must not sweep through every page at keyboard repeat
    /// rate — each press moves one page.
    #[test]
    fn a_held_arrow_does_not_sweep_through_pages() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let repeated = Event::KeyPressed {
            key: Key::Named(key::Named::ArrowRight),
            modified_key: Key::Named(key::Named::ArrowRight),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::CTRL,
            text: None,
            repeat: true,
        };
        assert_eq!(page_cycle_direction(&repeated), None);
    }

    #[test]
    fn control_number_shortcuts_map_to_display_ordinals() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let pressed = |number: &'static str| Event::KeyPressed {
            key: Key::Character(number.into()),
            modified_key: Key::Character(number.into()),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::CTRL,
            text: Some(number.into()),
            repeat: false,
        };
        assert_eq!(display_shortcut(&pressed("0")), Some(0));
        assert_eq!(display_shortcut(&pressed("1")), Some(1));
        assert_eq!(display_shortcut(&pressed("2")), Some(2));
        assert_eq!(display_shortcut(&pressed("3")), Some(3));
        assert_eq!(display_shortcut(&pressed("4")), None);
    }

    #[test]
    fn control_n_is_reserved_for_project_task_creation() {
        let event = browser_key_event("n", false, true, false, false).expect("key event");
        assert!(new_task_requested(&event));
        assert!(!new_task_requested(&typed("n")));
        let shifted = browser_key_event("n", true, true, false, false).expect("key event");
        assert!(!new_task_requested(&shifted));
    }

    #[test]
    fn control_alt_l_is_an_in_app_snap_shortcut() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let pressed = |modifiers| Event::KeyPressed {
            key: Key::Character("l".into()),
            modified_key: Key::Character("l".into()),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers,
            text: Some("l".into()),
            repeat: false,
        };

        assert!(snap_layout_requested(&pressed(
            Modifiers::CTRL | Modifiers::ALT
        )));
        assert!(!snap_layout_requested(&pressed(Modifiers::CTRL)));
        assert!(!snap_layout_requested(&pressed(
            Modifiers::CTRL | Modifiers::ALT | Modifiers::LOGO
        )));
        let option_translated = Event::KeyPressed {
            key: Key::Character("¬".into()),
            modified_key: Key::Character("l".into()),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::CTRL | Modifiers::ALT,
            text: Some("¬".into()),
            repeat: false,
        };
        assert!(snap_layout_requested(&option_translated));
    }

    #[test]
    fn focus_emphasis_is_bounded_and_settles_to_zero() {
        for millis in [0, 100, 350, 700, 1_000, 1_399, 1_400, 2_000] {
            let value = focus_emphasis_curve(Duration::from_millis(millis));
            assert!((0.0..=1.0).contains(&value));
        }
        assert_eq!(
            focus_emphasis_curve(FOCUS_EMPHASIS_DURATION),
            0.0,
            "the pulse must stop requesting visual emphasis"
        );
    }

    #[test]
    fn surface_hover_is_host_owned_and_clears_on_exit() {
        let mut app = test_app();
        drop(app.update(Message::SurfaceHover(Some("sample".into()))));
        assert_eq!(app.hovered_surface_id.as_deref(), Some("sample"));
        drop(app.update(Message::SurfaceHover(None)));
        assert!(app.hovered_surface_id.is_none());
    }

    #[test]
    fn brackets_page_the_resident_widget_catalog_and_wrap() {
        let mut app = test_app();
        app.document.id = "widget-catalog-all".into();
        app.document.surfaces = (0..13)
            .map(|index| Surface {
                id: format!("widget-{index}"),
                title: format!("Widget {index}"),
                content: SurfaceContent::Unsupported(serde_json::json!({
                    "kind": "test_widget"
                })),
                inputs: Vec::new(),
            })
            .collect();
        let first = app.document.surfaces[..6]
            .iter()
            .map(|surface| surface.id.clone())
            .collect::<Vec<_>>();
        app.document.layout = Layout::Canvas {
            placements: ::canvas::compose::grid(&first, 2, 3),
        };

        assert!(app.cycle_widget_catalog_page(1));
        let Layout::Canvas { placements } = &app.document.layout else {
            panic!("catalog stays on a canvas")
        };
        assert_eq!(placements[0].surface_id, "widget-6");
        assert_eq!(placements.len(), 6);

        assert!(app.cycle_widget_catalog_page(-1));
        let Layout::Canvas { placements } = &app.document.layout else {
            panic!("catalog stays on a canvas")
        };
        assert_eq!(placements[0].surface_id, "widget-0");
    }

    #[test]
    fn workspace_cycle_shortcuts_do_not_claim_macos_control_arrows() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let pressed = |key: Key, modifiers| Event::KeyPressed {
            key: key.clone(),
            modified_key: key,
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers,
            text: None,
            repeat: false,
        };

        assert_eq!(
            workspace_cycle_direction(&pressed(
                Key::Named(key::Named::ArrowUp),
                Modifiers::CTRL | Modifiers::SHIFT,
            )),
            Some(-1)
        );
        assert_eq!(
            workspace_cycle_direction(&pressed(
                Key::Named(key::Named::ArrowDown),
                Modifiers::CTRL | Modifiers::SHIFT,
            )),
            Some(1)
        );
        assert_eq!(
            workspace_cycle_direction(&pressed(Key::Named(key::Named::ArrowUp), Modifiers::CTRL,)),
            None
        );
        assert_eq!(
            workspace_cycle_direction(&pressed(
                Key::Named(key::Named::ArrowDown),
                Modifiers::CTRL | Modifiers::SHIFT | Modifiers::ALT,
            )),
            None
        );
    }

    #[test]
    fn f3_toggles_diagnostics_without_changing_the_workspace() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let f3 = || Event::KeyPressed {
            key: Key::Named(key::Named::F3),
            modified_key: Key::Named(key::Named::F3),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        };
        assert!(diagnostics_toggle_requested(&f3()));
        let mut app = test_app();
        let revision = app.document.revision;
        assert!(app.hud_visible);
        drop(app.update(Message::TerminalInput(f3())));
        assert!(!app.hud_visible);
        assert_eq!(app.document.revision, revision);
        drop(app.update(Message::TerminalInput(f3())));
        assert!(app.hud_visible);
    }

    #[test]
    fn f4_previews_applies_and_cancels_workspace_themes() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let pressed = |named| Event::KeyPressed {
            key: Key::Named(named),
            modified_key: Key::Named(named),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::empty(),
            text: None,
            repeat: false,
        };
        let mut app = test_app();
        let original = app.project_palette();

        let f4 = pressed(key::Named::F4);
        assert!(theme_picker_requested(&f4));
        drop(app.update(Message::TerminalInput(f4)));
        assert!(app.theme_picker.is_some());
        drop(app.update(Message::TerminalInput(pressed(key::Named::ArrowRight))));
        assert_ne!(app.presented_palette(), original);
        drop(app.update(Message::TerminalInput(pressed(key::Named::Escape))));
        assert!(app.theme_picker.is_none());
        assert_eq!(app.project_palette(), original);

        drop(app.update(Message::TerminalInput(pressed(key::Named::F4))));
        drop(app.update(Message::TerminalInput(pressed(key::Named::ArrowRight))));
        let preview = app.presented_palette();
        drop(app.update(Message::TerminalInput(pressed(key::Named::Enter))));
        assert!(app.theme_picker.is_none());
        assert_eq!(app.project_palette(), preview);
        assert!(matches!(app.document.theme, ThemeRef::Selection { .. }));
    }

    #[test]
    fn overlays_anchor_to_the_selected_edge_displays() {
        let mut app = test_app();
        app.displays = vec![
            DisplayInfo {
                id: "left".into(),
                name: "Left".into(),
                frame: Rect {
                    x: -1_920,
                    y: 0,
                    width: 1_920,
                    height: 1_200,
                },
                primary: false,
            },
            DisplayInfo {
                id: "middle".into(),
                name: "Middle".into(),
                frame: Rect {
                    x: 0,
                    y: 0,
                    width: 1_920,
                    height: 1_200,
                },
                primary: true,
            },
            DisplayInfo {
                id: "right".into(),
                name: "Right".into(),
                frame: Rect {
                    x: 1_920,
                    y: 0,
                    width: 1_920,
                    height: 1_200,
                },
                primary: false,
            },
        ];
        app.selected_display_ids = vec!["left".into(), "middle".into(), "right".into()];

        assert!(app.overlay_on_viewport(OverlayTarget::Leftmost, app.displays[0].frame));
        assert!(!app.overlay_on_viewport(OverlayTarget::Leftmost, app.displays[1].frame));
        assert!(app.overlay_on_viewport(OverlayTarget::Rightmost, app.displays[2].frame));
        assert!(!app.overlay_on_viewport(OverlayTarget::Rightmost, app.displays[1].frame));
        assert!(app.overlay_on_viewport(OverlayTarget::Center, app.displays[1].frame));
        assert!(!app.overlay_on_viewport(OverlayTarget::Center, app.displays[0].frame));
        assert!(app.overlay_on_viewport(OverlayTarget::All, app.displays[1].frame));
        assert!(!app.overlay_on_viewport(OverlayTarget::Hidden, app.displays[0].frame));
    }

    #[test]
    fn overlay_fails_closed_without_selected_displays() {
        let mut app = test_app();
        app.selected_display_ids.clear();
        let any_frame = app.displays[0].frame;
        assert!(!app.overlay_on_viewport(OverlayTarget::Center, any_frame));
        assert!(!app.overlay_on_viewport(OverlayTarget::Leftmost, any_frame));
        assert!(!app.overlay_on_viewport(OverlayTarget::Rightmost, any_frame));
        assert!(app.overlay_on_viewport(OverlayTarget::All, any_frame));
        assert!(!app.overlay_on_viewport(OverlayTarget::Hidden, any_frame));
    }

    #[test]
    fn jump_history_back_and_forward() {
        let mut hist = JumpHistory::new(JumpEntry::current("a", None));
        assert!(!hist.can_go_back());
        assert!(!hist.can_go_forward());
        hist.jump(JumpEntry::current("b", None));
        assert!(hist.can_go_back());
        assert!(!hist.can_go_forward());
        assert_eq!(hist.back().unwrap().project_id, "a");
        assert!(!hist.can_go_back());
        assert!(hist.can_go_forward());
        assert_eq!(hist.forward().unwrap().project_id, "b");
    }

    #[test]
    fn jump_history_truncates_forward_on_new_jump() {
        let mut hist = JumpHistory::new(JumpEntry::current("a", None));
        hist.jump(JumpEntry::current("b", None));
        hist.jump(JumpEntry::current("c", None));
        hist.back();
        // Now at "b"; a new jump should truncate "c"
        hist.jump(JumpEntry::current("d", None));
        assert!(!hist.can_go_forward(), "forward history must be truncated");
        assert_eq!(hist.back().unwrap().project_id, "b");
    }

    #[test]
    fn jump_history_records_view_id() {
        let mut hist = JumpHistory::new(JumpEntry::current("dock", None));
        hist.jump(JumpEntry::current("dock", Some("bifrost/runtime")));
        let entry = hist.entries.last().unwrap();
        assert_eq!(entry.view_id.as_deref(), Some("bifrost/runtime"));
    }

    #[test]
    fn plain_question_mark_opens_help_only_from_desktop_focus() {
        use iced::keyboard::{Event, Key, Location, Modifiers, key};
        let question = || Event::KeyPressed {
            key: Key::Character("?".into()),
            modified_key: Key::Character("?".into()),
            physical_key: key::Physical::Unidentified(key::NativeCode::Unidentified),
            location: Location::Standard,
            modifiers: Modifiers::SHIFT,
            text: Some("?".into()),
            repeat: false,
        };
        let mut app = test_app();
        app.desktop_focused = false;
        drop(app.update(Message::TerminalInput(question())));
        assert!(!app.shortcuts_visible);

        app.desktop_focused = true;
        drop(app.update(Message::TerminalInput(question())));
        assert!(app.shortcuts_visible);
    }

    #[test]
    fn canvas_visibility_closes_and_recreates_every_selected_host_window() {
        let mut app = test_app();
        app.displays = (0..3)
            .map(|index| DisplayInfo {
                id: format!("display-{index}"),
                name: format!("Display {index}"),
                frame: Rect {
                    x: index * 1_920,
                    y: 0,
                    width: 1_920,
                    height: 1_200,
                },
                primary: index == 0,
            })
            .collect();
        app.selected_display_ids = vec!["display-0".into(), "display-1".into(), "display-2".into()];
        drop(app.replace_host_windows(app.selected_display_frames()));
        assert_eq!(app.windows.len(), 3);

        let hidden = execute(&mut app, "hide", Command::SetVisibility { visible: false });
        assert!(matches!(
            hidden.outcome,
            Outcome::Ok {
                receipt: Receipt::VisibilitySet { visible: false }
            }
        ));
        assert!(!app.snapshot().visible);
        assert!(app.windows.is_empty());

        let shown = execute(&mut app, "show", Command::SetVisibility { visible: true });
        assert!(matches!(
            shown.outcome,
            Outcome::Ok {
                receipt: Receipt::VisibilitySet { visible: true }
            }
        ));
        assert!(app.snapshot().visible);
        assert_eq!(app.windows.len(), 3);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn canvas_frame_uses_the_complete_display() {
        let display = canvas_control::Display {
            id: "right".into(),
            name: "Right".into(),
            frame: canvas_control::Rect {
                x: 3840,
                y: 0,
                width: 1920,
                height: 1200,
            },
            visible_frame: canvas_control::Rect {
                x: 3840,
                y: 25,
                width: 1920,
                height: 1175,
            },
            primary: false,
        };
        assert_eq!(
            platform::canvas_frame(&display),
            Rect {
                x: 3840,
                y: 0,
                width: 1920,
                height: 1200,
            }
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn canvas_hides_system_chrome_only_in_kiosk_mode() {
        use objc2_app_kit::NSApplicationPresentationOptions;

        let windowed = platform::canvas_presentation_options(false);
        assert!(windowed.is_empty());

        let kiosk = platform::canvas_presentation_options(true);
        assert!(
            kiosk.contains(
                NSApplicationPresentationOptions::NSApplicationPresentationAutoHideMenuBar
            )
        );
        assert!(
            kiosk.contains(NSApplicationPresentationOptions::NSApplicationPresentationAutoHideDock)
        );
    }

    #[test]
    fn interaction_intents_update_authoritative_canvas_layout() {
        let mut app = test_app();
        app.document.layout = Layout::Canvas {
            placements: vec![Placement {
                surface_id: "sample".into(),
                bounds: NormalizedRect {
                    x: 0.1,
                    y: 0.1,
                    width: 0.4,
                    height: 0.4,
                },
                layer: 0,
            }],
        };
        let before = app.runtime_revision;
        app.apply_interaction_intents(vec![
            InteractionIntent::Focus {
                surface_id: "sample".into(),
            },
            InteractionIntent::SetBounds {
                surface_id: "sample".into(),
                bounds: LogicalRect {
                    x: 256.0,
                    y: 160.0,
                    width: 512.0,
                    height: 320.0,
                },
            },
        ]);
        let Layout::Canvas { placements } = &app.document.layout else {
            panic!("interaction replaced Canvas layout");
        };
        assert_eq!(app.document.focused_surface_id.as_deref(), Some("sample"));
        assert_eq!(placements[0].bounds.x, 0.2);
        assert_eq!(placements[0].bounds.y, 0.2);
        assert_eq!(placements[0].bounds.width, 0.4);
        assert_eq!(placements[0].bounds.height, 0.4);
        assert!(app.runtime_revision > before);
    }

    #[test]
    fn orbitable_content_claims_direct_move_but_leaves_resize_handles() {
        assert!(content_claims_surface_drag(HitRegion::Body, true));
        assert!(content_claims_surface_drag(HitRegion::TopBar, true));
        assert!(!content_claims_surface_drag(
            HitRegion::Resize {
                handle: ResizeHandle::SouthEast,
            },
            true,
        ));
        assert!(!content_claims_surface_drag(HitRegion::Body, false));
    }

    #[test]
    fn window_relative_pointer_focuses_and_promotes_on_a_nonzero_display_origin() {
        let mut app = test_app();
        app.frame.x = 3_840;
        let window = iced::window::Id::unique();
        app.windows.insert(window, app.frame);
        app.interaction = interaction_engine(
            app.frame,
            app.document.chrome,
            ::canvas::theme_pack::ThemeChrome::default().titlebar_height,
        );
        app.document.layout = Layout::Canvas {
            placements: vec![
                Placement {
                    surface_id: "lower".into(),
                    bounds: NormalizedRect {
                        x: 0.1,
                        y: 0.1,
                        width: 0.5,
                        height: 0.5,
                    },
                    layer: 0,
                },
                Placement {
                    surface_id: "upper".into(),
                    bounds: NormalizedRect {
                        x: 0.3,
                        y: 0.3,
                        width: 0.5,
                        height: 0.5,
                    },
                    layer: 1,
                },
            ],
        };
        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::CursorMoved {
                position: Point::new(180.0, 180.0),
            },
        );
        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            false
        );
        assert_eq!(app.document.focused_surface_id.as_deref(), Some("lower"));
        let Layout::Canvas { placements } = &app.document.layout else {
            panic!("expected Canvas layout");
        };
        assert!(placements[0].layer > placements[1].layer);
    }

    #[test]
    fn pointer_on_second_display_uses_shared_virtual_desktop_coordinates() {
        let mut app = test_app();
        app.frame = Rect {
            x: 0,
            y: 0,
            width: 3_840,
            height: 1_200,
        };
        app.interaction = interaction_engine(
            app.frame,
            app.document.chrome,
            ::canvas::theme_pack::ThemeChrome::default().titlebar_height,
        );
        let right = iced::window::Id::unique();
        app.windows.insert(
            right,
            Rect {
                x: 1_920,
                y: 0,
                width: 1_920,
                height: 1_200,
            },
        );
        app.document.layout = Layout::Canvas {
            placements: vec![Placement {
                surface_id: "right-surface".into(),
                bounds: NormalizedRect {
                    x: 0.5,
                    y: 0.1,
                    width: 0.3,
                    height: 0.5,
                },
                layer: 0,
            }],
        };
        let _ = app.handle_mouse(
            right,
            iced::mouse::Event::CursorMoved {
                position: Point::new(200.0, 240.0),
            },
        );
        let _ = app.handle_mouse(
            right,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
        );
        assert_eq!(
            app.cursor,
            Some(LogicalPoint {
                x: 2_120.0,
                y: 240.0
            })
        );
        assert_eq!(
            app.document.focused_surface_id.as_deref(),
            Some("right-surface")
        );
    }

    #[test]
    fn minimized_dock_appears_and_restore_promotes_the_surface() {
        let mut app = test_app();
        app.document.surfaces = vec![Surface {
            id: "invoice".into(),
            title: "Invoice".into(),
            content: SurfaceContent::Scene {
                scene: Scene { nodes: Vec::new() },
            },
            inputs: Vec::new(),
        }];
        app.document.layout = Layout::Canvas {
            placements: vec![Placement {
                surface_id: "invoice".into(),
                bounds: NormalizedRect {
                    x: 0.1,
                    y: 0.1,
                    width: 0.5,
                    height: 0.5,
                },
                layer: 0,
            }],
        };
        assert!(app.minimized_dock().is_none());
        app.minimized_surfaces.insert("invoice".into());
        assert!(app.minimized_dock().is_some());
        assert_eq!(app.snapshot().minimized_surface_ids, ["invoice"]);
        let revision = app.runtime_revision;
        drop(app.update(Message::RestoreSurface("invoice".into())));
        assert!(app.minimized_surfaces.is_empty());
        assert_eq!(app.document.focused_surface_id.as_deref(), Some("invoice"));
        assert!(app.runtime_revision > revision);
    }

    #[test]
    fn surface_controls_use_compact_standard_upper_left_hit_targets() {
        let mut app = test_app();
        let window = iced::window::Id::unique();
        app.windows.insert(window, app.frame);
        app.document.surfaces.push(Surface {
            id: "sample".into(),
            title: "Sample".into(),
            content: SurfaceContent::Scene {
                scene: Scene { nodes: Vec::new() },
            },
            inputs: Vec::new(),
        });
        app.document.layout = Layout::Canvas {
            placements: vec![Placement {
                surface_id: "sample".into(),
                bounds: NormalizedRect {
                    x: 0.1,
                    y: 0.1,
                    width: 0.4,
                    height: 0.4,
                },
                layer: 0,
            }],
        };
        let left = f64::from(INITIAL_FRAME.width) * 0.1;
        let top = f64::from(INITIAL_FRAME.height) * 0.1;
        for (offset, action) in [
            (8.0, WindowAction::Close),
            (26.0, WindowAction::Minimize),
            (44.0, WindowAction::Maximize),
        ] {
            app.cursor = Some(LogicalPoint {
                x: left + offset,
                y: top + 10.0,
            });
            assert_eq!(
                app.hit_test_surface_control(),
                Some(("sample".into(), action))
            );
        }
        app.cursor = Some(LogicalPoint {
            x: left + 60.0,
            y: top + 10.0,
        });
        assert_eq!(app.hit_test_surface_control(), None);

        assert_eq!(
            window_action_label(WindowAction::Maximize, false),
            "Maximize"
        );
        assert_eq!(window_action_label(WindowAction::Maximize, true), "Restore");

        let original_layout = app.document.layout.clone();
        app.cursor = Some(LogicalPoint {
            x: left + 44.0,
            y: top + 10.0,
        });
        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            false
        );
        assert!(app.maximized_surfaces.contains("sample"));
        app.cursor = Some(LogicalPoint { x: 44.0, y: 10.0 });
        assert_eq!(
            app.hit_test_surface_control(),
            Some(("sample".into(), WindowAction::Maximize))
        );
        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            false
        );
        assert!(!app.maximized_surfaces.contains("sample"));
        assert_eq!(app.document.layout, original_layout);
        assert_eq!(
            app.transition
                .as_ref()
                .and_then(|transition| transition.from_placements.get("sample")),
            Some(&[0.0, 0.0, 1.0, 1.0])
        );

        // A widget that never asked for the quiet strip is still closable: the
        // dots are the only affordance a person at the wall has, so a surface
        // must not lose them by default.
        let plain = canvas_widgets::WidgetSpec::from_json(
            br#"{"id":"cpu","kind":"stat","label":"CPU","value":33}"#,
        )
        .unwrap();
        app.document.surfaces[0].content = SurfaceContent::Widget { spec: plain };
        app.decoded.clear();
        app.ensure_scene_decoded();
        app.cursor = Some(LogicalPoint {
            x: left + 8.0,
            y: top + 10.0,
        });
        assert_eq!(
            app.widget_chrome("sample"),
            Some(canvas_widgets::WidgetChrome::Standard)
        );
        assert_eq!(
            app.hit_test_surface_control(),
            Some(("sample".into(), WindowAction::Close))
        );

        // Drag-only is opt-in, and hit testing matches the visible controls:
        // no dots drawn, nothing to click.
        let spec = canvas_widgets::WidgetSpec::from_json(
            br#"{"id":"cpu","kind":"stat","label":"CPU","value":33,"chrome":"drag_only"}"#,
        )
        .unwrap();
        app.document.surfaces[0].content = SurfaceContent::Widget { spec };
        app.decoded.clear();
        app.ensure_scene_decoded();
        assert_eq!(
            app.widget_chrome("sample"),
            Some(canvas_widgets::WidgetChrome::DragOnly)
        );
        assert_eq!(app.hit_test_surface_control(), None);
        assert_eq!(app.interaction_surfaces()[0].surface_id, "sample");

        // Instruments choose frameless chrome when the spec leaves the choice
        // on auto, while an explicit standard override remains available to
        // Lua and JSON authors.
        let clock = canvas_widgets::WidgetSpec::from_json(
            br#"{"id":"clock","kind":"clock","mode":"analog"}"#,
        )
        .unwrap();
        app.document.surfaces[0].content = SurfaceContent::Widget { spec: clock };
        app.decoded.clear();
        app.ensure_scene_decoded();
        assert_eq!(
            app.widget_chrome("sample"),
            Some(canvas_widgets::WidgetChrome::Frameless)
        );
        assert_eq!(app.hit_test_surface_control(), None);
    }

    #[test]
    fn surface_title_modes_cover_hidden_focused_and_always() {
        assert!(!surface_title_visible(SurfaceTitleMode::Hidden, false));
        assert!(!surface_title_visible(SurfaceTitleMode::Hidden, true));
        assert!(surface_title_visible(SurfaceTitleMode::Presentation, false));
        assert!(surface_title_visible(SurfaceTitleMode::Presentation, true));
        assert!(!surface_title_visible(SurfaceTitleMode::Focused, false));
        assert!(surface_title_visible(SurfaceTitleMode::Focused, true));
        assert!(surface_title_visible(SurfaceTitleMode::Always, false));
        assert!(surface_title_visible(SurfaceTitleMode::Always, true));
    }

    #[test]
    fn chart_layout_helpers_keep_legends_and_ticks_readable() {
        let wide = legend_layout(["Revenue", "Cost"].into_iter(), 800.0);
        assert_eq!(wide.rows, 1);
        let narrow = legend_layout(["Revenue", "Cost", "Forecast"].into_iter(), 110.0);
        assert!(narrow.rows > 1);
        assert_eq!(narrow.items.len(), 3);

        let ticks = chart_ticks(-10.0, 10.0, 5);
        assert_eq!(ticks, vec![-10.0, -5.0, 0.0, 5.0, 10.0]);
    }

    #[test]
    fn workspace_spacing_round_trips_and_reveals_background() {
        let frame = Rect {
            x: 3_840,
            y: 0,
            width: 1_000,
            height: 800,
        };
        let chrome = SurfaceChrome {
            title: SurfaceTitleMode::Focused,
            canvas_padding: 12,
            surface_gap: 8,
        };
        let left = NormalizedRect {
            x: 0.0,
            y: 0.0,
            width: 0.5,
            height: 1.0,
        };
        let right = NormalizedRect { x: 0.5, ..left };
        let left_visible = visible_surface_bounds(left, frame, chrome);
        let right_visible = visible_surface_bounds(right, frame, chrome);
        assert_eq!(left_visible.x, 16.0);
        assert_eq!(left_visible.y, 16.0);
        assert_eq!(right_visible.x - (left_visible.x + left_visible.width), 8.0);
        assert_eq!(
            normalize_visible_surface_bounds(left_visible, frame, chrome),
            left
        );

        let parked = LogicalRect {
            x: -200.0,
            ..left_visible
        };
        let normalized = normalize_visible_surface_bounds(parked, frame, chrome);
        assert!(normalized.x < 0.0);
        assert_eq!(normalized.width, left.width);
        let visible = visible_surface_bounds(normalized, frame, chrome);
        assert!((visible.x - parked.x).abs() < 0.001);
        assert!((visible.width - parked.width).abs() < 0.001);
    }

    #[test]
    fn interactive_snapping_uses_the_authored_layout_viewport() {
        let frame = Rect {
            x: 0,
            y: 0,
            width: 1_000,
            height: 800,
        };
        let chrome = SurfaceChrome {
            title: SurfaceTitleMode::Focused,
            canvas_padding: 12,
            surface_gap: 8,
        };
        let authored = visible_surface_bounds(
            NormalizedRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            frame,
            chrome,
        );
        assert_eq!(interaction_workspace_bounds(frame, chrome), authored);
        assert_eq!(authored.x, 16.0);
        assert_eq!(authored.y, 16.0);
        assert_eq!(authored.width, 968.0);
        assert_eq!(authored.height, 768.0);
    }

    #[test]
    fn alfalfa_rosters_fold_into_the_catalog_with_case_preserving_ids() {
        let mut catalog = Catalog::parse(
            r#"{"schema_version":1,"projects":[{"id":"dock","name":"Dock",
            "theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]}]}"#,
        )
        .unwrap();
        // An empty projects array must fall through to repos, not "succeed".
        assert!(!merge_projects_body(&mut catalog, "[]"));
        assert!(merge_projects_body(
            &mut catalog,
            r#"[{"project":{"id":"JARVIS","name":"JARVIS"}}]"#
        ));
        assert_eq!(catalog.projects.len(), 2);
        merge_repos_body(&mut catalog, r#"[{"id":"JARVIS"},{"id":"LibreChat"}]"#);
        assert_eq!(catalog.projects.len(), 3, "existing JARVIS not duplicated");
        assert!(catalog.projects.iter().any(|p| p.id == "LibreChat"));
    }

    #[test]
    fn project_workspace_switch_saves_outgoing_state_and_restores_it() {
        let mut app = test_app();
        let wrong = execute(
            &mut app,
            "save-wrong-label",
            Command::WorkspaceSave {
                name: "studio".into(),
            },
        );
        assert!(matches!(wrong.outcome, Outcome::Error { .. }));

        let dock = app.document.project_id.clone();
        let saved = execute(
            &mut app,
            "save-dock",
            Command::WorkspaceSave { name: dock.clone() },
        );
        assert!(matches!(saved.outcome, Outcome::Ok { .. }));
        app.active_project_workspace = None;
        app.document.chrome.canvas_padding = 10;
        drop(execute(
            &mut app,
            "recall-dock-after-bootstrap",
            Command::WorkspaceSwitch { name: dock.clone() },
        ));
        assert_eq!(app.document.chrome.canvas_padding, 0);

        let studio = WorkspaceDocument {
            pages: Vec::new(),
            active_page: 0,
            schema_version: SCHEMA_VERSION,
            id: "studio-workspace".into(),
            project_id: "studio".into(),
            revision: 1,
            theme: ThemeRef::Project,
            background: Background::default(),
            chrome: SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: Layout::Empty,
            bindings: Vec::new(),
            focused_surface_id: None,
        };
        app.workspaces.upsert(studio);
        workspace_store::save(&app.workspace_store_path, &app.workspaces, &app.catalog).unwrap();
        app.document.chrome.canvas_padding = 21;

        let switched = execute(
            &mut app,
            "switch-studio",
            Command::WorkspaceSwitch {
                name: "studio".into(),
            },
        );
        assert!(matches!(switched.outcome, Outcome::Ok { .. }));
        assert_eq!(app.document.project_id, "studio");
        assert_eq!(
            workspace_store::load(&app.workspace_store_path, &app.catalog)
                .unwrap()
                .0
                .get(&dock)
                .unwrap()
                .chrome
                .canvas_padding,
            21
        );

        drop(execute(
            &mut app,
            "switch-dock",
            Command::WorkspaceSwitch { name: dock },
        ));
        assert_eq!(app.document.project_id, "dock");
        assert_eq!(app.document.chrome.canvas_padding, 21);
        if let Some(parent) = app.workspace_store_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn project_workspace_switch_replaces_same_named_live_image_source() {
        fn live_document(project_id: &str, source_id: &str) -> WorkspaceDocument {
            WorkspaceDocument {
                pages: Vec::new(),
                active_page: 0,
                schema_version: SCHEMA_VERSION,
                id: format!("{project_id}-workspace"),
                project_id: project_id.into(),
                revision: 1,
                theme: ThemeRef::Project,
                background: Background::default(),
                chrome: SurfaceChrome::default(),
                surfaces: vec![Surface {
                    id: "project-html".into(),
                    title: "Dashboard".into(),
                    content: SurfaceContent::LiveImage {
                        source_id: source_id.into(),
                    },
                    inputs: Vec::new(),
                }],
                layout: Layout::Surface {
                    surface_id: "project-html".into(),
                },
                bindings: Vec::new(),
                focused_surface_id: None,
            }
        }

        let mut app = test_app();
        let outgoing = app.document.project_id.clone();
        let incoming = app
            .catalog
            .projects
            .iter()
            .find(|project| project.id != outgoing)
            .expect("second bundled project")
            .id
            .clone();
        app.document = live_document(&outgoing, "ohm-dashboard-html");
        app.active_project_workspace = Some(outgoing.clone());
        app.ensure_scene_decoded();
        app.workspaces.upsert(app.document.clone());
        app.workspaces
            .upsert(live_document(&incoming, "other-dashboard-html"));

        let switched = execute(
            &mut app,
            "switch-live-dashboard",
            Command::WorkspaceSwitch { name: incoming },
        );
        assert!(matches!(switched.outcome, Outcome::Ok { .. }));
        let ContentSource::LiveImage { source_id, .. } =
            &app.decoded.get("project-html").unwrap().source
        else {
            panic!("live dashboard")
        };
        assert_eq!(source_id, "other-dashboard-html");
        if let Some(parent) = app.workspace_store_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn applying_a_document_persists_it_for_the_next_project_round_trip() {
        let mut app = test_app();
        let project = app.document.project_id.clone();
        let mut edited = app.document.clone();
        edited.chrome.canvas_padding = 17;

        let reply = execute(
            &mut app,
            "apply-durable-workspace",
            Command::ApplyDocument { document: edited },
        );
        assert!(matches!(reply.outcome, Outcome::Ok { .. }));
        assert_eq!(
            app.active_project_workspace.as_deref(),
            Some(project.as_str())
        );
        let stored = workspace_store::load(&app.workspace_store_path, &app.catalog)
            .expect("saved workspace store")
            .0;
        assert_eq!(stored.get(&project).unwrap().chrome.canvas_padding, 17);

        if let Some(parent) = app.workspace_store_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[cfg(feature = "lua-scenes")]
    #[test]
    fn workspace_switch_hydrates_from_retained_owner_snapshots() {
        let mut app = test_app();
        app.workspaces
            .sync(
                vec![::canvas::control::ProjectWorkspaceSpec {
                    project_id: "fab".into(),
                    name: "Fab".into(),
                    status: "active".into(),
                    theme: canvas_core::ThemeSelection::Coherence {
                        mode: canvas_core::ThemeMode::Dark,
                        accent: canvas_core::Accent::Info,
                    },
                }],
                &app.catalog,
            )
            .unwrap();
        app.catalog = app.workspaces.effective_catalog(&app.catalog).unwrap();
        app.stream_inbox
            .push(::canvas::stream_binding::StreamEnvelope {
                schema_version: ::canvas::stream_binding::STREAM_SCHEMA_VERSION,
                owner: canvas_fab::OWNER.into(),
                stream: canvas_fab::METRICS_STREAM.into(),
                contract: "canvas.widget.snapshot.v1".into(),
                sequence: 42,
                emitted_at_ms: ::canvas::stream_binding::now_ms(),
                project_id: Some("fab".into()),
                payload: serde_json::json!({
                    "totals":{"projects":3,"recentRuns":420,"activeWebsites":0,"failingWebsites":0},
                    "worker":{"concurrency":4,"cpusPerRun":2}
                }),
            });

        let switched = execute(
            &mut app,
            "switch-fab-from-cache",
            Command::WorkspaceSwitch { name: "fab".into() },
        );

        assert!(matches!(switched.outcome, Outcome::Ok { .. }));
        let ContentSource::Widget { spec } = &app.decoded["fab-metrics"].source else {
            panic!("Fab metrics widget")
        };
        assert_eq!(spec.data.state, canvas_widgets::WidgetDataState::Ready);
        let canvas_widgets::WidgetKind::StatusList { nodes, .. } = &spec.kind else {
            panic!("Fab metrics status list")
        };
        assert_eq!(nodes[0].detail.as_deref(), Some("3"));
    }

    fn live_plan() -> MediaPlan {
        MediaPlan {
            viewport_logical: LogicalSize {
                width: 100.0,
                height: 100.0,
            },
            source_dimensions: SourceDimensions::RasterPixels(PixelSize {
                width: 1,
                height: 1,
            }),
            target_px: PixelSize {
                width: 1,
                height: 1,
            },
            display_logical: LogicalSize {
                width: 100.0,
                height: 100.0,
            },
            scale_factor: 1.0,
            cache_key: MediaCacheKey {
                source_id: "live:test".into(),
                variant: MediaVariant::Raster,
                pixel_size: PixelSize {
                    width: 1,
                    height: 1,
                },
            },
        }
    }

    fn live_surface(path: PathBuf) -> RuntimeSurface {
        RuntimeSurface {
            snapshot: SurfaceSnapshot {
                id: "cam".into(),
                title: "Cam".into(),
                content: ContentKind::Image,
                asset_sha256: None,
                page_index: None,
                page_count: None,
                source_pixels: None,
                rendered_pixels: None,
                display_logical: None,
                scale_factor: None,
            },
            source: ContentSource::LiveImage {
                source_id: "cam".into(),
                path,
                seen: None,
            },
            handle: iced::widget::image::Handle::from_rgba(1, 1, vec![0, 0, 0, 0]),
            source_pixels: [1, 1],
            media_plan: live_plan(),
            upload_pixels: [1, 1],
        }
    }

    fn live_ready(handle: iced::widget::image::Handle, sha: Option<String>) -> PreparedFrame {
        let plan = live_plan();
        let upload = [plan.target_px.width, plan.target_px.height];
        PreparedFrame {
            handle,
            source_pixels: [1, 1],
            media_plan: plan,
            upload_pixels: upload,
            sha256: sha,
        }
    }

    #[test]
    fn live_frames_claim_once_and_apply_supersedes_to_newest() {
        let dir = std::env::temp_dir().join(format!("canvas-live-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cam.jpg");
        let mut app = test_app();
        let stamp1 = {
            let img: image::RgbImage = image::RgbImage::from_pixel(4, 4, image::Rgb([9, 9, 9]));
            img.save(&path).unwrap();
            let meta = std::fs::metadata(&path).unwrap();
            app.decoded.insert("cam".into(), live_surface(path.clone()));
            drop(app.reload_live_images());
            let stamp = app.live_pending.get("cam").copied().unwrap();
            assert_eq!(stamp.1, meta.len());
            // A second pass must not spawn a duplicate decode for the in-flight
            // claim — that is the difference between 2 and 200 decodes.
            drop(app.reload_live_images());
            assert_eq!(app.live_pending.len(), 1);
            stamp
        };
        // A newer frame lands while the first is still decoding.
        let stamp2 = {
            let img: image::RgbImage =
                image::RgbImage::from_pixel(64, 64, image::Rgb([200, 10, 10]));
            img.save_with_format(&path, image::ImageFormat::Jpeg)
                .unwrap();
            drop(app.reload_live_images());
            assert_eq!(
                app.live_pending.get("cam").copied(),
                Some(stamp1),
                "the superseding frame waits for the in-flight claim to resolve"
            );
            let meta = std::fs::metadata(&path).unwrap();
            (meta.modified().unwrap(), meta.len())
        };
        // The first result applies, clearing the claim; the next pass then
        // claims the newest bytes.
        app.apply_live_frame(
            "cam",
            stamp1,
            Ok(live_ready(
                iced::widget::image::Handle::from_rgba(4, 4, vec![9; 64]),
                Some("one".into()),
            )),
        );
        assert!(app.live_pending.is_empty());
        assert_eq!(
            app.decoded["cam"].snapshot.asset_sha256.as_deref(),
            Some("one")
        );
        drop(app.reload_live_images());
        assert_eq!(app.live_pending.get("cam").copied(), Some(stamp2));
        // A stale result (its stamp no longer claimed) must not regress the
        // wall to an older frame.
        app.apply_live_frame(
            "cam",
            stamp1,
            Ok(live_ready(
                iced::widget::image::Handle::from_rgba(4, 4, vec![9; 64]),
                Some("stale".into()),
            )),
        );
        assert_eq!(
            app.decoded["cam"].snapshot.asset_sha256.as_deref(),
            Some("one")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_live_frames_log_once_not_every_tick() {
        let dir = std::env::temp_dir().join(format!("canvas-live-corrupt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cam.jpg");
        std::fs::write(&path, b"definitely not a jpeg").unwrap();
        let mut app = test_app();
        app.decoded.insert("cam".into(), live_surface(path.clone()));
        drop(app.reload_live_images());
        let stamp = app.live_pending.get("cam").copied().unwrap();
        app.apply_live_frame("cam", stamp, Err("decode failed".into()));
        let events_after = app.events.len();
        // Same bytes, same refusal: no re-claim and no event storm until the
        // producer writes something new.
        drop(app.reload_live_images());
        assert!(app.live_pending.is_empty());
        assert_eq!(app.events.len(), events_after);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn widget_surface_survives_workspace_store_and_runtime_rehydration() {
        let mut app = test_app();
        let spec = canvas_widgets::WidgetSpec::from_json(
            br#"{"schema_version":2,"id":"demo-meter","kind":"meter","value":0.68,"style":"vu"}"#,
        )
        .unwrap();
        app.document.surfaces.push(Surface {
            id: "widget-meter".into(),
            title: "demo-meter".into(),
            content: SurfaceContent::Widget { spec: spec.clone() },
            inputs: Vec::new(),
        });
        app.document.layout = Layout::Surface {
            surface_id: "widget-meter".into(),
        };
        app.document.revision += 1;

        let project_id = app.document.project_id.clone();
        let saved = execute(
            &mut app,
            "save-widget-workspace",
            Command::WorkspaceSave {
                name: project_id.clone(),
            },
        );
        assert!(matches!(saved.outcome, Outcome::Ok { .. }));

        let stored = workspace_store::load(&app.workspace_store_path, &app.catalog)
            .unwrap()
            .0
            .get(&project_id)
            .cloned()
            .unwrap();
        app.document = stored;
        app.decoded.clear();
        app.ensure_scene_decoded();

        let runtime = app.decoded.get("widget-meter").unwrap();
        assert_eq!(runtime.snapshot.content, ContentKind::Widget);
        assert!(matches!(
            &runtime.source,
            ContentSource::Widget { spec: restored } if restored == &spec
        ));
        if let Some(parent) = app.workspace_store_path.parent() {
            let _ = std::fs::remove_dir_all(parent);
        }
    }

    #[test]
    fn surface_page_identity_wins_over_stale_focus() {
        let layout = Layout::Surface {
            surface_id: "mesh".into(),
        };
        assert_eq!(single_surface_id(&layout, Some("daw")), Some("mesh"));
        assert_eq!(single_surface_id(&layout, None), Some("mesh"));
    }

    #[test]
    fn stack_page_uses_its_active_surface_without_focus() {
        let layout = Layout::Stack {
            surface_ids: vec!["daw".into(), "mesh".into()],
            active_surface_id: "mesh".into(),
        };
        assert_eq!(single_surface_id(&layout, None), Some("mesh"));
    }

    #[test]
    fn workspace_sync_is_idempotent_and_exposes_machine_readable_summaries() {
        let mut app = test_app();
        app.document.background = Background::Solid {
            color: [8, 16, 24, 255],
        };
        let projects = vec![::canvas::control::ProjectWorkspaceSpec {
            project_id: "gh-1288853134".into(),
            name: "ohmOS".into(),
            status: "paused".into(),
            theme: canvas_core::ThemeSelection::Coherence {
                mode: canvas_core::ThemeMode::Dark,
                accent: canvas_core::Accent::Success,
            },
        }];
        let first = execute(
            &mut app,
            "sync-projects",
            Command::WorkspaceSync {
                projects: projects.clone(),
            },
        );
        assert!(matches!(
            first.outcome,
            Outcome::Ok {
                receipt: Receipt::WorkspacesSynced { created: 1, .. }
            }
        ));
        let second = execute(
            &mut app,
            "sync-projects-again",
            Command::WorkspaceSync { projects },
        );
        assert!(matches!(
            second.outcome,
            Outcome::Ok {
                receipt: Receipt::WorkspacesSynced {
                    created: 0,
                    updated: 0,
                    ..
                }
            }
        ));
        let summary = app
            .snapshot()
            .workspace_summaries
            .into_iter()
            .find(|workspace| workspace.project_id == "gh-1288853134")
            .unwrap();
        assert_eq!(summary.name, "ohmOS");
        assert_eq!(summary.status, "paused");
        assert_eq!(summary.surface_count, 0);
        assert!(!summary.active);
        assert_eq!(
            app.workspaces.get("dock").unwrap().background,
            Background::Solid {
                color: [8, 16, 24, 255]
            }
        );
    }

    #[test]
    fn startup_restores_the_initial_projects_saved_desktop() {
        let root = std::env::temp_dir().join(format!(
            "canvas-startup-workspace-test-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let path = root.join("workspaces.json");
        let catalog = Catalog::bundled().unwrap();
        let project_id = catalog.projects[0].id.clone();
        let mut document = WorkspaceDocument {
            pages: Vec::new(),
            active_page: 0,
            schema_version: SCHEMA_VERSION,
            id: "saved-project-desktop".into(),
            project_id: project_id.clone(),
            revision: 7,
            theme: ThemeRef::Project,
            background: Background::default(),
            chrome: SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: Layout::Empty,
            bindings: Vec::new(),
            focused_surface_id: None,
        };
        document.chrome.canvas_padding = 18;
        let mut store = workspace_store::WorkspaceStore::default();
        store.upsert(document);
        workspace_store::save(&path, &store, &catalog).unwrap();

        let (_sender, receiver) = mpsc::sync_channel(1);
        let app = App::new(
            Arc::new(Mutex::new(receiver)),
            SocketGuard {
                path: root.join("missing-test-socket"),
            },
            INITIAL_FRAME,
            path,
            false,
        )
        .unwrap();

        assert_eq!(app.document.id, "saved-project-desktop");
        assert_eq!(app.document.chrome.canvas_padding, 18);
        assert_eq!(
            app.active_project_workspace.as_deref(),
            Some(project_id.as_str())
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn canvas_window_opens_maximized_with_native_chrome() {
        let frame = Rect {
            x: 3840,
            y: 0,
            width: 1920,
            height: 1200,
        };
        let settings = canvas_window_settings(frame, false);
        assert_eq!(settings.size, Size::new(1920.0, 1200.0));
        assert!(matches!(
            settings.position,
            iced::window::Position::Specific(Point { x: 3840.0, y: 0.0 })
        ));
        assert!(settings.decorations);
        assert!(settings.maximized);
        assert!(!settings.fullscreen);
        assert!(!settings.exit_on_close_request);
        assert!(settings.visible);
    }

    #[test]
    fn canvas_kiosk_window_stays_borderless_and_fullscreen() {
        let frame = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1200,
        };
        let settings = canvas_window_settings(frame, true);
        assert!(!settings.decorations);
        assert!(settings.fullscreen);
        assert!(!settings.maximized);
        assert!(!settings.exit_on_close_request);
    }

    #[test]
    fn switcher_overlay_stays_borderless_and_windowed() {
        let frame = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1200,
        };
        let settings = switcher_window_settings(frame);
        assert!(!settings.decorations);
        assert!(!settings.fullscreen);
        assert!(!settings.maximized);
    }

    #[test]
    fn kiosk_mode_requires_an_explicit_opt_in() {
        assert!(!kiosk_from_env(None));
        assert!(!kiosk_from_env(Some("")));
        assert!(!kiosk_from_env(Some("0")));
        assert!(!kiosk_from_env(Some("true")));
        assert!(kiosk_from_env(Some("1")));
        assert!(kiosk_from_env(Some(" 1 ")));
    }

    #[test]
    fn desktop_double_click_only_zooms_repeated_presses_at_one_spot() {
        let at = Instant::now();
        let here = LogicalPoint { x: 400.0, y: 300.0 };
        let near = LogicalPoint { x: 403.0, y: 301.0 };
        let far = LogicalPoint { x: 460.0, y: 300.0 };

        assert!(!double_click(None, at, here));
        assert!(double_click(Some((at, here)), at, near));
        assert!(!double_click(Some((at, here)), at, far));
        assert!(!double_click(
            Some((at, here)),
            at + DOUBLE_CLICK_INTERVAL + Duration::from_millis(1),
            here
        ));
    }

    #[test]
    fn double_clicking_the_desktop_zooms_the_host_window() {
        let mut app = test_app();
        let window = iced::window::Id::unique();
        app.windows.insert(window, INITIAL_FRAME);
        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::CursorMoved {
                position: Point::new(200.0, 200.0),
            },
        );

        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            false
        );
        assert!(app.desktop_press.is_some());
        assert_ne!(app.status, "Zoomed the Canvas window.");

        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            false
        );
        assert_eq!(app.status, "Zoomed the Canvas window.");
        assert!(app.desktop_press.is_none());
    }

    #[test]
    fn kiosk_desktop_ignores_the_windowed_zoom_gesture() {
        let mut app = test_app();
        app.kiosk = true;
        let window = iced::window::Id::unique();
        app.windows.insert(window, INITIAL_FRAME);
        let _ = app.handle_mouse(
            window,
            iced::mouse::Event::CursorMoved {
                position: Point::new(200.0, 200.0),
            },
        );
        for _ in 0..2 {
            let _ = app.handle_mouse(
                window,
                iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
            false
            );
        }
        assert_ne!(app.status, "Zoomed the Canvas window.");
        assert!(app.desktop_press.is_none());
    }

    #[test]
    fn animated_background_honors_its_frame_cap() {
        let cadence = render_cadence(
            false,
            true,
            false,
            false,
            Duration::from_millis(33),
            Duration::from_millis(100),
            Duration::from_millis(1_000),
        );

        assert_eq!(cadence, Duration::from_millis(100));
    }

    #[test]
    fn screensaver_keeps_its_own_frame_cadence() {
        let cadence = render_cadence(
            true,
            true,
            false,
            false,
            Duration::from_millis(33),
            Duration::from_millis(100),
            Duration::from_millis(1_000),
        );

        assert_eq!(cadence, Duration::from_millis(33));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn canvas_windows_stay_in_the_active_space_outside_kiosk() {
        use objc2_app_kit::NSWindowCollectionBehavior;

        let behavior = platform::canvas_collection_behavior(false);
        assert!(!behavior.contains(NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(!behavior.contains(NSWindowCollectionBehavior::Stationary));

        let kiosk = platform::canvas_collection_behavior(true);
        assert!(kiosk.contains(NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(kiosk.contains(NSWindowCollectionBehavior::Stationary));
    }

    #[test]
    fn every_resize_handle_uses_its_matching_directional_cursor() {
        for handle in [ResizeHandle::East, ResizeHandle::West] {
            assert_eq!(
                resize_cursor(handle),
                mouse::Interaction::ResizingHorizontally
            );
        }
        for handle in [ResizeHandle::North, ResizeHandle::South] {
            assert_eq!(
                resize_cursor(handle),
                mouse::Interaction::ResizingVertically
            );
        }
        for handle in [ResizeHandle::NorthEast, ResizeHandle::SouthWest] {
            assert_eq!(
                resize_cursor(handle),
                mouse::Interaction::ResizingDiagonallyUp
            );
        }
        for handle in [ResizeHandle::NorthWest, ResizeHandle::SouthEast] {
            assert_eq!(
                resize_cursor(handle),
                mouse::Interaction::ResizingDiagonallyDown
            );
        }
    }

    #[test]
    fn workspace_identity_uses_the_catalog_name_and_falls_back_to_id() {
        let catalog = Catalog::bundled().unwrap();
        let project = &catalog.projects[0];

        assert_eq!(workspace_identity_name(&catalog, &project.id), project.name);
        assert_eq!(
            workspace_identity_name(&catalog, "local-tool"),
            "local-tool"
        );
    }
}
