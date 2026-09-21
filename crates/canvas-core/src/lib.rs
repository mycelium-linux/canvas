//! Project identity and control policy. No desktop, transport or inference ownership.

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt, fs,
    path::{Path, PathBuf},
};

pub use coherence_tokens;
pub mod appearance;
pub mod boot_budget;
pub mod desktop;
pub mod desktop_theme;
pub mod display_watch;
pub mod global_hotkey;
pub mod layout;
pub mod manifest_query;
pub mod theme_file;
pub mod unibus;
pub mod working_set;

/// The set of stream contracts Canvas recognises for ingress.
///
/// Declaring these in one place — rather than having every producer,
/// binder and test invent its own string literal — lets ingress
/// validation reject events that carry an unknown contract, and lets
/// binding validation fail early when a binding's source contract does
/// not match what the widget can consume.
///
/// Adding a contract is a data change to this constant, not a rebuild of
/// validation logic: [`is_declared_contract`] checks membership here.
pub const DECLARED_CONTRACTS: &[&str] = &[
    "canvas.agent.activity.v1",
    "canvas.agent.attention.v1",
    "canvas.briefing.beat.v1",
    "canvas.image.rgba.v1",
    "canvas.music.nowplaying.v1",
    "canvas.prometheus.v1",
    "canvas.quant.candle.v1",
    "canvas.sse.v1",
    "canvas.widget.snapshot.v1",
    "jarvis.presence.v1",
    "map.v1",
    "market.ohlcv.raw.v1",
    "panopticon.map.v1",
];

/// Whether a contract name is declared in the registry. Used by both
/// StreamEnvelope validation (ingress) and DataBinding validation (apply time).
pub fn is_declared_contract(contract: &str) -> bool {
    DECLARED_CONTRACTS.contains(&contract)
}

/// Filesystem root for Canvas runtime configuration: binders, views, and
/// frame files (see [`frame_file`]).
///
/// The single source of truth: Canvas's `runtime_config::read` reads from
/// here, and frame producers write to it — they must agree exactly or the
/// wall goes silent. `CANVAS_CONFIG_DIR` overrides it for tests and daemons
/// that keep an isolated runtime tree.
pub fn runtime_root() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("CANVAS_CONFIG_DIR") {
        return Some(PathBuf::from(path));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    #[cfg(target_os = "macos")]
    return Some(home.join("Library/Application Support/Dock/canvas"));
    #[cfg(not(target_os = "macos"))]
    Some(
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
            .join("dock/canvas"),
    )
}

/// Whether a frame source id is safe to name a file: lowercase identifier
/// characters only, so an id coming from a workspace document or a command
/// line can never carry a path, a traversal, or an extension trick.
pub fn is_safe_frame_id(source_id: &str) -> bool {
    !source_id.is_empty()
        && source_id.len() <= 64
        && source_id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Where a frame producer writes the current image for a `live_image`
/// surface: `<root>/frames/<source_id>.jpg`. Producers and the Canvas host
/// resolve through this one function so the two can never disagree.
pub fn frame_file(root: &Path, source_id: &str) -> Option<PathBuf> {
    is_safe_frame_id(source_id).then(|| root.join("frames").join(format!("{source_id}.jpg")))
}

/// Volatile native-video frame exchange. Unlike `live_image`, these RGBA
/// frames update many times per second and must never churn the SD-backed
/// configuration directory. The producer atomically replaces one fixed-size
/// frame in the per-user runtime directory; Canvas uploads it into its own
/// wgpu surface.
#[cfg(unix)]
pub fn video_frame_file(source_id: &str) -> Option<PathBuf> {
    if !is_safe_frame_id(source_id) {
        return None;
    }
    #[cfg(target_os = "macos")]
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    #[cfg(not(target_os = "macos"))]
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    Some(
        runtime
            .join("dock/canvas/video-frames")
            .join(format!("{source_id}.rgba")),
    )
}

/// Where a media edge publishes the current canonical
/// `camera.detections.v1` snapshot for a named camera. The document is a
/// complete latest-value replacement and carries its own expiry timestamp.
pub fn detection_file(root: &Path, source_id: &str) -> Option<PathBuf> {
    is_safe_frame_id(source_id).then(|| root.join("detections").join(format!("{source_id}.json")))
}

/// Land bytes at `path` by sibling-write + rename: a reader either sees the
/// previous complete file or the new one, never a truncated hybrid. A live
/// frame read mid-write would decode as garbage and the host would reject
/// it — this makes that race structurally impossible.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension("jpg.tmp");
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)
}

/// Filesystem location of the Canvas `stream.sock` UNIX datagram socket.
///
/// The single source of truth for where producers (feeders, SSE connectors)
/// should send `canvas.widget.snapshot.v1` envelopes: macOS uses the app-support
/// directory; other Unixes use `XDG_RUNTIME_DIR` exactly as Canvas binds it.
/// Returns `None` when the owning environment variable is unset, so a caller can
/// fail loud rather than silently sending to the wrong file.
#[cfg(unix)]
pub fn stream_socket_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME").map(PathBuf::from)?;
        Some(home.join("Library/Application Support/Dock/canvas/stream.sock"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        Some(PathBuf::from(runtime).join("dock/canvas/stream.sock"))
    }
}

#[derive(Debug)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn check(ok: bool, message: impl Into<String>) -> Result<()> {
    if ok {
        Ok(())
    } else {
        Err(Error(message.into()))
    }
}
/// Grammar for project ids. Deliberately case-tolerant: Alfalfa owns these
/// identities and its roster carries human capitalization (`JARVIS`,
/// `LibreChat`); refusing it would force a lowercase shadow of the truth.
/// Ids are identity tokens here, not filenames — document ids remain
/// lowercase `identifier` grammar, and resource ids never widen.
fn project_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.as_bytes()[0].is_ascii_lowercase()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn visible(s: &str) -> bool {
    !s.trim().is_empty() && !s.chars().any(char::is_control)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema_version: u32,
    pub projects: Vec<Project>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub theme: ThemeSelection,
    pub resources: Vec<Resource>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    pub id: String,
    pub title: String,
    pub host: Host,
    pub kind: ResourceKind,
    pub target: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    Mac,
    AgoraOne,
    EdgeRunner,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    MacApp,
    RemoteDesktop,
    TerminalSession,
    AgentSession,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThemeMode {
    Light,
    Dark,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Accent {
    Brand,
    Info,
    Success,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectTheme {
    pub mode: ThemeMode,
    pub accent: Accent,
}

impl ThemeSelection {
    pub fn to_project_theme(&self) -> Option<ProjectTheme> {
        match self {
            ThemeSelection::Coherence { mode, accent } => Some(ProjectTheme {
                mode: *mode,
                accent: *accent,
            }),
            ThemeSelection::Palette { .. } => None,
        }
    }
}

/// A project's theme source: a Coherence mode, or an imported palette in the
/// canonical role set. This is the only theme boundary Canvas surfaces read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "source", rename_all = "snake_case", deny_unknown_fields)]
pub enum ThemeSelection {
    /// Derived from upstream Coherence tokens for a mode and accent.
    Coherence { mode: ThemeMode, accent: Accent },
    /// An imported palette (Dock theme file or rice palette adapter output).
    Palette { palette: ThemePalette },
}
/// Canonical role set for every Canvas surface. Roles serialize as `#rrggbb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThemePalette {
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub background: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub background_strong: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub surface: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub surface_muted: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub text: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub text_muted: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub primary: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub success: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub warning: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub danger: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub info: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub border: [u8; 3],
    #[serde(with = "theme_hex")]
    #[schemars(with = "String")]
    pub border_strong: [u8; 3],
}
/// `#rrggbb` hex codec for canonical palette roles.
pub mod theme_hex {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(rgb: &[u8; 3], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<[u8; 3], D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        parse_hex(&value).map_err(serde::de::Error::custom)
    }

    pub fn parse_hex(value: &str) -> Result<[u8; 3], String> {
        let digits = value
            .strip_prefix('#')
            .ok_or_else(|| format!("{value} must be #rrggbb"))?;
        if digits.len() != 6 {
            return Err(format!("{value} is not #rrggbb"));
        }
        let byte = |start: usize| {
            u8::from_str_radix(&digits[start..start + 2], 16)
                .map_err(|_| format!("{value} is not #rrggbb"))
        };
        Ok([byte(0)?, byte(2)?, byte(4)?])
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ThemeMetrics {
    pub space_1: f32,
    pub space_2: f32,
    pub space_3: f32,
    pub space_4: f32,
    pub space_5: f32,
    pub radius_xs: f32,
    pub radius_sm: f32,
    pub radius_md: f32,
    pub radius_lg: f32,
    /// Compatibility alias for the default component radius.
    pub radius: f32,
    /// The floor of the scale, for labels a renderer draws itself and cannot
    /// reflow — axis ticks, cell headings. Below `small`, and deliberately the
    /// only thing that is: a widget picking its own smaller size is how eight
    /// unreadable literals got into the library.
    pub micro: f32,
    pub body: f32,
    pub small: f32,
    pub large: f32,
    pub title: f32,
    pub display: f32,
    /// The one size a value is allowed to dominate at. Every hero number in the
    /// library shares it, so a stat beside a gauge beside a clock reads as one
    /// scale instead of three near-misses.
    pub hero: f32,
}

/// A resolved, renderer-neutral Coherence theme. Consumers select semantic
/// roles from this value instead of inventing component-local colors or sizes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisualTheme {
    pub mode: ThemeMode,
    pub palette: ThemePalette,
    pub surface_raised: [u8; 3],
    pub surface_accent: [u8; 3],
    pub surface_accent_strong: [u8; 3],
    pub focus: [u8; 3],
    /// Optional categorical identity palette for charts. Semantic status
    /// colours remain separate; renderers derive a safe ramp when absent.
    pub categorical: Option<[[u8; 3]; 6]>,
    pub metrics: ThemeMetrics,
    pub panel_opacity: f32,
    pub muted_opacity: f32,
    pub grid_opacity: f32,
    pub shadow_opacity: f32,
    pub shadow_blur: f32,
    pub shadow_offset_y: f32,
}
impl ThemeSelection {
    pub fn coherence(mode: ThemeMode, accent: Accent) -> Self {
        Self::Coherence { mode, accent }
    }

    pub fn palette(self) -> ThemePalette {
        match self {
            Self::Coherence { mode, accent } => ProjectTheme { mode, accent }.palette(),
            Self::Palette { palette } => palette,
        }
    }

    pub fn metrics(self) -> ThemeMetrics {
        ThemeMetrics::shared()
    }

    pub fn visual_theme(self) -> VisualTheme {
        let mode = self.mode();
        let palette = self.palette();
        let (surface_raised, surface_accent, surface_accent_strong, focus) = match self {
            Self::Coherence {
                mode: ThemeMode::Light,
                ..
            } => (
                coherence_tokens::light::COLOR_SURFACE_RAISED,
                coherence_tokens::light::COLOR_SURFACE_ACCENT,
                coherence_tokens::light::COLOR_SURFACE_ACCENT_STRONG,
                coherence_tokens::light::COLOR_FOCUS,
            ),
            Self::Coherence {
                mode: ThemeMode::Dark,
                ..
            } => (
                coherence_tokens::dark::COLOR_SURFACE_RAISED,
                coherence_tokens::dark::COLOR_SURFACE_ACCENT,
                coherence_tokens::dark::COLOR_SURFACE_ACCENT_STRONG,
                coherence_tokens::dark::COLOR_FOCUS,
            ),
            Self::Palette { .. } => (
                palette.surface,
                palette.surface_muted,
                mix_rgb(palette.surface_muted, palette.primary, 0.24),
                palette.primary,
            ),
        };
        VisualTheme {
            mode,
            palette,
            surface_raised,
            surface_accent,
            surface_accent_strong,
            focus,
            categorical: None,
            metrics: self.metrics(),
            panel_opacity: 0.92,
            muted_opacity: 0.68,
            grid_opacity: 0.24,
            shadow_opacity: if mode == ThemeMode::Dark { 0.30 } else { 0.16 },
            shadow_blur: 18.0,
            shadow_offset_y: 6.0,
        }
    }

    /// Explicit for Coherence themes; inferred from background luminance for
    /// imported palettes so consumers never branch on the theme source.
    pub fn mode(self) -> ThemeMode {
        match self {
            Self::Coherence { mode, .. } => mode,
            Self::Palette { palette } => {
                if luminance(palette.background) < 0.5 {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                }
            }
        }
    }

    /// Accent identity for tooling that reports it; imported palettes expose
    /// their accent through `primary`, so a neutral identity is correct.
    pub fn accent(self) -> Accent {
        match self {
            Self::Coherence { accent, .. } => accent,
            Self::Palette { .. } => Accent::Brand,
        }
    }
}

fn luminance(rgb: [u8; 3]) -> f64 {
    let linearize = |channel: u8| {
        let c = f64::from(channel) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linearize(rgb[0]) + 0.7152 * linearize(rgb[1]) + 0.0722 * linearize(rgb[2])
}
impl ThemeMetrics {
    /// Both current upstream modes share geometry. Never hand-maintain tokens.
    /// The scale this process is rendering at.
    ///
    /// Reads the runtime appearance rather than a literal, so every font size,
    /// gap and radius in the library follows one configurable number. Every
    /// widget already routes through here, which is why this is the whole
    /// wiring rather than a change per call site.
    pub fn shared() -> Self {
        Self::scaled(crate::appearance::current().density.scale)
    }

    /// The same scale, read from further away.
    ///
    /// A 5760-wide wall seen from across a room is not a laptop at arm's length,
    /// but the Coherence tokens are sized for the laptop. Rather than let each
    /// widget pick its own bigger number - which is how eight unreadable
    /// literals got into the library in the first place - one factor multiplies
    /// the whole scale, so the relationships between `small`, `body` and `hero`
    /// survive being made legible.
    ///
    /// Spacing and radii scale too. Type that grows while its padding does not
    /// reads as cramped rather than large, which is a different kind of hard to
    /// look at.
    pub fn scaled(factor: f32) -> Self {
        // A non-finite or absurd factor would silently produce an unrenderable
        // theme, so it is clamped rather than trusted.
        let factor = if factor.is_finite() {
            factor.clamp(0.5, 4.0)
        } else {
            1.0
        };
        let base = Self::tokens();
        ThemeMetrics {
            space_1: base.space_1 * factor,
            space_2: base.space_2 * factor,
            space_3: base.space_3 * factor,
            space_4: base.space_4 * factor,
            space_5: base.space_5 * factor,
            radius_xs: base.radius_xs * factor,
            radius_sm: base.radius_sm * factor,
            radius_md: base.radius_md * factor,
            radius_lg: base.radius_lg * factor,
            radius: base.radius * factor,
            micro: base.micro * factor,
            body: base.body * factor,
            small: base.small * factor,
            large: base.large * factor,
            title: base.title * factor,
            display: base.display * factor,
            hero: base.hero * factor,
        }
    }

    fn tokens() -> Self {
        use coherence_tokens::light as t;
        ThemeMetrics {
            space_1: t::SPACE_1,
            space_2: t::SPACE_2,
            space_3: t::SPACE_3,
            space_4: t::SPACE_4,
            space_5: t::SPACE_5,
            radius_xs: t::RADIUS_XS,
            radius_sm: t::RADIUS_SM,
            radius_md: t::RADIUS_MD,
            radius_lg: t::RADIUS_LG,
            radius: t::RADIUS_MD,
            // Half a `small`→`body` step below `small`, so the floor moves with
            // the scale instead of being a literal that outlives it.
            micro: t::FONT_SIZE_SM - (t::FONT_SIZE_BASE - t::FONT_SIZE_SM) / 2.0,
            body: t::FONT_SIZE_BASE,
            small: t::FONT_SIZE_SM,
            large: t::FONT_SIZE_LG,
            title: t::FONT_SIZE_XL,
            display: t::FONT_SIZE_2XL,
            // Half again the largest text token: far enough above `display` to
            // read as the value rather than a louder heading, and one ratio in
            // one place rather than 1.5×, 1.7× and 2.25× at three call sites.
            hero: t::FONT_SIZE_2XL * 1.5,
        }
    }
}

fn mix_rgb(a: [u8; 3], b: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|index| {
        (f32::from(a[index]) * (1.0 - amount) + f32::from(b[index]) * amount).round() as u8
    })
}
impl ProjectTheme {
    pub fn palette(self) -> ThemePalette {
        use coherence_tokens::{Mode, dark, light};
        let (p, info, border, background_strong, surface_muted, border_strong) = match self.mode {
            ThemeMode::Dark => (
                Mode::Dark.palette(),
                dark::COLOR_INFO,
                dark::COLOR_BORDER,
                dark::COLOR_BACKGROUND_STRONG,
                dark::COLOR_SURFACE_MUTED,
                dark::COLOR_BORDER_STRONG,
            ),
            ThemeMode::Light => (
                Mode::Light.palette(),
                light::COLOR_INFO,
                light::COLOR_BORDER,
                light::COLOR_BACKGROUND_STRONG,
                light::COLOR_SURFACE_MUTED,
                light::COLOR_BORDER_STRONG,
            ),
        };
        ThemePalette {
            background: p.background,
            background_strong,
            surface: p.surface,
            surface_muted,
            text: p.text,
            text_muted: p.muted,
            primary: match self.accent {
                Accent::Brand => p.primary,
                Accent::Info => info,
                Accent::Success => p.success,
            },
            success: p.success,
            danger: p.danger,
            warning: p.warning,
            info,
            border,
            border_strong,
        }
    }
    pub fn metrics(self) -> ThemeMetrics {
        ThemeMetrics::shared()
    }
}
impl Catalog {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        Self::parse(
            &fs::read_to_string(path.as_ref())
                .map_err(|e| Error(format!("{}: {e}", path.as_ref().display())))?,
        )
    }
    pub fn bundled() -> Result<Self> {
        Self::parse(include_str!("../../../config/projects.json"))
    }
    pub fn parse(input: &str) -> Result<Self> {
        let c: Self = serde_json::from_str(input)
            .map_err(|e| Error(format!("Invalid project catalog: {e}")))?;
        c.validate()?;
        Ok(c)
    }
    pub fn project(&self, id: &str) -> Result<&Project> {
        self.projects
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| Error(format!("Unknown project: {id}")))
    }
    pub fn validate(&self) -> Result<()> {
        check(
            self.schema_version == 1,
            "Unsupported catalog schema_version",
        )?;
        check(
            !self.projects.is_empty(),
            "At least one project is required",
        )?;
        let mut ids = BTreeSet::new();
        for p in &self.projects {
            check(
                project_id(&p.id) && ids.insert(&p.id),
                format!("Invalid or duplicate project id: {}", p.id),
            )?;
            check(
                visible(&p.name),
                "Project name must be nonempty and printable",
            )?;
            let mut resources = BTreeSet::new();
            for r in &p.resources {
                check(
                    identifier(&r.id) && resources.insert(&r.id),
                    format!("Invalid or duplicate resource id: {}", r.id),
                )?;
                check(
                    visible(&r.title) && visible(&r.target),
                    "Resource title and target must be nonempty and printable",
                )?;
                check(
                    r.kind != ResourceKind::MacApp || r.host == Host::Mac,
                    "Mac apps require a Mac host",
                )?;
                if matches!(
                    r.kind,
                    ResourceKind::TerminalSession | ResourceKind::AgentSession
                ) {
                    check(
                        identifier(&r.target),
                        "Session target must be a managed project/session identifier",
                    )?;
                }
                if r.kind == ResourceKind::RemoteDesktop {
                    check(
                        r.target.parse::<std::net::IpAddr>().is_ok(),
                        "Desktop target must be an explicit IP address",
                    )?;
                    check(
                        r.host != Host::EdgeRunner,
                        "Edge Runner is a session provider, not a desktop transport",
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Active,
    Idle,
    Paused,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Control {
    Human,
    Agent,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Human,
    Agent,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectState {
    pub lifecycle: Lifecycle,
    pub control: Control,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceState {
    pub schema_version: u32,
    pub selected_project: String,
    pub projects: BTreeMap<String, ProjectState>,
}
#[derive(Debug, Clone)]
pub enum Action {
    SelectProject(String),
    TakeControl(String),
    ReleaseControl(String),
}
impl WorkspaceState {
    pub fn new(catalog: &Catalog) -> Result<Self> {
        catalog.validate()?;
        let selected_project = catalog.projects[0].id.clone();
        let projects = catalog
            .projects
            .iter()
            .map(|p| {
                (
                    p.id.clone(),
                    ProjectState {
                        lifecycle: if p.id == selected_project {
                            Lifecycle::Active
                        } else {
                            Lifecycle::Idle
                        },
                        control: Control::Human,
                    },
                )
            })
            .collect();
        Ok(Self {
            schema_version: 1,
            selected_project,
            projects,
        })
    }
    pub fn validate(&self, catalog: &Catalog) -> Result<()> {
        check(
            self.schema_version == 1,
            "Unsupported workspace state version",
        )?;
        catalog.project(&self.selected_project)?;
        check(
            self.projects.len() == catalog.projects.len(),
            "State/catalog projects differ",
        )?;
        for (id, state) in &self.projects {
            catalog.project(id)?;
            check(
                state.lifecycle != Lifecycle::Active || id == &self.selected_project,
                "Only selected project can be active",
            )?;
        }
        check(
            self.projects.contains_key(&self.selected_project),
            "Missing selected project state",
        )?;
        check(
            self.projects[&self.selected_project].lifecycle != Lifecycle::Idle,
            "Selected project must be active or explicitly paused",
        )?;
        Ok(())
    }
    pub fn apply(&mut self, actor: Actor, action: Action) -> Result<()> {
        let id = match &action {
            Action::SelectProject(id) | Action::TakeControl(id) | Action::ReleaseControl(id) => id,
        };
        let state = self
            .projects
            .get(id)
            .ok_or_else(|| Error(format!("Unknown project: {id}")))?;
        if actor == Actor::Agent {
            check(
                matches!(action, Action::SelectProject(_)),
                "Only the human can grant or take control",
            )?;
            check(
                state.control == Control::Agent,
                "Project is under human control",
            )?;
            let current = &self.projects[&self.selected_project];
            check(
                current.control == Control::Agent,
                "Cannot switch away from a human-controlled project",
            )?;
        }
        match action {
            Action::SelectProject(id) => {
                let old = self
                    .projects
                    .get_mut(&self.selected_project)
                    .ok_or_else(|| Error("Missing selected project state".into()))?;
                if old.lifecycle == Lifecycle::Active {
                    old.lifecycle = Lifecycle::Idle;
                }
                let new = self.projects.get_mut(&id).unwrap();
                // Selecting a paused project never thaws its processes implicitly.
                if new.lifecycle != Lifecycle::Paused {
                    new.lifecycle = Lifecycle::Active;
                }
                self.selected_project = id;
            }
            Action::TakeControl(id) => self.projects.get_mut(&id).unwrap().control = Control::Human,
            Action::ReleaseControl(id) => {
                self.projects.get_mut(&id).unwrap().control = Control::Agent
            }
        }
        Ok(())
    }
    /// Call only after the platform reports a successful pause/resume.
    /// This policy model never freezes processes or claims transport/session ownership.
    pub fn acknowledge_pause(&mut self, id: &str, paused: bool) -> Result<()> {
        let project = self
            .projects
            .get_mut(id)
            .ok_or_else(|| Error(format!("Unknown project: {id}")))?;
        project.lifecycle = if paused {
            Lifecycle::Paused
        } else if id == self.selected_project {
            Lifecycle::Active
        } else {
            Lifecycle::Idle
        };
        Ok(())
    }
    pub fn load(path: impl AsRef<Path>, catalog: &Catalog) -> Result<Self> {
        let state: Self = serde_json::from_str(
            &fs::read_to_string(path.as_ref()).map_err(|e| Error(e.to_string()))?,
        )
        .map_err(|e| Error(format!("Invalid workspace state: {e}")))?;
        state.validate(catalog)?;
        Ok(state)
    }
    pub fn save(&self, path: impl AsRef<Path>, catalog: &Catalog) -> Result<()> {
        self.validate(catalog)?;
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| Error(e.to_string()))?;
        }
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(|e| Error(e.to_string()))?,
        )
        .map_err(|e| Error(e.to_string()))?;
        fs::rename(&tmp, path).map_err(|e| Error(e.to_string()))
    }
}

/// Local cooperative state coordination. OS account/file permissions remain the
/// security boundary; an actor flag is not authentication of a remote caller.
pub struct StateStore {
    pub path: std::path::PathBuf,
}
struct LockFile(std::path::PathBuf);
impl Drop for LockFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
impl StateStore {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn load(&self, catalog: &Catalog) -> Result<WorkspaceState> {
        match fs::metadata(&self.path) {
            Ok(_) => WorkspaceState::load(&self.path, catalog),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => WorkspaceState::new(catalog),
            Err(e) => Err(Error(format!(
                "Cannot inspect {}: {e}",
                self.path.display()
            ))),
        }
    }
    /// Serialize read-modify-write across the GUI and CLI. A stale crash lock is
    /// surfaced for inspection instead of silently overwriting another process.
    pub fn update<F>(&self, catalog: &Catalog, change: F) -> Result<WorkspaceState>
    where
        F: FnOnce(&mut WorkspaceState) -> Result<()>,
    {
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent).map_err(|e| Error(e.to_string()))?;
        }
        let lock = self.path.with_extension("lock");
        let mut handle = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
            .map_err(|e| {
                Error(format!(
                    "Cannot lock {}: {e}. If stale, inspect its PID before removing it.",
                    lock.display()
                ))
            })?;
        let _guard = LockFile(lock);
        use std::io::Write;
        writeln!(handle, "{}", std::process::id()).map_err(|e| Error(e.to_string()))?;
        let mut state = self.load(catalog)?;
        change(&mut state)?;
        state.save(&self.path, catalog)?;
        Ok(state)
    }
}

#[cfg(test)]
mod metrics_scale_tests {
    use super::*;

    #[test]
    fn the_default_scale_is_unchanged() {
        assert_eq!(ThemeMetrics::shared(), ThemeMetrics::scaled(1.0));
    }

    #[test]
    fn scaling_grows_every_size_together() {
        let one = ThemeMetrics::shared();
        let big = ThemeMetrics::scaled(1.5);
        assert!((big.body - one.body * 1.5).abs() < 0.001);
        assert!((big.hero - one.hero * 1.5).abs() < 0.001);
        assert!(
            (big.space_2 - one.space_2 * 1.5).abs() < 0.001,
            "type that grows while its padding does not reads as cramped"
        );
        assert!((big.radius_md - one.radius_md * 1.5).abs() < 0.001);
    }

    #[test]
    fn the_order_of_the_scale_survives_being_enlarged() {
        for factor in [0.5, 1.0, 1.75, 4.0] {
            let m = ThemeMetrics::scaled(factor);
            assert!(m.micro < m.small, "micro is the floor at {factor}");
            assert!(m.small < m.body);
            assert!(m.body < m.large);
            assert!(m.large < m.title);
            assert!(m.title < m.display);
            assert!(m.display < m.hero, "hero stays the one dominant size");
        }
    }

    #[test]
    fn an_absurd_or_broken_factor_cannot_produce_an_unrenderable_theme() {
        // Non-finite is not "very large" - it is a broken input, and the safe
        // reading of a broken size factor is the default rather than the
        // maximum.
        assert_eq!(ThemeMetrics::scaled(f32::NAN), ThemeMetrics::shared());
        assert_eq!(ThemeMetrics::scaled(f32::INFINITY), ThemeMetrics::shared());
        assert_eq!(
            ThemeMetrics::scaled(f32::NEG_INFINITY),
            ThemeMetrics::shared()
        );
        // A finite but absurd factor clamps rather than falling back, because it
        // is a real request, just an unreasonable one.
        assert_eq!(ThemeMetrics::scaled(99.0), ThemeMetrics::scaled(4.0));
        assert_eq!(ThemeMetrics::scaled(0.0), ThemeMetrics::scaled(0.5));
        assert_eq!(ThemeMetrics::scaled(-3.0), ThemeMetrics::scaled(0.5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn catalog() -> Catalog {
        Catalog::bundled().unwrap()
    }
    #[test]
    fn native_video_frame_paths_are_available_and_bounded() {
        assert!(video_frame_file("browser-agent-one").is_some());
        assert!(video_frame_file("../escape").is_none());
        assert!(video_frame_file("Browser Mixed Case").is_none());
    }
    #[test]
    fn themes_are_upstream_and_visually_distinct() {
        let c = catalog();
        let a = c.projects[0].theme.palette();
        let b = c.projects[1].theme.palette();
        assert_ne!(a.background, b.background);
        assert_eq!(a.primary, coherence_tokens::dark::COLOR_INFO);
        assert_eq!(b.primary, coherence_tokens::light::COLOR_BRAND);
    }
    #[test]
    fn visual_themes_carry_coherence_beyond_color() {
        let c = catalog();
        let dark = c.projects[0].theme.visual_theme();
        let light = c.projects[1].theme.visual_theme();
        assert_eq!(
            dark.surface_raised,
            coherence_tokens::dark::COLOR_SURFACE_RAISED
        );
        assert_eq!(
            light.surface_accent,
            coherence_tokens::light::COLOR_SURFACE_ACCENT
        );
        assert_ne!(dark.surface_raised, light.surface_raised);
        assert_eq!(dark.metrics.display, coherence_tokens::dark::FONT_SIZE_2XL);
        assert_eq!(dark.metrics.radius_lg, coherence_tokens::dark::RADIUS_LG);
        assert!(dark.shadow_opacity > light.shadow_opacity);
    }
    #[test]
    fn the_type_scale_is_one_ascending_run_from_micro_to_hero() {
        let m = ThemeMetrics::shared();
        let scale = [
            m.micro, m.small, m.body, m.large, m.title, m.display, m.hero,
        ];
        for step in scale.windows(2) {
            assert!(step[1] > step[0], "type scale is not ascending: {scale:?}");
        }
        // The two ends are the ones a widget is tempted to invent, so they are
        // pinned by relationship rather than by value: a floor that stays below
        // `small` but within reading distance of it, and a hero that outruns
        // every heading by more than the largest step inside the scale.
        assert!(m.micro > m.small * 0.75);
        let widest_inner_step = scale[..scale.len() - 1]
            .windows(2)
            .map(|step| step[1] - step[0])
            .fold(0.0_f32, f32::max);
        assert!(m.hero - m.display > widest_inner_step);
    }
    #[test]
    fn the_scale_follows_its_tokens_rather_than_pasted_sizes() {
        // Every size token traces back to the upstream scale. A literal here
        // survives a token change silently, which is how a widget ends up 5px
        // off the scale it claims to follow.
        use coherence_tokens::light as t;
        let m = ThemeMetrics::shared();
        assert_eq!(m.small, t::FONT_SIZE_SM);
        assert_eq!(m.display, t::FONT_SIZE_2XL);
        assert_eq!(
            m.micro,
            t::FONT_SIZE_SM - (t::FONT_SIZE_BASE - t::FONT_SIZE_SM) / 2.0
        );
        assert_eq!(m.hero, t::FONT_SIZE_2XL * 1.5);
    }
    #[test]
    fn imported_palette_resolves_the_complete_visual_contract() {
        let palette = catalog().projects[0].theme.palette();
        let visual = ThemeSelection::Palette { palette }.visual_theme();
        assert_eq!(visual.palette, palette);
        assert_eq!(visual.surface_raised, palette.surface);
        assert_eq!(visual.focus, palette.primary);
        assert_ne!(visual.surface_accent_strong, palette.surface_muted);
    }
    #[test]
    fn unsupported_schema_and_duplicate_ids_rejected() {
        let mut c = catalog();
        c.schema_version = 2;
        assert!(c.validate().is_err());
        c.schema_version = 1;
        c.projects.push(c.projects[0].clone());
        assert!(c.validate().is_err());
    }
    #[test]
    fn malformed_resources_rejected() {
        let mut c = catalog();
        c.projects[0].resources[0].host = Host::AgoraOne;
        assert!(c.validate().is_err());
        c = catalog();
        c.projects[0].resources[1].target = "host; echo injected".into();
        assert!(c.validate().is_err());
    }
    #[test]
    fn project_switch_preserves_paused_background_and_control() {
        let mut s = WorkspaceState::new(&catalog()).unwrap();
        s.acknowledge_pause("dock", true).unwrap();
        s.apply(Actor::Human, Action::SelectProject("studio".into()))
            .unwrap();
        assert_eq!(s.projects["dock"].lifecycle, Lifecycle::Paused);
        assert_eq!(s.projects["dock"].control, Control::Human);
        s.apply(Actor::Human, Action::SelectProject("dock".into()))
            .unwrap();
        assert_eq!(s.projects["dock"].lifecycle, Lifecycle::Paused);
    }
    #[test]
    fn switching_only_changes_visibility() {
        let mut s = WorkspaceState::new(&catalog()).unwrap();
        s.apply(Actor::Human, Action::SelectProject("studio".into()))
            .unwrap();
        assert_eq!(s.projects["dock"].lifecycle, Lifecycle::Idle);
        assert_eq!(s.projects["studio"].lifecycle, Lifecycle::Active);
    }
    #[test]
    fn agent_cannot_grant_itself_control_or_steal_focus() {
        let mut s = WorkspaceState::new(&catalog()).unwrap();
        assert!(
            s.apply(Actor::Agent, Action::ReleaseControl("dock".into()))
                .is_err()
        );
        s.apply(Actor::Human, Action::ReleaseControl("studio".into()))
            .unwrap();
        assert!(
            s.apply(Actor::Agent, Action::SelectProject("studio".into()))
                .is_err()
        );
        s.apply(Actor::Human, Action::ReleaseControl("dock".into()))
            .unwrap();
        s.apply(Actor::Agent, Action::SelectProject("studio".into()))
            .unwrap();
        s.apply(Actor::Human, Action::TakeControl("studio".into()))
            .unwrap();
        assert!(
            s.apply(Actor::Agent, Action::SelectProject("dock".into()))
                .is_err()
        );
    }
    #[test]
    fn failed_action_leaves_selection_unchanged() {
        let mut s = WorkspaceState::new(&catalog()).unwrap();
        assert!(
            s.apply(Actor::Human, Action::SelectProject("missing".into()))
                .is_err()
        );
        assert_eq!(s.selected_project, "dock");
    }
    #[test]
    fn resume_background_project_does_not_steal_focus() {
        let mut s = WorkspaceState::new(&catalog()).unwrap();
        s.acknowledge_pause("studio", true).unwrap();
        s.acknowledge_pause("studio", false).unwrap();
        assert_eq!(s.selected_project, "dock");
        assert_eq!(s.projects["studio"].lifecycle, Lifecycle::Idle);
    }
    #[test]
    fn corrupted_state_rejected() {
        let mut s = WorkspaceState::new(&catalog()).unwrap();
        s.projects.get_mut("studio").unwrap().lifecycle = Lifecycle::Active;
        assert!(s.validate(&catalog()).is_err());
    }
    #[test]
    fn file_store_roundtrip_and_lock_contention() {
        let path = std::env::temp_dir()
            .join(format!("canvas-core-test-{}", std::process::id()))
            .join("state.json");
        let store = StateStore::new(path.clone());
        let c = catalog();
        store
            .update(&c, |s| {
                s.apply(Actor::Human, Action::SelectProject("studio".into()))
            })
            .unwrap();
        assert_eq!(store.load(&c).unwrap().selected_project, "studio");
        fs::write(path.with_extension("lock"), "other-process").unwrap();
        assert!(store.update(&c, |_| Ok(())).is_err());
        fs::remove_file(path.with_extension("lock")).unwrap();
        fs::remove_file(&path).unwrap();
        fs::remove_dir(path.parent().unwrap()).unwrap();
    }
}
