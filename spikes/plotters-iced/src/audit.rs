//! An instrumented `DrawingBackend` that records what plotters asks for, so the
//! spike can report real primitive counts (the wall-size cost question) and
//! catch requests the iced backend silently drops.

use plotters_backend::{
    BackendColor, BackendCoord, BackendStyle, BackendTextStyle, DrawingBackend, DrawingErrorKind,
    FontTransform,
};
use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

#[derive(Debug)]
pub struct AuditError;

impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "audit backend error")
    }
}
impl std::error::Error for AuditError {}

#[derive(Debug, Clone, Default)]
pub struct Counts {
    pub pixels: usize,
    pub lines: usize,
    pub rects_filled: usize,
    pub rects_stroked: usize,
    pub paths: usize,
    pub path_vertices: usize,
    pub circles: usize,
    pub polygons: usize,
    pub polygon_vertices: usize,
    pub texts: usize,
    /// Calls plotters made that the iced backend implements as a no-op.
    pub bitmaps_dropped: usize,
    /// (content, requested transform) for every text plotters asked to draw.
    pub text_calls: Vec<(String, &'static str)>,
}

impl Counts {
    /// Every one of these becomes at least one `canvas::Frame` operation.
    pub fn frame_ops(&self) -> usize {
        self.pixels
            + self.lines
            + self.rects_filled
            + self.rects_stroked
            + self.paths
            + self.circles
            + self.polygons
            + self.texts
    }
    pub fn rotated_texts(&self) -> Vec<&(String, &'static str)> {
        self.text_calls.iter().filter(|(_, t)| *t != "none").collect()
    }
}

/// `into_drawing_area` consumes the backend, so the tally lives behind a shared
/// handle the caller keeps.
pub struct AuditBackend {
    size: (u32, u32),
    counts: Rc<RefCell<Counts>>,
}

impl AuditBackend {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            size: (width, height),
            counts: Rc::new(RefCell::new(Counts::default())),
        }
    }

    /// Keep this before handing the backend to plotters.
    pub fn handle(&self) -> Rc<RefCell<Counts>> {
        Rc::clone(&self.counts)
    }
}

fn transform_name(t: FontTransform) -> &'static str {
    match t {
        FontTransform::None => "none",
        FontTransform::Rotate90 => "Rotate90",
        FontTransform::Rotate180 => "Rotate180",
        FontTransform::Rotate270 => "Rotate270",
    }
}

impl DrawingBackend for AuditBackend {
    type ErrorType = AuditError;

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
        _p: BackendCoord,
        color: BackendColor,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if color.alpha != 0.0 {
            self.counts.borrow_mut().pixels += 1;
        }
        Ok(())
    }

    fn draw_line<S: BackendStyle>(
        &mut self,
        _a: BackendCoord,
        _b: BackendCoord,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha != 0.0 {
            self.counts.borrow_mut().lines += 1;
        }
        Ok(())
    }

    fn draw_rect<S: BackendStyle>(
        &mut self,
        _a: BackendCoord,
        _b: BackendCoord,
        style: &S,
        fill: bool,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha != 0.0 {
            if fill {
                self.counts.borrow_mut().rects_filled += 1;
            } else {
                self.counts.borrow_mut().rects_stroked += 1;
            }
        }
        Ok(())
    }

    fn draw_path<S: BackendStyle, I: IntoIterator<Item = BackendCoord>>(
        &mut self,
        path: I,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let n = path.into_iter().count();
        if style.color().alpha != 0.0 {
            self.counts.borrow_mut().paths += 1;
            self.counts.borrow_mut().path_vertices += n;
        }
        Ok(())
    }

    fn draw_circle<S: BackendStyle>(
        &mut self,
        _c: BackendCoord,
        _r: u32,
        style: &S,
        _fill: bool,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha != 0.0 {
            self.counts.borrow_mut().circles += 1;
        }
        Ok(())
    }

    fn fill_polygon<S: BackendStyle, I: IntoIterator<Item = BackendCoord>>(
        &mut self,
        vert: I,
        style: &S,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        let n = vert.into_iter().count();
        if style.color().alpha != 0.0 {
            self.counts.borrow_mut().polygons += 1;
            self.counts.borrow_mut().polygon_vertices += n;
        }
        Ok(())
    }

    fn draw_text<S: BackendTextStyle>(
        &mut self,
        text: &str,
        style: &S,
        _pos: BackendCoord,
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        if style.color().alpha != 0.0 {
            self.counts.borrow_mut().texts += 1;
            self.counts
                .borrow_mut()
                .text_calls
                .push((text.to_owned(), transform_name(style.transform())));
        }
        Ok(())
    }

    fn estimate_text_size<S: BackendTextStyle>(
        &self,
        text: &str,
        style: &S,
    ) -> Result<(u32, u32), DrawingErrorKind<Self::ErrorType>> {
        // Crude on purpose. The real backend defers to iced's shaper -- see the
        // report for the size bug that lives there.
        let size = style.size() as u32;
        Ok(((text.chars().count() as u32 * size * 6) / 10, size))
    }

    fn blit_bitmap(
        &mut self,
        _pos: BackendCoord,
        _dim: (u32, u32),
        _src: &[u8],
    ) -> Result<(), DrawingErrorKind<Self::ErrorType>> {
        self.counts.borrow_mut().bitmaps_dropped += 1;
        Ok(())
    }
}
