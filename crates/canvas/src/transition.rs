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

/// Default transition duration: fast enough to feel immediate, slow enough to
/// follow a layout shift across three displays.
pub const TRANSITION_DURATION: Duration = Duration::from_millis(280);

/// The easing used for all Canvas transitions — EaseOutCubic, matching the
/// prior hand-rolled implementation but sourced from the shared `lilt` easing
/// table so the palette and placement transitions agree with widget animations.
const TRANSITION_EASING: Easing = Easing::EaseOutCubic;

/// Eased progress of a transition that started at `started`, 1.0 once done.
pub fn progress_at(started: Instant, duration: Duration) -> f32 {
    let elapsed = started.elapsed().as_secs_f32();
    let t = (elapsed / duration.as_secs_f32()).clamp(0.0, 1.0);
    TRANSITION_EASING.value(t)
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
    /// Previous normalized surface bounds, keyed by surface id.
    pub from_placements: std::collections::BTreeMap<String, [f32; 4]>,
    pub from_palette: ThemePalette,
}

impl Transition {
    pub fn new(
        started: Instant,
        from_placements: std::collections::BTreeMap<String, [f32; 4]>,
        from_palette: ThemePalette,
    ) -> Self {
        Self {
            started,
            duration: TRANSITION_DURATION,
            from_placements,
            from_palette,
        }
    }

    /// Eased interpolation factor; 1.0 means the transition is finished.
    pub fn factor(&self) -> f32 {
        progress_at(self.started, self.duration)
    }

    pub fn done(&self) -> bool {
        self.started.elapsed() >= self.duration
    }

    /// Interpolated bounds for a surface: lerps from its previous bounds when
    /// it has one; new surfaces appear at their target bounds (snap).
    pub fn placement(&self, surface_id: &str, target: [f32; 4]) -> [f32; 4] {
        let Some(from) = self.from_placements.get(surface_id) else {
            return target;
        };
        let t = self.factor();
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
        assert_eq!(TRANSITION_EASING.value(0.0), 0.0);
        assert_eq!(TRANSITION_EASING.value(1.0), 1.0);
        assert!(TRANSITION_EASING.value(0.25) > 0.25);
        assert!(TRANSITION_EASING.value(0.25) < TRANSITION_EASING.value(0.75));
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
        let transition = Transition::new(started, from.clone(), palette(0));
        assert!(!transition.done());
        let mid = transition.placement("a", [0.4, 0.4, 0.6, 0.6]);
        assert!(mid[0] > 0.0 && mid[0] < 0.4);

        let finished = Transition::new(
            Instant::now() - TRANSITION_DURATION - Duration::from_millis(50),
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
    fn new_surfaces_snap_to_target() {
        let transition = Transition::new(
            Instant::now(),
            std::collections::BTreeMap::new(),
            palette(0),
        );
        assert_eq!(
            transition.placement("brand-new", [0.1, 0.2, 0.3, 0.4]),
            [0.1, 0.2, 0.3, 0.4]
        );
    }
}
