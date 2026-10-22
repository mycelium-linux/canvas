//! Canonical renderer-independent Canvas layout intent and validation.
use crate::{Result, check};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
pub const MAX_ITEMS: usize = 1_024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum Layout {
    Empty,
    Surface {
        surface_id: String,
    },
    Split {
        axis: Axis,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
    Stack {
        surface_ids: Vec<String>,
        active_surface_id: String,
    },
    Canvas {
        placements: Vec<Placement>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(schemars::JsonSchema)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct Placement {
    pub surface_id: String,
    pub bounds: NormalizedRect,
    #[serde(default)]
    pub layer: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Layout {
    pub fn validate<'a>(
        &'a self,
        surface_ids: &BTreeSet<&'a str>,
        placed: &mut BTreeSet<&'a str>,
    ) -> Result<()> {
        match self {
            Self::Empty => Ok(()),
            Self::Surface { surface_id } => place(surface_id, surface_ids, placed),
            Self::Split {
                ratio,
                first,
                second,
                ..
            } => {
                check(
                    ratio.is_finite() && (0.05..=0.95).contains(ratio),
                    "Split ratio must be between 0.05 and 0.95",
                )?;
                first.validate(surface_ids, placed)?;
                second.validate(surface_ids, placed)
            }
            Self::Stack {
                surface_ids: stack,
                active_surface_id,
            } => {
                check(!stack.is_empty(), "A stack must contain a surface")?;
                check(
                    stack.iter().any(|id| id == active_surface_id),
                    "Stack active surface is not in the stack",
                )?;
                for surface_id in stack {
                    place(surface_id, surface_ids, placed)?;
                }
                Ok(())
            }
            Self::Canvas { placements } => {
                check(
                    !placements.is_empty() && placements.len() <= MAX_ITEMS,
                    "A Canvas layout must contain 1 to 1024 placements",
                )?;
                for placement in placements {
                    placement.bounds.validate()?;
                    place(&placement.surface_id, surface_ids, placed)?;
                }
                Ok(())
            }
        }
    }
}

fn place<'a>(
    surface_id: &'a str,
    surface_ids: &BTreeSet<&'a str>,
    placed: &mut BTreeSet<&'a str>,
) -> Result<()> {
    check(
        surface_ids.contains(surface_id),
        format!("Layout references unknown surface: {surface_id}"),
    )?;
    check(
        placed.insert(surface_id),
        format!("Layout places a surface more than once: {surface_id}"),
    )
}

impl NormalizedRect {
    pub fn validate(self) -> Result<()> {
        check(
            [self.x, self.y, self.width, self.height]
                .iter()
                .all(|value| value.is_finite())
                && self.y >= 0.0
                && self.width > 0.0
                && self.height > 0.0
                && self.width <= 1.0
                && self.height <= 1.0
                && self.x < 1.0
                && self.x + self.width > 0.0
                && self.y < 1.0,
            "Canvas bounds must be a positive, reachable rectangle in normalized workspace space",
        )
    }
}

#[cfg(test)]
mod compatibility_tests {
    use super::*;
    #[test]
    fn existing_bsp_json_round_trips_without_a_schema_migration() {
        let raw = serde_json::json!({"kind":"split","axis":"horizontal","ratio":0.5,
            "first":{"kind":"surface","surface_id":"video"},
            "second":{"kind":"stack","surface_ids":["clock","network"],"active_surface_id":"clock"}});
        let layout: Layout = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&layout).unwrap(), raw);
        layout
            .validate(
                &BTreeSet::from(["video", "clock", "network"]),
                &mut BTreeSet::new(),
            )
            .unwrap();
    }
    #[test]
    fn malformed_layouts_still_fail_loudly() {
        let layout = Layout::Surface {
            surface_id: "missing".into(),
        };
        assert!(
            layout
                .validate(&BTreeSet::new(), &mut BTreeSet::new())
                .is_err()
        );
        assert!(
            serde_json::from_value::<Layout>(
                serde_json::json!({"kind":"surface","surface_id":"video","unexpected":true})
            )
            .is_err()
        );
    }
}
