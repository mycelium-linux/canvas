//! What a surface shows before it has anything to show.
//!
//! A window that has been created but has not presented yet used to say so in a
//! sentence. A sentence is the wrong shape for a state that lasts a few hundred
//! milliseconds: it reads as an error, it does not tell the eye whether anything
//! is happening, and it flashes long enough to be noticed but not long enough to
//! be read.
//!
//! The mark is a dotted Lissajous ring — the same one `widget.spinner` draws in
//! Lua, evaluated here because the host owns this window's frames and the
//! producer that will replace it has not spoken yet. Three and two: the curve
//! only closes after a full turn, so the eye keeps finding new segments instead
//! of a shape it has already learned.

use iced::widget::canvas::{self, Geometry, Path};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme};

pub struct LoadingMark {
    /// Radians, advanced by the host's frame clock.
    pub phase: f32,
    pub ink: Color,
    pub muted: Color,
}

impl LoadingMark {
    const DOTS: usize = 26;
    const RADIUS: f32 = 26.0;
}

impl<Message> canvas::Program<Message> for LoadingMark {
    type State = ();

    fn draw(
        &self,
        _: &(),
        renderer: &Renderer,
        _: &Theme,
        bounds: Rectangle,
        _: iced::mouse::Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let centre = Point::new(bounds.width * 0.5, bounds.height * 0.5 - 12.0);
        for index in 0..Self::DOTS {
            let u = index as f32 / Self::DOTS as f32;
            let angle = u * std::f32::consts::TAU + self.phase;
            let point = Point::new(
                centre.x + Self::RADIUS * 0.92 * (3.0 * angle).sin(),
                centre.y + Self::RADIUS * 0.92 * (2.0 * angle + std::f32::consts::FRAC_PI_2).sin(),
            );
            // The head is bright and the tail fades, which is what makes a ring
            // of dots read as going somewhere rather than as a pattern.
            let mut colour = self.ink;
            colour.a = 0.12 + 0.88 * u;
            let radius = 1.4 + 2.0 * u;
            frame.fill(
                &Path::circle(point, radius),
                colour,
            );
        }
        vec![frame.into_geometry()]
    }
}
