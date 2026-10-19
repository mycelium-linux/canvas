//! Native surface adapter: use the same durable layout and BSP solver as desktop.
use canvas_core::{
    layout::Rect,
    workspace_layout::{Layout, NormalizedRect, Placement},
};

/// Only an explicitly edge-to-edge, single ordinary stream overlays shell chrome.
pub fn workspace_viewport(
    document: &canvas_workspace::WorkspaceDocument,
    width: u32,
    height: u32,
    shell_height: u32,
    has_header: bool,
) -> Result<Rect, String> {
    use canvas_workspace::{SurfaceContent, SurfaceTitleMode};
    let immersive = !has_header
        && document.chrome.title == SurfaceTitleMode::Hidden
        && document.chrome.canvas_padding == 0
        && document.chrome.surface_gap == 0
        && matches!(document.active_layout(), Layout::Surface { surface_id }
            if document.surfaces.iter().any(|surface| surface.id == *surface_id
                && matches!(surface.content, SurfaceContent::Stream { matte: None, .. })));
    let inset = if immersive { 0 } else { shell_height };
    if width == 0 || height <= inset || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err("Invalid workspace output extent".into());
    }
    Ok(Rect {
        x: 0,
        y: inset as i32,
        width,
        height: height - inset,
    })
}

pub fn video_frame(width: u32, height: u32, windowed: bool) -> Result<Rect, String> {
    if width == 0 || height == 0 {
        return Err("Output extent must be positive".into());
    }
    let layout = if windowed {
        Layout::Canvas {
            placements: vec![Placement {
                surface_id: "komo4".into(),
                bounds: NormalizedRect {
                    x: 0.04,
                    y: 0.12,
                    width: 0.55,
                    height: ((width as f32 * 0.55 * 9.0 / 16.0) / height as f32).min(0.65),
                },
                layer: 0,
            }],
        }
    } else {
        Layout::Surface {
            surface_id: "komo4".into(),
        }
    };
    let placements = canvas_core::layout_tree::solve(
        &layout,
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        },
    )
    .map_err(|error| error.to_string())?;
    placements
        .first()
        .map(|placement| placement.frame)
        .ok_or_else(|| "Video layout has no surface".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn stream_document() -> canvas_workspace::WorkspaceDocument {
        serde_json::from_value(serde_json::json!({
            "schema_version":1,"id":"tv-deck","project_id":"deck","revision":1,
            "theme":{"source":"project"},
            "chrome":{"title":"hidden","canvas_padding":0,"surface_gap":0},
            "surfaces":[{"id":"video","title":"Deck","content":{"kind":"stream","url":"http://127.0.0.1/deck.m3u8"}}],
            "layout":{"kind":"surface","surface_id":"video"}
        })).unwrap()
    }
    #[test]
    fn immersive_stream_uses_the_full_display_and_dashboards_reserve_chrome() {
        let mut document = stream_document();
        for (width, height) in [(1920, 1080), (3840, 2160), (1280, 720)] {
            assert_eq!(
                workspace_viewport(&document, width, height, 30, false).unwrap(),
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height
                }
            );
            let header = workspace_viewport(&document, width, height, 30, true).unwrap();
            assert_eq!((header.y, header.height), (30, height - 30));
        }
        document.chrome = Default::default();
        assert_eq!(
            workspace_viewport(&document, 1920, 1080, 30, false)
                .unwrap()
                .height,
            1050
        );
        assert!(workspace_viewport(&document, 0, 1080, 30, false).is_err());
        assert!(workspace_viewport(&document, 1920, 30, 30, false).is_err());
        document = stream_document();
        document.surfaces[0].content = canvas_workspace::SurfaceContent::Unsupported(
            serde_json::json!({"kind":"future_widget"}),
        );
        assert_eq!(
            workspace_viewport(&document, 1920, 1080, 30, false)
                .unwrap()
                .y,
            30
        );
        document = stream_document();
        document.layout = Layout::Canvas { placements: vec![] };
        assert_eq!(
            workspace_viewport(&document, 1920, 1080, 30, false)
                .unwrap()
                .y,
            30
        );
    }
    #[test]
    fn measured_output_is_geometry_authority() {
        for (width, height) in [(1920, 1080), (1280, 720), (3840, 2160), (800, 1280)] {
            let frame = video_frame(width, height, true).unwrap();
            assert!(frame.x >= 0 && frame.y >= 0);
            assert!(frame.x as u32 + frame.width <= width);
            assert!(frame.y as u32 + frame.height <= height);
            assert_eq!(
                video_frame(width, height, false).unwrap(),
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height
                }
            );
        }
        assert!(video_frame(0, 1080, true).is_err());
    }
}
