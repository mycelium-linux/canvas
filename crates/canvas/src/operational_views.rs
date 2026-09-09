//! Built-in named operational views.
//!
//! The durable payload is the ordinary revisioned shared-view document. This
//! catalog only maps a human-facing project/view name to that existing format.

use crate::{
    DataBinding, DataSource, DeliveryPolicy, Error, InputPort, InputTarget, Layout, NormalizedRect,
    Placement, Result, Surface, SurfaceContent, WorkspaceDocument, view::SharedViewDocument,
};
use std::path::{Path, PathBuf};

pub const NAMES: &[&str] = &["bifrost/runtime", "elmers/now", "fleet/runtime"];

pub fn load(name: &str) -> Result<SharedViewDocument> {
    let relative = view_path(name)?;
    if let Some(bytes) = crate::runtime_config::read(&relative, 256 * 1024)? {
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

/// Materialize a view as ordinary movable widget surfaces. Existing surfaces
/// remain in the document with their state intact; the new layout merely stops
/// presenting them until another saved view recalls them.
pub fn materialize(name: &str, document: &WorkspaceDocument) -> Result<WorkspaceDocument> {
    let view = load(name)?;
    let mut next = document.clone();
    let mut placements = Vec::with_capacity(view.widgets.len());
    let mut first = None;
    for (layer, (widget_id, placed)) in view.widgets.into_iter().enumerate() {
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
            bounds: NormalizedRect {
                x: placed.rect.x,
                y: placed.rect.y,
                width: placed.rect.width,
                height: placed.rect.height,
            },
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
        return Ok(document.clone());
    }
    next.revision = document.revision.saturating_add(1);
    Ok(next)
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
    use canvas_widgets::WidgetKind;
    match &spec.kind {
        WidgetKind::Stat { label, .. } => label.clone(),
        WidgetKind::TimeSeries { title, .. }
        | WidgetKind::Histogram { title, .. }
        | WidgetKind::Heatmap { title, .. }
        | WidgetKind::RankedBar { title, .. }
        | WidgetKind::StackedBar { title, .. }
        | WidgetKind::StateTimeline { title, .. }
        | WidgetKind::SloBurnRate { title, .. }
        | WidgetKind::Table { title, .. }
        | WidgetKind::EventLog { title, .. }
        | WidgetKind::NodeSummary { title, .. } => title.clone(),
        _ => spec.id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Background, SurfaceChrome, ThemeRef};
    use canvas_core::Catalog;

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
    fn materializing_a_view_is_idempotent_and_preserves_hidden_work() {
        let catalog = Catalog::parse(
            r#"{"schema_version":1,"projects":[{"id":"dock","name":"Dock","theme":{"source":"coherence","mode":"dark","accent":"info"},"resources":[]}]}"#,
        )
        .unwrap();
        let base = WorkspaceDocument {
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
        };
        let first = materialize("bifrost/runtime", &base).unwrap();
        first.validate(&catalog).unwrap();
        assert_eq!(first.bindings.len(), 8);
        assert!(
            first
                .surfaces
                .iter()
                .all(|surface| surface.inputs.len() == 1)
        );
        assert!(first.bindings.iter().all(|binding| {
            binding.source.contract == "canvas.widget.snapshot.v1"
                && binding.adapter.as_deref() == Some("widget-snapshot-v1")
        }));
        let second = materialize("bifrost/runtime", &first).unwrap();
        second.validate(&catalog).unwrap();
        assert_eq!(first, second);
    }
}
