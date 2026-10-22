use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use canvas_core::model_camera::ViewerCamera;
use iced::{Point, Size};
use serde::{Deserialize, Serialize};

use crate::orbit::{OrbitAction, OrbitController};
use crate::view::TimelineAction;

const PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    Press(Point),
    Drag(Point, Size),
    Release,
    Zoom(f32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Input {
    Gesture(Gesture),
    Camera(OrbitAction),
    Timeline(TimelineAction),
    ToggleOrbit,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub pose: ViewerCamera,
    pub status: String,
}

/// Result of applying viewer input. Local presentation changes are reported
/// independently from camera publication, which is deliberately throttled.
#[derive(Debug, Clone, PartialEq)]
pub struct Update {
    pub changed: bool,
    pub output: Option<Output>,
}

/// Tool-normalized timeline presentation. Tool-specific Lua binders choose
/// glyphs and labels; the reusable viewer only owns selection semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Timeline {
    pub stream_id: String,
    pub items: Vec<TimelineItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimelineItem {
    pub steps_back: u32,
    pub glyph: String,
    pub label: String,
    #[serde(default)]
    pub component: Option<String>,
    #[serde(default)]
    pub feature: Option<u64>,
}

#[derive(Debug, Default)]
pub struct Viewer {
    controllers: BTreeMap<String, OrbitController>,
    published: BTreeMap<String, Instant>,
    timelines: BTreeMap<String, Timeline>,
}

impl Viewer {
    pub fn timeline(&self, stream_id: &str) -> Option<&Timeline> {
        self.timelines.get(stream_id)
    }

    pub fn set_timeline(&mut self, timeline: Timeline) {
        self.timelines.insert(timeline.stream_id.clone(), timeline);
    }
    pub fn controller(&self, stream_id: &str) -> Option<&OrbitController> {
        self.controllers.get(stream_id)
    }

    pub fn controller_mut(&mut self, stream_id: &str) -> &mut OrbitController {
        self.controllers.entry(stream_id.to_owned()).or_default()
    }

    pub fn is_grabbed(&self, stream_id: &str) -> bool {
        self.controller(stream_id)
            .is_some_and(OrbitController::is_grabbed)
    }

    pub fn apply(&mut self, stream_id: &str, input: Input, now: Instant, unix_ms: u64) -> Update {
        let controller = self.controllers.entry(stream_id.to_owned()).or_default();
        let (changed, force, instant) = match input {
            Input::Gesture(Gesture::Press(at)) => {
                controller.stop_auto_orbit();
                controller.press(at);
                (true, true, false)
            }
            Input::Gesture(Gesture::Drag(to, size)) => (controller.drag(to, size), false, false),
            Input::Gesture(Gesture::Release) => {
                controller.release();
                (true, true, false)
            }
            Input::Gesture(Gesture::Zoom(steps)) => {
                controller.stop_auto_orbit();
                (controller.zoom(steps), false, false)
            }
            Input::Camera(action) => {
                controller.stop_auto_orbit();
                (action.apply(controller), true, true)
            }
            Input::Timeline(action) => {
                let changed = match action {
                    TimelineAction::Older => controller.step_older(),
                    TimelineAction::Newer => controller.step_newer(),
                    TimelineAction::Latest => controller.follow_latest(),
                    TimelineAction::Jump(back) => controller.select_steps_back(back),
                };
                (changed, true, true)
            }
            Input::ToggleOrbit => {
                controller.toggle_enabled();
                (true, true, true)
            }
        };
        if !changed {
            return Update {
                changed: false,
                output: None,
            };
        }
        if !force
            && self
                .published
                .get(stream_id)
                .is_some_and(|last| now.duration_since(*last) < PUBLISH_INTERVAL)
        {
            return Update {
                changed: true,
                output: None,
            };
        }
        self.published.insert(stream_id.to_owned(), now);
        let pose = if instant {
            controller.pose(stream_id, unix_ms).with_instant()
        } else {
            controller.pose(stream_id, unix_ms)
        };
        let status = if matches!(input, Input::ToggleOrbit) {
            format!(
                "{stream_id}: orbit {}",
                if controller.enabled() { "on" } else { "off" }
            )
        } else if let Some(view) = controller.named_view() {
            format!("{stream_id}: {view} view")
        } else {
            format!(
                "{stream_id}: {:.0} deg, {:.0} deg up",
                controller.yaw(),
                controller.pitch()
            )
        };
        Update {
            changed: true,
            output: Some(Output { pose, status }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_input_and_toggle_share_one_orbit_state() {
        let mut viewer = Viewer::default();
        let now = Instant::now();
        let initial = viewer
            .apply("part", Input::ToggleOrbit, now, 1)
            .output
            .expect("toggle publishes");
        assert!(!initial.pose.auto_orbit);
        let enabled = viewer
            .apply("part", Input::ToggleOrbit, now, 2)
            .output
            .expect("toggle publishes");
        assert!(enabled.pose.auto_orbit);
        let manual = viewer
            .apply(
                "part",
                Input::Gesture(Gesture::Zoom(1.0)),
                now + PUBLISH_INTERVAL,
                3,
            )
            .output
            .expect("zoom publishes");
        assert!(!manual.pose.auto_orbit);
    }

    #[test]
    fn direct_timeline_jump_is_an_instant_pose() {
        let mut viewer = Viewer::default();
        let output = viewer
            .apply(
                "part",
                Input::Timeline(TimelineAction::Jump(7)),
                Instant::now(),
                1,
            )
            .output
            .expect("jump publishes");
        assert_eq!(output.pose.steps_back, Some(7));
        assert!(output.pose.instant);
    }

    #[test]
    fn tool_timeline_is_retained_by_model_stream() {
        let mut viewer = Viewer::default();
        viewer.set_timeline(Timeline {
            stream_id: "part".into(),
            items: vec![TimelineItem {
                steps_back: 0,
                glyph: "↥".into(),
                label: "#8".into(),
                component: Some("lid".into()),
                feature: Some(42),
            }],
        });
        let item = &viewer.timeline("part").expect("timeline").items[0];
        assert_eq!(item.glyph, "↥");
        assert_eq!(item.component.as_deref(), Some("lid"));
    }

    #[test]
    fn throttled_drag_still_reports_a_local_change() {
        let mut viewer = Viewer::default();
        let now = Instant::now();
        let _ = viewer.apply(
            "part",
            Input::Gesture(Gesture::Press(Point::new(10.0, 10.0))),
            now,
            1,
        );
        let update = viewer.apply(
            "part",
            Input::Gesture(Gesture::Drag(
                Point::new(30.0, 20.0),
                Size::new(200.0, 200.0),
            )),
            now + Duration::from_millis(1),
            2,
        );
        assert!(update.changed);
        assert!(update.output.is_none());
        assert_ne!(viewer.controller("part").expect("controller").yaw(), 45.0);
    }
}
