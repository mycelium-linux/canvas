//! Renderer-neutral direct manipulation for Canvas surfaces.
//!
//! An iced, touch, or XR adapter converts its native input into [`PointerEvent`]
//! values in Canvas logical coordinates. The engine owns only the transient
//! gesture and emits deterministic [`InteractionIntent`] values. The caller
//! remains responsible for validating and applying those intents to the
//! authoritative workspace document. XR adapters should ray-cast in their own
//! renderer and submit the resulting logical surface point with a stable pointer
//! ID; this module never owns GPU state or injects operating-system input.

use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, error, fmt};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogicalRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl LogicalRect {
    pub fn right(self) -> f64 {
        self.x + self.width
    }

    pub fn bottom(self) -> f64 {
        self.y + self.height
    }

    pub fn contains(self, point: LogicalPoint) -> bool {
        point.x >= self.x
            && point.x <= self.right()
            && point.y >= self.y
            && point.y <= self.bottom()
    }
}

/// Current renderer-owned geometry. Higher `z_order` values are in front;
/// equal values resolve to the later entry in the supplied slice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceGeometry {
    pub surface_id: String,
    pub bounds: LogicalRect,
    pub z_order: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeHandle {
    North,
    NorthEast,
    East,
    SouthEast,
    South,
    SouthWest,
    West,
    NorthWest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum HitRegion {
    Body,
    TopBar,
    Resize { handle: ResizeHandle },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HitTarget {
    pub surface_id: String,
    pub region: HitRegion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerPhase {
    Press,
    Move,
    Release,
    Cancel,
}

/// A normalized primary-pointer event. Mouse and touch adapters should filter
/// secondary buttons before constructing this value.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointerEvent {
    pub pointer_id: u64,
    pub phase: PointerPhase,
    pub position: LogicalPoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InteractionConfig {
    pub workspace_bounds: LogicalRect,
    pub minimum_surface_size: LogicalPoint,
    pub resize_handle_size: f64,
    /// Height of the draggable window chrome. The remaining body belongs to
    /// the embedded surface after it has been focused and promoted.
    pub top_bar_height: f64,
    pub snap_distance: f64,
    /// Distance from a workspace edge at which a move gesture offers a
    /// half-screen or quarter-screen dock zone, like a desktop window manager.
    pub snap_zone_trigger: f64,
    /// Grid spacing. `None` disables grid snapping while retaining workspace
    /// edge snapping.
    pub grid_size: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionIntent {
    Focus {
        surface_id: String,
    },
    PromoteToFront {
        surface_id: String,
    },
    CapturePointer {
        pointer_id: u64,
        surface_id: String,
    },
    SetBounds {
        surface_id: String,
        bounds: LogicalRect,
    },
    ReleasePointer {
        pointer_id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionError(pub &'static str);

impl fmt::Display for InteractionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl error::Error for InteractionError {}

#[derive(Debug, Clone)]
struct ActiveGesture {
    pointer_id: u64,
    surface_id: String,
    region: HitRegion,
    origin: LogicalPoint,
    original_bounds: LogicalRect,
    current_bounds: LogicalRect,
    /// Last position seen for this gesture; a release that arrives without a
    /// fresh move (pointer left the display) still commits where it stalled.
    last_position: LogicalPoint,
    /// Half- or quarter-screen zone offered by the current move gesture.
    snap_zone: Option<LogicalRect>,
}

/// Stateful pointer capture with no renderer or document ownership.
#[derive(Debug, Clone)]
pub struct InteractionEngine {
    config: InteractionConfig,
    active: Option<ActiveGesture>,
}

impl InteractionEngine {
    pub fn new(config: InteractionConfig) -> Result<Self, InteractionError> {
        validate_config(config)?;
        Ok(Self {
            config,
            active: None,
        })
    }

    pub fn captured_pointer(&self) -> Option<u64> {
        self.active.as_ref().map(|active| active.pointer_id)
    }

    /// The dock zone currently offered to an active move gesture, for the
    /// renderer to preview. `None` while resizing or when no edge is near.
    pub fn snap_preview(&self) -> Option<LogicalRect> {
        self.active.as_ref().and_then(|active| active.snap_zone)
    }

    pub fn handle(
        &mut self,
        event: PointerEvent,
        surfaces: &[SurfaceGeometry],
    ) -> Result<Vec<InteractionIntent>, InteractionError> {
        validate_point(event.position)?;
        validate_surfaces(surfaces)?;

        match event.phase {
            PointerPhase::Press => self.press(event, surfaces),
            PointerPhase::Move => self.update(event, false),
            PointerPhase::Release => self.update(event, true),
            PointerPhase::Cancel => self.cancel(event),
        }
    }

    fn press(
        &mut self,
        event: PointerEvent,
        surfaces: &[SurfaceGeometry],
    ) -> Result<Vec<InteractionIntent>, InteractionError> {
        // A fresh press while a gesture is still captured means the previous
        // release never arrived — the pointer left the display, or focus was
        // stolen. Recover rather than wedge: the interrupted gesture keeps the
        // bounds it last reported (they were applied live), and the new press
        // proceeds normally. Without this one lost event freezes input.
        self.active = None;
        let Some(hit) = hit_test(
            surfaces,
            event.position,
            self.config.resize_handle_size,
            self.config.top_bar_height,
        )?
        else {
            return Ok(Vec::new());
        };
        let surface = surfaces
            .iter()
            .find(|surface| surface.surface_id == hit.surface_id)
            .ok_or(InteractionError("hit surface disappeared"))?;
        let surface_id = surface.surface_id.clone();
        let mut intents = vec![
            InteractionIntent::Focus {
                surface_id: surface_id.clone(),
            },
            InteractionIntent::PromoteToFront {
                surface_id: surface_id.clone(),
            },
        ];
        // Body drags move the surface (macOS-style direct manipulation);
        // resizing stays reserved for the edge handles. A press-release with
        // no movement is still a click for whatever content was pressed.
        self.active = Some(ActiveGesture {
            pointer_id: event.pointer_id,
            surface_id: surface_id.clone(),
            region: hit.region,
            origin: event.position,
            original_bounds: surface.bounds,
            current_bounds: surface.bounds,
            last_position: event.position,
            snap_zone: None,
        });
        intents.push(InteractionIntent::CapturePointer {
            pointer_id: event.pointer_id,
            surface_id,
        });
        Ok(intents)
    }

    /// Commit a captured gesture whose release event never arrived (pointer
    /// left the display mid-drag). Applies the same edge/corner snap a normal
    /// release would, so a drag that stalls at the frame still docks.
    pub fn force_release(&mut self) -> Vec<InteractionIntent> {
        let Some(active) = self.active.as_ref() else {
            return Vec::new();
        };
        let pointer_id = active.pointer_id;
        let zone = match active.region {
            HitRegion::TopBar | HitRegion::Body => {
                snap_zone_for_move(self.config, active.last_position)
            }
            HitRegion::Resize { .. } => None,
        };
        let final_bounds = zone.unwrap_or(active.current_bounds);
        let mut intents = Vec::with_capacity(2);
        if final_bounds != active.current_bounds {
            intents.push(InteractionIntent::SetBounds {
                surface_id: active.surface_id.clone(),
                bounds: final_bounds,
            });
        }
        intents.push(InteractionIntent::ReleasePointer { pointer_id });
        self.active = None;
        intents
    }

    fn update(
        &mut self,
        event: PointerEvent,
        release: bool,
    ) -> Result<Vec<InteractionIntent>, InteractionError> {
        let Some(active) = self.active.as_mut() else {
            return Ok(Vec::new());
        };
        if active.pointer_id != event.pointer_id {
            return Ok(Vec::new());
        }

        let next = manipulate(self.config, active, event.position);
        active.last_position = event.position;
        active.snap_zone = match active.region {
            HitRegion::TopBar | HitRegion::Body => snap_zone_for_move(self.config, event.position),
            HitRegion::Resize { .. } => None,
        };
        let mut intents = Vec::with_capacity(2);
        let final_bounds = if release {
            active.snap_zone.unwrap_or(next)
        } else {
            next
        };
        if final_bounds != active.current_bounds {
            active.current_bounds = final_bounds;
            intents.push(InteractionIntent::SetBounds {
                surface_id: active.surface_id.clone(),
                bounds: final_bounds,
            });
        }
        if release {
            intents.push(InteractionIntent::ReleasePointer {
                pointer_id: event.pointer_id,
            });
            self.active = None;
        }
        Ok(intents)
    }

    fn cancel(&mut self, event: PointerEvent) -> Result<Vec<InteractionIntent>, InteractionError> {
        let Some(active) = self.active.as_ref() else {
            return Ok(Vec::new());
        };
        if active.pointer_id != event.pointer_id {
            return Ok(Vec::new());
        }

        let mut intents = Vec::with_capacity(2);
        if active.current_bounds != active.original_bounds {
            intents.push(InteractionIntent::SetBounds {
                surface_id: active.surface_id.clone(),
                bounds: active.original_bounds,
            });
        }
        intents.push(InteractionIntent::ReleasePointer {
            pointer_id: event.pointer_id,
        });
        self.active = None;
        Ok(intents)
    }
}

/// Resolve the frontmost surface and its resize region without allocating or
/// mutating the supplied geometry.
pub fn hit_test(
    surfaces: &[SurfaceGeometry],
    point: LogicalPoint,
    resize_handle_size: f64,
    top_bar_height: f64,
) -> Result<Option<HitTarget>, InteractionError> {
    validate_point(point)?;
    if !resize_handle_size.is_finite() || resize_handle_size < 0.0 {
        return Err(InteractionError(
            "resize handle size must be finite and non-negative",
        ));
    }
    if !top_bar_height.is_finite() || top_bar_height < 0.0 {
        return Err(InteractionError(
            "top bar height must be finite and non-negative",
        ));
    }
    for surface in surfaces {
        validate_rect(surface.bounds)?;
    }

    let hit = surfaces
        .iter()
        .enumerate()
        .filter(|(_, surface)| {
            surface.bounds.contains(point)
                || within_margin(surface.bounds, point, OUTER_RESIZE_MARGIN)
        })
        .max_by_key(|(index, surface)| (surface.z_order, *index));
    Ok(hit.map(|(_, surface)| {
        let resize = resize_region_band(
            surface.bounds,
            point,
            resize_handle_size,
            OUTER_RESIZE_MARGIN,
        );
        let in_top_bar = point.y <= surface.bounds.y + top_bar_height
            && point.y >= surface.bounds.y - OUTER_RESIZE_MARGIN;
        // The north edge overlaps the title bar, and a title bar that resizes
        // when you grab it is a title bar you cannot drag: the top edge follows
        // the hand while the bottom stays still, which reads as the window
        // jumping down on click. Inside the title bar the north handle is
        // limited to the outermost few points; the sides keep their targets.
        let north_band = if in_top_bar {
            (resize_handle_size * 0.4).max(3.0)
        } else {
            resize_handle_size
        };
        let resize = match resize {
            HitRegion::Resize { handle }
                if in_top_bar
                    && matches!(
                        handle,
                        ResizeHandle::North
                            | ResizeHandle::NorthEast
                            | ResizeHandle::NorthWest
                    )
                    && point.y > surface.bounds.y + north_band =>
            {
                HitRegion::TopBar
            }
            other => other,
        };
        let region = if matches!(resize, HitRegion::Resize { .. }) {
            resize
        } else if in_top_bar {
            HitRegion::TopBar
        } else {
            HitRegion::Body
        };
        HitTarget {
            surface_id: surface.surface_id.clone(),
            region,
        }
    }))
}

fn manipulate(
    config: InteractionConfig,
    active: &mut ActiveGesture,
    point: LogicalPoint,
) -> LogicalRect {
    match active.region {
        HitRegion::TopBar | HitRegion::Body => move_rect(config, active, point),
        HitRegion::Resize { handle } => resize_rect(config, active, point, handle),
    }
}

/// The dock zone a move gesture earns at its current pointer position:
/// corner wins quarter zones, a lone edge wins half zones. The caller previews
/// this rectangle while the pointer is inside and commits it on release,
/// the way macOS Window-tiling and Windows Snap do.
fn snap_zone_for_move(config: InteractionConfig, point: LogicalPoint) -> Option<LogicalRect> {
    let trigger = config.snap_zone_trigger;
    if trigger <= 0.0 {
        return None;
    }
    let workspace = config.workspace_bounds;
    let near_west = point.x - workspace.x <= trigger;
    let near_east = workspace.right() - point.x <= trigger;
    let near_north = point.y - workspace.y <= trigger;
    let near_south = workspace.bottom() - point.y <= trigger;
    let half = workspace.width / 2.0;
    let middle = workspace.height / 2.0;
    let x_left = workspace.x;
    let x_right = workspace.x + half;
    let y_top = workspace.y;
    let y_bottom = workspace.y + middle;
    let zone = match (near_west, near_east, near_north, near_south) {
        (true, _, true, _) => LogicalRect {
            x: x_left,
            y: y_top,
            width: half,
            height: middle,
        },
        (true, _, _, true) => LogicalRect {
            x: x_left,
            y: y_bottom,
            width: half,
            height: middle,
        },
        (_, true, true, _) => LogicalRect {
            x: x_right,
            y: y_top,
            width: half,
            height: middle,
        },
        (_, true, _, true) => LogicalRect {
            x: x_right,
            y: y_bottom,
            width: half,
            height: middle,
        },
        (true, _, false, false) => LogicalRect {
            x: x_left,
            y: y_top,
            width: half,
            height: workspace.height,
        },
        (_, true, false, false) => LogicalRect {
            x: x_right,
            y: y_top,
            width: half,
            height: workspace.height,
        },
        (false, false, true, _) => LogicalRect {
            x: workspace.x,
            y: y_top,
            width: workspace.width,
            height: middle,
        },
        (false, false, _, true) => LogicalRect {
            x: workspace.x,
            y: y_bottom,
            width: workspace.width,
            height: middle,
        },
        _ => return None,
    };
    Some(zone)
}

/// How much of a window must stay on screen, in logical points. Enough to grab
/// the title bar, not enough to force a window back inside.
const MIN_VISIBLE_GRAB: f64 = 96.0;

fn move_rect(
    config: InteractionConfig,
    active: &mut ActiveGesture,
    point: LogicalPoint,
) -> LogicalRect {
    let mut bounds = active.original_bounds;
    bounds.x += point.x - active.origin.x;
    bounds.y += point.y - active.origin.y;
    bounds.x = snap_translation(
        bounds.x,
        bounds.width,
        config.workspace_bounds.x,
        config.workspace_bounds.right(),
        config.grid_size,
        config.snap_distance,
    );
    bounds.y = snap_translation(
        bounds.y,
        bounds.height,
        config.workspace_bounds.y,
        config.workspace_bounds.bottom(),
        config.grid_size,
        config.snap_distance,
    );
    // A window may hang off the wall, but never entirely: a strip stays on
    // screen so it can be grabbed again. Full containment was worse than the
    // problem it solved — parking a window half off the edge is a normal thing
    // to want, and refusing it makes the desktop feel nailed down.
    let keep_x = MIN_VISIBLE_GRAB.min(bounds.width);
    let keep_y = MIN_VISIBLE_GRAB.min(bounds.height);
    let intended = bounds;
    bounds.x = bounds.x.clamp(
        config.workspace_bounds.x - (bounds.width - keep_x),
        config.workspace_bounds.right() - keep_x,
    );
    bounds.y = bounds.y.clamp(
        config.workspace_bounds.y - (bounds.height - keep_y),
        config.workspace_bounds.bottom() - keep_y,
    );
    // Re-anchor whenever the window could not follow the pointer: without this
    // the grab point keeps travelling with the hand, and dragging back does
    // nothing until the pointer has retraced the distance the window was held
    // back. That rubber band is what makes a drag feel like it fights you.
    active.origin.x += bounds.x - intended.x;
    active.origin.y += bounds.y - intended.y;
    bounds
}

fn resize_rect(
    config: InteractionConfig,
    active: &ActiveGesture,
    point: LogicalPoint,
    handle: ResizeHandle,
) -> LogicalRect {
    let original = active.original_bounds;
    let delta = LogicalPoint {
        x: point.x - active.origin.x,
        y: point.y - active.origin.y,
    };
    let (west, east, north, south) = handle_edges(handle);
    let mut left = original.x;
    let mut right = original.right();
    let mut top = original.y;
    let mut bottom = original.bottom();

    if west {
        left = snap_coordinate(
            original.x + delta.x,
            config.workspace_bounds.x,
            config.workspace_bounds.right(),
            config.grid_size,
            config.snap_distance,
        )
        .clamp(
            config.workspace_bounds.x,
            right - config.minimum_surface_size.x,
        );
    } else if east {
        right = snap_coordinate(
            original.right() + delta.x,
            config.workspace_bounds.x,
            config.workspace_bounds.right(),
            config.grid_size,
            config.snap_distance,
        )
        .clamp(
            left + config.minimum_surface_size.x,
            config.workspace_bounds.right(),
        );
    }

    if north {
        top = snap_coordinate(
            original.y + delta.y,
            config.workspace_bounds.y,
            config.workspace_bounds.bottom(),
            config.grid_size,
            config.snap_distance,
        )
        .clamp(
            config.workspace_bounds.y,
            bottom - config.minimum_surface_size.y,
        );
    } else if south {
        bottom = snap_coordinate(
            original.bottom() + delta.y,
            config.workspace_bounds.y,
            config.workspace_bounds.bottom(),
            config.grid_size,
            config.snap_distance,
        )
        .clamp(
            top + config.minimum_surface_size.y,
            config.workspace_bounds.bottom(),
        );
    }

    LogicalRect {
        x: left,
        y: top,
        width: right - left,
        height: bottom - top,
    }
}

/// How far outside a window a resize press may land and still grab its edge.
const OUTER_RESIZE_MARGIN: f64 = 8.0;

fn within_margin(bounds: LogicalRect, point: LogicalPoint, margin: f64) -> bool {
    point.x >= bounds.x - margin
        && point.x <= bounds.right() + margin
        && point.y >= bounds.y - margin
        && point.y <= bounds.bottom() + margin
}

fn resize_region(bounds: LogicalRect, point: LogicalPoint, handle: f64) -> HitRegion {
    resize_region_band(bounds, point, handle, OUTER_RESIZE_MARGIN)
}

/// Which edge of `bounds`, if any, the point is grabbing.
///
/// `inside` is how far into the window counts as the edge; `outside` is how far
/// beyond it does. The outside band is what makes an edge forgiving: a hand
/// arriving from the desktop side still catches the edge it was aiming for.
fn resize_region_band(
    bounds: LogicalRect,
    point: LogicalPoint,
    inside: f64,
    outside: f64,
) -> HitRegion {
    let horizontal = nearest_edge_band(
        point.x - bounds.x,
        bounds.right() - point.x,
        inside,
        outside,
    );
    let vertical = nearest_edge_band(
        point.y - bounds.y,
        bounds.bottom() - point.y,
        inside,
        outside,
    );
    let resize = match (horizontal, vertical) {
        (Some(Edge::Low), Some(Edge::Low)) => Some(ResizeHandle::NorthWest),
        (None, Some(Edge::Low)) => Some(ResizeHandle::North),
        (Some(Edge::High), Some(Edge::Low)) => Some(ResizeHandle::NorthEast),
        (Some(Edge::High), None) => Some(ResizeHandle::East),
        (Some(Edge::High), Some(Edge::High)) => Some(ResizeHandle::SouthEast),
        (None, Some(Edge::High)) => Some(ResizeHandle::South),
        (Some(Edge::Low), Some(Edge::High)) => Some(ResizeHandle::SouthWest),
        (Some(Edge::Low), None) => Some(ResizeHandle::West),
        (None, None) => None,
    };
    resize.map_or(HitRegion::Body, |handle| HitRegion::Resize { handle })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Edge {
    Low,
    High,
}

fn nearest_edge_band(
    low_distance: f64,
    high_distance: f64,
    inside: f64,
    outside: f64,
) -> Option<Edge> {
    let low = low_distance <= inside && low_distance >= -outside;
    let high = high_distance <= inside && high_distance >= -outside;
    match (low, high) {
        (true, true) if high_distance < low_distance => Some(Edge::High),
        (true, _) => Some(Edge::Low),
        (_, true) => Some(Edge::High),
        _ => None,
    }
}

fn handle_edges(handle: ResizeHandle) -> (bool, bool, bool, bool) {
    let west = matches!(
        handle,
        ResizeHandle::West | ResizeHandle::NorthWest | ResizeHandle::SouthWest
    );
    let east = matches!(
        handle,
        ResizeHandle::East | ResizeHandle::NorthEast | ResizeHandle::SouthEast
    );
    let north = matches!(
        handle,
        ResizeHandle::North | ResizeHandle::NorthEast | ResizeHandle::NorthWest
    );
    let south = matches!(
        handle,
        ResizeHandle::South | ResizeHandle::SouthEast | ResizeHandle::SouthWest
    );
    (west, east, north, south)
}

fn snap_translation(
    position: f64,
    extent: f64,
    low: f64,
    high: f64,
    grid: Option<f64>,
    distance: f64,
) -> f64 {
    // Candidate order is the stable tie-break: workspace edges, then the
    // leading grid edge, then the trailing grid edge.
    let candidates = [
        low - position,
        high - (position + extent),
        grid_delta(position, low, high, grid),
        grid_delta(position + extent, low, high, grid),
    ];
    position + closest_delta(candidates, distance)
}

fn snap_coordinate(value: f64, low: f64, high: f64, grid: Option<f64>, distance: f64) -> f64 {
    value
        + closest_delta(
            [
                low - value,
                high - value,
                grid_delta(value, low, high, grid),
                f64::INFINITY,
            ],
            distance,
        )
}

fn grid_delta(value: f64, origin: f64, high: f64, grid: Option<f64>) -> f64 {
    grid.map_or(f64::INFINITY, |spacing| {
        let line = ((value - origin) / spacing)
            .round()
            .mul_add(spacing, origin);
        line.clamp(origin, high) - value
    })
}

fn closest_delta(candidates: [f64; 4], distance: f64) -> f64 {
    let mut best = f64::INFINITY;
    for candidate in candidates {
        if candidate.abs() <= distance && candidate.abs() < best.abs() {
            best = candidate;
        }
    }
    if best.is_finite() { best } else { 0.0 }
}

fn validate_config(config: InteractionConfig) -> Result<(), InteractionError> {
    validate_rect(config.workspace_bounds)?;
    validate_point(config.minimum_surface_size)?;
    if config.minimum_surface_size.x <= 0.0 || config.minimum_surface_size.y <= 0.0 {
        return Err(InteractionError("minimum surface size must be positive"));
    }
    if config.minimum_surface_size.x > config.workspace_bounds.width
        || config.minimum_surface_size.y > config.workspace_bounds.height
    {
        return Err(InteractionError(
            "minimum surface size must fit within workspace bounds",
        ));
    }
    if !config.resize_handle_size.is_finite() || config.resize_handle_size < 0.0 {
        return Err(InteractionError(
            "resize handle size must be finite and non-negative",
        ));
    }
    if !config.top_bar_height.is_finite()
        || config.top_bar_height < 0.0
        || config.top_bar_height > config.workspace_bounds.height
    {
        return Err(InteractionError(
            "top bar height must be finite and fit within workspace bounds",
        ));
    }
    if !config.snap_distance.is_finite() || config.snap_distance < 0.0 {
        return Err(InteractionError(
            "snap distance must be finite and non-negative",
        ));
    }
    if !config.snap_zone_trigger.is_finite() || config.snap_zone_trigger < 0.0 {
        return Err(InteractionError(
            "snap zone trigger must be finite and non-negative",
        ));
    }
    if config
        .grid_size
        .is_some_and(|grid| !grid.is_finite() || grid <= 0.0)
    {
        return Err(InteractionError("grid size must be finite and positive"));
    }
    Ok(())
}

/// Validate only that geometry is *interpretable* — finite, positive,
/// uniquely named. Placement is never a rejection condition: a surface
/// parked off the wall by an older Canvas must remain draggable back,
/// because refusing every pointer event when a surface is out of bounds is
/// a death spiral — the one state where the user most needs to drag is the
/// state where dragging is disallowed.
fn validate_surfaces(surfaces: &[SurfaceGeometry]) -> Result<(), InteractionError> {
    let mut ids = BTreeSet::new();
    for surface in surfaces {
        if surface.surface_id.is_empty() {
            return Err(InteractionError("surface ID must not be empty"));
        }
        if !ids.insert(surface.surface_id.as_str()) {
            return Err(InteractionError("surface IDs must be unique"));
        }
        validate_rect(surface.bounds)?;
    }
    Ok(())
}

fn validate_rect(bounds: LogicalRect) -> Result<(), InteractionError> {
    validate_point(LogicalPoint {
        x: bounds.x,
        y: bounds.y,
    })?;
    if !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err(InteractionError(
            "logical rectangle dimensions must be finite and positive",
        ));
    }
    if !bounds.right().is_finite() || !bounds.bottom().is_finite() {
        return Err(InteractionError("logical rectangle exceeds finite bounds"));
    }
    Ok(())
}

fn validate_point(point: LogicalPoint) -> Result<(), InteractionError> {
    if point.x.is_finite() && point.y.is_finite() {
        Ok(())
    } else {
        Err(InteractionError("logical point must be finite"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> LogicalRect {
        LogicalRect {
            x,
            y,
            width,
            height,
        }
    }

    fn point(x: f64, y: f64) -> LogicalPoint {
        LogicalPoint { x, y }
    }

    fn surface(id: &str, bounds: LogicalRect, z_order: i64) -> SurfaceGeometry {
        SurfaceGeometry {
            surface_id: id.to_owned(),
            bounds,
            z_order,
        }
    }

    fn config() -> InteractionConfig {
        InteractionConfig {
            workspace_bounds: rect(0.0, 0.0, 1_000.0, 800.0),
            minimum_surface_size: point(100.0, 80.0),
            resize_handle_size: 10.0,
            top_bar_height: 24.0,
            snap_distance: 8.0,
            snap_zone_trigger: 40.0,
            grid_size: Some(50.0),
        }
    }

    fn event(pointer_id: u64, phase: PointerPhase, x: f64, y: f64) -> PointerEvent {
        PointerEvent {
            pointer_id,
            phase,
            position: point(x, y),
        }
    }

    #[test]
    fn hit_test_prefers_highest_z_then_later_order_and_finds_handles() {
        let surfaces = vec![
            surface("back", rect(0.0, 0.0, 200.0, 200.0), 1),
            surface("front-first", rect(20.0, 20.0, 200.0, 200.0), 2),
            surface("front-last", rect(40.0, 40.0, 200.0, 200.0), 2),
        ];
        assert_eq!(
            hit_test(&surfaces, point(100.0, 100.0), 10.0, 24.0).unwrap(),
            Some(HitTarget {
                surface_id: "front-last".to_owned(),
                region: HitRegion::Body,
            })
        );
        assert_eq!(
            hit_test(&surfaces, point(40.0, 40.0), 10.0, 24.0).unwrap(),
            Some(HitTarget {
                surface_id: "front-last".to_owned(),
                region: HitRegion::Resize {
                    handle: ResizeHandle::NorthWest,
                },
            })
        );
        assert_eq!(
            hit_test(&surfaces, point(100.0, 55.0), 10.0, 24.0).unwrap(),
            Some(HitTarget {
                surface_id: "front-last".to_owned(),
                region: HitRegion::TopBar,
            })
        );
    }

    #[test]
    fn content_click_focuses_promotes_and_moves_without_capture_loss() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        assert_eq!(
            engine
                .handle(event(4, PointerPhase::Press, 200.0, 180.0), &surfaces)
                .unwrap(),
            vec![
                InteractionIntent::Focus {
                    surface_id: "card".to_owned(),
                },
                InteractionIntent::PromoteToFront {
                    surface_id: "card".to_owned(),
                },
                InteractionIntent::CapturePointer {
                    pointer_id: 4,
                    surface_id: "card".to_owned(),
                },
            ]
        );
        assert_eq!(engine.captured_pointer(), Some(4));
        // A press-release without movement is a click: no bounds change.
        assert_eq!(
            engine
                .handle(event(4, PointerPhase::Release, 200.0, 180.0), &surfaces)
                .unwrap(),
            vec![InteractionIntent::ReleasePointer { pointer_id: 4 }]
        );
        assert_eq!(engine.captured_pointer(), None);
    }

    #[test]
    fn top_bar_drag_captures_and_moves_surface() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        let pressed = engine
            .handle(event(4, PointerPhase::Press, 200.0, 115.0), &surfaces)
            .unwrap();
        assert!(matches!(
            pressed.as_slice(),
            [
                InteractionIntent::Focus { .. },
                InteractionIntent::PromoteToFront { .. },
                InteractionIntent::CapturePointer { pointer_id: 4, .. }
            ]
        ));
        assert_eq!(engine.captured_pointer(), Some(4));
        assert_eq!(
            engine
                .handle(event(4, PointerPhase::Move, 250.0, 165.0), &surfaces)
                .unwrap(),
            vec![InteractionIntent::SetBounds {
                surface_id: "card".to_owned(),
                bounds: rect(150.0, 150.0, 300.0, 200.0),
            }]
        );
    }

    #[test]
    fn dragging_past_the_corner_keeps_a_grabbable_strip_on_screen() {
        // A window may hang off the wall — parking one half off the edge is a
        // normal thing to want — but it must never strand completely outside,
        // where it can no longer be grabbed.
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(8, PointerPhase::Press, 200.0, 115.0), &surfaces)
            .unwrap();
        let moved = engine
            .handle(event(8, PointerPhase::Move, 2_000.0, 2_000.0), &surfaces)
            .unwrap();
        for intent in &moved {
            if let InteractionIntent::SetBounds { bounds, .. } = intent {
                assert!(
                    bounds.x < 1_000.0 && bounds.y < 800.0,
                    "a strip must stay on screen: {bounds:?}"
                );
                assert!(
                    bounds.x + bounds.width >= MIN_VISIBLE_GRAB
                        && bounds.y + bounds.height >= MIN_VISIBLE_GRAB,
                    "the strip must be big enough to grab: {bounds:?}"
                );
            }
        }
        let released = engine
            .handle(event(8, PointerPhase::Release, 2_000.0, 2_000.0), &surfaces)
            .unwrap();
        assert!(released.iter().any(|intent| matches!(
            intent,
            InteractionIntent::SetBounds { .. }
        )));
    }

    #[test]
    fn a_window_may_park_partly_off_the_left_edge() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(9, PointerPhase::Press, 200.0, 115.0), &surfaces)
            .unwrap();
        let intents = engine
            .handle(event(9, PointerPhase::Move, -2_000.0, -2_000.0), &surfaces)
            .unwrap();
        let InteractionIntent::SetBounds { bounds, .. } = &intents[0] else {
            panic!("expected move");
        };
        assert_eq!(
            *bounds,
            rect(
                -300.0 + MIN_VISIBLE_GRAB,
                -200.0 + MIN_VISIBLE_GRAB,
                300.0,
                200.0
            ),
            "dragged fully off the wall, only the grab strip remains inside"
        );
    }

    #[test]
    fn movement_snaps_deterministically_to_grid_and_workspace_edges() {
        let surfaces = vec![surface("card", rect(113.0, 113.0, 200.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(1, PointerPhase::Press, 200.0, 125.0), &surfaces)
            .unwrap();
        let intents = engine
            .handle(event(1, PointerPhase::Move, 234.0, 159.0), &surfaces)
            .unwrap();
        assert_eq!(
            intents,
            vec![InteractionIntent::SetBounds {
                surface_id: "card".to_owned(),
                bounds: rect(150.0, 150.0, 200.0, 200.0),
            }]
        );

        let mut edge_engine = InteractionEngine::new(config()).unwrap();
        edge_engine
            .handle(event(2, PointerPhase::Press, 200.0, 125.0), &surfaces)
            .unwrap();
        let edge = edge_engine
            .handle(event(2, PointerPhase::Move, 92.0, 17.0), &surfaces)
            .unwrap();
        assert_eq!(
            edge,
            vec![InteractionIntent::SetBounds {
                surface_id: "card".to_owned(),
                bounds: rect(0.0, 0.0, 200.0, 200.0),
            }]
        );

        assert_eq!(
            snap_translation(810.0, 200.0, 0.0, 1_020.0, Some(50.0), 10.0),
            820.0,
            "workspace edge wins an equal-distance tie with the grid"
        );
    }

    #[test]
    fn body_drag_moves_surface_without_edge_handles() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(7, PointerPhase::Press, 200.0, 180.0), &surfaces)
            .unwrap();
        assert_eq!(
            engine
                .handle(event(7, PointerPhase::Move, 300.0, 260.0), &surfaces)
                .unwrap(),
            vec![InteractionIntent::SetBounds {
                surface_id: "card".to_owned(),
                bounds: rect(200.0, 180.0, 300.0, 200.0),
            }]
        );
        assert_eq!(engine.snap_preview(), None, "mid-screen offers no zone");
    }

    #[test]
    fn lost_release_force_commits_edge_dock() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(6, PointerPhase::Press, 200.0, 180.0), &surfaces)
            .unwrap();
        // Drag to the bottom-left corner; the pointer then leaves the display
        // and the button-release is never delivered. force_release docks.
        engine
            .handle(event(6, PointerPhase::Move, 5.0, 795.0), &surfaces)
            .unwrap();
        assert!(engine.captured_pointer().is_some(), "still captured");
        let intents = engine.force_release();
        assert_eq!(
            intents,
            vec![
                InteractionIntent::SetBounds {
                    surface_id: "card".to_owned(),
                    bounds: rect(0.0, 400.0, 500.0, 400.0),
                },
                InteractionIntent::ReleasePointer { pointer_id: 6 },
            ]
        );
        assert_eq!(engine.captured_pointer(), None, "gesture cleared");
    }

    #[test]
    fn a_new_press_recovers_a_wedged_gesture() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(1, PointerPhase::Press, 200.0, 180.0), &surfaces)
            .unwrap();
        engine
            .handle(event(1, PointerPhase::Move, 250.0, 250.0), &surfaces)
            .unwrap();
        // Release was lost; the gesture is still captured. A fresh press must
        // not be swallowed - it recovers control rather than wedging forever.
        let intents = engine
            .handle(event(2, PointerPhase::Press, 200.0, 180.0), &surfaces)
            .unwrap();
        assert!(intents.iter().any(|intent| matches!(
            intent,
            InteractionIntent::CapturePointer { pointer_id: 2, .. }
        )));
    }

    #[test]
    fn move_near_side_edge_previews_and_commits_half_zone() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(2, PointerPhase::Press, 200.0, 180.0), &surfaces)
            .unwrap();
        engine
            .handle(event(2, PointerPhase::Move, 10.0, 400.0), &surfaces)
            .unwrap();
        assert_eq!(
            engine.snap_preview(),
            Some(rect(0.0, 0.0, 500.0, 800.0)),
            "left edge previews the left half"
        );
        assert_eq!(
            engine
                .handle(event(2, PointerPhase::Release, 10.0, 400.0), &surfaces)
                .unwrap(),
            vec![
                InteractionIntent::SetBounds {
                    surface_id: "card".to_owned(),
                    bounds: rect(0.0, 0.0, 500.0, 800.0),
                },
                InteractionIntent::ReleasePointer { pointer_id: 2 },
            ]
        );
    }

    #[test]
    fn move_into_corner_commits_quarter_zone_and_resize_previews_nothing() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(3, PointerPhase::Press, 200.0, 180.0), &surfaces)
            .unwrap();
        engine
            .handle(event(3, PointerPhase::Move, 5.0, 5.0), &surfaces)
            .unwrap();
        assert_eq!(engine.snap_preview(), Some(rect(0.0, 0.0, 500.0, 400.0)));
        let intents = engine
            .handle(event(3, PointerPhase::Release, 5.0, 5.0), &surfaces)
            .unwrap();
        assert!(intents.contains(&InteractionIntent::SetBounds {
            surface_id: "card".to_owned(),
            bounds: rect(0.0, 0.0, 500.0, 400.0),
        }));

        engine
            .handle(event(9, PointerPhase::Press, 400.0, 300.0), &surfaces)
            .unwrap();
        engine
            .handle(event(9, PointerPhase::Move, 5.0, 5.0), &surfaces)
            .unwrap();
        assert_eq!(engine.snap_preview(), None, "resize never docks");
    }

    #[test]
    fn resize_handles_snap_and_enforce_minimum_size() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(3, PointerPhase::Press, 400.0, 300.0), &surfaces)
            .unwrap();
        let intents = engine
            .handle(event(3, PointerPhase::Move, 151.0, 151.0), &surfaces)
            .unwrap();
        assert_eq!(
            intents,
            vec![InteractionIntent::SetBounds {
                surface_id: "card".to_owned(),
                bounds: rect(100.0, 100.0, 100.0, 80.0),
            }]
        );
    }

    #[test]
    fn cancel_restores_original_bounds_and_ignores_other_pointers() {
        let surfaces = vec![surface("card", rect(100.0, 100.0, 300.0, 200.0), 1)];
        let mut engine = InteractionEngine::new(config()).unwrap();
        engine
            .handle(event(5, PointerPhase::Press, 200.0, 115.0), &surfaces)
            .unwrap();
        assert!(
            engine
                .handle(event(6, PointerPhase::Move, 400.0, 400.0), &surfaces)
                .unwrap()
                .is_empty()
        );
        engine
            .handle(event(5, PointerPhase::Move, 300.0, 280.0), &surfaces)
            .unwrap();
        assert_eq!(
            engine
                .handle(event(5, PointerPhase::Cancel, 0.0, 0.0), &surfaces)
                .unwrap(),
            vec![
                InteractionIntent::SetBounds {
                    surface_id: "card".to_owned(),
                    bounds: rect(100.0, 100.0, 300.0, 200.0),
                },
                InteractionIntent::ReleasePointer { pointer_id: 5 },
            ]
        );
        assert_eq!(engine.captured_pointer(), None);
    }
}

#[cfg(test)]
mod grab_tests {
    use super::*;

    fn card() -> Vec<SurfaceGeometry> {
        vec![SurfaceGeometry {
            surface_id: "card".into(),
            bounds: LogicalRect {
                x: 100.0,
                y: 100.0,
                width: 300.0,
                height: 200.0,
            },
            z_order: 1,
        }]
    }

    fn hit(x: f64, y: f64) -> Option<HitRegion> {
        hit_test(&card(), LogicalPoint { x, y }, 14.0, 26.0)
            .unwrap()
            .map(|target| target.region)
    }

    #[test]
    fn an_edge_can_be_grabbed_from_just_outside_the_window() {
        assert_eq!(
            hit(93.0, 200.0),
            Some(HitRegion::Resize {
                handle: ResizeHandle::West
            })
        );
        assert_eq!(
            hit(407.0, 200.0),
            Some(HitRegion::Resize {
                handle: ResizeHandle::East
            })
        );
        assert_eq!(
            hit(250.0, 307.0),
            Some(HitRegion::Resize {
                handle: ResizeHandle::South
            })
        );
    }

    #[test]
    fn a_corner_grabs_from_outside_on_both_axes() {
        assert_eq!(
            hit(406.0, 306.0),
            Some(HitRegion::Resize {
                handle: ResizeHandle::SouthEast
            })
        );
    }

    #[test]
    fn far_outside_is_nobody() {
        assert_eq!(hit(500.0, 500.0), None);
        assert_eq!(hit(60.0, 100.0), None);
    }

    #[test]
    fn the_middle_of_the_title_bar_still_moves_the_window() {
        assert_eq!(hit(250.0, 110.0), Some(HitRegion::TopBar));
    }
}
