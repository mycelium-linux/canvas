//! The fitted pixel-content rectangle shared by rendering and input routing.
use crate::interaction::LogicalRect;

pub fn matte_padding(source_pixels: [u32; 2]) -> f32 {
    (source_pixels[0].min(source_pixels[1]) as f32 * 0.008).clamp(6.0, 12.0)
}

pub fn fitted_content(
    bounds: LogicalRect,
    source: [u32; 2],
    title_height: f64,
) -> Option<LogicalRect> {
    if source.contains(&0) {
        return None;
    }
    let padding = f64::from(matte_padding(source));
    let width = bounds.width - 2.0 * padding;
    let height = bounds.height - title_height - 2.0 * padding;
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let scale = (width / f64::from(source[0])).min(height / f64::from(source[1]));
    let fitted_width = f64::from(source[0]) * scale;
    let fitted_height = f64::from(source[1]) * scale;
    Some(LogicalRect {
        x: bounds.x + padding + (width - fitted_width) / 2.0,
        y: bounds.y + title_height + padding + (height - fitted_height) / 2.0,
        width: fitted_width,
        height: fitted_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fitted_input_tracks_letterboxing_title_and_matte() {
        let bounds = LogicalRect {
            x: 100.0,
            y: 50.0,
            width: 800.0,
            height: 600.0,
        };
        let fitted = fitted_content(bounds, [1280, 720], 24.0).unwrap();
        assert!((fitted.width / fitted.height - 16.0 / 9.0).abs() < 1e-9);
        assert!((fitted.x + fitted.width / 2.0 - 500.0).abs() < 1e-9);
        assert!((fitted.y + fitted.height / 2.0 - 362.0).abs() < 1e-9);
        assert!(fitted.y > bounds.y + 24.0);
        assert!(fitted_content(bounds, [0, 720], 0.0).is_none());
    }
}
