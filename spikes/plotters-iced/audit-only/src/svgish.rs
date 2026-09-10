//! An SVG `DrawingBackend` that reproduces plotters-iced2's semantics exactly,
//! so the spike can be *looked at* without a GPU:
//!
//!   * `draw_text` ignores `style.transform()`   (plotters-iced2 backend/mod.rs:236)
//!   * `blit_bitmap` is a no-op                  (plotters-iced2 backend/mod.rs:299)
//!   * `estimate_text_size` measures at the renderer's DEFAULT size rather than
//!     the requested size (plotters-iced2 backend/mod.rs:280 passes
//!     `self.backend.default_size()` instead of `style.size()`).
//!
//! Glyph advance is approximated; the layout *bugs* are reproduced faithfully,
//! the letterforms are not. Set FAITHFUL=0 to measure text honestly instead and
//! see what the same chart looks like once that bug is fixed.

use plotters_backend::{
    BackendColor, BackendCoord, BackendStyle, BackendTextStyle, DrawingBackend, DrawingErrorKind,
    FontStyle, text_anchor,
};
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

/// iced's default text size.
const DEFAULT_SIZE: f64 = 16.0;

#[derive(Debug)]
pub struct SvgError;
impl fmt::Display for SvgError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "svg backend error")
    }
}
impl std::error::Error for SvgError {}

fn paint(c: &BackendColor) -> String {
    format!("rgb({},{},{})", c.rgb.0, c.rgb.1, c.rgb.2)
}

pub struct SvgishBackend {
    size: (u32, u32),
    /// `into_drawing_area` consumes the backend, so the caller keeps a handle.
    body: Rc<RefCell<String>>,
    faithful: bool,
}

impl SvgishBackend {
    pub fn new(width: u32, height: u32, background: [u8; 3], faithful: bool) -> Self {
        let body = format!(
            r#"<rect x="0" y="0" width="{width}" height="{height}" fill="rgb({},{},{})"/>"#,
            background[0], background[1], background[2]
        );
        Self {
            size: (width, height),
            body: Rc::new(RefCell::new(body)),
            faithful,
        }
    }

    /// Keep this before handing the backend to plotters.
    pub fn handle(&self) -> Rc<RefCell<String>> {
        Rc::clone(&self.body)
    }

    /// Padded to a square canvas: macOS `qlmanage` always renders into a square
    /// thumbnail and will otherwise scale and crop the chart.
    pub fn wrap(width: u32, height: u32, body: &str) -> String {
        let _ = height;
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{width}" viewBox="0 0 {width} {width}"><rect x="0" y="0" width="{width}" height="{width}" fill="rgb(40,40,40)"/>{body}</svg>"#
        )
    }
}

impl DrawingBackend for SvgishBackend {
    type ErrorType = SvgError;

    fn get_size(&self) -> (u32, u32) {
        self.size
    }
    fn ensure_prepared(&mut self) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        Ok(())
    }
    fn present(&mut self) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        Ok(())
    }

    fn draw_pixel(
        &mut self,
        p: BackendCoord,
        color: BackendColor,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if color.alpha != 0.0 {
            self.body.borrow_mut().push_str(&format!(
                r#"<rect x="{}" y="{}" width="1" height="1" fill="{}" fill-opacity="{}"/>"#,
                p.0,
                p.1,
                paint(&color),
                color.alpha
            ));
        }
        Ok(())
    }

    fn draw_line<S: BackendStyle>(
        &mut self,
        a: BackendCoord,
        b: BackendCoord,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let c = style.color();
        if c.alpha != 0.0 {
            self.body.borrow_mut().push_str(&format!(
                r#"<line x1="{}" y1="{}" x2="{}" y2="{}" stroke="{}" stroke-opacity="{}" stroke-width="{}"/>"#,
                a.0, a.1, b.0, b.1, paint(&c), c.alpha, style.stroke_width()
            ));
        }
        Ok(())
    }

    fn draw_rect<S: BackendStyle>(
        &mut self,
        a: BackendCoord,
        b: BackendCoord,
        style: &S,
        fill: bool,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let c = style.color();
        if c.alpha == 0.0 {
            return Ok(());
        }
        let (x, y) = (a.0.min(b.0), a.1.min(b.1));
        let (w, h) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
        if fill {
            self.body.borrow_mut().push_str(&format!(
                r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{}" fill-opacity="{}"/>"#,
                paint(&c),
                c.alpha
            ));
        } else {
            self.body.borrow_mut().push_str(&format!(
                r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="none" stroke="{}" stroke-opacity="{}" stroke-width="{}"/>"#,
                paint(&c), c.alpha, style.stroke_width()
            ));
        }
        Ok(())
    }

    fn draw_path<S: BackendStyle, I: IntoIterator<Item = BackendCoord>>(
        &mut self,
        path: I,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let c = style.color();
        let pts: Vec<String> = path.into_iter().map(|p| format!("{},{}", p.0, p.1)).collect();
        if c.alpha != 0.0 && !pts.is_empty() {
            self.body.borrow_mut().push_str(&format!(
                r#"<polyline points="{}" fill="none" stroke="{}" stroke-opacity="{}" stroke-width="{}" stroke-linejoin="round" stroke-linecap="round"/>"#,
                pts.join(" "), paint(&c), c.alpha, style.stroke_width()
            ));
        }
        Ok(())
    }

    fn draw_circle<S: BackendStyle>(
        &mut self,
        center: BackendCoord,
        radius: u32,
        style: &S,
        fill: bool,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let c = style.color();
        if c.alpha == 0.0 {
            return Ok(());
        }
        let common = format!(r#"cx="{}" cy="{}" r="{radius}""#, center.0, center.1);
        self.body.borrow_mut().push_str(&if fill {
            format!(
                r#"<circle {common} fill="{}" fill-opacity="{}"/>"#,
                paint(&c),
                c.alpha
            )
        } else {
            format!(
                r#"<circle {common} fill="none" stroke="{}" stroke-opacity="{}" stroke-width="{}"/>"#,
                paint(&c),
                c.alpha,
                style.stroke_width()
            )
        });
        Ok(())
    }

    fn fill_polygon<S: BackendStyle, I: IntoIterator<Item = BackendCoord>>(
        &mut self,
        vert: I,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let c = style.color();
        let pts: Vec<String> = vert.into_iter().map(|p| format!("{},{}", p.0, p.1)).collect();
        if c.alpha != 0.0 && !pts.is_empty() {
            self.body.borrow_mut().push_str(&format!(
                r#"<polygon points="{}" fill="{}" fill-opacity="{}"/>"#,
                pts.join(" "),
                paint(&c),
                c.alpha
            ));
        }
        Ok(())
    }

    fn draw_text<S: BackendTextStyle>(
        &mut self,
        text: &str,
        style: &S,
        pos: BackendCoord,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let c = style.color();
        if c.alpha == 0.0 {
            return Ok(());
        }
        let anchor = match style.anchor().h_pos {
            text_anchor::HPos::Left => "start",
            text_anchor::HPos::Center => "middle",
            text_anchor::HPos::Right => "end",
        };
        let baseline = match style.anchor().v_pos {
            text_anchor::VPos::Top => "text-before-edge",
            text_anchor::VPos::Center => "central",
            text_anchor::VPos::Bottom => "text-after-edge",
        };
        let weight = if matches!(style.style(), FontStyle::Bold) {
            "bold"
        } else {
            "normal"
        };
        // NOTE: style.transform() is deliberately ignored -- this is what
        // plotters-iced2 does, and reproducing it is the point.
        let escaped = text
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        self.body.borrow_mut().push_str(&format!(
            r#"<text x="{}" y="{}" fill="{}" fill-opacity="{}" font-family="sans-serif" font-size="{}" font-weight="{weight}" text-anchor="{anchor}" dominant-baseline="{baseline}">{escaped}</text>"#,
            pos.0, pos.1, paint(&c), c.alpha, style.size()
        ));
        Ok(())
    }

    fn estimate_text_size<S: BackendTextStyle>(
        &self,
        text: &str,
        style: &S,
    ) -> Result<(u32, u32), DrawingErrorKind<Self::ErrorType>> {
        // The bug under test: plotters-iced2 measures at the renderer default
        // size, not at the size the caller asked for.
        let measured_at = if self.faithful {
            DEFAULT_SIZE
        } else {
            style.size()
        };
        let w = (text.chars().count() as f64 * measured_at * 0.55).ceil() as u32;
        let h = (measured_at * 1.2).ceil() as u32;
        Ok((w, h))
    }

    fn blit_bitmap(
        &mut self,
        _pos: BackendCoord,
        _dim: (u32, u32),
        _src: &[u8],
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        Ok(()) // exactly as plotters-iced2 does: silently nothing
    }
}
