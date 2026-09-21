//! Built-in named operational views.
//!
//! The durable payload is the ordinary revisioned shared-view document. This
//! catalog only maps a human-facing project/view name to that existing format.

use crate::{
    DataBinding, DataSource, DeliveryPolicy, Error, InputPort, InputTarget, Layout, NormalizedRect,
    Placement, Result, Surface, SurfaceContent, WorkspaceDocument,
    view::{PlacedWidget, SharedViewDocument},
};
use canvas_core::ThemeMetrics;
use canvas_widgets::WidgetKind;
use std::{
    ops::Range,
    path::{Path, PathBuf},
};

pub const NAMES: &[&str] = &["bifrost/runtime", "elmers/now", "fleet/runtime"];

/// Bytes a stored view may occupy. One ceiling for both directions, so a
/// document that saves is a document that loads back.
const MAX_VIEW_BYTES: u64 = 256 * 1024;

pub fn load(name: &str) -> Result<SharedViewDocument> {
    let relative = view_path(name)?;
    if let Some(bytes) = crate::runtime_config::read(&relative, MAX_VIEW_BYTES)? {
        let document: SharedViewDocument = serde_json::from_slice(&bytes)
            .map_err(|error| Error(format!("Invalid runtime operational view {name}: {error}")))?;
        document.validate()?;
        if document.id != name.replace('/', "-") {
            return Err(Error(format!(
                "Runtime operational view {name} has mismatched id {}",
                document.id
            )));
        }
        return Ok(document);
    }
    let source = match name {
        "bifrost/runtime" => include_str!("../../../config/views/bifrost-runtime.json"),
        "elmers/now" => include_str!("../../../config/views/elmers-now.json"),
        "fleet/runtime" => include_str!("../../../config/views/fleet-runtime.json"),
        _ => return Err(Error(format!("Unknown operational view: {name}"))),
    };
    let document: SharedViewDocument = serde_json::from_str(source)
        .map_err(|error| Error(format!("Invalid built-in operational view {name}: {error}")))?;
    document.validate()?;
    Ok(document)
}

/// Save a view to the runtime root, where [`load`] will find it ahead of the
/// built-in copy.
///
/// Validated before it is written, never after: a document that would not load
/// must not reach the disk, because [`load`] refuses a malformed view and the
/// operator would be left with a name that no longer resolves. The same id rule
/// [`load`] enforces is enforced here, so a view cannot be saved under a name it
/// will be rejected under.
///
/// This is what makes a view a document rather than a constant, and therefore
/// what lets a view record anything — a position, a layout the operator moved,
/// a parameter they set.
pub fn save(name: &str, document: &SharedViewDocument) -> Result<PathBuf> {
    let relative = view_path(name)?;
    document.validate()?;
    let expected = name.replace('/', "-");
    if document.id != expected {
        return Err(Error(format!(
            "Cannot save operational view {name}: document id is {}, expected {expected}",
            document.id
        )));
    }
    let serialized = serde_json::to_vec_pretty(document)
        .map_err(|error| Error(format!("Cannot serialize operational view {name}: {error}")))?;
    crate::runtime_config::write(&relative, &serialized, MAX_VIEW_BYTES)
}

fn view_path(name: &str) -> Result<PathBuf> {
    if name.is_empty()
        || name.len() > 128
        || !name.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
    {
        return Err(Error(format!("Invalid operational view name: {name}")));
    }
    Ok(Path::new("views").join(name).with_extension("json"))
}

/// The room a materialized view has to lay itself out in.
///
/// Passed in rather than derived here because both halves are runtime facts a
/// catalog of views cannot know: the frame is whichever displays Canvas
/// currently occupies, and the metrics belong to the active theme. Widget
/// minimums are quoted in logical pixels by [`canvas_widgets::size_hint`], so a
/// normalized rect only becomes checkable once it is mapped onto a real frame.
#[derive(Debug, Clone, Copy)]
pub struct Fit {
    /// Logical pixels the normalized X axis maps onto.
    pub width: f32,
    /// Logical pixels the normalized Y axis maps onto.
    pub height: f32,
    /// The type scale the widgets will be drawn at. Every minimum derives from
    /// it, so a bigger scale asks for a bigger placement on its own.
    pub metrics: ThemeMetrics,
}

/// A materialized view, and what the frame was too small to honour.
pub struct Materialized {
    pub document: WorkspaceDocument,
    /// View widget ids still below their readable minimum after the layout took
    /// every pixel it could from their neighbours. Empty is the normal case;
    /// non-empty means the view wants more room than this frame has, which is
    /// worth saying out loud rather than drawing an unreadable smear.
    pub undersized: Vec<String>,
}

/// Materialize a view as ordinary movable widget surfaces. Existing surfaces
/// remain in the document with their state intact; the new layout merely stops
/// presenting them until another saved view recalls them.
pub fn materialize(name: &str, document: &WorkspaceDocument, fit: Fit) -> Result<Materialized> {
    let view = load(name)?;
    let widgets = view.widgets.into_iter().collect::<Vec<_>>();
    let (bounds, undersized) = fitted_bounds(&widgets, fit);
    let mut next = document.clone();
    let mut placements = Vec::with_capacity(widgets.len());
    let mut first = None;
    for (layer, (widget_id, placed)) in widgets.into_iter().enumerate() {
        let surface_id = format!("view-{}-{widget_id}", view.id);
        first.get_or_insert_with(|| surface_id.clone());
        let source = widget_source(&placed.spec)?;
        let inputs = source.as_ref().map_or_else(Vec::new, |_| {
            vec![InputPort {
                id: "snapshot".into(),
                contract: "canvas.widget.snapshot.v1".into(),
            }]
        });
        let surface = Surface {
            id: surface_id.clone(),
            title: widget_title(&placed.spec),
            content: SurfaceContent::Widget { spec: placed.spec },
            inputs,
        };
        if let Some(existing) = next
            .surfaces
            .iter_mut()
            .find(|candidate| candidate.id == surface_id)
        {
            *existing = surface;
        } else {
            next.surfaces.push(surface);
        }
        placements.push(Placement {
            surface_id: surface_id.clone(),
            bounds: bounds[layer],
            layer: layer as i32,
        });
        if let Some((owner, stream)) = source {
            let binding = DataBinding {
                id: format!("{surface_id}-snapshot"),
                source: DataSource {
                    owner,
                    stream,
                    contract: "canvas.widget.snapshot.v1".into(),
                },
                target: InputTarget {
                    surface_id: surface_id.clone(),
                    input_id: "snapshot".into(),
                },
                adapter: Some("widget-snapshot-v1".into()),
                delivery: DeliveryPolicy::Latest,
            };
            if let Some(existing) = next
                .bindings
                .iter_mut()
                .find(|candidate| candidate.id == binding.id)
            {
                *existing = binding;
            } else {
                next.bindings.push(binding);
            }
        }
    }
    next.layout = Layout::Canvas { placements };
    next.focused_surface_id = first;
    if next.surfaces == document.surfaces
        && next.layout == document.layout
        && next.focused_surface_id == document.focused_surface_id
    {
        return Ok(Materialized {
            document: document.clone(),
            undersized,
        });
    }
    next.revision = document.revision.saturating_add(1);
    Ok(Materialized {
        document: next,
        undersized,
    })
}

/// Normalized coordinates closer together than this are the same edge: view
/// files are hand-written decimals, so one widget's `0.21 + 0.49` is not
/// bit-identical to its neighbour's `0.70`. Well under a pixel on any desktop.
const EDGE_EPSILON: f32 = 1.0 / 32_768.0;

/// Slack under half a logical pixel is not worth acting on or reporting.
const PIXEL_EPSILON: f32 = 0.5;

/// How much a kind turns extra room into more information rather than just
/// bigger ink.
///
/// A time series draws more samples, a table more rows, a heatmap more cells; a
/// stat shows the same number however large it is set. When the layout has to
/// find pixels for a widget that is too small to read, it takes them from the
/// second group first.
fn appetite(kind: &WidgetKind) -> f32 {
    match kind {
        WidgetKind::TimeSeries { .. }
        | WidgetKind::Histogram { .. }
        | WidgetKind::SpectralWaterfall { .. }
        | WidgetKind::Heatmap { .. }
        | WidgetKind::RankedBar { .. }
        | WidgetKind::StackedBar { .. }
        | WidgetKind::PieChart { .. }
        | WidgetKind::Sankey { .. }
        | WidgetKind::Hierarchy { .. }
        | WidgetKind::MetricBoard { .. }
        | WidgetKind::Distribution { .. }
        | WidgetKind::RichCard { .. }
        | WidgetKind::ApprovalCard { .. }
        | WidgetKind::CommandPalette { .. }
        | WidgetKind::MediaTransport { .. }
        | WidgetKind::ModelTelemetry { .. }
        | WidgetKind::SpanTimeline { .. }
        | WidgetKind::StateTimeline { .. }
        | WidgetKind::RateBars { .. }
        | WidgetKind::Table { .. }
        | WidgetKind::EventLog { .. }
        | WidgetKind::StatusList { .. }
        | WidgetKind::NeuralGraph { .. }
        | WidgetKind::DagGraph { .. }
        | WidgetKind::SpatialTracks { .. }
        | WidgetKind::Chart { .. }
        | WidgetKind::Quant { .. }
        | WidgetKind::Module { .. }
        | WidgetKind::Launcher { .. } => 1.0,
        WidgetKind::Stat { .. }
        | WidgetKind::MediaCard { .. }
        | WidgetKind::Gauge { .. }
        | WidgetKind::Meter { .. }
        | WidgetKind::Clock { .. }
        | WidgetKind::Ticker { .. }
        | WidgetKind::Status { .. }
        | WidgetKind::Text { .. } => 0.0,
    }
}

/// Resolve the view's normalized rects into placements no widget is too small
/// to be read in, and name the ones the frame could not manage.
///
/// The two axes are solved independently. `preferred_aspect` is deliberately
/// not consulted: it is a preference, and charts are supposed to reflow to
/// whatever shape they are given rather than defend a ratio.
fn fitted_bounds(
    widgets: &[(String, PlacedWidget)],
    fit: Fit,
) -> (Vec<NormalizedRect>, Vec<String>) {
    let mut bounds = widgets
        .iter()
        .map(|(_, placed)| NormalizedRect {
            x: placed.rect.x,
            y: placed.rect.y,
            width: placed.rect.width,
            height: placed.rect.height,
        })
        .collect::<Vec<_>>();
    if widgets.is_empty()
        || !fit.width.is_finite()
        || !fit.height.is_finite()
        || fit.width <= 0.0
        || fit.height <= 0.0
    {
        // Nothing is known about the room available, so the view's own
        // arrangement is the best answer there is.
        return (bounds, Vec::new());
    }
    let hints = widgets
        .iter()
        .map(|(_, placed)| canvas_widgets::size_hint(&placed.spec.kind, &fit.metrics))
        .collect::<Vec<_>>();
    let appetites = widgets
        .iter()
        .map(|(_, placed)| appetite(&placed.spec.kind))
        .collect::<Vec<_>>();

    let horizontal = solve_axis(
        &bounds
            .iter()
            .map(|rect| (rect.x, rect.width))
            .collect::<Vec<_>>(),
        &hints
            .iter()
            .map(|hint| (hint.min_width, hint.comfortable_width))
            .collect::<Vec<_>>(),
        &appetites,
        fit.width,
    );
    let vertical = solve_axis(
        &bounds
            .iter()
            .map(|rect| (rect.y, rect.height))
            .collect::<Vec<_>>(),
        &hints
            .iter()
            .map(|hint| (hint.min_height, hint.comfortable_height))
            .collect::<Vec<_>>(),
        &appetites,
        fit.height,
    );

    let mut undersized = Vec::new();
    for (index, rect) in bounds.iter_mut().enumerate() {
        (rect.x, rect.width) = horizontal[index];
        (rect.y, rect.height) = vertical[index];
        if rect.width * fit.width + PIXEL_EPSILON < hints[index].min_width
            || rect.height * fit.height + PIXEL_EPSILON < hints[index].min_height
        {
            undersized.push(widgets[index].0.clone());
        }
    }
    (bounds, undersized)
}

/// Resize one axis so every widget clears its minimum, without moving any
/// widget past a neighbour.
///
/// The grid is the set of edges the view's own rects begin and end on, plus the
/// two frame edges, so every widget covers an exact run of tracks. Tracks are
/// laid end to end and only ever change size, so two runs that were disjoint
/// stay disjoint: overlapping surfaces are structurally impossible here rather
/// than merely untested for.
///
/// `spans` and `needs` are `(start, length)` normalized and `(min, comfortable)`
/// in logical pixels, one per widget; the answer is the fitted `(start, length)`
/// in the same order.
fn solve_axis(
    spans: &[(f32, f32)],
    needs: &[(f32, f32)],
    appetites: &[f32],
    extent: f32,
) -> Vec<(f32, f32)> {
    let mut edges = Vec::with_capacity(spans.len() * 2 + 2);
    edges.push(0.0);
    edges.push(1.0);
    for (start, length) in spans {
        edges.push(start.clamp(0.0, 1.0));
        edges.push((start + length).clamp(0.0, 1.0));
    }
    edges.retain(|edge: &f32| edge.is_finite());
    edges.sort_by(|a, b| a.partial_cmp(b).expect("edges were filtered to finite"));
    edges.dedup_by(|later, earlier| (*later - *earlier).abs() <= EDGE_EPSILON);
    let index_of = |value: f32| -> usize {
        edges
            .iter()
            .position(|edge| (edge - value).abs() <= EDGE_EPSILON)
            .unwrap_or_else(|| {
                edges
                    .partition_point(|edge| *edge < value)
                    .min(edges.len() - 1)
            })
    };

    let original = edges
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) * extent)
        .collect::<Vec<f32>>();
    let mut runs = Vec::with_capacity(spans.len());
    let mut tracks = Tracks::new(&original, extent);
    for (index, (start, length)) in spans.iter().enumerate() {
        let run = index_of(start.clamp(0.0, 1.0))..index_of((start + length).clamp(0.0, 1.0));
        if !run.is_empty() {
            tracks.claim(&run, needs[index], appetites[index]);
        }
        runs.push(run);
    }

    let sized = tracks.solve();
    let mut offset = 0.0;
    let mut fitted = Vec::with_capacity(edges.len());
    for (index, edge) in edges.iter().enumerate() {
        let moved = offset / extent;
        // Snap back to the view's own number wherever nothing actually moved,
        // so a layout the frame already suits comes out of this untouched
        // rather than a rounding error away from it.
        fitted.push(if (moved - edge).abs() <= EDGE_EPSILON {
            *edge
        } else {
            moved
        });
        if let Some(size) = sized.get(index) {
            offset += size;
        }
    }
    runs.into_iter()
        .map(|run| {
            let start = fitted[run.start];
            (start, (fitted[run.end] - start).max(0.0))
        })
        .collect()
}

/// The tracks of one axis, with what the widgets covering each one need.
///
/// Needs are split across a widget's tracks in proportion to the room the view
/// already gave them, so a track only has to satisfy the hungriest widget over
/// it and any widget's run adds back up to at least what it asked for. That
/// turns a constraint system into a per-track floor, which is what keeps this
/// a single pass with no iteration to converge or fail to.
struct Tracks<'a> {
    original: &'a [f32],
    /// Below this a widget over the track renders an unreadable smear.
    floor: Vec<f32>,
    /// The level a widget over the track stops gaining from, never below
    /// `floor`. Room above it is the surplus the layout is free to move.
    comfortable: Vec<f32>,
    appetite: Vec<f32>,
    extent: f32,
}

impl<'a> Tracks<'a> {
    fn new(original: &'a [f32], extent: f32) -> Self {
        Self {
            original,
            floor: vec![0.0; original.len()],
            comfortable: vec![0.0; original.len()],
            appetite: vec![0.0; original.len()],
            extent,
        }
    }

    fn claim(&mut self, run: &Range<usize>, needs: (f32, f32), appetite: f32) {
        let total = self.original[run.clone()].iter().sum::<f32>();
        for track in run.clone() {
            self.appetite[track] = self.appetite[track].max(appetite);
            if total <= 0.0 {
                continue;
            }
            let share = self.original[track] / total;
            self.floor[track] = self.floor[track].max(needs.0 * share);
            self.comfortable[track] = self.comfortable[track]
                .max(needs.1 * share)
                .max(self.floor[track]);
        }
    }

    /// The fitted track sizes.
    ///
    /// The layout never grows a widget past what the view gave it — the saved
    /// arrangement is the intent, and inflating a stat because the frame is
    /// wide would reshuffle a layout that had nothing wrong with it. Slack is
    /// spent the other way round: when something is too small, comfortable is
    /// the level everything else is protected down to, and the surplus above it
    /// is taken from the kinds that gain nothing from it first.
    fn solve(&self) -> Vec<f32> {
        self.attempt(&self.comfortable, false)
            .or_else(|| self.attempt(&self.floor, true))
            .unwrap_or_else(|| {
                // Even every floor at once does not fit: the frame is genuinely
                // too small for this view. Hold the proportions rather than
                // running off the frame or overlapping, and let the caller name
                // the widgets that came out unreadable.
                let floors = self.floor.iter().sum::<f32>();
                if floors <= 0.0 {
                    return self.original.to_vec();
                }
                let scale = self.extent / floors;
                self.floor.iter().map(|floor| floor * scale).collect()
            })
    }

    /// Raise every track to `target`, then buy the growth back out of surplus:
    /// first from tracks a frugal kind holds beyond what it can use, then from
    /// the same surplus on a kind that would have used it, and — only when
    /// `spend_comfort` — from the room between comfortable and the floor.
    /// `None` when the axis could not pay for `target`.
    fn attempt(&self, target: &[f32], spend_comfort: bool) -> Option<Vec<f32>> {
        let mut size = self
            .original
            .iter()
            .zip(target)
            .map(|(original, target)| original.max(*target))
            .collect::<Vec<f32>>();
        let mut excess = size.iter().sum::<f32>() - self.extent;
        excess = drain(&mut size, &self.comfortable, excess, |track| {
            self.appetite[track] <= 0.0
        });
        excess = drain(&mut size, &self.comfortable, excess, |_| true);
        if spend_comfort {
            excess = drain(&mut size, &self.floor, excess, |_| true);
        }
        (excess <= PIXEL_EPSILON).then_some(size)
    }
}

/// Take `excess` pixels back from the tracks `participates` allows, never below
/// `limit`, in proportion to how far above it each one sits. Returns what could
/// not be taken.
fn drain(
    size: &mut [f32],
    limit: &[f32],
    excess: f32,
    participates: impl Fn(usize) -> bool,
) -> f32 {
    if excess <= 0.0 {
        return 0.0;
    }
    let available = (0..size.len())
        .filter(|track| participates(*track))
        .map(|track| (size[track] - limit[track]).max(0.0))
        .sum::<f32>();
    if available <= 0.0 {
        return excess;
    }
    let take = excess.min(available);
    for track in 0..size.len() {
        if participates(track) {
            let room = (size[track] - limit[track]).max(0.0);
            size[track] -= room / available * take;
        }
    }
    excess - take
}

fn widget_source(spec: &canvas_widgets::WidgetSpec) -> Result<Option<(String, String)>> {
    let Some(source) = spec.data.source.as_deref() else {
        return Ok(None);
    };
    let Some((transport, identity)) = source.split_once(':') else {
        return Err(Error(format!(
            "Invalid operational widget source: {source}"
        )));
    };
    if transport != "prometheus" && transport != "sse" {
        return Err(Error(format!(
            "Unsupported operational widget transport: {transport}"
        )));
    }
    let Some((owner, stream)) = identity.split_once('/') else {
        return Err(Error(format!(
            "Invalid operational widget source: {source}"
        )));
    };
    let stream = format!("{transport}-{stream}");
    if owner.is_empty() || stream.is_empty() {
        return Err(Error(format!(
            "Invalid operational widget source: {source}"
        )));
    }
    Ok(Some((owner.into(), stream)))
}

fn widget_title(spec: &canvas_widgets::WidgetSpec) -> String {
    match &spec.kind {
        WidgetKind::Stat { label, .. } => label.clone(),
        WidgetKind::TimeSeries { title, .. }
        | WidgetKind::Histogram { title, .. }
        | WidgetKind::SpectralWaterfall { title, .. }
        | WidgetKind::Heatmap { title, .. }
        | WidgetKind::RankedBar { title, .. }
        | WidgetKind::StackedBar { title, .. }
        | WidgetKind::Sankey { title, .. }
        | WidgetKind::Hierarchy { title, .. }
        | WidgetKind::MetricBoard { title, .. }
        | WidgetKind::Distribution { title, .. }
        | WidgetKind::RichCard { title, .. }
        | WidgetKind::ApprovalCard { title, .. }
        | WidgetKind::MediaTransport { title, .. }
        | WidgetKind::ModelTelemetry { title, .. }
        | WidgetKind::SpanTimeline { title, .. }
        | WidgetKind::StateTimeline { title, .. }
        | WidgetKind::RateBars { title, .. }
        | WidgetKind::Table { title, .. }
        | WidgetKind::EventLog { title, .. }
        | WidgetKind::StatusList { title, .. }
        | WidgetKind::NeuralGraph { title, .. }
        | WidgetKind::DagGraph { title, .. } => title.clone(),
        _ => spec.id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Background, SurfaceChrome, ThemeRef};
    use canvas_core::Catalog;

    fn metrics() -> ThemeMetrics {
        canvas_core::ThemeSelection::coherence(
            canvas_core::ThemeMode::Dark,
            canvas_core::Accent::Brand,
        )
        .metrics()
    }

    /// A desktop with room to spare, where every view fits as its author drew it.
    fn roomy() -> Fit {
        Fit {
            width: 3_840.0,
            height: 2_160.0,
            metrics: metrics(),
        }
    }

    fn fit(width: f32, height: f32) -> Fit {
        Fit {
            width,
            height,
            metrics: metrics(),
        }
    }

    fn empty_document() -> WorkspaceDocument {
        WorkspaceDocument {
            schema_version: crate::SCHEMA_VERSION,
            id: "dock".into(),
            project_id: "dock".into(),
            revision: 1,
            theme: ThemeRef::Project,
            background: Background::Theme,
            chrome: SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: Layout::Empty,
            bindings: Vec::new(),
            focused_surface_id: None,
        }
    }

    fn placements(document: &WorkspaceDocument) -> Vec<Placement> {
        match &document.layout {
            Layout::Canvas { placements } => placements.clone(),
            _ => panic!("operational views materialize as a canvas"),
        }
    }

    /// Every widget of a view, paired with the placement it was given.
    fn placed_with_hints(name: &str, fit: Fit) -> Vec<(NormalizedRect, canvas_widgets::SizeHint)> {
        let view = load(name).expect("built-in view loads");
        let materialized =
            materialize(name, &empty_document(), fit).expect("built-in view materializes");
        placements(&materialized.document)
            .into_iter()
            .zip(view.widgets.values())
            .map(|(placement, widget)| {
                (
                    placement.bounds,
                    canvas_widgets::size_hint(&widget.spec.kind, &fit.metrics),
                )
            })
            .collect()
    }

    fn overlaps(a: NormalizedRect, b: NormalizedRect) -> bool {
        let gap = EDGE_EPSILON;
        a.x + a.width > b.x + gap
            && b.x + b.width > a.x + gap
            && a.y + a.height > b.y + gap
            && b.y + b.height > a.y + gap
    }

    #[test]
    fn every_named_view_is_a_valid_shared_view_document() {
        for name in NAMES {
            let view = load(name).unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(!view.widgets.is_empty());
            assert!(!view.sources.is_empty());
        }
    }

    #[test]
    fn names_are_exact_and_unknown_views_fail_loudly() {
        assert!(load("Bifrost Runtime").is_err());
        assert!(load("bifrost/runtime").is_ok());
    }

    #[test]
    fn saving_refuses_a_name_load_would_refuse() {
        let view = load("fleet/runtime").expect("built-in view");
        assert!(
            save("Fleet Runtime", &view).is_err(),
            "a view must not be saved under a name load() will not resolve"
        );
        assert!(save("", &view).is_err());
        assert!(save("../escape", &view).is_err());
    }

    #[test]
    fn saving_refuses_a_document_whose_id_does_not_match_the_name() {
        let view = load("fleet/runtime").expect("built-in view");
        assert_eq!(view.id, "fleet-runtime");
        // load() rejects this pairing on the way back in, so save() has to
        // reject it on the way out - otherwise the operator writes a file that
        // makes the name stop resolving.
        assert!(save("bifrost/runtime", &view).is_err());
    }

    #[test]
    fn saving_refuses_a_document_that_would_not_validate() {
        let mut view = load("fleet/runtime").expect("built-in view");
        view.schema_version = 0;
        assert!(
            view.validate().is_err(),
            "precondition: this document is invalid"
        );
        assert!(
            save("fleet/runtime", &view).is_err(),
            "an invalid document must never reach the disk"
        );
    }

    #[test]
    fn materializing_a_view_is_idempotent_and_preserves_hidden_work() {
        let catalog = Catalog::parse(
            r#"{"schema_version":1,"projects":[{"id":"dock","name":"Dock","theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]}]}"#,
        )
        .unwrap();
        let base = empty_document();
        let first = materialize("bifrost/runtime", &base, roomy()).unwrap();
        first.document.validate(&catalog).unwrap();
        assert_eq!(first.document.bindings.len(), 8);
        assert!(
            first
                .document
                .surfaces
                .iter()
                .all(|surface| surface.inputs.len() == 1)
        );
        assert!(first.document.bindings.iter().all(|binding| {
            binding.source.contract == "canvas.widget.snapshot.v1"
                && binding.adapter.as_deref() == Some("widget-snapshot-v1")
        }));
        let second = materialize("bifrost/runtime", &first.document, roomy()).unwrap();
        second.document.validate(&catalog).unwrap();
        assert_eq!(first.document, second.document);
    }

    #[test]
    fn a_frame_with_room_to_spare_places_widgets_exactly_where_the_view_says() {
        // The saved arrangement is the intent. Enforcing minimums must be a
        // repair for a frame too small to honour it, never a reason to reshuffle
        // a layout that had nothing wrong with it.
        for name in NAMES {
            let view = load(name).expect("built-in view loads");
            let materialized =
                materialize(name, &empty_document(), roomy()).expect("view materializes");
            for (placement, widget) in placements(&materialized.document)
                .iter()
                .zip(view.widgets.values())
            {
                assert_eq!(
                    (placement.bounds.x, placement.bounds.y),
                    (widget.rect.x, widget.rect.y),
                    "{name} moved a widget that fitted"
                );
                assert!(
                    (placement.bounds.width - widget.rect.width).abs() <= EDGE_EPSILON
                        && (placement.bounds.height - widget.rect.height).abs() <= EDGE_EPSILON,
                    "{name} resized a widget that fitted"
                );
            }
            assert!(
                materialized.undersized.is_empty(),
                "{name} reported {:?} undersized on a 4K desktop",
                materialized.undersized
            );
        }
    }

    #[test]
    fn a_cramped_frame_grows_widgets_to_their_minimum_out_of_their_neighbours() {
        // Small enough that the stats and the gauge are below their minimum
        // width, but not so small that the view cannot fit at all.
        let cramped = fit(640.0, 560.0);
        for name in NAMES {
            let placed = placed_with_hints(name, cramped);
            let tight = placed.iter().any(|(bounds, hint)| {
                bounds.width * cramped.width < hint.comfortable_width
                    || bounds.height * cramped.height < hint.comfortable_height
            });
            assert!(tight, "{name} is not actually cramped at this size");
            for (bounds, hint) in &placed {
                assert!(
                    bounds.width * cramped.width + PIXEL_EPSILON >= hint.min_width,
                    "{name} left a widget {}px wide, below its {}px minimum",
                    bounds.width * cramped.width,
                    hint.min_width
                );
                assert!(
                    bounds.height * cramped.height + PIXEL_EPSILON >= hint.min_height,
                    "{name} left a widget {}px tall, below its {}px minimum",
                    bounds.height * cramped.height,
                    hint.min_height
                );
            }
        }
    }

    #[test]
    fn no_frame_size_ever_makes_two_widgets_overlap() {
        // The property that makes minimum enforcement safe: pixels are moved
        // between tracks of one grid, never taken by drawing one widget over
        // another. A frame far too small to fit the view is included on
        // purpose — that case degrades, it does not pile up.
        for name in NAMES {
            for (width, height) in [
                (3_840.0, 2_160.0),
                (1_280.0, 800.0),
                (900.0, 700.0),
                (640.0, 560.0),
                (320.0, 240.0),
            ] {
                let placed = placed_with_hints(name, fit(width, height));
                for (index, (bounds, _)) in placed.iter().enumerate() {
                    assert!(
                        bounds.x >= -EDGE_EPSILON
                            && bounds.y >= -EDGE_EPSILON
                            && bounds.x + bounds.width <= 1.0 + EDGE_EPSILON
                            && bounds.y + bounds.height <= 1.0 + EDGE_EPSILON,
                        "{name} at {width}x{height} placed a widget off the frame: {bounds:?}"
                    );
                    for (other, _) in &placed[index + 1..] {
                        assert!(
                            !overlaps(*bounds, *other),
                            "{name} at {width}x{height} overlapped {bounds:?} with {other:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_frame_too_small_for_the_view_says_so_instead_of_drawing_a_smear() {
        let tiny = fit(320.0, 240.0);
        let materialized =
            materialize("bifrost/runtime", &empty_document(), tiny).expect("view materializes");
        assert!(
            !materialized.undersized.is_empty(),
            "a 320x240 frame cannot hold eight widgets and must report it"
        );
        let view = load("bifrost/runtime").expect("built-in view loads");
        assert!(
            materialized
                .undersized
                .iter()
                .all(|id| view.widgets.contains_key(id)),
            "reported ids must name widgets: {:?}",
            materialized.undersized
        );
    }

    #[test]
    fn the_surplus_a_stat_cannot_use_is_spent_before_a_chart_is_squeezed() {
        // fleet/runtime's top row is four stats and gauges beside nothing else,
        // and its bottom half is charts. As the frame narrows, the row of
        // numbers should give up its slack first: a stat reads the same at 200px
        // as at 400, while a time series draws fewer samples.
        let wide = placed_with_hints("fleet/runtime", fit(1_600.0, 900.0));
        let narrow = placed_with_hints("fleet/runtime", fit(760.0, 620.0));
        let share = |placed: &[(NormalizedRect, canvas_widgets::SizeHint)], appetite: f32| {
            let view = load("fleet/runtime").expect("built-in view loads");
            placed
                .iter()
                .zip(view.widgets.values())
                .filter(|(_, widget)| super::appetite(&widget.spec.kind) == appetite)
                .map(|((bounds, _), _)| bounds.height)
                .sum::<f32>()
        };
        assert!(
            share(&narrow, 0.0) < share(&wide, 0.0) + EDGE_EPSILON,
            "the frugal kinds should not have grown their share"
        );
        assert!(
            share(&narrow, 1.0) >= share(&wide, 1.0) - EDGE_EPSILON,
            "the kinds that use room should keep at least their share"
        );
    }

    #[test]
    fn an_unknown_frame_leaves_the_view_exactly_as_its_author_drew_it() {
        // Canvas can be asked to materialize before any display has answered,
        // where the frame is zero-sized. Guessing at minimums against a frame
        // of nothing would scramble the layout for no reason.
        let blank = fit(0.0, 0.0);
        let materialized =
            materialize("elmers/now", &empty_document(), blank).expect("view materializes");
        let view = load("elmers/now").expect("built-in view loads");
        for (placement, widget) in placements(&materialized.document)
            .iter()
            .zip(view.widgets.values())
        {
            assert_eq!(placement.bounds.x, widget.rect.x);
            assert_eq!(placement.bounds.width, widget.rect.width);
        }
        assert!(materialized.undersized.is_empty());
    }
}
