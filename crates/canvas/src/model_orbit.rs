//! Turning pointer and key input into a camera pose for a streamed model.
//!
//! The model is rendered on another machine and arrives as video, so none of
//! this can move the picture directly — it decides a pose and something else
//! carries it back to the renderer. That separation is why this file has no
//! GPU, no filesystem and no clock in it: the part worth being sure about is
//! the arithmetic between a drag and an angle, and it is testable on its own.
//!
//! Drag, keybind and view cube all end in the same two numbers. A cube face is
//! not a special kind of camera move, it is a named pose, so the three ways of
//! aiming cannot drift apart.

use canvas_core::model_camera::{
    MAX_DISTANCE, MAX_PITCH_DEGREES, MIN_DISTANCE, ViewerCamera, named_view, view_at,
};
use iced::{Point, Size};

/// Yaw covered by dragging the full width of the surface.
///
/// Three quarters of a turn across the whole surface: enough to get behind a
/// part without letting go, while a small correction stays small. Scaling to
/// the surface rather than to pixels means the same gesture turns the model
/// the same amount on a laptop tile and on the TV.
const YAW_PER_WIDTH: f32 = 270.0;

/// Pitch covered by dragging the full height. Less than a half turn because
/// pitch stops at the poles and cannot usefully wrap.
const PITCH_PER_HEIGHT: f32 = 180.0;

/// How far one arrow key press turns the model.
const NUDGE_DEGREES: f32 = 15.0;

/// Multiplier applied per zoom step.
const ZOOM_STEP: f32 = 1.12;

#[derive(Debug, Clone, PartialEq)]
pub struct OrbitController {
    yaw: f32,
    pitch: f32,
    distance: f32,
    steps_back: Option<u32>,
    /// Where the pointer was at the last movement, while a drag is in flight.
    anchor: Option<Point>,
}

impl Default for OrbitController {
    fn default() -> Self {
        // Opens where a CAD tool opens: the corner that shows three faces.
        let (yaw, pitch) = named_view("iso").unwrap_or((45.0, 35.264));
        Self {
            yaw,
            pitch,
            distance: 1.0,
            steps_back: None,
            anchor: None,
        }
    }
}

impl OrbitController {
    pub fn yaw(&self) -> f32 {
        self.yaw
    }

    pub fn pitch(&self) -> f32 {
        self.pitch
    }

    pub fn distance(&self) -> f32 {
        self.distance
    }

    pub fn is_grabbed(&self) -> bool {
        self.anchor.is_some()
    }

    pub fn steps_back(&self) -> Option<u32> {
        self.steps_back
    }

    pub fn step_older(&mut self) -> bool {
        let next = self.steps_back.unwrap_or(0).saturating_add(1);
        if self.steps_back == Some(next) {
            return false;
        }
        self.steps_back = Some(next);
        true
    }

    pub fn step_newer(&mut self) -> bool {
        let Some(current) = self.steps_back else {
            return false;
        };
        self.steps_back = Some(current.saturating_sub(1));
        true
    }

    pub fn follow_latest(&mut self) -> bool {
        if self.steps_back == Some(0) {
            return false;
        }
        self.steps_back = Some(0);
        true
    }

    /// The named view currently being looked down, for a cube to highlight.
    pub fn named_view(&self) -> Option<&'static str> {
        view_at(self.yaw, self.pitch)
    }

    pub fn press(&mut self, at: Point) {
        self.anchor = Some(at);
    }

    pub fn release(&mut self) {
        self.anchor = None;
    }

    /// Continue a drag. Returns whether the pose actually moved.
    ///
    /// Movement is measured against the previous position rather than the
    /// press, so the pose accumulates. Measuring from the press would make the
    /// model spring back to its starting angle whenever the pointer returned
    /// near where it went down, which is not how anything holds still.
    pub fn drag(&mut self, to: Point, surface: Size) -> bool {
        let Some(anchor) = self.anchor else {
            return false;
        };
        if surface.width <= 1.0 || surface.height <= 1.0 {
            return false;
        }
        let dx = (to.x - anchor.x) / surface.width;
        let dy = (to.y - anchor.y) / surface.height;
        self.anchor = Some(to);
        if dx == 0.0 && dy == 0.0 {
            return false;
        }
        // Dragging right turns the model's right side toward the viewer, and
        // dragging down tips its top toward them: the model follows the hand,
        // which is the convention every CAD tool shares.
        self.yaw = wrap_degrees(self.yaw - dx * YAW_PER_WIDTH);
        self.pitch =
            (self.pitch + dy * PITCH_PER_HEIGHT).clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES);
        true
    }

    /// Scroll or pinch. Positive zooms in.
    pub fn zoom(&mut self, steps: f32) -> bool {
        if !steps.is_finite() || steps == 0.0 {
            return false;
        }
        let before = self.distance;
        self.distance = (self.distance / ZOOM_STEP.powf(steps)).clamp(MIN_DISTANCE, MAX_DISTANCE);
        self.distance != before
    }

    /// Jump to a named view — a cube face, or the key bound to it.
    pub fn snap(&mut self, view: &str) -> bool {
        let Some((yaw, pitch)) = named_view(view) else {
            return false;
        };
        self.yaw = yaw;
        self.pitch = pitch;
        true
    }

    /// Turn by one arrow key press.
    pub fn nudge(&mut self, yaw_steps: f32, pitch_steps: f32) -> bool {
        if yaw_steps == 0.0 && pitch_steps == 0.0 {
            return false;
        }
        self.yaw = wrap_degrees(self.yaw + yaw_steps * NUDGE_DEGREES);
        self.pitch =
            (self.pitch + pitch_steps * NUDGE_DEGREES).clamp(-MAX_PITCH_DEGREES, MAX_PITCH_DEGREES);
        true
    }

    /// Back to the opening view, including distance.
    pub fn reset(&mut self) -> bool {
        *self = Self::default();
        true
    }

    /// The pose to publish. Stamped by the caller, which owns the clock.
    pub fn pose(&self, stream_id: &str, at_unix_ms: u64) -> ViewerCamera {
        let mut pose = ViewerCamera::new(
            stream_id,
            self.yaw,
            self.pitch,
            self.distance,
            self.is_grabbed(),
            at_unix_ms,
        );
        pose.steps_back = self.steps_back;
        pose
    }
}

fn wrap_degrees(value: f32) -> f32 {
    if !value.is_finite() {
        return 0.0;
    }
    let wrapped = value % 360.0;
    if wrapped > 180.0 {
        wrapped - 360.0
    } else if wrapped < -180.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// The keys that aim the camera, and what they mean.
///
/// A table rather than a match arm per key so the same list can be shown to
/// someone as the surface's help text. A control nobody can discover may as
/// well not be bound.
pub const KEYBINDS: [(&str, &str, &str); 11] = [
    ("1", "front", "Front"),
    ("2", "back", "Back"),
    ("3", "right", "Right"),
    ("4", "left", "Left"),
    ("5", "top", "Top"),
    ("6", "bottom", "Bottom"),
    ("7", "iso", "Isometric"),
    ("8", "iso_rear", "Isometric, rear"),
    ("0", "reset", "Reset the view"),
    ("arrows", "nudge", "Turn 15°"),
    ("+ / -", "zoom", "Zoom"),
];

/// What a key does, if anything.
///
/// Returns `None` for keys this surface does not claim, so they fall through
/// to whatever else the workspace binds them to rather than being swallowed.
pub fn action_for_key(key: &str) -> Option<OrbitAction> {
    match key {
        "0" => Some(OrbitAction::Reset),
        "ArrowLeft" => Some(OrbitAction::Nudge(-1.0, 0.0)),
        "ArrowRight" => Some(OrbitAction::Nudge(1.0, 0.0)),
        "ArrowUp" => Some(OrbitAction::Nudge(0.0, 1.0)),
        "ArrowDown" => Some(OrbitAction::Nudge(0.0, -1.0)),
        "+" | "=" => Some(OrbitAction::Zoom(1.0)),
        "-" | "_" => Some(OrbitAction::Zoom(-1.0)),
        other => KEYBINDS
            .iter()
            .find(|(binding, view, _)| *binding == other && *view != "reset")
            .map(|(_, view, _)| OrbitAction::Snap(view)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrbitAction {
    Snap(&'static str),
    Nudge(f32, f32),
    Zoom(f32),
    Reset,
}

impl OrbitAction {
    pub fn apply(self, controller: &mut OrbitController) -> bool {
        match self {
            Self::Snap(view) => controller.snap(view),
            Self::Nudge(yaw, pitch) => controller.nudge(yaw, pitch),
            Self::Zoom(steps) => controller.zoom(steps),
            Self::Reset => controller.reset(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface() -> Size {
        Size::new(800.0, 600.0)
    }

    #[test]
    fn it_opens_on_the_isometric_corner() {
        let controller = OrbitController::default();
        assert_eq!(controller.named_view(), Some("iso"));
        assert_eq!(controller.distance(), 1.0);
        assert!(!controller.is_grabbed());
    }

    /// Movement accumulates from the last position, not from the press.
    /// Measuring from the press would snap the model back to its starting
    /// angle every time the pointer passed near where it went down.
    #[test]
    fn a_drag_accumulates_rather_than_measuring_from_the_press() {
        let mut controller = OrbitController::default();
        let start = controller.yaw();
        controller.press(Point::new(400.0, 300.0));
        controller.drag(Point::new(500.0, 300.0), surface());
        let after_first = controller.yaw();
        controller.drag(Point::new(600.0, 300.0), surface());
        let after_second = controller.yaw();

        let first = after_first - start;
        let second = after_second - after_first;
        assert!((first - second).abs() < 0.01, "{first} then {second}");
        // And returning the pointer to the press point must not undo the turn.
        controller.drag(Point::new(400.0, 300.0), surface());
        assert!((controller.yaw() - start).abs() < 0.01);
    }

    #[test]
    fn dragging_the_full_width_turns_three_quarters_around() {
        // Started from the front so the arithmetic is readable: half a width
        // is half of YAW_PER_WIDTH, dragged right, so yaw goes negative.
        let mut controller = OrbitController::default();
        controller.snap("front");
        controller.press(Point::new(0.0, 300.0));
        controller.drag(Point::new(400.0, 300.0), surface());
        assert!(
            (controller.yaw() + YAW_PER_WIDTH / 2.0).abs() < 0.01,
            "half a width turned to {}",
            controller.yaw()
        );

        // A full width is 270 degrees, which is past the wrap: the pose must
        // report the equivalent heading rather than a number that keeps
        // growing. -270 and +90 are the same direction.
        let mut controller = OrbitController::default();
        controller.snap("front");
        controller.press(Point::new(0.0, 300.0));
        controller.drag(Point::new(800.0, 300.0), surface());
        assert!(
            (controller.yaw() - 90.0).abs() < 0.01,
            "{}",
            controller.yaw()
        );
    }

    /// The same gesture must mean the same thing on a small tile and a wall,
    /// which is why the sensitivity is relative to the surface.
    #[test]
    fn sensitivity_follows_the_surface_not_the_pixels() {
        let mut small = OrbitController::default();
        small.press(Point::new(0.0, 0.0));
        small.drag(Point::new(200.0, 0.0), Size::new(200.0, 150.0));

        let mut large = OrbitController::default();
        large.press(Point::new(0.0, 0.0));
        large.drag(Point::new(1920.0, 0.0), Size::new(1920.0, 1080.0));

        assert!((small.yaw() - large.yaw()).abs() < 0.01);
    }

    #[test]
    fn dragging_without_pressing_does_nothing() {
        let mut controller = OrbitController::default();
        assert!(!controller.drag(Point::new(50.0, 50.0), surface()));
        assert_eq!(controller, OrbitController::default());
    }

    /// A surface can be laid out at zero width for a frame during a transition;
    /// dividing by it would put NaN into the pose and stop the camera dead.
    #[test]
    fn a_collapsed_surface_cannot_produce_a_broken_pose() {
        let mut controller = OrbitController::default();
        controller.press(Point::new(10.0, 10.0));
        assert!(!controller.drag(Point::new(50.0, 50.0), Size::new(0.0, 0.0)));
        assert!(controller.yaw().is_finite() && controller.pitch().is_finite());
    }

    #[test]
    fn pitch_stops_at_the_pole_however_far_the_drag_goes() {
        let mut controller = OrbitController::default();
        controller.press(Point::new(400.0, 0.0));
        for _ in 0..20 {
            controller.drag(Point::new(400.0, 600.0), surface());
            controller.press(Point::new(400.0, 0.0));
        }
        assert_eq!(controller.pitch(), MAX_PITCH_DEGREES);
    }

    /// Yaw is a heading, so it wraps rather than growing. A minute of spinning
    /// must not leave the pose carrying a number like 7000.
    #[test]
    fn yaw_wraps_instead_of_accumulating() {
        let mut controller = OrbitController::default();
        controller.press(Point::new(0.0, 300.0));
        for _ in 0..40 {
            controller.drag(Point::new(800.0, 300.0), surface());
            controller.press(Point::new(0.0, 300.0));
        }
        assert!(controller.yaw().abs() <= 180.0, "{}", controller.yaw());
    }

    #[test]
    fn every_face_key_snaps_to_its_view() {
        for (binding, view, _) in KEYBINDS {
            if view == "reset" || view == "nudge" || view == "zoom" {
                continue;
            }
            let mut controller = OrbitController::default();
            let action = action_for_key(binding).expect("bound");
            assert!(action.apply(&mut controller));
            assert_eq!(controller.named_view(), Some(view), "{binding}");
        }
    }

    /// Keys the surface does not claim must fall through, or a model on the
    /// board would eat typing meant for the prompt beside it.
    #[test]
    fn unclaimed_keys_are_left_alone() {
        assert!(action_for_key("q").is_none());
        assert!(action_for_key("9").is_none());
        assert!(action_for_key("Enter").is_none());
        assert!(action_for_key("").is_none());
    }

    #[test]
    fn arrows_turn_and_reset_returns_to_the_opening_view() {
        let mut controller = OrbitController::default();
        assert!(action_for_key("ArrowRight").unwrap().apply(&mut controller));
        assert!(
            (controller.yaw() - (OrbitController::default().yaw() + NUDGE_DEGREES)).abs() < 0.01
        );
        assert!(action_for_key("ArrowUp").unwrap().apply(&mut controller));
        assert!(controller.pitch() > OrbitController::default().pitch());

        controller.zoom(3.0);
        assert!(action_for_key("0").unwrap().apply(&mut controller));
        assert_eq!(controller, OrbitController::default());
    }

    #[test]
    fn zoom_is_bounded_in_both_directions() {
        let mut controller = OrbitController::default();
        for _ in 0..200 {
            controller.zoom(1.0);
        }
        assert_eq!(controller.distance(), MIN_DISTANCE);
        for _ in 0..400 {
            controller.zoom(-1.0);
        }
        assert_eq!(controller.distance(), MAX_DISTANCE);
        // Once pinned, a further step is not a change worth publishing.
        assert!(!controller.zoom(-1.0));
        assert!(!controller.zoom(f32::NAN));
    }

    /// The pose carries the grab, because a held camera must not expire out
    /// from under someone studying one angle.
    #[test]
    fn the_pose_reports_whether_the_pointer_is_down() {
        let mut controller = OrbitController::default();
        assert!(!controller.pose("m", 5).grabbed);
        controller.press(Point::new(1.0, 1.0));
        let held = controller.pose("transmog-step", 5);
        assert!(held.grabbed);
        assert_eq!(held.stream_id, "transmog-step");
        assert_eq!(held.at_unix_ms, 5);
        controller.release();
        assert!(!controller.pose("m", 6).grabbed);
    }

    #[test]
    fn timeline_cursor_is_absolute_from_the_latest_step() {
        let mut controller = OrbitController::default();
        assert_eq!(controller.pose("m", 1).steps_back, None);
        assert!(controller.step_older());
        assert!(controller.step_older());
        assert_eq!(controller.pose("m", 2).steps_back, Some(2));
        assert!(controller.step_newer());
        assert_eq!(controller.pose("m", 3).steps_back, Some(1));
        assert!(controller.follow_latest());
        assert_eq!(controller.pose("m", 4).steps_back, Some(0));
        assert!(!controller.follow_latest());
    }

    /// Whatever the input, the pose handed to the renderer is in range.
    #[test]
    fn every_route_produces_a_pose_the_renderer_accepts() {
        let mut controller = OrbitController::default();
        controller.press(Point::new(0.0, 0.0));
        for step in 0..50 {
            controller.drag(
                Point::new(step as f32 * 97.0, step as f32 * -83.0),
                surface(),
            );
            controller.zoom(if step % 2 == 0 { 2.0 } else { -3.0 });
        }
        let pose = controller.pose("m", 0);
        assert!(pose.yaw_degrees.abs() <= 180.0);
        assert!(pose.pitch_degrees.abs() <= MAX_PITCH_DEGREES);
        assert!((MIN_DISTANCE..=MAX_DISTANCE).contains(&pose.distance));
        let [x, y, z] = pose.direction();
        assert!(x.is_finite() && y.is_finite() && z.is_finite());
        assert!(((x * x + y * y + z * z).sqrt() - 1.0).abs() < 1e-4);
    }
}
