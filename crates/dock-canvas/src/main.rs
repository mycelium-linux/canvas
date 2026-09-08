use base64::Engine as _;
use dock_canvas::chart::{ChartDocument, ChartKind};
use dock_canvas::control::{
    CONTROL_VERSION, CaptureChromeSnapshot, Command, ContentKind, ControlReply, ControlRequest,
    ErrorCode, MAX_ACTIVE_INDICATORS, MAX_CONTROL_MESSAGE, MeshSnapshot, MountedViewSnapshot,
    Navigation, Outcome, OverlaySettings, OverlayTarget, PresencePreview, PresenceSnapshot,
    ProjectWorkspaceSnapshot, Receipt, RuntimeSnapshot, ScreensaverSettings, ScreensaverSnapshot,
    StatusIndicator, SurfaceSnapshot,
};
use dock_canvas::display::{
    DisplayInfo, DisplayRegion, DisplayTarget, DisplayTopology, ResolvedDisplayTarget,
};
use dock_canvas::hud;
use dock_canvas::interaction::{
    HitRegion, InteractionConfig, InteractionEngine, InteractionIntent, LogicalPoint, LogicalRect,
    PointerEvent, PointerPhase, ResizeHandle, SurfaceGeometry, hit_test,
};
use dock_canvas::media::{
    LogicalSize, MediaCacheKey, MediaLimits, MediaPlan, MediaVariant, PixelSize, SourceDimensions,
    plan_pdf_page, plan_raster,
};
use dock_canvas::view::{
    SharedViewDocument, SourcePosition, ViewNavigation, ViewOperation, ViewRect, ViewSource,
    VisualSourceKind, VisualSourceRef,
};
use dock_canvas::workspace_store::{self, WorkspaceStore};
use dock_canvas::{
    AssetRef, Axis, Background, BackgroundFit, GradientStop, Layout, NodeContent, NormalizedRect,
    PatchOperation, Placement, SCHEMA_VERSION, Scene, SceneNode, Surface, SurfaceChrome,
    SurfaceContent, SurfaceTitleMode, ThemeRef, WorkspaceDocument, WorkspacePatch,
};
use dock_core::global_hotkey::{GlobalHotkey, HotkeySpec};
use dock_core::{Catalog, ThemePalette, VisualTheme, desktop::WallpaperRequest, layout::Rect};
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

mod indicator;
mod presence;
mod screensaver;
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
const SURFACE_TOP_BAR_GRAB_WIDTH: f64 = 72.0;
const CANVAS_TOGGLE_KEYS: &str = "Ctrl-Option-C";
const JARVIS_TOGGLE_KEYS: &str = "Ctrl-Option-J";
const SHORTCUT_HELP_KEYS: &str = "Ctrl-Option-?";
const FOCUSED_HELP_KEYS: &str = "?";

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
    Widget {
        spec: dock_widgets::WidgetSpec,
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
        result: Result<dock_core::desktop::WallpaperReceipt, Failure>,
    },
    LinkOpened(String),
    TerminalInput(iced::keyboard::Event),
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
    selected_display_ids: Vec<String>,
    canvas_visible: bool,
    overlays: OverlaySettings,
    visibility_hotkey: Option<GlobalHotkey>,
    shortcuts_hotkey: Option<GlobalHotkey>,
    jarvis_hotkey: Option<GlobalHotkey>,
    shortcuts_visible: bool,
    jarvis_visible: bool,
    scale_factor: f64,
    receiver: PendingReceiver,
    _socket: SocketGuard,
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
    screensaver_fps: f32,
}

impl App {
    fn new(
        receiver: PendingReceiver,
        socket: SocketGuard,
        frame: Rect,
        workspace_store_path: PathBuf,
    ) -> Result<Self, String> {
        let base_catalog = Catalog::bundled().expect("bundled project catalog must remain valid");
        let workspaces = workspace_store::load(&workspace_store_path, &base_catalog)
            .map_err(|error| error.to_string())?;
        let catalog = workspaces
            .effective_catalog(&base_catalog)
            .map_err(|error| error.to_string())?;
        let project_id = workspaces
            .active_project_id
            .clone()
            .unwrap_or_else(|| catalog.projects[0].id.clone());
        let restored_document = workspaces.get(&project_id).cloned();
        let active_project_workspace = restored_document
            .as_ref()
            .map(|document| document.project_id.clone());
        let displays = platform::displays().unwrap_or_default();
        let display_frames = displays.iter().map(|display| display.frame).collect();
        let desktop_frame = DisplayTopology::new(displays.clone())
            .and_then(|topology| topology.resolve(&DisplayTarget::All))
            .map(|resolved| resolved.frame)
            .unwrap_or(frame);
        let selected_display_ids = displays
            .iter()
            .find(|display| display.frame == frame)
            .map(|display| vec![display.id.clone()])
            .unwrap_or_default();
        let mut shortcut_errors = Vec::new();
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
        let mut app = Self {
            catalog,
            document: restored_document.unwrap_or_else(|| WorkspaceDocument {
                schema_version: SCHEMA_VERSION,
                id: "canvas-runtime".into(),
                project_id,
                revision: 1,
                theme: ThemeRef::Project,
                background: dock_canvas::Background::default(),
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
            selected_display_ids,
            canvas_visible: true,
            overlays: OverlaySettings::default(),
            visibility_hotkey,
            shortcuts_hotkey,
            jarvis_hotkey,
            shortcuts_visible: false,
            jarvis_visible: true,
            scale_factor: 1.0,
            receiver,
            _socket: socket,
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
            events: Vec::new(),
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
            screensaver_fps: 0.0,
        };
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
    fn presented_palette(&self) -> dock_core::ThemePalette {
        let target = self.project_palette();
        self.transition
            .as_ref()
            .map(|t| t.palette(target))
            .unwrap_or(target)
    }

    fn presented_visual_theme(&self) -> VisualTheme {
        let mut visual = self
            .document
            .resolved_theme(&self.catalog)
            .expect("validated Canvas document theme")
            .visual_theme();
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

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Tick => {
                let toggle_visibility = self
                    .visibility_hotkey
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
                if self.metrics_last.elapsed() >= METRICS_REFRESH {
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
                let mut tasks = vec![self.drain_commands()];
                if toggle_visibility {
                    tasks.push(self.set_canvas_visible(!self.canvas_visible));
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
                Task::batch(tasks)
            }
            Message::WindowReady(window) => {
                if let Err(error) = platform::make_windows_shareable() {
                    self.status = format!("Canvas capture setup failed: {error}");
                    eprintln!("{}", self.status);
                }
                let frame = self.windows.get(&window).copied().unwrap_or(self.frame);
                iced::window::set_mode(window, iced::window::Mode::Windowed)
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
            Message::TerminalInput(kbd_event) => {
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
        let (reply, _replies) = mpsc::sync_channel(1);
        self.execute(Pending {
            request: ControlRequest {
                schema_version: CONTROL_VERSION,
                request_id: "key-workspace".into(),
                expected_revision: Some(self.runtime_revision),
                command: Command::WorkspaceSwitch {
                    name: projects[next].clone(),
                },
            },
            reply,
        })
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
                    next.active_project_id = Some(name.clone());
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
                    self.fail(
                        pending,
                        Failure {
                            code: ErrorCode::InvalidRequest,
                            message: format!("workspace '{name}' not found"),
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
            } => match dock_widgets::WidgetSpec::from_file(&spec_path) {
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
                            inputs: Vec::new(),
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
            },
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
                self.answer(
                    pending,
                    Outcome::Ok {
                        receipt: Receipt::ScreensaverShaderSet {
                            index,
                            name: shader.name.to_string(),
                        },
                    },
                );
                Task::none()
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
        if (!self.screensaver_settings.enabled && document_shader.is_none())
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
            capture_chrome: Some({
                let palette = self.presented_palette();
                CaptureChromeSnapshot {
                    danger: palette.danger,
                    warning: palette.warning,
                    success: palette.success,
                }
            }),
            active_indicators: self.indicators.values().cloned().collect(),
            mesh: self.mesh.clone(),
            presence: self.presence.clone(),
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
            Event::CursorLeft
                if !self.primary_pointer_down && self.cursor_window == Some(window) =>
            {
                self.cursor = None;
                self.cursor_window = None;
                None
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

    fn widget_chrome(&self, surface_id: &str) -> Option<dock_widgets::WidgetChrome> {
        let surface = self.decoded.get(surface_id)?;
        match &surface.source {
            ContentSource::Widget { spec } => Some(spec.chrome),
            _ => None,
        }
    }

    fn hit_test_surface_control(&self) -> Option<(String, WindowAction)> {
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
                == Some(dock_widgets::WidgetChrome::DragOnly)
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
        let placements = ids
            .into_iter()
            .enumerate()
            .map(|(index, surface_id)| {
                existing.get(&surface_id).cloned().unwrap_or_else(|| {
                    let (width, height) = self
                        .decoded
                        .get(&surface_id)
                        .map(|surface| {
                            (
                                (surface.media_plan.display_logical.width
                                    / f64::from(self.frame.width))
                                .clamp(0.18, 0.92) as f32,
                                (surface.media_plan.display_logical.height
                                    / f64::from(self.frame.height))
                                .clamp(0.18, 0.92) as f32,
                            )
                        })
                        .unwrap_or((0.72, 0.72));
                    let cascade = (index % 6) as f32 * 0.025;
                    Placement {
                        surface_id,
                        bounds: NormalizedRect {
                            x: ((1.0 - width) * 0.5 + cascade).min(1.0 - width),
                            y: ((1.0 - height) * 0.5 + cascade).min(1.0 - height),
                            width,
                            height,
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
                        _ => true,
                    }
            })
        });
        self.shared_views
            .retain(|id, _| self.decoded.contains_key(id));
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
        self.document
            .resolved_theme(&self.catalog)
            .expect("validated Canvas document theme")
            .palette()
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
                let text_nodes: Vec<&SceneNode> = scene
                    .nodes
                    .iter()
                    .filter(|n| matches!(n.content, NodeContent::Text { .. }))
                    .collect();
                let children: Vec<Element<'_, Message>> = text_nodes
                    .iter()
                    .map(|n| {
                        let NodeContent::Text { text: label } = &n.content else {
                            return container(text("")).into();
                        };
                        let [x, y] = scene_node_position(n.transform.translation, viewport);
                        let font_size =
                            (n.transform.scale[0] * n.transform.scale[1] * 24.0).max(10.0);
                        container(text(label).size(font_size).color(color(palette.text)))
                            .width(Length::Shrink)
                            .height(Length::Shrink)
                            .padding(iced::Padding {
                                top: y,
                                right: 0.0,
                                bottom: 0.0,
                                left: x,
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
                let palette = self.presented_palette();
                Canvas::new(ChartProgram {
                    chart: chart.clone(),
                    axis_color: color(palette.text),
                    muted_color: color(palette.text_muted),
                    surface_color: color(palette.surface),
                    series_colors: vec![
                        color(palette.primary),
                        color(palette.success),
                        color(palette.danger),
                        color(palette.warning),
                    ],
                })
                .width(width)
                .height(height)
                .into()
            }
            ContentSource::Stream { url } => {
                let palette = self.project_palette();
                container(
                    iced::widget::Column::<Message>::new()
                        .push(
                            text("Connecting to stream...")
                                .size(20)
                                .color(color(palette.text)),
                        )
                        .push(text(url.clone()).size(12).color(color(palette.text_muted))),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .into()
            }
            ContentSource::Widget { spec } => {
                let visual = self.presented_visual_theme();
                dock_widgets::live::element(spec, &visual)
            }
            ContentSource::Image { .. } | ContentSource::Pdf { .. } => {
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
        widget_chrome: Option<dock_widgets::WidgetChrome>,
    ) -> Element<'a, Message> {
        let palette = self.presented_palette();
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
        if widget_chrome == Some(dock_widgets::WidgetChrome::DragOnly) {
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
        let palette = self.presented_palette();
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
                let surfaces = placements
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
                        let content = self.surface_widget(
                            surface,
                            Length::Fill,
                            Length::Fill,
                            LogicalSize {
                                width: bounds.width.max(1.0),
                                height: (bounds.height - SURFACE_CONTROL_BAR_HEIGHT).max(1.0),
                            },
                        );
                        let focused = self.document.focused_surface_id.as_deref()
                            == Some(placement.surface_id.as_str());
                        let maximized = self.maximized_surfaces.contains(&placement.surface_id);
                        let edge = if focused {
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
                                    a: if focused { 0.88 } else { 0.62 },
                                    ..edge
                                },
                                width: if focused { 1.5 } else { 1.0 },
                                radius: 9.0.into(),
                            },
                            shadow: iced::Shadow {
                                color: Color::from_rgba8(
                                    0,
                                    0,
                                    0,
                                    if focused { 0.38 } else { 0.24 },
                                ),
                                offset: iced::Vector::new(0.0, if focused { 8.0 } else { 5.0 }),
                                blur_radius: if focused { 24.0 } else { 16.0 },
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
                        viewport.height = (viewport.height - SURFACE_CONTROL_BAR_HEIGHT).max(1.0);
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

        let mut layers: Vec<Element<'_, Message>> = Vec::new();
        if self.screensaver_settings.enabled {
            layers.push(screensaver::view(
                self.screensaver_index,
                self.screensaver_elapsed,
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
                Background::Shader { shader_id, .. } => layers.push(screensaver::view(
                    screensaver::index_for(*shader_id),
                    self.screensaver_elapsed,
                )),
                Background::Theme | Background::Solid { .. } | Background::Gradient { .. } => {}
            }
        }
        layers.push(content);
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
            let signature = container(text("FPL").size(13).color(Color::WHITE))
                .padding([8, 12])
                .style(move |_| container::Style {
                    background: Some(Color::from_rgba8(255, 255, 255, 0.10).into()),
                    border: iced::Border {
                        color: Color::from_rgba8(255, 255, 255, 0.16),
                        width: 1.0,
                        radius: 9.0.into(),
                    },
                    shadow: iced::Shadow {
                        color: Color::from_rgba8(0, 0, 0, 0.22),
                        offset: iced::Vector::new(0.0, 5.0),
                        blur_radius: 16.0,
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
        if self.overlay_on_viewport(self.overlays.system_hud, viewport) {
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

        if self.screensaver_settings.enabled
            || matches!(self.document.background, Background::Shader { .. })
        {
            let attribution_bottom = if self.overlay_on_viewport(self.overlays.fpl_mark, viewport) {
                64.0
            } else {
                18.0
            };
            layers.push(
                container(
                    text(screensaver::attribution(self.screensaver_index))
                        .size(13)
                        .color(Color { a: 0.55, ..muted }),
                )
                .width(Length::Fill)
                .height(Length::Fill)
                .padding(iced::Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom: attribution_bottom,
                    left: 20.0,
                })
                .align_x(iced::alignment::Horizontal::Left)
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
                .is_none_or(|x| viewport.x == x),
            OverlayTarget::Center => {
                let frames = selected.collect::<Vec<_>>();
                if frames.is_empty() {
                    true
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
                .is_none_or(|x| viewport.x == x),
        }
    }
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
         Ctrl-↑ / Ctrl-↓        Swap project workspace\n\
         Ctrl-← / Ctrl-→        Saved views (reserved)\n\
         \n\
         Canvas\n\
         Ctrl-1 / 2 / 3         Toggle left / center / right display\n\
         Ctrl-0                 Show Canvas on every display\n\
         {CANVAS_TOGGLE_KEYS:<22} Toggle Canvas\n\
         {JARVIS_TOGGLE_KEYS:<22} Toggle JARVIS presence\n\
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
    if !modifiers.control() || modifiers.alt() || modifiers.logo() {
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

fn interaction_engine(frame: Rect) -> InteractionEngine {
    InteractionEngine::new(InteractionConfig {
        workspace_bounds: LogicalRect {
            x: 0.0,
            y: 0.0,
            width: f64::from(frame.width.max(1)),
            height: f64::from(frame.height.max(1)),
        },
        minimum_surface_size: LogicalPoint { x: 160.0, y: 120.0 },
        minimum_visible_top_bar_width: SURFACE_TOP_BAR_GRAB_WIDTH,
        resize_handle_size: SURFACE_RESIZE_HANDLE_SIZE,
        top_bar_height: SURFACE_CONTROL_BAR_HEIGHT,
        snap_distance: 12.0,
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
            dock_canvas::chart::parse_delimited(&content, b',', title, kind)
        }
        ref extension if extension == "tsv" => {
            dock_canvas::chart::parse_delimited(&content, b'\t', title, kind)
        }
        ref extension if extension == "json" => {
            dock_canvas::chart::parse_json(&content, title, kind)
        }
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
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let Some(region) = attention_geometry(
            bounds.size(),
            self.source_pixels,
            self.viewport,
            self.attention,
        ) else {
            return vec![frame.into_geometry()];
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
                &mut frame,
                label,
                Point::new(region.x + 5.0, (region.y - 18.0).max(4.0)),
                12.0,
                self.color,
                iced::alignment::Horizontal::Left,
            );
        }
        vec![frame.into_geometry()]
    }
}

struct ChartProgram {
    chart: ChartDocument,
    axis_color: Color,
    muted_color: Color,
    surface_color: Color,
    series_colors: Vec<Color>,
}

impl<Message> canvas::Program<Message> for ChartProgram {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let Some((min_x, max_x, min_y, max_y)) = chart_extents(&self.chart) else {
            chart_text(
                &mut frame,
                "No chart data",
                Point::new(20.0, 24.0),
                16.0,
                self.muted_color,
                iced::alignment::Horizontal::Left,
            );
            return vec![frame.into_geometry()];
        };

        chart_text(
            &mut frame,
            &self.chart.title,
            Point::new(18.0, 12.0),
            16.0,
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
                &mut frame,
                &item.label,
                Point::new(item.x + 13.0, item.y),
                11.0,
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
            return vec![frame.into_geometry()];
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
                &mut frame,
                &format_chart_value(*tick),
                Point::new(left - 7.0, y),
                10.0,
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
                &mut frame,
                &format_chart_value(*tick),
                Point::new(x, bottom + 7.0),
                10.0,
                self.muted_color,
                iced::alignment::Horizontal::Center,
            );
        }

        let y_label = axis_title(&self.chart.y_axis);
        chart_text(
            &mut frame,
            &y_label,
            Point::new(12.0, top - 20.0),
            11.0,
            self.muted_color,
            iced::alignment::Horizontal::Left,
        );
        let x_label = axis_title(&self.chart.x_axis);
        chart_text(
            &mut frame,
            &x_label,
            Point::new((left + right) / 2.0, bounds.height - 15.0),
            11.0,
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
                    let stroke = canvas::Stroke::default()
                        .with_width(2.0)
                        .with_color(series_color);
                    for pair in series.points.windows(2) {
                        frame.stroke(
                            &canvas::Path::line(
                                Point::new(sx(pair[0].x), sy(pair[0].y)),
                                Point::new(sx(pair[1].x), sy(pair[1].y)),
                            ),
                            stroke,
                        );
                    }
                    for point in &series.points {
                        frame.fill(
                            &canvas::Path::circle(Point::new(sx(point.x), sy(point.y)), 2.7),
                            series_color,
                        );
                    }
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
                &mut frame,
                &value,
                Point::new(tooltip_x + 9.0, tooltip_y + 5.0),
                10.0,
                self.axis_color,
                iced::alignment::Horizontal::Left,
            );
        }

        vec![frame.into_geometry()]
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
) -> Option<(usize, dock_canvas::chart::ChartPoint, Point)>
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

fn axis_title(axis: &dock_canvas::chart::ChartAxis) -> String {
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
            "dock-canvas-{}-{}",
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
) -> Result<dock_core::desktop::WallpaperReceipt, Failure> {
    platform::set_wallpaper(&request).map_err(Failure::platform)?;
    Ok(dock_core::desktop::WallpaperReceipt {
        project: request.project,
        display_id: request.display_id,
        wallpaper: request.wallpaper,
    })
}

mod platform {
    use dock_canvas::display::DisplayInfo;
    use dock_core::desktop::WallpaperRequest;
    use dock_core::layout::Rect;

    #[cfg(target_os = "macos")]
    pub(super) fn canvas_frame(display: &dock_control::Display) -> Rect {
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
        match dock_control::request(dock_control::Operation::Displays)? {
            dock_control::Response::Displays { displays } => Ok(displays
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
    let socket = dock_canvas::control::socket_path()?;
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
        .name("dock-canvas-control".into())
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
        if let Err(error) = dock_control::verify_peer(&stream) {
            eprintln!("Canvas rejected peer: {error}");
            continue;
        }
        let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
        let bytes = match dock_canvas::control::read_frame(&stream) {
            Ok(bytes) => bytes,
            Err(message) => {
                write_error_reply(&mut stream, false, ErrorCode::InvalidRequest, &message);
                continue;
            }
        };
        let trimmed = std::str::from_utf8(&bytes).unwrap_or("").trim().to_string();
        let is_text = !trimmed.starts_with('{');
        let request = if is_text {
            dock_canvas::control::text::parse_line(&trimmed)
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
    let text = dock_canvas::control::text::format_reply(reply);
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
        return Err(
            "dock-canvas takes no commands; control it through its owner-local socket".into(),
        );
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
        let cadence = if app.screensaver_settings.enabled || background_animated {
            let screensaver = app.screensaver_clock.interval();
            if animated {
                screensaver.min(Duration::from_millis(16))
            } else {
                screensaver
            }
        } else if animated {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(33)
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

#[cfg(test)]
mod tests {
    use super::*;
    use dock_canvas::{BackgroundShader, control::MeshNode};

    fn test_shared_view(id: &str, revision: u64) -> SharedViewDocument {
        SharedViewDocument {
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
                path: std::env::temp_dir().join(format!(
                    "dock-canvas-missing-test-socket-{}",
                    std::process::id()
                )),
            },
            INITIAL_FRAME,
            std::env::temp_dir()
                .join(format!(
                    "dock-canvas-test-workspaces-{}-{store_id}",
                    std::process::id(),
                ))
                .join("workspaces.json"),
        )
        .unwrap();
        app.frame = INITIAL_FRAME;
        app.interaction = interaction_engine(INITIAL_FRAME);
        app
    }

    fn execute(app: &mut App, request_id: &str, command: Command) -> ControlReply {
        let (reply, replies) = mpsc::sync_channel(1);
        drop(app.execute(Pending {
            request: ControlRequest {
                schema_version: CONTROL_VERSION,
                request_id: request_id.into(),
                expected_revision: None,
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
                patch: dock_canvas::view::SharedViewPatch {
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
        app.catalog.projects[0].theme = dock_core::ThemeSelection::Palette { palette };

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

    fn indicator(id: &str, state: dock_canvas::control::IndicatorState) -> StatusIndicator {
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
        let root = std::env::temp_dir().join(format!("dock-canvas-test-{}", std::process::id()));
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
        let root =
            std::env::temp_dir().join(format!("dock-canvas-link-test-{}", std::process::id()));
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
        use dock_canvas::control::IndicatorState;

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
        use dock_canvas::control::{
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
        assert_eq!(app.snapshot().presence, Some(snapshot));
        assert_eq!(app.document, durable_document);
    }

    #[test]
    fn presence_preview_frames_refresh_without_restarting_or_reordering() {
        use dock_canvas::control::{PresencePreview, PresencePreviewFace};

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
        use dock_canvas::control::IndicatorState;

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

    fn mesh_node(id: &str, label: &str, state: dock_canvas::control::MeshNodeState) -> MeshNode {
        MeshNode {
            id: id.into(),
            label: label.into(),
            state,
            detail: None,
        }
    }

    #[test]
    fn set_mesh_replaces_snapshot_and_is_idempotent() {
        use dock_canvas::control::MeshNodeState;

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
                target: dock_canvas::display::DisplayTarget::All,
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
                target: dock_canvas::display::DisplayTarget::Exact {
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
        let display = dock_control::Display {
            id: "right".into(),
            name: "Right".into(),
            frame: dock_control::Rect {
                x: 3840,
                y: 0,
                width: 1920,
                height: 1200,
            },
            visible_frame: dock_control::Rect {
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

        let spec = dock_widgets::WidgetSpec::from_json(
            br#"{"id":"cpu","kind":"stat","label":"CPU","value":33}"#,
        )
        .unwrap();
        app.document.surfaces[0].content = SurfaceContent::Widget { spec };
        app.decoded.clear();
        app.ensure_scene_decoded();
        app.cursor = Some(LogicalPoint {
            x: left + 8.0,
            y: top + 10.0,
        });
        assert_eq!(
            app.widget_chrome("sample"),
            Some(dock_widgets::WidgetChrome::DragOnly)
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
    fn widget_surface_survives_workspace_store_and_runtime_rehydration() {
        let mut app = test_app();
        let spec = dock_widgets::WidgetSpec::from_json(
            br#"{"schema_version":1,"id":"demo-meter","kind":"meter","value":0.68,"style":"vu"}"#,
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
        let projects = vec![dock_canvas::control::ProjectWorkspaceSpec {
            project_id: "gh-1288853134".into(),
            name: "ohmOS".into(),
            status: "paused".into(),
            theme: dock_core::ThemeSelection::Coherence {
                mode: dock_core::ThemeMode::Dark,
                accent: dock_core::Accent::Success,
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
            "dock-canvas-startup-workspace-test-{}",
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
}
