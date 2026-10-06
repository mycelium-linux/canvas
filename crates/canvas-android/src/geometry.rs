//! Native surface adapter: use the same durable layout and BSP solver as desktop.
use canvas_core::{
    layout::Rect,
    workspace_layout::{Layout, NormalizedRect, Placement},
};

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
