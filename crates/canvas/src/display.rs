//! Renderer-neutral display targeting and virtual desktop projection.

use crate::{
    NormalizedRect,
    interaction::{LogicalPoint, LogicalRect},
};
use canvas_core::layout::Rect;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_DISPLAYS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DisplayInfo {
    pub id: String,
    pub name: String,
    pub frame: Rect,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum DisplayTarget {
    #[default]
    All,
    Exact {
        display_ids: Vec<String>,
    },
    /// One-based displays ordered left-to-right, then top-to-bottom. `0` means all.
    Ordinals {
        ordinals: Vec<usize>,
    },
}

/// A coarse, deterministic destination inside one physical display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "mcp", derive(schemars::JsonSchema))]
pub enum DisplayRegion {
    Full,
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl DisplayRegion {
    pub fn normalized_bounds(
        self,
        display: Rect,
        desktop: Rect,
        margin: u16,
    ) -> Result<NormalizedRect, String> {
        let margin = f64::from(margin);
        let width = f64::from(display.width);
        let height = f64::from(display.height);
        if margin * 2.0 >= width || margin * 2.0 >= height {
            return Err("display placement margin leaves no usable area".into());
        }
        let (x, y, region_width, region_height) = match self {
            Self::Full => (margin, margin, width - margin * 2.0, height - margin * 2.0),
            Self::Left => (
                margin,
                margin,
                width * 0.5 - margin * 1.5,
                height - margin * 2.0,
            ),
            Self::Right => (
                width * 0.5 + margin * 0.5,
                margin,
                width * 0.5 - margin * 1.5,
                height - margin * 2.0,
            ),
            Self::Top => (
                margin,
                margin,
                width - margin * 2.0,
                height * 0.5 - margin * 1.5,
            ),
            Self::Bottom => (
                margin,
                height * 0.5 + margin * 0.5,
                width - margin * 2.0,
                height * 0.5 - margin * 1.5,
            ),
            Self::Center => (width * 0.15, height * 0.12, width * 0.7, height * 0.76),
        };
        if region_width <= 0.0 || region_height <= 0.0 {
            return Err("display placement region is too small for its margin".into());
        }
        let desktop_width = f64::from(desktop.width.max(1));
        let desktop_height = f64::from(desktop.height.max(1));
        Ok(NormalizedRect {
            x: ((f64::from(display.x - desktop.x) + x) / desktop_width) as f32,
            y: ((f64::from(display.y - desktop.y) + y) / desktop_height) as f32,
            width: (region_width / desktop_width) as f32,
            height: (region_height / desktop_height) as f32,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedDisplayTarget {
    pub display_ids: Vec<String>,
    pub frame: Rect,
}

#[derive(Debug, Clone)]
pub struct DisplayTopology {
    displays: BTreeMap<String, DisplayInfo>,
}

impl DisplayTopology {
    pub fn new(displays: Vec<DisplayInfo>) -> Result<Self, String> {
        if displays.is_empty() || displays.len() > MAX_DISPLAYS {
            return Err(format!(
                "display inventory must contain 1-{MAX_DISPLAYS} displays"
            ));
        }
        let mut by_id = BTreeMap::new();
        for display in displays {
            validate_display(&display)?;
            if by_id.insert(display.id.clone(), display).is_some() {
                return Err("display inventory contains duplicate IDs".into());
            }
        }
        Ok(Self { displays: by_id })
    }

    pub fn displays(&self) -> impl Iterator<Item = &DisplayInfo> {
        self.displays.values()
    }

    pub fn resolve(&self, target: &DisplayTarget) -> Result<ResolvedDisplayTarget, String> {
        let ids = match target {
            DisplayTarget::All => self.displays.keys().cloned().collect::<Vec<_>>(),
            DisplayTarget::Exact { display_ids } => {
                if display_ids.is_empty() || display_ids.len() > MAX_DISPLAYS {
                    return Err(format!("display target must contain 1-{MAX_DISPLAYS} IDs"));
                }
                let mut seen = BTreeSet::new();
                for id in display_ids {
                    if !seen.insert(id.as_str()) {
                        return Err("display target contains duplicate IDs".into());
                    }
                    if !self.displays.contains_key(id) {
                        return Err(format!("display target is disconnected or unknown: {id}"));
                    }
                }
                display_ids.clone()
            }
            DisplayTarget::Ordinals { ordinals } => {
                if ordinals.is_empty() || ordinals.len() > MAX_DISPLAYS {
                    return Err(format!(
                        "display ordinals must contain 1-{MAX_DISPLAYS} values"
                    ));
                }
                if ordinals == &[0] {
                    self.displays.keys().cloned().collect::<Vec<_>>()
                } else {
                    if ordinals.contains(&0) {
                        return Err("display ordinal 0 must be used alone".into());
                    }
                    let mut ordered = self.displays.values().collect::<Vec<_>>();
                    ordered.sort_by_key(|display| (display.frame.x, display.frame.y));
                    let mut seen = BTreeSet::new();
                    let mut ids = Vec::with_capacity(ordinals.len());
                    for ordinal in ordinals {
                        if !seen.insert(*ordinal) {
                            return Err("display target contains duplicate ordinals".into());
                        }
                        let display = ordered.get(ordinal - 1).ok_or_else(|| {
                            format!("display ordinal {ordinal} is disconnected or unknown")
                        })?;
                        ids.push(display.id.clone());
                    }
                    ids
                }
            }
        };
        let selected = ids.iter().map(|id| &self.displays[id]).collect::<Vec<_>>();
        Ok(ResolvedDisplayTarget {
            frame: bounding_frame(selected.iter().map(|display| display.frame))?,
            display_ids: ids,
        })
    }

    pub fn get(&self, id: &str) -> Option<&DisplayInfo> {
        self.displays.get(id)
    }
}

impl ResolvedDisplayTarget {
    pub fn viewport_origin(&self, display: &DisplayInfo) -> LogicalPoint {
        LogicalPoint {
            x: f64::from(display.frame.x - self.frame.x),
            y: f64::from(display.frame.y - self.frame.y),
        }
    }

    pub fn to_virtual(&self, display: &DisplayInfo, local: LogicalPoint) -> LogicalPoint {
        let origin = self.viewport_origin(display);
        LogicalPoint {
            x: local.x + origin.x,
            y: local.y + origin.y,
        }
    }

    pub fn to_local(&self, display: &DisplayInfo, bounds: LogicalRect) -> LogicalRect {
        let origin = self.viewport_origin(display);
        LogicalRect {
            x: bounds.x - origin.x,
            y: bounds.y - origin.y,
            ..bounds
        }
    }
}

fn validate_display(display: &DisplayInfo) -> Result<(), String> {
    if display.id.is_empty()
        || display.id.len() > 128
        || display.id.chars().any(char::is_control)
        || display.name.trim().is_empty()
        || display.name.len() > 256
        || display.name.chars().any(char::is_control)
        || display.frame.width == 0
        || display.frame.height == 0
    {
        return Err("display inventory contains invalid metadata or geometry".into());
    }
    display
        .frame
        .x
        .checked_add_unsigned(display.frame.width)
        .and_then(|_| display.frame.y.checked_add_unsigned(display.frame.height))
        .ok_or_else(|| "display geometry overflows desktop coordinates".to_string())?;
    Ok(())
}

fn bounding_frame(frames: impl Iterator<Item = Rect>) -> Result<Rect, String> {
    let mut frames = frames;
    let first = frames.next().ok_or("display target is empty")?;
    let (mut left, mut top) = (i64::from(first.x), i64::from(first.y));
    let (mut right, mut bottom) = (left + i64::from(first.width), top + i64::from(first.height));
    for frame in frames {
        left = left.min(i64::from(frame.x));
        top = top.min(i64::from(frame.y));
        right = right.max(i64::from(frame.x) + i64::from(frame.width));
        bottom = bottom.max(i64::from(frame.y) + i64::from(frame.height));
    }
    Ok(Rect {
        x: i32::try_from(left).map_err(|_| "desktop left edge overflows")?,
        y: i32::try_from(top).map_err(|_| "desktop top edge overflows")?,
        width: u32::try_from(right - left).map_err(|_| "desktop width overflows")?,
        height: u32::try_from(bottom - top).map_err(|_| "desktop height overflows")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(id: &str, x: i32, y: i32) -> DisplayInfo {
        DisplayInfo {
            id: id.into(),
            name: id.into(),
            frame: Rect {
                x,
                y,
                width: 1920,
                height: 1200,
            },
            primary: id == "left",
        }
    }

    #[test]
    fn exact_connected_target_resolves_stably_with_negative_origins() {
        let topology = DisplayTopology::new(vec![
            display("left", -1920, 0),
            display("middle", 0, 0),
            display("right", 1920, 0),
        ])
        .unwrap();
        let target = topology
            .resolve(&DisplayTarget::Exact {
                display_ids: vec!["left".into(), "middle".into(), "right".into()],
            })
            .unwrap();
        assert_eq!(target.display_ids, ["left", "middle", "right"]);
        assert_eq!(
            target.frame,
            Rect {
                x: -1920,
                y: 0,
                width: 5760,
                height: 1200,
            }
        );
        assert_eq!(
            target.to_virtual(
                topology.get("right").unwrap(),
                LogicalPoint { x: 24.0, y: 30.0 }
            ),
            LogicalPoint { x: 3864.0, y: 30.0 }
        );
    }

    #[test]
    fn surface_projection_preserves_size_across_a_display_seam() {
        let topology =
            DisplayTopology::new(vec![display("left", 0, 0), display("right", 1920, 0)]).unwrap();
        let target = topology.resolve(&DisplayTarget::All).unwrap();
        let crossing = LogicalRect {
            x: 1800.0,
            y: 100.0,
            width: 400.0,
            height: 300.0,
        };
        assert_eq!(
            target.to_local(topology.get("left").unwrap(), crossing),
            crossing
        );
        assert_eq!(
            target.to_local(topology.get("right").unwrap(), crossing),
            LogicalRect {
                x: -120.0,
                ..crossing
            }
        );
    }

    #[test]
    fn display_regions_resolve_inside_one_screen_of_a_shared_desktop() {
        let desktop = Rect {
            x: -1920,
            y: 0,
            width: 5760,
            height: 1200,
        };
        let right = display("right", 1920, 0).frame;
        let bounds = DisplayRegion::Full
            .normalized_bounds(right, desktop, 24)
            .unwrap();
        assert!(bounds.x > 2.0 / 3.0);
        assert!(bounds.x + bounds.width < 1.0);
        assert!((bounds.y - 0.02).abs() < 0.001);
        assert!(
            DisplayRegion::Full
                .normalized_bounds(right, desktop, 700)
                .is_err()
        );
    }

    #[test]
    fn unknown_and_duplicate_targets_fail_but_disjoint_screens_are_supported() {
        let topology =
            DisplayTopology::new(vec![display("left", 0, 0), display("island", 5000, 0)]).unwrap();
        for target in [
            DisplayTarget::Exact {
                display_ids: vec!["missing".into()],
            },
            DisplayTarget::Exact {
                display_ids: vec!["left".into(), "left".into()],
            },
            DisplayTarget::Ordinals {
                ordinals: vec![0, 1],
            },
        ] {
            assert!(topology.resolve(&target).is_err());
        }
        assert!(topology.resolve(&DisplayTarget::All).is_ok());
        assert_eq!(
            topology
                .resolve(&DisplayTarget::Ordinals {
                    ordinals: vec![1, 2],
                })
                .unwrap()
                .display_ids,
            ["left", "island"]
        );
    }
}
