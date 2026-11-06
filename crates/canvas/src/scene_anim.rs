//! Scene transitions: what the host does between two retained scenes.
//!
//! A Spore presents *states*, not frames. When the same node id comes back with a
//! different transform or opacity, the host eases from the old value to the new
//! one, so a panel opens instead of appearing and a row slides instead of jumping.
//! The producer stays a pure function of its snapshot, which is the whole reason
//! this belongs here rather than in Lua.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use canvas_workspace::{NodeContent, Scene, SceneNode};

/// Long enough to read as motion, short enough that a repeated toggle never feels
/// queued. `out_cubic` because a UI element decelerating into place is what the
/// eye expects; a linear ramp reads mechanical.
const DEFAULT_MS: u64 = 170;

struct Track {
    from: Scene,
    to: Scene,
    started: Instant,
    duration: Duration,
    /// Set by `begin`: the panel is opening, so the *next* present completes the
    /// easing rather than starting a new one.
    seeded: bool,
}

#[derive(Default)]
pub struct Animator {
    tracks: BTreeMap<String, Track>,
}

impl Animator {
    /// Record a newly presented scene as the target of a transition.
    ///
    /// A scene with no predecessor snaps: the first frame of a window should not
    /// fade in from nothing, because there was nothing.
    pub fn present(&mut self, key: &str, scene: &Scene, now: Instant) {
        let Some(previous) = self.current(key, now) else {
            self.tracks.insert(
                key.to_owned(),
                Track {
                    from: scene.clone(),
                    to: scene.clone(),
                    started: now,
                    duration: Duration::ZERO,
                    seeded: false,
                },
            );
            return;
        };
        // A seeded open keeps its clock: the panel eases from collapsed to
        // whatever it now says, and restarting the timer on every present would
        // turn an animation into a permanent lag.
        if let Some(track) = self.tracks.get_mut(key)
            && track.seeded
            && now.saturating_duration_since(track.started) < track.duration
        {
            track.to = scene.clone();
            return;
        }
        // Otherwise a producer has to ask: a scene that changes every frame (a
        // live animation) must not be smoothed into mush by a default easing.
        let duration = Duration::from_millis(transition_ms(scene));
        if std::env::var_os("CANVAS_TRANSITION_TRACE").is_some() {
            eprintln!("scene transition {key}: {duration:?}");
        }
        if duration.is_zero() || !changes(&previous, scene) {
            self.tracks.insert(
                key.to_owned(),
                Track {
                    from: scene.clone(),
                    to: scene.clone(),
                    started: now,
                    duration: Duration::ZERO,
                    seeded: false,
                },
            );
            return;
        }
        self.tracks.insert(
            key.to_owned(),
            Track {
                from: previous,
                to: scene.clone(),
                started: now,
                duration,
                seeded: false,
            },
        );
    }

    /// Seed the previous state so the next `present` eases in from nothing.
    ///
    /// Opening a panel is a change of *presence*, not of content: the scene is the
    /// same one it will show once it is open, so without a seeded predecessor
    /// there is nothing to interpolate from and the panel simply appears.
    pub fn begin(&mut self, key: &str, scene: &Scene, now: Instant) {
        let mut collapsed = scene.clone();
        for node in &mut collapsed.nodes {
            match &mut node.content {
                NodeContent::Rect { opacity, radius, .. } => {
                    *opacity = 0.0;
                    *radius *= 0.9;
                }
                NodeContent::Text { opacity, .. } => *opacity = 0.0,
                _ => {}
            }
            node.transform.scale = [
                node.transform.scale[0] * 0.985,
                node.transform.scale[1] * 0.985,
                node.transform.scale[2],
            ];
        }
        self.tracks.insert(
            key.to_owned(),
            Track {
                from: collapsed.clone(),
                to: collapsed,
                started: now,
                // An opening always eases: presence is the thing being animated,
                // and the producer has nothing to declare about it.
                duration: Duration::from_millis(match transition_ms(scene) {
                    0 => DEFAULT_MS,
                    declared => declared,
                }),
                seeded: true,
            },
        );
    }

    /// The scene as it should be drawn right now: interpolated while a transition
    /// is running, the target afterwards.
    pub fn frame(&self, key: &str, now: Instant) -> Option<Scene> {
        let track = self.tracks.get(key)?;
        if track.duration.is_zero() {
            return Some(track.to.clone());
        }
        let elapsed = now.saturating_duration_since(track.started);
        if elapsed >= track.duration {
            return Some(track.to.clone());
        }
        let t = elapsed.as_secs_f32() / track.duration.as_secs_f32();
        Some(blend(&track.from, &track.to, ease_out_cubic(t)))
    }

    /// Whether anything is mid-transition, so the compositor can keep drawing
    /// without pinning itself to 60 Hz forever.
    pub fn animating(&self, now: Instant) -> bool {
        self.tracks.values().any(|track| {
            !track.duration.is_zero()
                && now.saturating_duration_since(track.started) < track.duration
        })
    }

    fn current(&self, key: &str, now: Instant) -> Option<Scene> {
        self.frame(key, now)
    }
}

fn transition_ms(scene: &Scene) -> u64 {
    // A tuning knob, and the only way to see a 170 ms transition in a screenshot.
    if let Ok(value) = std::env::var("CANVAS_TRANSITION_MS")
        && let Ok(ms) = value.parse::<u64>()
    {
        return ms;
    }
    scene
        .nodes
        .iter()
        .filter_map(|node| node.transition_ms)
        .min()
        // No declaration means snap. A default here would smooth every animated
        // producer into a lag, which is the opposite of what the host is for.
        .unwrap_or(0)
}

/// Whether anything worth easing actually differs. A scene that only changed a
/// label should not slide.
fn changes(from: &Scene, to: &Scene) -> bool {
    to.nodes.iter().any(|node| {
        let Some(previous) = from.nodes.iter().find(|other| other.id == node.id) else {
            return false;
        };
        previous.transform.translation != node.transform.translation
            || previous.transform.scale != node.transform.scale
            || opacity(&previous.content) != opacity(&node.content)
    })
}

fn opacity(content: &NodeContent) -> f32 {
    match content {
        NodeContent::Rect { opacity, .. } | NodeContent::Text { opacity, .. } => *opacity,
        _ => 1.0,
    }
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let inverted = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inverted * inverted * inverted
}

fn blend(from: &Scene, to: &Scene, t: f32) -> Scene {
    Scene {
        nodes: to
            .nodes
            .iter()
            .map(|node| {
                let Some(previous) = from.nodes.iter().find(|other| other.id == node.id) else {
                    // A new node has nothing to ease from; it arrives with the
                    // frame that introduced it.
                    return node.clone();
                };
                SceneNode {
                    transform: canvas_workspace::Transform3 {
                        translation: lerp3(previous.transform.translation, node.transform.translation, t),
                        rotation: node.transform.rotation,
                        scale: lerp3(previous.transform.scale, node.transform.scale, t),
                    },
                    content: blend_content(&previous.content, &node.content, t),
                    ..node.clone()
                }
            })
            .collect(),
    }
}

fn blend_content(from: &NodeContent, to: &NodeContent, t: f32) -> NodeContent {
    match (from, to) {
        (
            NodeContent::Rect {
                opacity: a, radius: ra, ..
            },
            NodeContent::Rect {
                ink,
                rgb,
                opacity: b,
                radius: rb,
            },
        ) => NodeContent::Rect {
            ink: *ink,
            rgb: *rgb,
            opacity: a + (b - a) * t,
            radius: ra + (rb - ra) * t,
        },
        (
            NodeContent::Text { opacity: a, .. },
            NodeContent::Text { text, ink, rgb, opacity: b },
        ) => NodeContent::Text {
            text: text.clone(),
            ink: *ink,
            rgb: *rgb,
            opacity: a + (b - a) * t,
        },
        _ => to.clone(),
    }
}

fn lerp3(from: [f32; 3], to: [f32; 3], t: f32) -> [f32; 3] {
    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_workspace::{Scene, SceneNode, SemanticInk, Transform3};

    /// Opacity of a node's content, for assertions about easing.
    trait ContentOpacity {
        fn content_opacity(&self) -> f32;
    }
    impl ContentOpacity for SceneNode {
        fn content_opacity(&self) -> f32 {
            opacity(&self.content)
        }
    }

    fn panel_declaring(opacity: f32, y: f32, transition_ms: Option<u64>) -> Scene {
        Scene {
            nodes: vec![SceneNode {
                id: "panel".into(),
                parent_id: None,
                transform: Transform3 {
                    translation: [0.0, y, 0.0],
                    ..Default::default()
                },
                content: NodeContent::Rect {
                    ink: Some(SemanticInk::Surface),
                    rgb: None,
                    opacity,
                    radius: 8.0,
                },
                transition_ms,
            }],
        }
    }

    fn panel(opacity: f32, y: f32) -> Scene {
        panel_declaring(opacity, y, Some(DEFAULT_MS))
    }

    #[test]
    fn a_seeded_begin_eases_in_from_nothing() {
        let mut animator = Animator::default();
        let start = Instant::now();
        let open = panel(1.0, 0.0);
        animator.begin("k", &open, start);
        animator.present("k", &open, start);
        let mid = start + Duration::from_millis(DEFAULT_MS / 2);
        let opacity = animator.frame("k", mid).unwrap().nodes[0].content_opacity();
        assert!((0.2..0.95).contains(&opacity), "eased in to {opacity}");
        assert!(animator.animating(mid));
    }

    #[test]
    fn a_change_eases_and_a_repeat_does_not() {
        let mut animator = Animator::default();
        let start = Instant::now();
        animator.present("k", &panel_declaring(0.0, 0.0, None), start);
        assert_eq!(animator.frame("k", start).unwrap().nodes[0].content_opacity(), 0.0);

        animator.present("k", &panel_declaring(1.0, 1.0, Some(DEFAULT_MS)), start);
        let mid = start + Duration::from_millis(DEFAULT_MS / 2);
        let value = animator.frame("k", mid).unwrap().nodes[0].content_opacity();
        assert!((0.4..1.0).contains(&value), "eased opacity {value}");
        assert!(animator.animating(mid));

        let end = start + Duration::from_millis(DEFAULT_MS + 1);
        assert!(!animator.animating(end));
        assert_eq!(animator.frame("k", end).unwrap().nodes[0].content_opacity(), 1.0);
    }

    #[test]
    fn a_new_node_arrives_without_easing() {
        let mut animator = Animator::default();
        let start = Instant::now();
        animator.present("k", &panel(1.0, 0.0), start);
        let mut next = panel(1.0, 0.0);
        next.nodes.push(SceneNode {
            id: "extra".into(),
            parent_id: None,
            transform: Transform3::default(),
            content: NodeContent::Group,
            transition_ms: None,
        });
        animator.present("k", &next, start);
        assert!(!animator.animating(start), "an addition is not a transition");
    }
}


