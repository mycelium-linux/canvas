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
    Scene, SceneNode, Surface, SurfaceChrome, SurfaceContent, SurfaceTitleMode, ThemeRef,
    WorkspaceDocument, WorkspacePatch,
};
use base64::Engine as _;
use canvas_core::display_watch::{DisplayWatch, WatchStatus};
use canvas_core::global_hotkey::{GlobalHotkey, HotkeySpec};
use canvas_core::{
    Catalog, ThemeMetrics, ThemePalette, VisualTheme, desktop::WallpaperRequest, layout::Rect,
};
use iced::mouse;
use iced::widget::canvas::{Canvas, Geometry};
use iced::widget::{
    button, canvas, column, container, image as image_widget, markdown, mouse_area, row,
    scrollable, space, stack, text, tooltip,
};
use iced::{
    Color, ContentFit, Element, Length, Point, Rectangle, Renderer, Size, Subscription, Task, Theme,
};
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

mod fpl_mark;
mod indicator;
mod presence;
use ::canvas::screensaver;
mod terminal;
mod transition;

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
const SURFACE_CONTROL_BAR_HEIGHT: f64 = 24.0;
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
    Widget {
        spec: canvas_widgets::WidgetSpec,
    },
}

#[derive(Debug, Clone)]
struct RuntimeSurface {
    snapshot: SurfaceSnapshot,
    source: ContentSource,
    handle: iced::widget::image::Handle,
    source_pixels: [u32; 2],
    media_plan: MediaPlan,
    upload_pixels: [u32; 2],
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

#[derive(Debug, Clone)]
enum Message {
    Tick,
    WindowReady(iced::window::Id),
    WindowEvent(iced::window::Id, iced::window::Event),
    MouseEvent(iced::window::Id, iced::mouse::Event),
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
        pending: Box<Pending>,
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
    TerminalInput(iced::keyboard::Event),
    ThemePickerSelect(usize),
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
    match layout {
        Layout::Canvas { placements } => placements
            .iter()
            .map(|placement| placement.surface_id.clone())
            .collect(),
        _ => Vec::new(),
    }
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
        SurfaceTitleMode::Focused => focused,
        SurfaceTitleMode::Always => true,
    }
}

struct App {
    catalog: Catalog,
    document: WorkspaceDocument,
    runtime_revision: u64,
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
    project_switcher: Option<::canvas::project_switcher::ProjectSwitcher>,
    jarvis_visible: bool,
    hud_visible: bool,
    scale_factor: f64,
    receiver: PendingReceiver,
    _socket: SocketGuard,
    #[cfg(feature = "lua-scenes")]
    stream_inbox: ::canvas::stream_binding::StreamInbox,
    #[cfg(feature = "lua-scenes")]
    _stream_socket: Option<::canvas::stream_binding::StreamSocketGuard>,
    #[cfg(feature = "lua-scenes")]
    stream_sequences: BTreeMap<String, u64>,
    /// Surface -> stamp of the frame currently being decoded off-thread.
    /// Blocks duplicate spawns and lets a stale arrival be discarded.
    live_pending: BTreeMap<String, (std::time::SystemTime, u64)>,
    /// This screen's position in the retained stream log. Held rather than
    /// implied by consumption, so a gap can be named instead of looking like a
    /// quiet producer.
    #[cfg(feature = "lua-scenes")]
    stream_position: u64,
    windows: BTreeMap<iced::window::Id, Rect>,
    decoded: BTreeMap<String, RuntimeSurface>,
    shared_views: BTreeMap<String, SharedViewDocument>,
    indicators: BTreeMap<String, StatusIndicator>,
    mesh: Option<MeshSnapshot>,
    presence: Option<PresenceSnapshot>,
    presence_received: Option<Instant>,
    presence_preview: Option<PresencePreviewVisual>,
    visible_indicator_id: Option<String>,
    indicator_started: Instant,
    metrics: hud::LocalMetrics,
    metrics_last: Instant,
    system: sysinfo::System,
    networks: sysinfo::Networks,
    status: String,
    interaction: InteractionEngine,
    snap_preview: Option<LogicalRect>,
    cursor: Option<LogicalPoint>,
    cursor_window: Option<iced::window::Id>,
    desktop_focused: bool,
    primary_pointer_down: bool,
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
    /// Content identity of runtime theme packs; changes trigger hot reload.
    theme_pack_fingerprint: String,
    theme_packs: Vec<::canvas::theme_pack::ThemePack>,
    theme_pack_ids: BTreeMap<String, String>,
    theme_pack_last_check: Instant,
}

impl App {
    fn new(
        receiver: PendingReceiver,
        socket: SocketGuard,
        frame: Rect,
        workspace_store_path: PathBuf,
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
        let mut project_id = workspaces
            .active_project_id
            .clone()
            .unwrap_or_else(|| catalog.projects[0].id.clone());
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
        for note in theme_notes { store_notes.push(format!("theme pack unusable: {note}")); }
        let theme_pack_ids = workspaces.projects.keys().filter_map(|project_id| {
            read_theme_pack_id(project_id).map(|id| (project_id.clone(), id))
        }).collect();
        let mut app = Self {
            catalog,
            document: restored_document.unwrap_or_else(|| WorkspaceDocument {
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
            }),
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
            layout_policy: ::canvas::view::LayoutPolicy::default(),
            snapped_view: None,
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
            jarvis_visible: true,
            hud_visible: false,
            scale_factor: 1.0,
            receiver,
            _socket: socket,
            #[cfg(feature = "lua-scenes")]
            stream_inbox,
            #[cfg(feature = "lua-scenes")]
            _stream_socket: stream_socket,
            #[cfg(feature = "lua-scenes")]
            stream_sequences: BTreeMap::new(),
            live_pending: BTreeMap::new(),
            #[cfg(feature = "lua-scenes")]
            stream_position: 0,
            windows: BTreeMap::new(),
            decoded: BTreeMap::new(),
            shared_views: BTreeMap::new(),
            indicators: BTreeMap::new(),
            mesh: None,
            presence: None,
            presence_received: None,
            presence_preview: None,
            visible_indicator_id: None,
            indicator_started: Instant::now(),
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
            interaction: interaction_engine(desktop_frame),
            snap_preview: None,
            cursor: None,
            cursor_window: None,
            desktop_focused: false,
            primary_pointer_down: false,
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
            theme_pack_fingerprint: ::canvas::theme_pack::fingerprint(),
            theme_packs,
            theme_pack_ids,
            theme_pack_last_check: Instant::now(),
        };
        app.publish_web_themes();
        app.ensure_scene_decoded();
        app.reset_background_animation();
        Ok(app)
    }

    /// Capture the presentation just before a workspace change so the view can
    /// interpolate from it. Call before mutating `document` or the theme.
    fn start_transition(&mut self) {
        let from_placements = match &self.document.layout {
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
        let mut visual = self.effective_theme(&self.document).visual_theme();
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
            self.document.layout = previous;
            self.bump();
            self.status = "Restored your arrangement.".into();
            self.emit_event("layout_restored".to_owned());
            return;
        }
        let Some(view_id) = self.snapped_view.clone() else {
            // Nothing has been materialized, so there is no saved arrangement to
            // snap to. Say so rather than silently doing nothing.
            self.status = "No saved view is active to snap to.".into();
            return;
        };
        match ::canvas::operational_views::materialize(&view_id, &self.document, self.view_fit()) {
            Ok(snapped) => {
                if snapped.document.layout == self.document.layout {
                    self.status = format!("Layout already matches {view_id}.");
                    return;
                }
                // Take the whole materialized document, not just its layout: the
                // saved layout references the view's surfaces, and if any were
                // closed since it was opened, a layout-only assignment would
                // reference a surface that no longer exists. materialize starts
                // from the current document, so existing surfaces are preserved.
                self.free_layout = Some(self.document.layout.clone());
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
        match message {
            Message::Tick => {
                if self.theme_pack_last_check.elapsed() >= Duration::from_secs(1) {
                    self.theme_pack_last_check = Instant::now();
                    let theme_pack_fingerprint = ::canvas::theme_pack::fingerprint();
                    if theme_pack_fingerprint != self.theme_pack_fingerprint {
                        self.theme_pack_fingerprint = theme_pack_fingerprint;
                        let (packs, notes) = ::canvas::theme_pack::load();
                        self.theme_packs = packs;
                        for note in notes { self.events.push(format!("theme_pack_invalid {note}")); }
                        self.publish_web_themes();
                        self.bump();
                    }
                }
                let clock_second = now_unix_ms() / 1_000;
                if clock_second != self.last_clock_second
                    && self.document.surfaces.iter().any(|surface| {
                        matches!(
                            surface.content,
                            SurfaceContent::Widget {
                                spec: canvas_widgets::WidgetSpec {
                                    kind: canvas_widgets::WidgetKind::Clock { .. },
                                    ..
                                }
                            }
                        )
                    })
                {
                    self.last_clock_second = clock_second;
                    self.bump();
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
                #[cfg(feature = "lua-scenes")]
                self.drain_stream_events();
                let mut tasks = vec![self.drain_commands(), self.reload_live_images()];
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
                if let Err(error) = platform::make_windows_shareable() {
                    self.status = format!("Canvas capture setup failed: {error}");
                    eprintln!("{}", self.status);
                }
                let frame = self.windows.get(&window).copied().unwrap_or(self.frame);
                iced::window::set_mode(window, iced::window::Mode::Fullscreen)
                    .chain(iced::window::move_to(
                        window,
                        Point::new(frame.x as f32, frame.y as f32),
                    ))
                    .chain(iced::window::resize(
                        window,
                        Size::new(frame.width as f32, frame.height as f32),
                    ))
                    .chain(
                        iced::window::scale_factor(window)
                            .map(move |scale| Message::ScaleFactorChanged(window, scale)),
                    )
            }
            Message::WindowEvent(window, event) => {
                let mut metrics_changed = false;
                match event {
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
                        self.interaction = interaction_engine(self.frame);
                    }
                    self.replan_rasters();
                }
                Task::none()
            }
            Message::MouseEvent(window, event) => {
                self.handle_mouse(window, event);
                Task::none()
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
            Message::TerminalInput(kbd_event) => {
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
                    use ::canvas::project_switcher::Action as SwitchAction;
                    match switcher.handle(&kbd_event, &self.catalog) {
                        SwitchAction::Consumed => {
                            self.bump();
                            return Task::none();
                        }
                        SwitchAction::Apply(project) => {
                            self.project_switcher = None;
                            return self.switch_project(project);
                        }
                        SwitchAction::Cancel => {
                            self.project_switcher = None;
                            self.status = "Kept the current project.".into();
                            self.bump();
                            return Task::none();
                        }
                        SwitchAction::Ignored => {}
                    }
                }
                if background_picker_requested(&kbd_event) {
                    return self.toggle_background_picker();
                }
                if theme_picker_requested(&kbd_event) {
                    return self.toggle_theme_picker();
                }
                if project_picker_requested(&kbd_event) {
                    return self.toggle_project_picker();
                }
                if switcher_requested(&kbd_event) {
                    return self.toggle_project_switcher();
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
                if let Some(direction) = workspace_cycle_direction(&kbd_event) {
                    return self.cycle_project_workspace(direction);
                }
                if let Some(ordinal) = display_shortcut(&kbd_event) {
                    return self.toggle_display_ordinal(ordinal);
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
                let pending = *pending;
                match *result {
                    Ok((handle, source_pixels, media_plan)) => {
                        let Some(surface) = self.decoded.get_mut(&surface_id) else {
                            self.fail(
                                pending,
                                Failure {
                                    code: ErrorCode::NotFound,
                                    message: format!("Canvas surface disappeared: {surface_id}"),
                                },
                            );
                            return Task::none();
                        };
                        let ContentSource::Pdf {
                            page_index: current,
                            page_count,
                            ..
                        } = &mut surface.source
                        else {
                            self.fail(pending, Failure::unsupported("surface is not a PDF"));
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
                    Err(failure) => self.fail(pending, failure),
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
        let now = ::canvas::stream_binding::now_ms();
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

            let previous = self.stream_sequences.get(&binding.id).copied().unwrap_or(0);
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
            let ContentSource::Widget { spec } = &mut surface.source else {
                rejected = rejected.saturating_add(matching.len() as u64);
                continue;
            };
            match ::canvas::stream_binding::apply_named_adapter_batch(adapter, &matching, spec) {
                Ok(()) => {
                    let last = matching.last().expect("non-empty stream batch");
                    self.stream_sequences
                        .insert(binding.id.clone(), last.sequence);
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
                }
            }
        }

        if applied > 0 {
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
            let (window, open) = iced::window::open(canvas_window_settings(frame));
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
                .is_ok_and(|before| before.document.layout == self.document.layout);
        if !untouched {
            return;
        }
        match ::canvas::operational_views::materialize(&view_id, &self.document, self.view_fit()) {
            Ok(fitted) => {
                if fitted.document == self.document {
                    return;
                }
                let surface_ids = placement_surface_ids(&fitted.document.layout);
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
                self.bump();
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
        let mut placements = flatten_layout(&self.document.layout);
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
        self.document.layout = Layout::Canvas { placements };
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
            None => {
                Some(::canvas::theme_picker::ThemePicker::opened_from(
                    self.effective_theme(&self.document), &self.theme_packs,
                ))
            }
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
        self.project_switcher = self.project_switcher.take().or(Some(Default::default()));
        let visible = self.project_switcher.is_some();
        self.status = if visible {
            "Switch project: type to filter, arrows to navigate, Enter to switch.".into()
        } else {
            "Kept the current project.".into()
        };
        self.bump();
        self.emit_event(format!(
            "project_switcher {}",
            if visible { "shown" } else { "hidden" }
        ));
        Task::none()
    }

    fn toggle_project_picker(&mut self) -> Task<Message> {
        self.background_picker = None;
        self.theme_picker = None;
        self.project_picker = match self.project_picker {
            Some(_) => None,
            None => {
                // LRU order for the empty-query view: what's open now leads,
                // then recent departures (store already tracks them).
                let mut recency = vec![self.document.project_id.clone()];
                for recent in &self.workspaces.recent_project_ids {
                    if !recency.contains(recent) {
                        recency.push(recent.clone());
                    }
                }
                Some(::canvas::project_picker::ProjectPicker::with_recency(
                    recency,
                ))
            }
        };
        let open = self.project_picker.is_some();
        if open {
            // Re-sync from the Alfalfa index so projects added while Canvas
            // was running appear in the picker without a restart.
            Self::merge_alfalfa_projects(&mut self.catalog);
        }
        self.status = if open {
            "Choose a project: type to filter, Enter to switch.".into()
        } else {
            "Kept the current project.".into()
        };
        self.bump();
        self.emit_event(format!(
            "project_picker {}",
            if open { "shown" } else { "hidden" }
        ));
        if open && !self.canvas_visible {
            self.set_canvas_visible(true)
        } else {
            Task::none()
        }
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
        let selection_path = PathBuf::from("themes/selections")
            .join(format!("{}.txt", self.document.project_id));
        if let Some(pack_id) = &pack_id {
            if let Err(error) = ::canvas::runtime_config::write(&selection_path, pack_id.as_bytes(), 48) {
                self.events.push(format!("theme_pack_selection_failed {error}"));
            }
            self.theme_pack_ids.insert(self.document.project_id.clone(), pack_id.clone());
        } else if let Some(root) = ::canvas::runtime_config::root() {
            let _ = std::fs::remove_file(root.join(selection_path));
            self.theme_pack_ids.remove(&self.document.project_id);
        }
        self.publish_web_themes();
        let selected = theme.to_project_theme();
        self.status = selected.map_or_else(
            || pack_id.map_or_else(|| "Theme set to imported palette.".into(), |id| format!("Theme set to {id}.")),
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
            Command::Inspect | Command::ReadDocument
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
            Command::ApplyDocument { document } => {
                if let Err(error) = document.validate(&self.catalog) {
                    self.fail(pending, Failure::invalid(error.to_string()));
                    return Task::none();
                }
                if let Err(failure) = self.ensure_background_available(&document) {
                    self.fail(pending, failure);
                    return Task::none();
                }
                let presentation_changed = self.document.id != document.id
                    || self.document.background != document.background;
                let workspace_id = document.id.clone();
                self.start_transition();
                self.document = document;
                self.prune_decoded();
                self.active_project_workspace = None;
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
                match self.document.apply_patch(&patch, &self.catalog) {
                    Ok(document) => {
                        if let Err(failure) = self.ensure_background_available(&document) {
                            self.fail(pending, failure);
                            return Task::none();
                        }
                        let background_changed = self.document.background != document.background;
                        self.start_transition();
                        self.document = document;
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
                    self.interaction = interaction_engine(frame);
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
                if let Err(error) = platform::make_windows_shareable() {
                    self.fail(pending, Failure::platform(error));
                    return Task::none();
                }
                self.bump();
                self.status = "Canvas focused.".into();
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::Focused,
                    },
                );
                Task::batch(self.windows.keys().copied().map(iced::window::gain_focus))
            }
            Command::Navigate { surface_id, target } => {
                let Some(surface) = self.decoded.get(&surface_id) else {
                    self.fail(
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
                    self.fail(pending, Failure::unsupported("surface is not navigable"));
                    return Task::none();
                };
                let next = match target {
                    Navigation::Next => page_index.saturating_add(1),
                    Navigation::Previous => page_index.saturating_sub(1),
                    Navigation::Page { index } => index,
                };
                if next >= *page_count {
                    self.fail(
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
                        pending: Box::new(pending),
                        surface_id,
                        page_index: next,
                        result: Box::new(result),
                    },
                )
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
                self.indicators.insert(indicator_id.clone(), indicator);
                self.visible_indicator_id = Some(indicator_id.clone());
                if changed {
                    self.indicator_started = Instant::now();
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
                let removed = self.indicators.remove(&indicator_id).is_some();
                if self.visible_indicator_id.as_ref() == Some(&indicator_id) {
                    self.visible_indicator_id = self.indicators.keys().next_back().cloned();
                    self.indicator_started = Instant::now();
                }
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
                        let title = format!("Widget: {}", spec.id);
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
                            let mut placements = match &self.document.layout {
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
                            self.document.layout = Layout::Canvas { placements };
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
                        let surface_ids = placement_surface_ids(&opened.document.layout);
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
                        let mut placements = match &self.document.layout {
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
                        self.document.layout = Layout::Canvas { placements };
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
        let picker_open = self.background_picker.is_some() || self.project_switcher.is_some();
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
        let local = match &self.document.layout {
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

    fn handle_mouse(&mut self, window: iced::window::Id, event: iced::mouse::Event) {
        use iced::mouse::{Button, Event};

        let viewport = self.windows.get(&window).copied().unwrap_or(self.frame);
        let offset = LogicalPoint {
            x: f64::from(viewport.x - self.frame.x),
            y: f64::from(viewport.y - self.frame.y),
        };

        if let Event::ButtonPressed(Button::Left) = &event
            && let Some((surface_id, action)) = self.hit_test_surface_control()
        {
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
            return;
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
                    self.cursor = None;
                    self.cursor_window = None;
                    let intents = self.interaction.force_release();
                    self.apply_interaction_intents(intents);
                    let preview = self.interaction.snap_preview();
                    if preview != self.snap_preview {
                        self.snap_preview = preview;
                        self.bump();
                    }
                    return;
                } else {
                    self.cursor = None;
                    self.cursor_window = None;
                    None
                }
            }
            _ => None,
        };
        let (Some(phase), Some(position)) = (phase, self.cursor) else {
            return;
        };
        let surfaces = self.interaction_surfaces();
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
                }
                self.apply_interaction_intents(intents);
                let preview = self.interaction.snap_preview();
                if preview != self.snap_preview {
                    self.snap_preview = preview;
                    self.bump();
                }
            }
            Err(error) => self.status = format!("Canvas pointer rejected: {error}"),
        }
    }

    fn interaction_surfaces(&self) -> Vec<SurfaceGeometry> {
        let Layout::Canvas { placements } = &self.document.layout else {
            return Vec::new();
        };
        placements
            .iter()
            .filter(|placement| !self.minimized_surfaces.contains(&placement.surface_id))
            .map(|placement| SurfaceGeometry {
                surface_id: placement.surface_id.clone(),
                bounds: visible_surface_bounds(
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
                    self.frame,
                    self.document.chrome,
                ),
                z_order: if self.maximized_surfaces.contains(&placement.surface_id) {
                    i64::MAX
                } else {
                    i64::from(placement.layer)
                },
            })
            .collect()
    }

    fn widget_chrome(&self, surface_id: &str) -> Option<canvas_widgets::WidgetChrome> {
        let surface = self.decoded.get(surface_id)?;
        match &surface.source {
            ContentSource::Widget { spec } => Some(spec.chrome),
            _ => None,
        }
    }

    fn hit_test_surface_control(&self) -> Option<(String, WindowAction)> {
        if self.document.chrome.title == SurfaceTitleMode::Hidden {
            return None;
        }
        let position = self.cursor?;
        let frame = self.frame;
        let Layout::Canvas { placements } = &self.document.layout else {
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
            if self.widget_chrome(&placement.surface_id)
                == Some(canvas_widgets::WidgetChrome::DragOnly)
            {
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
            let top = bounds.y;
            let bottom = top + SURFACE_CONTROL_BAR_HEIGHT;
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
            SURFACE_CONTROL_BAR_HEIGHT,
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
                        changed = true;
                    }
                }
                InteractionIntent::PromoteToFront { surface_id } => {
                    let Layout::Canvas { placements } = &mut self.document.layout else {
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
                    let Layout::Canvas { placements } = &mut self.document.layout else {
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
        self.runtime_revision = self.runtime_revision.saturating_add(1);
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
            self.document.layout = Layout::Empty;
            self.document.revision = self.document.revision.saturating_add(1);
            return;
        }
        let existing = match &self.document.layout {
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
            self.document.layout = Layout::Canvas {
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
        self.document.layout = Layout::Canvas { placements };
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
                        SurfaceContent::LiveImage { source_id } => matches!(
                            &decoded.source,
                            ContentSource::LiveImage {
                                source_id: decoded_source_id,
                                ..
                            } if decoded_source_id == source_id
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
        self.decoded.clear();
        self.shared_views.clear();
        self.live_pending.clear();
        self.stream_sequences.clear();
        #[cfg(feature = "lua-scenes")]
        {
            // A workspace switch is a new reader of retained state. Replaying
            // the tail hydrates its widgets immediately; waiting only for new
            // packets leaves a quiet project saying "Loading" indefinitely.
            self.stream_position = self.stream_inbox.replay_tail(256);
            self.drain_stream_events();
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
            let selection = self.effective_theme(document);
            let visual = selection.visual_theme();
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
            let runtime = RuntimeSurface {
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
            self.decoded.insert(surface.id.clone(), runtime);
        }
    }

    fn project_palette(&self) -> ThemePalette {
        self.effective_theme(&self.document).palette()
    }

    fn selected_theme_pack(&self, project_id: &str) -> Option<::canvas::theme_pack::ThemePack> {
        let id = self.theme_pack_ids.get(project_id)?;
        self.theme_packs.iter().find(|pack| &pack.id == id).cloned()
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

    fn media_viewport(&self) -> LogicalSize {
        LogicalSize {
            width: f64::from(self.frame.width.max(320)),
            height: f64::from(self.frame.height.max(240)),
        }
    }

    fn surface_widget<'a>(
        &self,
        surface: &'a RuntimeSurface,
        width: Length,
        height: Length,
        viewport: LogicalSize,
    ) -> Element<'a, Message> {
        match &surface.source {
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
            ContentSource::Scene { scene } => {
                let palette = self.project_palette();
                // Only text nodes are renderable in 2D; Mesh/Camera/Light/I Group
                // nodes contribute transforms and are skipped for drawing but
                // influence child text nodes through the scene graph.
                let text_nodes: Vec<usize> = scene
                    .nodes
                    .iter()
                    .enumerate()
                    .filter(|(_, n)| matches!(n.content, NodeContent::Text { .. }))
                    .map(|(index, _)| index)
                    .collect();
                // Sort text nodes by Z (depth) so farther nodes are drawn first,
                // letting closer nodes appear on top.
                let mut sorted = text_nodes;
                sorted.sort_by(|a, b| {
                    let za = scene.nodes[*a].transform.translation[2];
                    let zb = scene.nodes[*b].transform.translation[2];
                    za.total_cmp(&zb)
                });
                let children: Vec<Element<'_, Message>> = sorted
                    .iter()
                    .map(|&index| {
                        let node = &scene.nodes[index];
                        let NodeContent::Text { text: label } = &node.content else {
                            return container(text("")).into();
                        };
                        let parents = scene_parent_chain(index, &scene.nodes);
                        let (translation, _rotation, scale) =
                            scene_node_effective(index, &scene.nodes, &parents);
                        let [pos_x, pos_y] = scene_node_position(translation, viewport);
                        let z_scale = scene_node_scale(translation[2]);
                        let font_size = (scale[0] * scale[1] * 24.0 * z_scale).max(10.0);
                        // Rotation is noted but not applied in 2D text rendering;
                        // full 3D mesh rendering with vertex shaders is a future
                        // backend task (dock-cov.44). Z-based scale and depth
                        // ordering provide the visible 3D structure.
                        container(text(label).size(font_size).color(color(palette.text)))
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
                container(stack(children))
                    .width(width)
                    .height(height)
                    .into()
            }
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
                    series_colors: canvas_widgets::live::series_colors(&palette),
                    metrics: visual.metrics,
                })
                .width(width)
                .height(height)
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
                // Module widgets are resolved by the Lua host (lua_module.rs)
                // before reaching the static renderer. If a raw Module spec
                // arrives here, it means the host hasn't bound it yet — show
                // a labeled panel rather than an empty blank.
                let visual = self.presented_visual_theme();
                canvas_widgets::live::element_at(
                    spec,
                    &visual,
                    canvas_widgets::live::chrono_now_at(self.last_clock_second as i64),
                )
            }
            ContentSource::Image { .. }
            | ContentSource::Pdf { .. }
            | ContentSource::LiveImage { .. } => {
                let view = self.shared_views.get(&surface.snapshot.id);
                let mut image = image_widget(surface.handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(width)
                    .height(height);
                if let Some(view) = view {
                    image = image.crop(view_crop_pixels(view.viewport, surface.source_pixels));
                }
                let picture: Element<'_, Message> = image.into();
                if let Some(view) = view
                    && let Some(attention) = &view.attention
                {
                    let palette = self.presented_palette();
                    stack![
                        picture,
                        Canvas::new(AttentionProgram {
                            viewport: view.viewport,
                            attention: attention.region,
                            source_pixels: surface.source_pixels,
                            color: color(palette.primary),
                            label: attention.label.clone(),
                        })
                        .width(width)
                        .height(height)
                    ]
                    .into()
                } else {
                    picture
                }
            }
        }
    }

    fn surface_controls<'a>(
        &self,
        title: &'a str,
        focused: bool,
        maximized: bool,
        widget_chrome: Option<canvas_widgets::WidgetChrome>,
    ) -> Element<'a, Message> {
        if self.document.chrome.title == SurfaceTitleMode::Hidden {
            return container(space::horizontal())
                .width(Length::Fill)
                .height(Length::Fixed(0.0))
                .into();
        }
        let visual_theme = self.presented_visual_theme();
        let palette = visual_theme.palette;
        let dot = |rgb| {
            let fill = color(rgb);
            container(space::horizontal())
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
        };
        let control = |rgb, label: &'static str| {
            tooltip(
                mouse_area(
                    container(dot(rgb))
                        .width(Length::Fixed(SURFACE_CONTROL_HIT_SIZE as f32))
                        .height(Length::Fixed(SURFACE_CONTROL_BAR_HEIGHT as f32))
                        .align_x(iced::alignment::Horizontal::Left)
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
                .height(Length::Fixed(SURFACE_CONTROL_BAR_HEIGHT as f32))
                .center_x(Length::Fill)
                .align_y(iced::alignment::Vertical::Center)
                .into();
        }
        let controls = row![
            control(
                palette.danger,
                window_action_label(WindowAction::Close, maximized)
            ),
            control(
                palette.warning,
                window_action_label(WindowAction::Minimize, maximized)
            ),
            control(
                palette.success,
                window_action_label(WindowAction::Maximize, maximized)
            ),
        ]
        .spacing(0)
        .align_y(iced::alignment::Vertical::Center);
        let show_title = surface_title_visible(self.document.chrome.title, focused);
        let bar = if show_title {
            row![
                controls,
                container(text(title).size(11).color(color(palette.text_muted))).padding(
                    iced::Padding {
                        top: 0.0,
                        right: 8.0,
                        bottom: 0.0,
                        left: 8.0,
                    }
                )
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
            .height(Length::Fixed(SURFACE_CONTROL_BAR_HEIGHT as f32))
            .align_x(iced::alignment::Horizontal::Left)
            .align_y(iced::alignment::Vertical::Top)
            .style(move |_| container::Style {
                background: Some(
                    Color::from_rgba8(
                        palette.surface[0],
                        palette.surface[1],
                        palette.surface[2],
                        0.78,
                    )
                    .into(),
                ),
                border: iced::Border {
                    color: Color::from_rgba8(
                        palette.border[0],
                        palette.border[1],
                        palette.border[2],
                        0.78,
                    ),
                    width: 1.0,
                    radius: 8.0.into(),
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

    fn has_live_images(&self) -> bool {
        self.decoded
            .values()
            .any(|surface| matches!(surface.source, ContentSource::LiveImage { .. }))
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
        for (surface_id, surface) in self.decoded.iter_mut() {
            let ContentSource::LiveImage {
                source_id,
                path,
                seen,
            } = &mut surface.source
            else {
                continue;
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
            self.live_pending.insert(surface_key.clone(), stamp);
            let path = path.clone();
            let source = format!("live:{source_id}");
            tasks.push(Task::perform(
                async move {
                    let sha = hash_file(&path).ok();
                    let (handle, _media_type, source_pixels, media_plan) =
                        prepare_raster(&path, source, viewport, scale_factor)
                            .map_err(|failure| failure.message)?;
                    let upload_pixels = [media_plan.target_px.width, media_plan.target_px.height];
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
        let ContentSource::LiveImage { seen, .. } = &mut surface.source else {
            return;
        };
        if *seen != Some(stamp) {
            return;
        }
        match result {
            Ok(frame) => {
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
        let viewport = self.windows.get(&window).copied().unwrap_or(self.frame);
        let viewport_x = f64::from(viewport.x - self.frame.x);
        let viewport_y = f64::from(viewport.y - self.frame.y);
        let visual_theme = self.presented_visual_theme();
        let palette = visual_theme.palette;
        let background: iced::Background = if self.screensaver_settings.enabled {
            Color::TRANSPARENT.into()
        } else {
            match &self.document.background {
                Background::Theme => iced::Gradient::Linear(
                    iced::gradient::Linear::new(core::f32::consts::PI / 2.0)
                        .add_stop(0.0, color(palette.background))
                        .add_stop(1.0, color(palette.background_strong)),
                )
                .into(),
                Background::Solid { color } => rgba(*color).into(),
                Background::Gradient {
                    angle_degrees,
                    stops,
                } => gradient(*angle_degrees, stops).into(),
                Background::Image { .. } | Background::Shader { .. } => Color::TRANSPARENT.into(),
                // A background written by a newer Canvas. It is retained
                // verbatim, but this build cannot draw it, so fall back to the
                // theme ground rather than leaving the desktop black.
                Background::Unsupported(_) => iced::Gradient::Linear(
                    iced::gradient::Linear::new(core::f32::consts::PI / 2.0)
                        .add_stop(0.0, color(palette.background))
                        .add_stop(1.0, color(palette.background_strong)),
                )
                .into(),
            }
        };
        let surface_color = color(palette.surface);
        let border_color = color(palette.border);
        let muted = color(palette.text_muted);
        let text_color = color(palette.text);

        let content: Element<'_, Message> = match &self.document.layout {
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
                            let animated = self.presented_bounds(&placement.surface_id, target);
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
                        let control_height =
                            if self.document.chrome.title == SurfaceTitleMode::Hidden {
                                0.0
                            } else {
                                SURFACE_CONTROL_BAR_HEIGHT
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
                        let maximized = self.maximized_surfaces.contains(&placement.surface_id);
                        let chrome_hidden = self.document.chrome.title == SurfaceTitleMode::Hidden;
                        let edge = if focused && !chrome_hidden {
                            color(palette.primary)
                        } else {
                            border_color
                        };
                        let plane = container(column![
                            self.surface_controls(
                                &surface.snapshot.title,
                                focused,
                                maximized,
                                self.widget_chrome(&placement.surface_id),
                            ),
                            content,
                        ])
                        .width(Length::Fixed(bounds.width as f32))
                        .height(Length::Fixed(bounds.height as f32))
                        .style(move |_| container::Style {
                            border: iced::Border {
                                color: Color {
                                    a: if chrome_hidden {
                                        0.52
                                    } else if focused {
                                        0.88
                                    } else {
                                        0.62
                                    },
                                    ..edge
                                },
                                width: if focused && !chrome_hidden { 1.5 } else { 1.0 },
                                radius: if chrome_hidden { 6.0 } else { 9.0 }.into(),
                            },
                            shadow: iced::Shadow {
                                color: Color::from_rgba8(
                                    0,
                                    0,
                                    0,
                                    if chrome_hidden {
                                        0.12
                                    } else if focused {
                                        0.38
                                    } else {
                                        0.24
                                    },
                                ),
                                offset: iced::Vector::new(
                                    0.0,
                                    if chrome_hidden {
                                        2.0
                                    } else if focused {
                                        8.0
                                    } else {
                                        5.0
                                    },
                                ),
                                blur_radius: if chrome_hidden {
                                    8.0
                                } else if focused {
                                    24.0
                                } else {
                                    16.0
                                },
                            },
                            ..Default::default()
                        });
                        Some(
                            container(plane)
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
            _ => self
                .document
                .focused_surface_id
                .as_ref()
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
                        if self.document.chrome.title != SurfaceTitleMode::Hidden {
                            viewport.height =
                                (viewport.height - SURFACE_CONTROL_BAR_HEIGHT).max(1.0);
                        }
                        let content =
                            self.surface_widget(surface, Length::Fill, Length::Fill, viewport);
                        container(column![
                            self.surface_controls(
                                &surface.snapshot.title,
                                true,
                                true,
                                self.widget_chrome(&surface.snapshot.id),
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
            layers.push(screensaver::animated_view_in(
                self.screensaver_index,
                self.screensaver_elapsed,
                desk,
                self.screensaver_clock.interval(),
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
                Background::Shader { shader_id, .. } => layers.push(screensaver::animated_view_in(
                    screensaver::index_for(*shader_id),
                    self.screensaver_elapsed,
                    desk,
                    self.screensaver_clock.interval(),
                )),
                Background::Theme | Background::Solid { .. } | Background::Gradient { .. } => {}
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
            layers.push(picker.view(&self.catalog, &self.document.project_id));
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
            let mark = fpl_mark::view(
                color(palette.primary),
                color(visual_theme.surface_raised),
                color(palette.text),
            );
            let project_label = workspace_name.to_uppercase();
            let identity = row![
                container(mark).padding([0, 2]),
                column![
                    text("WORKSPACE").size(8).color(Color { a: 0.56, ..muted }),
                    text(project_label).size(12).color(Color {
                        a: 0.92,
                        ..text_color
                    })
                ]
                .spacing(1),
                text("⌘K").size(9).color(Color { a: 0.42, ..muted })
            ]
            .spacing(11)
            .align_y(iced::Alignment::Center);
            let signature = container(identity)
                .padding([8, 11])
                .style(move |_| container::Style {
                    background: Some(
                        Color {
                            a: 0.92,
                            ..color(visual_theme.surface_raised)
                        }
                        .into(),
                    ),
                    border: iced::Border {
                        color: Color {
                            a: 0.58,
                            ..border_color
                        },
                        width: 1.0,
                        radius: 7.0.into(),
                    },
                    shadow: iced::Shadow {
                        color: Color::from_rgba8(0, 0, 0, visual_theme.shadow_opacity * 0.45),
                        offset: iced::Vector::new(0.0, 3.0),
                        blur_radius: 10.0,
                    },
                    ..Default::default()
                });
            layers.push(
                container(signature)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .padding(18)
                    .align_x(iced::alignment::Horizontal::Left)
                    .align_y(iced::alignment::Vertical::Bottom)
                    .into(),
            );
        }

        if let Some(active) = self
            .visible_indicator_id
            .as_ref()
            .and_then(|id| self.indicators.get(id))
        {
            layers.push(indicator::view(
                active,
                self.indicator_started.elapsed(),
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

        mouse_area(
            container(stack(layers))
                .width(Length::Fill)
                .height(Length::Fill)
                .style(move |_| container::Style {
                    background: Some(background),
                    ..Default::default()
                }),
        )
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
         Ctrl-P                 Find project\n\
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

fn key_to_bytes(event: &iced::keyboard::Event) -> Vec<u8> {
    use iced::keyboard::Event;
    match event {
        Event::KeyPressed { text, key, .. } => {
            if let Some(text) = text {
                return text.to_string().into_bytes();
            }
            use iced::keyboard::key::Named;
            match key {
                iced::keyboard::Key::Named(Named::Enter) => b"\r".to_vec(),
                iced::keyboard::Key::Named(Named::Backspace) => b"\x7f".to_vec(),
                iced::keyboard::Key::Named(Named::Tab) => b"\t".to_vec(),
                iced::keyboard::Key::Named(Named::ArrowUp) => b"\x1b[A".to_vec(),
                iced::keyboard::Key::Named(Named::ArrowDown) => b"\x1b[B".to_vec(),
                iced::keyboard::Key::Named(Named::ArrowRight) => b"\x1b[C".to_vec(),
                iced::keyboard::Key::Named(Named::ArrowLeft) => b"\x1b[D".to_vec(),
                iced::keyboard::Key::Named(Named::Escape) => b"\x1b".to_vec(),
                iced::keyboard::Key::Named(Named::Delete) => b"\x1b[3~".to_vec(),
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

fn workspace_cycle_direction(event: &iced::keyboard::Event) -> Option<i32> {
    use iced::keyboard::{Event, Key, key::Named};
    let Event::KeyPressed { key, modifiers, .. } = event else {
        return None;
    };
    if !modifiers.control() || !modifiers.shift() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key {
        Key::Named(Named::ArrowUp) => Some(-1),
        Key::Named(Named::ArrowDown) => Some(1),
        _ => None,
    }
}

fn display_shortcut(event: &iced::keyboard::Event) -> Option<usize> {
    use iced::keyboard::{Event, Key};
    let Event::KeyPressed {
        key,
        modifiers,
        repeat: false,
        ..
    } = event
    else {
        return None;
    };
    if !modifiers.control() || modifiers.alt() || modifiers.logo() {
        return None;
    }
    match key.as_ref() {
        Key::Character("0") => Some(0),
        Key::Character("1") => Some(1),
        Key::Character("2") => Some(2),
        Key::Character("3") => Some(3),
        _ => None,
    }
}

fn shortcut_help_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::Event;
    matches!(
        event,
        Event::KeyPressed {
            text: Some(text),
            modifiers,
            repeat: false,
            ..
        } if text.as_str() == "?"
            && !modifiers.control()
            && !modifiers.alt()
            && !modifiers.logo()
    )
}

fn background_picker_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key, key::Named};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Named(Named::F2),
            modifiers,
            repeat: false,
            ..
        } if modifiers.is_empty()
    )
}

fn diagnostics_toggle_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key, key::Named};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Named(Named::F3),
            modifiers,
            repeat: false,
            ..
        } if modifiers.is_empty()
    )
}

fn theme_picker_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key, key::Named};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Named(Named::F4),
            modifiers,
            repeat: false,
            ..
        } if modifiers.is_empty()
    )
}

fn project_picker_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key};
    // Key::Character carries a SmolStr, so the literal is compared rather than
    // pattern-matched.
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Character(character),
            modifiers,
            repeat: false,
            ..
        } if character == "p" && modifiers.control() && !modifiers.alt() && !modifiers.logo()
    )
}

fn switcher_requested(event: &iced::keyboard::Event) -> bool {
    use iced::keyboard::{Event, Key};
    matches!(
        event,
        Event::KeyPressed {
            key: Key::Character(character),
            modifiers,
            repeat: false,
            ..
        } if character == "p"
            && modifiers.control()
            && modifiers.shift()
            && !modifiers.alt()
            && !modifiers.logo()
    )
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

fn interaction_engine(frame: Rect) -> InteractionEngine {
    InteractionEngine::new(InteractionConfig {
        workspace_bounds: LogicalRect {
            x: 0.0,
            y: 0.0,
            width: f64::from(frame.width.max(1)),
            height: f64::from(frame.height.max(1)),
        },
        minimum_surface_size: LogicalPoint { x: 160.0, y: 120.0 },
        resize_handle_size: SURFACE_RESIZE_HANDLE_SIZE,
        top_bar_height: SURFACE_CONTROL_BAR_HEIGHT,
        snap_distance: 12.0,
        snap_zone_trigger: 44.0,
        grid_size: Some(8.0),
    })
    .expect("Canvas interaction defaults must remain valid")
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

fn view_crop_pixels(viewport: ViewRect, source: [u32; 2]) -> iced::Rectangle<u32> {
    let width = source[0].max(1);
    let height = source[1].max(1);
    let x = (viewport.x * width as f32).floor().max(0.0) as u32;
    let y = (viewport.y * height as f32).floor().max(0.0) as u32;
    let right = ((viewport.x + viewport.width) * width as f32)
        .ceil()
        .max((x + 1) as f32) as u32;
    let bottom = ((viewport.y + viewport.height) * height as f32)
        .ceil()
        .max((y + 1) as f32) as u32;
    iced::Rectangle {
        x: x.min(width - 1),
        y: y.min(height - 1),
        width: right.min(width).saturating_sub(x).max(1),
        height: bottom.min(height).saturating_sub(y).max(1),
    }
}

fn attention_geometry(
    canvas: Size,
    source: [u32; 2],
    viewport: ViewRect,
    attention: ViewRect,
) -> Option<Rectangle> {
    let left = attention.x.max(viewport.x);
    let top = attention.y.max(viewport.y);
    let right = (attention.x + attention.width).min(viewport.x + viewport.width);
    let bottom = (attention.y + attention.height).min(viewport.y + viewport.height);
    if right <= left || bottom <= top {
        return None;
    }

    let crop_width = source[0].max(1) as f32 * viewport.width;
    let crop_height = source[1].max(1) as f32 * viewport.height;
    let scale = (canvas.width / crop_width).min(canvas.height / crop_height);
    let drawn_width = crop_width * scale;
    let drawn_height = crop_height * scale;
    let offset_x = (canvas.width - drawn_width) / 2.0;
    let offset_y = (canvas.height - drawn_height) / 2.0;
    Some(Rectangle {
        x: offset_x + ((left - viewport.x) / viewport.width) * drawn_width,
        y: offset_y + ((top - viewport.y) / viewport.height) * drawn_height,
        width: ((right - left) / viewport.width) * drawn_width,
        height: ((bottom - top) / viewport.height) * drawn_height,
    })
}

struct AttentionProgram {
    viewport: ViewRect,
    attention: ViewRect,
    source_pixels: [u32; 2],
    color: Color,
    label: Option<String>,
}

impl<Message> canvas::Program<Message> for AttentionProgram {
    type State = canvas_widgets::live::CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let key = canvas_widgets::live::Key::new()
            .add_f32(self.viewport.x)
            .add_f32(self.viewport.y)
            .add_f32(self.viewport.width)
            .add_f32(self.viewport.height)
            .add_f32(self.attention.x)
            .add_f32(self.attention.y)
            .add_f32(self.attention.width)
            .add_f32(self.attention.height)
            .add_u64(self.source_pixels[0] as u64)
            .add_u64(self.source_pixels[1] as u64)
            .add_color(self.color)
            .add_str(self.label.as_deref().unwrap_or(""))
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let Some(region) = attention_geometry(
                bounds.size(),
                self.source_pixels,
                self.viewport,
                self.attention,
            ) else {
                return;
            };
            let path = canvas::Path::rectangle(region.position(), region.size());
            frame.fill(
                &path,
                Color {
                    a: 0.08,
                    ..self.color
                },
            );
            frame.stroke(
                &path,
                canvas::Stroke::default().with_width(2.0).with_color(Color {
                    a: 0.95,
                    ..self.color
                }),
            );
            if let Some(label) = &self.label {
                chart_text(
                    frame,
                    label,
                    Point::new(region.x + 5.0, (region.y - 18.0).max(4.0)),
                    12.0,
                    self.color,
                    iced::alignment::Horizontal::Left,
                );
            }
        })
    }
}

struct ChartProgram {
    chart: ChartDocument,
    axis_color: Color,
    muted_color: Color,
    surface_color: Color,
    series_colors: Vec<Color>,
    /// The chart draws its own labels, so it has to be handed the type scale
    /// rather than inheriting one from a widget it does not use.
    metrics: ThemeMetrics,
}

impl<Message> canvas::Program<Message> for ChartProgram {
    type State = canvas_widgets::live::CacheState;

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let total_points: usize = self.chart.series.iter().map(|s| s.points.len()).sum();
        let val_sum: f32 = self
            .chart
            .series
            .iter()
            .flat_map(|s| &s.points)
            .map(|p| p.y)
            .sum();
        let key = canvas_widgets::live::Key::new()
            .add_usize(total_points)
            .add_f32(val_sum)
            .add_usize(self.chart.kind as usize)
            .add_str(&self.chart.title)
            .add_f32(bounds.width)
            .add_f32(bounds.height)
            .build();
        state.draw_cached(renderer, bounds.size(), key, |frame| {
            let Some((min_x, max_x, min_y, max_y)) = chart_extents(&self.chart) else {
                chart_text(
                    frame,
                    "No chart data",
                    Point::new(20.0, 24.0),
                    self.metrics.body,
                    self.muted_color,
                    iced::alignment::Horizontal::Left,
                );
                return;
            };

            chart_text(
                frame,
                &self.chart.title,
                Point::new(18.0, 12.0),
                self.metrics.body,
                self.axis_color,
                iced::alignment::Horizontal::Left,
            );
            let legend = legend_layout(
                self.chart.series.iter().map(|series| series.name.as_str()),
                bounds.width.max(1.0),
            );
            for (index, item) in legend.items.iter().enumerate() {
                let color = self.series_colors[index % self.series_colors.len()];
                frame.fill(
                    &canvas::Path::circle(Point::new(item.x + 4.0, item.y + 6.0), 3.5),
                    color,
                );
                chart_text(
                    frame,
                    &item.label,
                    Point::new(item.x + 13.0, item.y),
                    self.metrics.micro,
                    self.muted_color,
                    iced::alignment::Horizontal::Left,
                );
            }

            let left = if bounds.width < 360.0 { 44.0 } else { 58.0 };
            let right = (bounds.width - 18.0).max(left + 1.0);
            let top = (48.0 + legend.rows as f32 * 18.0).min(bounds.height * 0.45);
            let bottom = (bounds.height - 36.0).max(top + 1.0);
            let plot_width = right - left;
            let plot_height = bottom - top;
            if plot_width <= 1.0 || plot_height <= 1.0 {
                return;
            }

            let x_ticks = chart_ticks(min_x, max_x, if bounds.width < 420.0 { 3 } else { 5 });
            let y_ticks = chart_ticks(min_y, max_y, if bounds.height < 300.0 { 3 } else { 5 });
            let sx = |x: f32| left + (x - min_x) / (max_x - min_x) * plot_width;
            let sy = |y: f32| bottom - (y - min_y) / (max_y - min_y) * plot_height;
            let grid_stroke = canvas::Stroke::default().with_width(1.0).with_color(Color {
                a: 0.14,
                ..self.axis_color
            });

            for tick in &y_ticks {
                let y = sy(*tick);
                frame.stroke(
                    &canvas::Path::line(Point::new(left, y), Point::new(right, y)),
                    grid_stroke,
                );
                chart_text(
                    frame,
                    &format_chart_value(*tick),
                    Point::new(left - 7.0, y),
                    self.metrics.micro,
                    self.muted_color,
                    iced::alignment::Horizontal::Right,
                );
            }
            for tick in &x_ticks {
                let x = sx(*tick);
                frame.stroke(
                    &canvas::Path::line(Point::new(x, top), Point::new(x, bottom)),
                    grid_stroke,
                );
                chart_text(
                    frame,
                    &format_chart_value(*tick),
                    Point::new(x, bottom + 7.0),
                    self.metrics.micro,
                    self.muted_color,
                    iced::alignment::Horizontal::Center,
                );
            }

            let y_label = axis_title(&self.chart.y_axis);
            chart_text(
                frame,
                &y_label,
                Point::new(12.0, top - 20.0),
                self.metrics.micro,
                self.muted_color,
                iced::alignment::Horizontal::Left,
            );
            let x_label = axis_title(&self.chart.x_axis);
            chart_text(
                frame,
                &x_label,
                Point::new((left + right) / 2.0, bounds.height - 15.0),
                self.metrics.micro,
                self.muted_color,
                iced::alignment::Horizontal::Center,
            );

            let total_points = self
                .chart
                .series
                .iter()
                .map(|series| series.points.len())
                .sum::<usize>()
                .max(1);
            let bar_width = (plot_width / total_points as f32 * 0.72).clamp(2.0, 28.0);
            for (series_index, series) in self.chart.series.iter().enumerate() {
                let series_color = self.series_colors[series_index % self.series_colors.len()];
                match self.chart.kind {
                    ChartKind::Line => {
                        let points: Vec<Point> = series
                            .points
                            .iter()
                            .map(|point| Point::new(sx(point.x), sy(point.y)))
                            .collect();
                        draw_chart_series_line(
                            frame,
                            &points,
                            bottom,
                            series_color,
                            CHART_AREA_ALPHA / self.chart.series.len().max(1) as f32,
                        );
                    }
                    ChartKind::Scatter => {
                        for point in &series.points {
                            frame.fill(
                                &canvas::Path::circle(Point::new(sx(point.x), sy(point.y)), 3.4),
                                series_color,
                            );
                        }
                    }
                    ChartKind::Bar => {
                        let group_offset = (series_index as f32
                            - (self.chart.series.len() - 1) as f32 / 2.0)
                            * bar_width;
                        for point in &series.points {
                            let x = sx(point.x) + group_offset;
                            let y = sy(point.y);
                            let base = sy(min_y.max(0.0));
                            frame.fill(
                                &canvas::Path::rectangle(
                                    Point::new(x - bar_width * 0.42, y.min(base)),
                                    Size::new(bar_width * 0.84, (base - y).abs().max(1.0)),
                                ),
                                series_color,
                            );
                        }
                    }
                }
            }

            if let Some(cursor) = cursor.position_in(bounds)
                && cursor.x >= left
                && cursor.x <= right
                && cursor.y >= top
                && cursor.y <= bottom
                && let Some((series_index, point, position)) =
                    nearest_chart_point(&self.chart, cursor, sx, sy, 20.0)
            {
                let series = &self.chart.series[series_index];
                let series_color = self.series_colors[series_index % self.series_colors.len()];
                frame.fill(&canvas::Path::circle(position, 5.0), self.surface_color);
                frame.stroke(
                    &canvas::Path::circle(position, 5.0),
                    canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(series_color),
                );
                let value = format!(
                    "{}  {} · {}",
                    series.name,
                    format_axis_value(point.x, self.chart.x_axis.unit.as_deref()),
                    format_axis_value(point.y, self.chart.y_axis.unit.as_deref())
                );
                let tooltip_width = (value.chars().count() as f32 * 6.5 + 18.0).min(plot_width);
                let tooltip_x = (position.x + 12.0).min(right - tooltip_width);
                let tooltip_y = (position.y - 32.0).max(top + 4.0);
                frame.fill(
                    &canvas::Path::rounded_rectangle(
                        Point::new(tooltip_x, tooltip_y),
                        Size::new(tooltip_width, 24.0),
                        6.0.into(),
                    ),
                    Color {
                        a: 0.94,
                        ..self.surface_color
                    },
                );
                chart_text(
                    frame,
                    &value,
                    Point::new(tooltip_x + 9.0, tooltip_y + 5.0),
                    self.metrics.micro,
                    self.axis_color,
                    iced::alignment::Horizontal::Left,
                );
            }
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
struct LegendItem {
    label: String,
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, PartialEq)]
struct LegendLayout {
    items: Vec<LegendItem>,
    rows: usize,
}

fn legend_layout<'a>(labels: impl Iterator<Item = &'a str>, width: f32) -> LegendLayout {
    let available = (width - 36.0).max(80.0);
    let mut x = 18.0;
    let mut row = 0usize;
    let mut items = Vec::new();
    for label in labels {
        let item_width = label.chars().count() as f32 * 6.5 + 30.0;
        if x > 18.0 && x + item_width > available + 18.0 {
            row += 1;
            x = 18.0;
        }
        items.push(LegendItem {
            label: label.to_owned(),
            x,
            y: 34.0 + row as f32 * 18.0,
        });
        x += item_width;
    }
    LegendLayout {
        rows: row + usize::from(!items.is_empty()),
        items,
    }
}

fn chart_extents(chart: &ChartDocument) -> Option<(f32, f32, f32, f32)> {
    let mut min_x = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    for point in chart.series.iter().flat_map(|series| &series.points) {
        min_x = min_x.min(point.x);
        max_x = max_x.max(point.x);
        min_y = min_y.min(point.y);
        max_y = max_y.max(point.y);
    }
    if !min_x.is_finite() {
        return None;
    }
    if min_x == max_x {
        min_x -= 0.5;
        max_x += 0.5;
    }
    if min_y == max_y {
        let padding = min_y.abs().max(1.0) * 0.1;
        min_y -= padding;
        max_y += padding;
    }
    Some((min_x, max_x, min_y, max_y))
}

fn chart_ticks(min: f32, max: f32, count: usize) -> Vec<f32> {
    let count = count.max(2);
    let step = (max - min) / (count - 1) as f32;
    (0..count).map(|index| min + step * index as f32).collect()
}

fn nearest_chart_point<F, G>(
    chart: &ChartDocument,
    cursor: Point,
    sx: F,
    sy: G,
    maximum_distance: f32,
) -> Option<(usize, ::canvas::chart::ChartPoint, Point)>
where
    F: Fn(f32) -> f32,
    G: Fn(f32) -> f32,
{
    chart
        .series
        .iter()
        .enumerate()
        .flat_map(|(series_index, series)| {
            let sx = &sx;
            let sy = &sy;
            series.points.iter().map(move |point| {
                let position = Point::new(sx(point.x), sy(point.y));
                let distance = (position.x - cursor.x).hypot(position.y - cursor.y);
                (distance, series_index, point.clone(), position)
            })
        })
        .filter(|(distance, _, _, _)| *distance <= maximum_distance)
        .min_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, series_index, point, position)| (series_index, point, position))
}

fn axis_title(axis: &::canvas::chart::ChartAxis) -> String {
    axis.unit.as_ref().map_or_else(
        || axis.label.clone(),
        |unit| format!("{} ({unit})", axis.label),
    )
}

fn format_chart_value(value: f32) -> String {
    if value.abs() >= 1_000_000.0 {
        format!("{:.1}m", value / 1_000_000.0)
    } else if value.abs() >= 1_000.0 {
        format!("{:.1}k", value / 1_000.0)
    } else if value.abs() >= 10.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

fn format_axis_value(value: f32, unit: Option<&str>) -> String {
    let value = format_chart_value(value);
    unit.map_or(value.clone(), |unit| {
        if unit == "$" || unit == "£" || unit == "€" {
            format!("{unit}{value}")
        } else {
            format!("{value} {unit}")
        }
    })
}

const CHART_LINE_WIDTH: f32 = 2.0;
/// Opacity of the area under a line at the line itself, falling to nothing at
/// the axis. Divided among overlaid series so a four-series chart does not silt
/// up.
const CHART_AREA_ALPHA: f32 = 0.22;
const CHART_HEAD_RADIUS: f32 = CHART_LINE_WIDTH * 1.6;

/// Draw one series: the area beneath it, the line as a single path, and a
/// filled head on the newest point.
///
/// Every vertex used to be the meeting of two separately stroked `Path::line`
/// segments, which on a 2px butt-capped stroke leaves a visible notch at each
/// one. One path with round joins removes them, and hands the tessellator one
/// shape instead of n.
///
/// This mirrors `canvas_widgets::live`'s private `draw_series_line`; the two
/// should become one call once that crate exports it.
fn draw_chart_series_line(
    frame: &mut canvas::Frame,
    points: &[Point],
    baseline: f32,
    stroke: Color,
    area_alpha: f32,
) {
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    if points.len() < 2 {
        return;
    }
    let crest = points.iter().map(|point| point.y).fold(baseline, f32::min);
    let area = canvas::Path::new(|path| {
        path.move_to(Point::new(first.x, baseline));
        for point in points {
            path.line_to(*point);
        }
        path.line_to(Point::new(last.x, baseline));
        path.close();
    });
    frame.fill(
        &area,
        canvas::gradient::Linear::new(Point::new(first.x, crest), Point::new(first.x, baseline))
            .add_stop(
                0.0,
                Color {
                    a: area_alpha,
                    ..stroke
                },
            )
            .add_stop(1.0, Color { a: 0.0, ..stroke }),
    );
    let line = canvas::Path::new(|path| {
        path.move_to(*first);
        for point in &points[1..] {
            path.line_to(*point);
        }
    });
    frame.stroke(
        &line,
        canvas::Stroke {
            style: canvas::Style::Solid(stroke),
            width: CHART_LINE_WIDTH,
            line_cap: canvas::LineCap::Round,
            line_join: canvas::LineJoin::Round,
            ..Default::default()
        },
    );
    // The newest point is what a live series is read for; without it the eye
    // has to work out which end of the line is now.
    frame.fill(&canvas::Path::circle(*last, CHART_HEAD_RADIUS), stroke);
}

fn chart_text(
    frame: &mut canvas::Frame,
    content: &str,
    position: Point,
    size: f32,
    color: Color,
    align_x: iced::alignment::Horizontal,
) {
    frame.fill_text(canvas::Text {
        content: content.to_owned(),
        position,
        color,
        size: iced::Pixels(size),
        align_x: align_x.into(),
        ..Default::default()
    });
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
    fn visit(layout: &Layout, bounds: NormalizedRect, placements: &mut Vec<Placement>) {
        match layout {
            Layout::Empty => {}
            Layout::Surface { surface_id } => placements.push(Placement {
                surface_id: surface_id.clone(),
                bounds,
                layer: placements.len() as i32,
            }),
            Layout::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let ratio = ratio.clamp(0.0, 1.0);
                let (first_bounds, second_bounds) = match axis {
                    Axis::Horizontal => (
                        NormalizedRect {
                            width: bounds.width * ratio,
                            ..bounds
                        },
                        NormalizedRect {
                            x: bounds.x + bounds.width * ratio,
                            width: bounds.width * (1.0 - ratio),
                            ..bounds
                        },
                    ),
                    Axis::Vertical => (
                        NormalizedRect {
                            height: bounds.height * ratio,
                            ..bounds
                        },
                        NormalizedRect {
                            y: bounds.y + bounds.height * ratio,
                            height: bounds.height * (1.0 - ratio),
                            ..bounds
                        },
                    ),
                };
                visit(first, first_bounds, placements);
                visit(second, second_bounds, placements);
            }
            Layout::Stack { surface_ids, .. } => {
                for surface_id in surface_ids {
                    placements.push(Placement {
                        surface_id: surface_id.clone(),
                        bounds,
                        layer: placements.len() as i32,
                    });
                }
            }
            Layout::Canvas {
                placements: existing,
            } => placements.extend(existing.iter().cloned()),
        }
    }

    let mut placements = Vec::new();
    visit(
        layout,
        NormalizedRect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        },
        &mut placements,
    );
    placements
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
    pub(super) fn canvas_presentation_options() -> objc2_app_kit::NSApplicationPresentationOptions {
        use objc2_app_kit::NSApplicationPresentationOptions;

        NSApplicationPresentationOptions::NSApplicationPresentationAutoHideDock
            | NSApplicationPresentationOptions::NSApplicationPresentationAutoHideMenuBar
    }

    #[cfg(target_os = "macos")]
    pub fn displays() -> Result<Vec<DisplayInfo>, String> {
        match canvas_control::request(canvas_control::Operation::Displays)? {
            canvas_control::Response::Displays { displays } => Ok(displays
                .iter()
                .map(|display| DisplayInfo {
                    id: display.id.clone(),
                    name: display.name.clone(),
                    frame: canvas_frame(display),
                    primary: display.primary,
                })
                .collect()),
            _ => Err("Dock Control did not return display geometry".into()),
        }
    }

    #[cfg(target_os = "macos")]
    pub fn display_frames() -> Result<Vec<Rect>, String> {
        Ok(displays()?
            .into_iter()
            .map(|display| display.frame)
            .collect())
    }

    #[cfg(target_os = "macos")]
    pub(super) fn canvas_collection_behavior() -> objc2_app_kit::NSWindowCollectionBehavior {
        use objc2_app_kit::NSWindowCollectionBehavior;

        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
    }

    #[cfg(target_os = "macos")]
    pub fn make_windows_shareable() -> Result<(), String> {
        use objc2_app_kit::{NSApplication, NSWindowSharingType};
        use objc2_foundation::MainThreadMarker;

        let marker = MainThreadMarker::new().ok_or("AppKit window setup left the main thread")?;
        let application = NSApplication::sharedApplication(marker);
        application.setPresentationOptions(canvas_presentation_options());
        for window in application.windows().iter() {
            window.setSharingType(NSWindowSharingType::NSWindowSharingReadOnly);
            unsafe { window.setCollectionBehavior(canvas_collection_behavior()) };
        }
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    pub fn make_windows_shareable() -> Result<(), String> {
        Ok(())
    }

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
        request_id: "invalid".into(),
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

fn canvas_window_settings(frame: Rect) -> iced::window::Settings {
    iced::window::Settings {
        size: Size::new(frame.width as f32, frame.height as f32),
        position: iced::window::Position::Specific(Point::new(frame.x as f32, frame.y as f32)),
        decorations: false,
        exit_on_close_request: false,
        ..iced::window::Settings::default()
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    let initial_frame = platform::preferred_canvas_frame(INITIAL_FRAME);
    let workspace_store_path = workspace_store::default_path()?;
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
            )
            .expect("Canvas project workspace store must remain valid");
            let (window, open) = iced::window::open(canvas_window_settings(initial_frame));
            app.windows.insert(window, initial_frame);
            (app, open.map(Message::WindowReady))
        },
        App::update,
        daemon_view,
    )
    .subscription(|app| {
        let animated = app.visible_indicator_id.is_some()
            || app.presence_preview.is_some()
            || app.transition.is_some();
        let background_animated = app.document.background.is_animated();
        // Background-only animation uses the configured background_max_fps, not
        // a hardcoded 60 Hz: shaders like Star Nest and Smoke Rings are
        // low-frequency motion where 20-24 fps is indistinguishable from 60.
        let background_interval = Duration::from_secs_f64(
            1.0 / f64::from(
                canvas_core::appearance::current()
                    .motion
                    .background_max_fps
                    .max(1),
            ),
        );
        // An open picker needs frames whatever the workspace background is doing.
        let cadence = if app.screensaver_settings.enabled
            || background_animated
            || app.background_picker.is_some()
        {
            let screensaver = app.screensaver_clock.interval();
            if animated {
                screensaver.min(background_interval)
            } else {
                screensaver
            }
        } else if animated {
            background_interval
        } else {
            Duration::from_millis(canvas_core::appearance::current().motion.idle_tick_ms as u64)
        };
        // Live camera frames deserve sub-second pickup without becoming a
        // steady redraw: the reload itself only fires when bytes change.
        let cadence = if app.has_live_images() {
            cadence.min(Duration::from_millis(400))
        } else {
            cadence
        };
        Subscription::batch([
            iced::time::every(cadence).map(|_| Message::Tick),
            iced::event::listen_with(|event, _, window| match event {
                iced::Event::Window(event) => Some(Message::WindowEvent(window, event)),
                iced::Event::Mouse(event) => Some(Message::MouseEvent(window, event)),
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
        app.interaction = interaction_engine(INITIAL_FRAME);
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
                content: NodeContent::Text { text: "hi".into() },
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
    fn snapping_with_no_active_view_reports_rather_than_panicking() {
        let mut app = test_app();
        let before = app.document.layout.clone();
        app.toggle_snapped_layout();
        assert_eq!(app.document.layout, before, "layout must not change");
        assert!(
            app.status.contains("No saved view"),
            "status was {:?}",
            app.status
        );
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
    fn canvas_uses_complete_display_instead_of_reserving_system_chrome() {
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
        use objc2_app_kit::NSApplicationPresentationOptions;
        let options = platform::canvas_presentation_options();
        assert!(
            options.contains(
                NSApplicationPresentationOptions::NSApplicationPresentationAutoHideMenuBar
            )
        );
        assert!(
            options
                .contains(NSApplicationPresentationOptions::NSApplicationPresentationAutoHideDock)
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
    fn window_relative_pointer_focuses_and_promotes_on_a_nonzero_display_origin() {
        let mut app = test_app();
        app.frame.x = 3_840;
        let window = iced::window::Id::unique();
        app.windows.insert(window, app.frame);
        app.interaction = interaction_engine(app.frame);
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
        app.handle_mouse(
            window,
            iced::mouse::Event::CursorMoved {
                position: Point::new(180.0, 180.0),
            },
        );
        app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
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
        app.interaction = interaction_engine(app.frame);
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
        app.handle_mouse(
            right,
            iced::mouse::Event::CursorMoved {
                position: Point::new(200.0, 240.0),
            },
        );
        app.handle_mouse(
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
        app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
        );
        assert!(app.maximized_surfaces.contains("sample"));
        app.cursor = Some(LogicalPoint { x: 44.0, y: 10.0 });
        assert_eq!(
            app.hit_test_surface_control(),
            Some(("sample".into(), WindowAction::Maximize))
        );
        app.handle_mouse(
            window,
            iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
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
    }

    #[test]
    fn surface_title_modes_cover_hidden_focused_and_always() {
        assert!(!surface_title_visible(SurfaceTitleMode::Hidden, false));
        assert!(!surface_title_visible(SurfaceTitleMode::Hidden, true));
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
            title: "Widget: demo-meter".into(),
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
    fn canvas_window_opens_as_a_persistent_borderless_desktop() {
        let frame = Rect {
            x: 3840,
            y: 0,
            width: 1920,
            height: 1200,
        };
        let settings = canvas_window_settings(frame);
        assert_eq!(settings.size, Size::new(1920.0, 1200.0));
        assert!(matches!(
            settings.position,
            iced::window::Position::Specific(Point { x: 3840.0, y: 0.0 })
        ));
        assert!(!settings.decorations);
        assert!(!settings.exit_on_close_request);
        assert!(settings.visible);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn canvas_windows_join_every_macos_space() {
        use objc2_app_kit::NSWindowCollectionBehavior;

        let behavior = platform::canvas_collection_behavior();
        assert!(behavior.contains(NSWindowCollectionBehavior::CanJoinAllSpaces));
        assert!(behavior.contains(NSWindowCollectionBehavior::FullScreenAuxiliary));
        assert!(behavior.contains(NSWindowCollectionBehavior::Stationary));
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
