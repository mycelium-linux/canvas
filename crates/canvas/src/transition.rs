//! Presentation-only transitions: logical state applies instantly, visuals
//! interpolate. Swap/recall stays fast — input is never locked to the
//! animation, and a dropped transition simply snaps to the target.
//!
//! Easing comes from `iced_core::animation::Easing` (re-exported from the
//! `lilt` crate), not a hand-rolled function: the same easing `lilt` uses for
//! the rest of the wall, and it carries correct interruption semantics so a
//! value that changes mid-transition retargets instead of snapping.

use canvas_core::ThemePalette;
use iced::animation::Easing;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    Fluid,
    Terminal,
    Precise,
}

impl Curve {
    pub fn from_theme(value: &str) -> Self {
        match value {
            "terminal" => Self::Terminal,
            "precise" => Self::Precise,
            _ => Self::Fluid,
        }
    }

    fn value(self, t: f32) -> f32 {
        match self {
            Self::Fluid => Easing::EaseOutCubic.value(t),
            // A quick linear acquisition reads like a terminal repaint while
            // remaining smooth and interruptible.
            Self::Terminal => Easing::Linear.value(t),
            // Imperial motion lands firmly without the luminous oversoften of
            // the fluid curve.
            Self::Precise => Easing::EaseOutQuad.value(t),
        }
    }
}

/// The bounds a brand-new surface starts from: its target shrunk about its
/// own center. Subtle enough to read as an arrival, not a zoom.
const ARRIVAL_SCALE: f32 = 0.86;

fn arrive_from(target: [f32; 4]) -> [f32; 4] {
    let [x, y, w, h] = target;
    let nw = w * ARRIVAL_SCALE;
    let nh = h * ARRIVAL_SCALE;
    [x + (w - nw) / 2.0, y + (h - nh) / 2.0, nw, nh]
}

/// Eased progress of a transition that started at `started`, 1.0 once done.
pub fn progress_at(started: Instant, duration: Duration, curve: Curve) -> f32 {
    if duration.is_zero() {
        return 1.0;
    }
    let elapsed = started.elapsed().as_secs_f32();
    let t = (elapsed / duration.as_secs_f32()).clamp(0.0, 1.0);
    curve.value(t)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

pub fn lerp_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (f32::from(a[0]) + (f32::from(b[0]) - f32::from(a[0])) * t).round() as u8,
        (f32::from(a[1]) + (f32::from(b[1]) - f32::from(a[1])) * t).round() as u8,
        (f32::from(a[2]) + (f32::from(b[2]) - f32::from(a[2])) * t).round() as u8,
    ]
}

pub fn lerp_palette(a: ThemePalette, b: ThemePalette, t: f32) -> ThemePalette {
    ThemePalette {
        background: lerp_rgb(a.background, b.background, t),
        background_strong: lerp_rgb(a.background_strong, b.background_strong, t),
        surface: lerp_rgb(a.surface, b.surface, t),
        surface_muted: lerp_rgb(a.surface_muted, b.surface_muted, t),
        text: lerp_rgb(a.text, b.text, t),
        text_muted: lerp_rgb(a.text_muted, b.text_muted, t),
        primary: lerp_rgb(a.primary, b.primary, t),
        success: lerp_rgb(a.success, b.success, t),
        warning: lerp_rgb(a.warning, b.warning, t),
        danger: lerp_rgb(a.danger, b.danger, t),
        info: lerp_rgb(a.info, b.info, t),
        border: lerp_rgb(a.border, b.border, t),
        border_strong: lerp_rgb(a.border_strong, b.border_strong, t),
    }
}

/// Snapshot of the presentation just before a workspace change. Only fields
/// that can be interpolated are captured; anything missing snaps instantly.
pub struct Transition {
    pub started: Instant,
    pub duration: Duration,
    pub curve: Curve,
    /// Previous normalized surface bounds, keyed by surface id.
    pub from_placements: std::collections::BTreeMap<String, [f32; 4]>,
    pub from_palette: ThemePalette,
}

impl Transition {
    pub fn new(
        started: Instant,
        duration: Duration,
        curve: Curve,
        from_placements: std::collections::BTreeMap<String, [f32; 4]>,
        from_palette: ThemePalette,
    ) -> Self {
        Self {
            started,
            duration,
            curve,
            from_placements,
            from_palette,
        }
    }

    /// Eased interpolation factor; 1.0 means the transition is finished.
    pub fn factor(&self) -> f32 {
        progress_at(self.started, self.duration, self.curve)
    }

    pub fn done(&self) -> bool {
        self.started.elapsed() >= self.duration
    }

    /// Interpolated bounds for a surface: lerps from its previous bounds when
    /// it has one. A surface with no predecessor — one that only exists in the
    /// incoming workspace — eases in from a shrunk, centered version of its
    /// target instead of snapping. This is what makes a workspace switch replay
    /// the arrival animation: different projects share few surface ids, so
    /// without it the only motion would be the background crossfade.
    pub fn placement(&self, surface_id: &str, target: [f32; 4]) -> [f32; 4] {
        let t = self.factor();
        let from = self
            .from_placements
            .get(surface_id)
            .copied()
            .unwrap_or_else(|| arrive_from(target));
        [
            lerp(from[0], target[0], t),
            lerp(from[1], target[1], t),
            lerp(from[2], target[2], t),
            lerp(from[3], target[3], t),
        ]
    }

    pub fn palette(&self, target: ThemePalette) -> ThemePalette {
        lerp_palette(self.from_palette, target, self.factor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(rgb: u8) -> ThemePalette {
        let c = [rgb, rgb, rgb];
        ThemePalette {
            background: c,
            background_strong: c,
            surface: c,
            surface_muted: c,
            text: c,
            text_muted: c,
            primary: c,
            success: c,
            warning: c,
            danger: c,
            info: c,
            border: c,
            border_strong: c,
        }
    }

    #[test]
    fn easing_starts_fast_and_lands_soft() {
        // EaseOutCubic from lilt: 0 → 0, 1 → 1, fast start, soft landing.
        assert_eq!(Curve::Fluid.value(0.0), 0.0);
        assert_eq!(Curve::Fluid.value(1.0), 1.0);
        assert!(Curve::Fluid.value(0.25) > 0.25);
        assert!(Curve::Fluid.value(0.25) < Curve::Fluid.value(0.75));
        assert_ne!(Curve::Fluid.value(0.5), Curve::Terminal.value(0.5));
        assert_ne!(Curve::Fluid.value(0.5), Curve::Precise.value(0.5));
    }

    #[test]
    fn lerp_is_bounded_and_exact_at_endpoints() {
        assert_eq!(lerp(0.0, 10.0, 0.0), 0.0);
        assert_eq!(lerp(0.0, 10.0, 1.0), 10.0);
        assert_eq!(lerp(0.0, 10.0, 0.5), 5.0);
        assert_eq!(lerp(0.0, 10.0, 2.0), 10.0);
        assert_eq!(lerp(0.0, 10.0, -1.0), 0.0);
        assert_eq!(lerp_rgb([0; 3], [255; 3], 0.5), [128; 3]);
    }

    #[test]
    fn palette_interpolation_covers_all_roles() {
        let mid = lerp_palette(palette(0), palette(255), 0.5);
        assert_eq!(mid.background, [128; 3]);
        assert_eq!(mid.border_strong, [128; 3]);
        assert_eq!(lerp_palette(palette(10), palette(20), 0.0), palette(10));
    }

    #[test]
    fn transition_finishes_and_snaps() {
        let started = Instant::now() - Duration::from_millis(10);
        let mut from = std::collections::BTreeMap::new();
        from.insert("a".to_string(), [0.0, 0.0, 0.5, 0.5]);
        let duration = Duration::from_millis(280);
        let transition = Transition::new(started, duration, Curve::Fluid, from.clone(), palette(0));
        assert!(!transition.done());
        let mid = transition.placement("a", [0.4, 0.4, 0.6, 0.6]);
        assert!(mid[0] > 0.0 && mid[0] < 0.4);

        let finished = Transition::new(
            Instant::now() - duration - Duration::from_millis(50),
            duration,
            Curve::Fluid,
            from,
            palette(0),
        );
        assert!(finished.done());
        assert_eq!(
            finished.placement("a", [0.4, 0.4, 0.6, 0.6]),
            [0.4, 0.4, 0.6, 0.6]
        );
        assert_eq!(finished.palette(palette(9)), palette(9));
    }

    #[test]
    fn new_surfaces_arrive_scaled_and_land_on_target() {
        let target = [0.1, 0.2, 0.3, 0.4];
        let fresh = Transition::new(
            Instant::now(),
            Duration::from_millis(280),
            Curve::Fluid,
            std::collections::BTreeMap::new(),
            palette(0),
        );
        let start = fresh.placement("brand-new", target);
        // Smaller than the target (an arrival, not a snap) and centered on it.
        assert!(start[2] < target[2] && start[3] < target[3], "{start:?}");
        let start_cx = start[0] + start[2] / 2.0;
        let start_cy = start[1] + start[3] / 2.0;
        assert!((start_cx - (target[0] + target[2] / 2.0)).abs() < 1e-6);
        assert!((start_cy - (target[1] + target[3] / 2.0)).abs() < 1e-6);
        // A finished transition puts a new surface exactly on its target, so
        // the arrival can never leave a surface resting at the shrunk bounds.
        let settled = Transition::new(
            Instant::now() - Duration::from_millis(330),
            Duration::from_millis(280),
            Curve::Fluid,
            std::collections::BTreeMap::new(),
            palette(0),
        );
        assert_eq!(settled.placement("brand-new", target), target);
    }

    #[test]
    fn zero_duration_motion_snaps_without_nan() {
        let transition = Transition::new(
            Instant::now(),
            Duration::ZERO,
            Curve::Fluid,
            std::collections::BTreeMap::new(),
            palette(0),
        );
        assert!(transition.done());
        assert_eq!(transition.factor(), 1.0);
    }
}
