//! Platform-independent project window layout policy.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

const MIN_PANE_WIDTH: u32 = 160;
const MIN_PANE_HEIGHT: u32 = 120;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaneRole {
    Browser,
    Tool,
    Auxiliary,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    Primary,
    Stack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Surface {
    pub id: String,
    pub role: PaneRole,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Placement {
    pub id: String,
    pub role: PaneRole,
    pub region: Region,
    pub frame: Rect,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LayoutPlan {
    pub area: Rect,
    pub placements: Vec<Placement>,
}

/// Give the first browser the primary region and place every other surface in
/// one shared stack region. Backends decide whether that stack is tabbed (Sway)
/// or overlapping and raised on focus (macOS).
pub fn browser_primary(area: Rect, surfaces: &[Surface]) -> Result<LayoutPlan, String> {
    validate_rect(area)?;
    if surfaces.is_empty() || surfaces.len() > 16 {
        return Err("Layout requires 1–16 surfaces".into());
    }
    if area.height < MIN_PANE_HEIGHT
        || area.width < MIN_PANE_WIDTH * u32::from(surfaces.len() > 1) + MIN_PANE_WIDTH
    {
        return Err("Layout area is too small for its primary and stack regions".into());
    }
    let mut ids = BTreeSet::new();
    if surfaces
        .iter()
        .any(|surface| surface.id.is_empty() || !ids.insert(surface.id.as_str()))
    {
        return Err("Layout surface IDs must be nonempty and unique".into());
    }

    let primary = surfaces
        .iter()
        .position(|surface| surface.role == PaneRole::Browser)
        .unwrap_or(0);
    let primary_width = if surfaces.len() == 1 {
        area.width
    } else {
        ((u64::from(area.width) * 2) / 3) as u32
    };
    let primary_width = if surfaces.len() == 1 {
        primary_width
    } else {
        primary_width.clamp(MIN_PANE_WIDTH, area.width.saturating_sub(MIN_PANE_WIDTH))
    };
    let primary_frame = Rect {
        width: primary_width,
        ..area
    };
    let stack_frame = Rect {
        x: area
            .x
            .checked_add(i32::try_from(primary_width).map_err(|_| "Layout width overflow")?)
            .ok_or("Layout coordinate overflow")?,
        y: area.y,
        width: area.width - primary_width,
        height: area.height,
    };

    let placements = surfaces
        .iter()
        .enumerate()
        .map(|(index, surface)| Placement {
            id: surface.id.clone(),
            role: surface.role,
            region: if index == primary {
                Region::Primary
            } else {
                Region::Stack
            },
            frame: if index == primary {
                primary_frame
            } else {
                stack_frame
            },
        })
        .collect();
    Ok(LayoutPlan { area, placements })
}

fn validate_rect(rect: Rect) -> Result<(), String> {
    let width = i32::try_from(rect.width).map_err(|_| "Layout width exceeds coordinate range")?;
    let height =
        i32::try_from(rect.height).map_err(|_| "Layout height exceeds coordinate range")?;
    rect.x
        .checked_add(width)
        .ok_or("Layout right edge exceeds coordinate range")?;
    rect.y
        .checked_add(height)
        .ok_or("Layout bottom edge exceeds coordinate range")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn surface(id: &str, role: PaneRole) -> Surface {
        Surface {
            id: id.into(),
            role,
        }
    }

    #[test]
    fn one_surface_uses_the_whole_area() {
        let area = Rect {
            x: -1440,
            y: 25,
            width: 1440,
            height: 875,
        };
        let plan = browser_primary(area, &[surface("terminal", PaneRole::Tool)]).unwrap();
        assert_eq!(plan.placements[0].frame, area);
        assert_eq!(plan.placements[0].region, Region::Primary);
    }

    #[test]
    fn browser_is_primary_and_tools_share_a_stack() {
        let area = Rect {
            x: 0,
            y: 24,
            width: 1200,
            height: 800,
        };
        let plan = browser_primary(
            area,
            &[
                surface("terminal", PaneRole::Tool),
                surface("browser", PaneRole::Browser),
                surface("agent", PaneRole::Tool),
            ],
        )
        .unwrap();
        assert_eq!(plan.placements[1].region, Region::Primary);
        assert_eq!(plan.placements[1].frame.width, 800);
        assert_eq!(plan.placements[0].region, Region::Stack);
        assert_eq!(plan.placements[0].frame, plan.placements[2].frame);
        assert_eq!(plan.placements[0].frame.x, 800);
    }

    #[test]
    fn invalid_or_ambiguous_inputs_fail_loudly() {
        let area = Rect {
            x: i32::MAX,
            y: 0,
            width: 320,
            height: 120,
        };
        assert!(browser_primary(area, &[surface("one", PaneRole::Tool)]).is_err());
        let area = Rect {
            x: 0,
            y: 0,
            width: 1000,
            height: 700,
        };
        assert!(browser_primary(area, &[]).is_err());
        assert!(
            browser_primary(
                area,
                &[
                    surface("same", PaneRole::Browser),
                    surface("same", PaneRole::Tool)
                ]
            )
            .is_err()
        );
    }
}
