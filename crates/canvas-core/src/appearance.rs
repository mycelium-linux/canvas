//! Every appearance and pacing constant, in one place, loadable at runtime.
//!
//! These values were literals scattered across the renderer, which meant tuning
//! the wall required a forty-minute rebuild and a restart. That is the wrong
//! feedback loop for anything decided by eye — you cannot dial in a font size or
//! a panel opacity when each attempt costs most of an hour, so in practice
//! nobody dials them and the first guess ships.
//!
//! Three rules hold this together:
//!
//! - **The defaults are exactly the old literals.** Adopting this file changes
//!   nothing on screen. Every value here was read out of the code it replaces,
//!   so a missing config and today's build render identically.
//! - **Every field is optional.** A config sets only what it overrides, so a
//!   one-line file that raises the type scale is a valid config and does not
//!   have to restate the other twenty-four values.
//! - **Nothing here can produce an unrenderable surface.** Values clamp to
//!   ranges that still draw. A typo should make the wall ugly, never blank —
//!   because a blank wall during a demo is indistinguishable from a crash.
//!
//! Unknown fields are refused rather than ignored: a misspelled key that is
//! silently dropped looks exactly like a setting that had no effect, and the
//! whole point is being able to tell whether a tweak did anything.

use serde::{Deserialize, Serialize};

fn clamp(value: f32, low: f32, high: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(low, high)
    } else {
        fallback
    }
}

/// Type size, spacing and corner radius.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Density {
    /// Multiplies the whole Coherence scale — type, spacing and radii together.
    /// A wall read from across a room wants more than a laptop at arm's length,
    /// and the tokens are sized for the laptop.
    pub scale: f32,
}

impl Default for Density {
    fn default() -> Self {
        Self { scale: 1.0 }
    }
}

impl Density {
    pub fn sanitized(self) -> Self {
        Self {
            scale: clamp(self.scale, 0.5, 4.0, 1.0),
        }
    }
}

/// How solid a panel is over whatever is behind it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Panels {
    /// 1.0 is opaque. Below that, the background shows through behind text, so
    /// contrast varies across a panel wherever the background is not flat —
    /// which is legible as eye strain long before anyone names it.
    pub opacity: f32,
    /// Opacity of secondary fills relative to the panel. Was a bare `* 0.3`.
    pub muted_factor: f32,
}

impl Default for Panels {
    fn default() -> Self {
        Self {
            opacity: 0.92,
            muted_factor: 0.3,
        }
    }
}

impl Panels {
    pub fn sanitized(self) -> Self {
        Self {
            // Never fully transparent: an invisible panel is a lost widget.
            opacity: clamp(self.opacity, 0.25, 1.0, 0.92),
            muted_factor: clamp(self.muted_factor, 0.0, 1.0, 0.3),
        }
    }
}

/// Everything that moves.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Motion {
    /// Surface and palette transitions. Zero disables them outright, which is
    /// the honest way to express "still" — not a very small non-zero number
    /// that still animates and still costs frames.
    pub transition_ms: u64,
    /// Frame ceiling for animated backgrounds.
    pub background_max_fps: u16,
}

impl Default for Motion {
    fn default() -> Self {
        Self {
            transition_ms: 280,
            background_max_fps: 60,
        }
    }
}

impl Motion {
    /// Nothing animates and nothing repaints for animation's sake.
    pub fn still() -> Self {
        Self {
            transition_ms: 0,
            background_max_fps: 0,
        }
    }

    pub fn is_still(self) -> bool {
        self.transition_ms == 0 && self.background_max_fps == 0
    }

    pub fn sanitized(self) -> Self {
        Self {
            // A transition longer than a couple of seconds reads as a fault.
            transition_ms: self.transition_ms.min(2_000),
            background_max_fps: self.background_max_fps.min(240),
        }
    }
}

/// How many rows, lanes and faces a surface will draw before it says there are
/// more. The caps themselves are not the honesty mechanism — the "+N more"
/// label is — but a cap that is too high produces an unreadable wall and one
/// that is too low hides work.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Limits {
    pub table_rows: usize,
    pub event_lines: usize,
    pub summary_nodes: usize,
    pub series_colors: usize,
    pub visible_occupants: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            table_rows: 8,
            event_lines: 7,
            summary_nodes: 8,
            series_colors: 6,
            visible_occupants: 4,
        }
    }
}

impl Limits {
    pub fn sanitized(self) -> Self {
        Self {
            table_rows: self.table_rows.clamp(1, 64),
            event_lines: self.event_lines.clamp(1, 64),
            summary_nodes: self.summary_nodes.clamp(1, 64),
            // The ramp is generated, but past a dozen the hues stop being
            // distinguishable and the guarantee the ramp exists for is gone.
            series_colors: self.series_colors.clamp(1, 12),
            visible_occupants: self.visible_occupants.clamp(1, 16),
        }
    }
}

/// Line, area and marker weights for drawn series.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Series {
    pub line_width: f32,
    pub area_alpha: f32,
    /// Multiplied by `line_width`, so a thicker line keeps its head in
    /// proportion instead of growing a dot that reads as a separate mark.
    pub head_radius_factor: f32,
}

impl Default for Series {
    fn default() -> Self {
        Self {
            line_width: 2.0,
            area_alpha: 0.22,
            head_radius_factor: 1.6,
        }
    }
}

impl Series {
    pub fn head_radius(self) -> f32 {
        self.line_width * self.head_radius_factor
    }

    pub fn sanitized(self) -> Self {
        Self {
            line_width: clamp(self.line_width, 0.5, 12.0, 2.0),
            area_alpha: clamp(self.area_alpha, 0.0, 1.0, 0.22),
            head_radius_factor: clamp(self.head_radius_factor, 0.0, 6.0, 1.6),
        }
    }
}

/// The generated categorical ramp.
///
/// Exposed because a wall in a bright room wants more chroma than one in a dark
/// office — but the status-hue guard is a correctness property, not taste: it is
/// what stops a data series being drawn in the colour that means "error".
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Ramp {
    pub chroma_min: f32,
    pub chroma_max: f32,
    pub lightness_min: f32,
    pub lightness_max: f32,
    pub lightness_step: f32,
    /// Degrees of hue reserved around each status colour.
    pub status_hue_guard: f32,
    /// Chroma retained by a value that has gone stale.
    pub stale_chroma: f32,
}

impl Default for Ramp {
    fn default() -> Self {
        Self {
            chroma_min: 0.08,
            chroma_max: 0.15,
            lightness_min: 0.30,
            lightness_max: 0.92,
            lightness_step: 0.12,
            status_hue_guard: 25.0,
            stale_chroma: 0.35,
        }
    }
}

impl Ramp {
    pub fn sanitized(self) -> Self {
        let default = Self::default();
        let chroma_min = clamp(self.chroma_min, 0.0, 0.4, default.chroma_min);
        let lightness_min = clamp(self.lightness_min, 0.05, 0.95, default.lightness_min);
        Self {
            chroma_min,
            // Ordering is enforced rather than trusted: an inverted range would
            // generate colours outside the ramp it is meant to bound.
            chroma_max: clamp(self.chroma_max, chroma_min, 0.4, default.chroma_max),
            lightness_min,
            lightness_max: clamp(self.lightness_max, lightness_min, 0.99, default.lightness_max),
            lightness_step: clamp(self.lightness_step, 0.01, 0.5, default.lightness_step),
            // Zero would let a series be drawn in the hue that means "error",
            // so the guard has a floor even when someone turns it down.
            status_hue_guard: clamp(self.status_hue_guard, 5.0, 90.0, default.status_hue_guard),
            stale_chroma: clamp(self.stale_chroma, 0.0, 1.0, default.stale_chroma),
        }
    }
}

/// How long an unheard-from agent stays fresh, stays visible, and how much more
/// salient a newcomer must be to take an occupied slot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Roster {
    pub stale_after_ms: u64,
    pub evict_after_ms: u64,
    pub admission_margin: u32,
}

impl Default for Roster {
    fn default() -> Self {
        Self {
            stale_after_ms: 15_000,
            evict_after_ms: 120_000,
            admission_margin: 10,
        }
    }
}

impl Roster {
    pub fn sanitized(self) -> Self {
        let stale_after_ms = self.stale_after_ms.clamp(1_000, 3_600_000);
        Self {
            stale_after_ms,
            // Evicting before going stale would remove a surface without ever
            // showing it as stale, which is the flicker the grace period exists
            // to prevent.
            evict_after_ms: self.evict_after_ms.max(stale_after_ms).min(86_400_000),
            admission_margin: self.admission_margin.min(1_000),
        }
    }
}

/// Presence rail and preview chrome.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Presence {
    pub rail_height: f32,
    pub preview_width: f32,
    pub preview_height: f32,
}

impl Default for Presence {
    fn default() -> Self {
        Self {
            rail_height: 58.0,
            preview_width: 360.0,
            preview_height: 270.0,
        }
    }
}

impl Presence {
    pub fn sanitized(self) -> Self {
        let default = Self::default();
        Self {
            rail_height: clamp(self.rail_height, 24.0, 240.0, default.rail_height),
            preview_width: clamp(self.preview_width, 120.0, 1_920.0, default.preview_width),
            preview_height: clamp(self.preview_height, 90.0, 1_080.0, default.preview_height),
        }
    }
}

/// Everything tunable, in one document.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Appearance {
    pub density: Density,
    pub panels: Panels,
    pub motion: Motion,
    pub limits: Limits,
    pub series: Series,
    pub ramp: Ramp,
    pub roster: Roster,
    pub presence: Presence,
}

impl Appearance {
    /// Clamp every group. Called once on load so the renderer never has to
    /// defend itself against a value from a file.
    pub fn sanitized(self) -> Self {
        Self {
            density: self.density.sanitized(),
            panels: self.panels.sanitized(),
            motion: self.motion.sanitized(),
            limits: self.limits.sanitized(),
            series: self.series.sanitized(),
            ramp: self.ramp.sanitized(),
            roster: self.roster.sanitized(),
            presence: self.presence.sanitized(),
        }
    }

    /// Parse a config, applying defaults for everything it does not mention.
    ///
    /// Fails loudly on an unknown key rather than ignoring it: a silently
    /// dropped setting is indistinguishable from one that had no effect, and
    /// telling those apart is the entire point of making these tunable.
    pub fn parse(json: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(json)
            .map(Self::sanitized)
            .map_err(|error| format!("invalid Canvas appearance config: {error}"))
    }

    /// Motion off, everything else untouched.
    pub fn still(self) -> Self {
        Self {
            motion: Motion::still(),
            ..self
        }
    }
}

/// The appearance this process is rendering with.
///
/// A lock rather than a `OnceLock` so a config can be re-read while Canvas is
/// running: the whole point of moving these out of the code was to stop a tweak
/// costing a rebuild, and a value that can only be set at startup still costs a
/// restart.
static CURRENT: std::sync::RwLock<Appearance> = std::sync::RwLock::new(Appearance::DEFAULT);

impl Appearance {
    /// The compile-time defaults, usable in a `const` context so the global has
    /// something valid before any config is read.
    pub const DEFAULT: Self = Self {
        density: Density { scale: 1.0 },
        panels: Panels {
            opacity: 0.92,
            muted_factor: 0.3,
        },
        motion: Motion {
            transition_ms: 280,
            background_max_fps: 60,
        },
        limits: Limits {
            table_rows: 8,
            event_lines: 7,
            summary_nodes: 8,
            series_colors: 6,
            visible_occupants: 4,
        },
        series: Series {
            line_width: 2.0,
            area_alpha: 0.22,
            head_radius_factor: 1.6,
        },
        ramp: Ramp {
            chroma_min: 0.08,
            chroma_max: 0.15,
            lightness_min: 0.30,
            lightness_max: 0.92,
            lightness_step: 0.12,
            status_hue_guard: 25.0,
            stale_chroma: 0.35,
        },
        roster: Roster {
            stale_after_ms: 15_000,
            evict_after_ms: 120_000,
            admission_margin: 10,
        },
        presence: Presence {
            rail_height: 58.0,
            preview_width: 360.0,
            preview_height: 270.0,
        },
    };
}

/// What the renderer should use right now.
pub fn current() -> Appearance {
    CURRENT
        .read()
        .map(|held| *held)
        .unwrap_or(Appearance::DEFAULT)
}

/// Install a new appearance, sanitizing first.
///
/// Returns the value actually installed, which may differ from what was asked
/// for — a caller that wants to report "your setting was clamped" needs to see
/// what landed rather than what it sent.
pub fn install(appearance: Appearance) -> Appearance {
    let sanitized = appearance.sanitized();
    if let Ok(mut held) = CURRENT.write() {
        *held = sanitized;
    }
    sanitized
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_exactly_the_constants_they_replace() {
        let a = Appearance::default();
        // Read out of the code this replaces; if one of these changes, the
        // change was a behaviour change and should be deliberate.
        assert_eq!(a.panels.opacity, 0.92, "canvas-core lib.rs panel_opacity");
        assert_eq!(a.panels.muted_factor, 0.3, "live.rs:261 opacity * 0.3");
        assert_eq!(a.motion.transition_ms, 280, "transition.rs TRANSITION_DURATION");
        assert_eq!(a.limits.table_rows, 8, "live.rs:518 take(8)");
        assert_eq!(a.limits.event_lines, 7, "live.rs:554 take(7)");
        assert_eq!(a.limits.summary_nodes, 8, "live.rs:595 take(8)");
        assert_eq!(a.limits.series_colors, 6, "live.rs SERIES_COLORS");
        assert_eq!(a.limits.visible_occupants, 4, "presence.rs MAX_VISIBLE_OCCUPANTS");
        assert_eq!(a.series.line_width, 2.0, "live.rs LINE_WIDTH");
        assert_eq!(a.series.area_alpha, 0.22, "live.rs AREA_ALPHA");
        assert_eq!(a.series.head_radius(), 3.2, "live.rs HEAD_RADIUS");
        assert_eq!(a.ramp.chroma_min, 0.08);
        assert_eq!(a.ramp.chroma_max, 0.15);
        assert_eq!(a.ramp.lightness_min, 0.30);
        assert_eq!(a.ramp.lightness_max, 0.92);
        assert_eq!(a.ramp.lightness_step, 0.12);
        assert_eq!(a.ramp.status_hue_guard, 25.0);
        assert_eq!(a.ramp.stale_chroma, 0.35, "live.rs STALE_CHROMA");
        assert_eq!(a.roster.stale_after_ms, 15_000);
        assert_eq!(a.roster.evict_after_ms, 120_000);
        assert_eq!(a.roster.admission_margin, 10);
        assert_eq!(a.presence.rail_height, 58.0);
        assert_eq!(a.presence.preview_width, 360.0);
        assert_eq!(a.presence.preview_height, 270.0);
        assert_eq!(a.density.scale, 1.0);
    }

    #[test]
    fn an_empty_config_is_the_defaults() {
        assert_eq!(Appearance::parse("{}").unwrap(), Appearance::default());
    }

    #[test]
    fn a_config_sets_only_what_it_overrides() {
        let a = Appearance::parse(r#"{"panels":{"opacity":1.0}}"#).unwrap();
        assert_eq!(a.panels.opacity, 1.0, "the override applies");
        assert_eq!(
            a.panels.muted_factor,
            Panels::default().muted_factor,
            "a sibling field is untouched"
        );
        assert_eq!(
            a.motion,
            Motion::default(),
            "an unmentioned group keeps its defaults"
        );
    }

    #[test]
    fn a_misspelled_key_is_refused_rather_than_ignored() {
        let error = Appearance::parse(r#"{"panels":{"opactiy":1.0}}"#).unwrap_err();
        assert!(error.contains("opactiy"), "{error}");
        assert!(
            Appearance::parse(r#"{"pannels":{"opacity":1.0}}"#).is_err(),
            "a dropped setting looks exactly like one that had no effect"
        );
    }

    #[test]
    fn no_value_can_make_a_panel_invisible() {
        let a = Appearance::parse(r#"{"panels":{"opacity":0.0}}"#).unwrap();
        assert!(a.panels.opacity >= 0.25, "an invisible panel is a lost widget");
    }

    #[test]
    fn a_broken_number_falls_back_instead_of_rendering_nothing() {
        let a = Appearance {
            panels: Panels {
                opacity: f32::NAN,
                muted_factor: f32::INFINITY,
            },
            ..Appearance::default()
        }
        .sanitized();
        assert_eq!(a.panels.opacity, 0.92);
        assert_eq!(a.panels.muted_factor, 0.3);
    }

    #[test]
    fn an_inverted_ramp_range_is_corrected_not_honoured() {
        let a = Appearance::parse(
            r#"{"ramp":{"chroma_min":0.30,"chroma_max":0.05,"lightness_min":0.8,"lightness_max":0.2}}"#,
        )
        .unwrap();
        assert!(a.ramp.chroma_max >= a.ramp.chroma_min);
        assert!(a.ramp.lightness_max >= a.ramp.lightness_min);
    }

    #[test]
    fn the_status_hue_guard_cannot_be_turned_off() {
        let a = Appearance::parse(r#"{"ramp":{"status_hue_guard":0.0}}"#).unwrap();
        assert!(
            a.ramp.status_hue_guard >= 5.0,
            "zero would let a data series be drawn in the colour that means error"
        );
    }

    #[test]
    fn eviction_can_never_precede_going_stale() {
        let a = Appearance::parse(
            r#"{"roster":{"stale_after_ms":60000,"evict_after_ms":1000}}"#,
        )
        .unwrap();
        assert!(
            a.roster.evict_after_ms >= a.roster.stale_after_ms,
            "a surface would vanish without ever being shown as stale"
        );
    }

    #[test]
    fn still_means_nothing_moves_at_all() {
        let a = Appearance::default().still();
        assert!(a.motion.is_still());
        assert_eq!(a.motion.transition_ms, 0);
        assert_eq!(a.motion.background_max_fps, 0);
        assert_eq!(a.density, Density::default(), "still changes only motion");
    }

    #[test]
    fn a_wall_sized_config_round_trips() {
        let wanted = Appearance::parse(
            r#"{"density":{"scale":1.6},"panels":{"opacity":1.0},
                "motion":{"transition_ms":0,"background_max_fps":0},
                "limits":{"table_rows":12}}"#,
        )
        .unwrap();
        assert_eq!(wanted.density.scale, 1.6);
        assert_eq!(wanted.panels.opacity, 1.0);
        assert!(wanted.motion.is_still());
        assert_eq!(wanted.limits.table_rows, 12);
        let json = serde_json::to_string(&wanted).unwrap();
        assert_eq!(Appearance::parse(&json).unwrap(), wanted);
    }
}
