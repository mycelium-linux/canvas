//! Hyperspace shader renderer using iced wgpu.
//! Renders WGSL shaders from hyperspace repo.
//!
//! Allow: `update` is the animation-tick API the host will call once widget
//! subscriptions land; it is staged, not abandoned.
#![allow(dead_code)]

use crate::live::{CacheState, Key};
use iced::mouse;
use iced::widget::canvas::{self, Canvas, Geometry, Path};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Size, Theme};

/// Hyperspace shader renderer
pub struct HyperspaceRenderer<'a> {
    shader_id: &'a str,
    speed: f32,
    scale: f32,
    time: f32,
}

impl<'a> HyperspaceRenderer<'a> {
    pub fn new(shader_id: &'a str, speed: f32, scale: f32) -> Self {
        Self {
            shader_id,
            speed,
            scale,
            time: 0.0,
        }
    }

    /// Update animation time
    pub fn update(&mut self, delta: f32) {
        self.time = (self.time + delta * self.speed).abs();
    }

    /// Convert to an owned iced Element
    pub fn into_element<Message: 'a>(self) -> Element<'a, Message> {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl<Message> canvas::Program<Message> for HyperspaceRenderer<'_> {
    type State = CacheState;

    fn draw(
        &self,
        state: &CacheState,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        if bounds.width < 10.0 || bounds.height < 10.0 {
            return Vec::new();
        }

        let key = Key::new()
            .add_str(self.shader_id)
            .add_f32(self.speed)
            .add_f32(self.scale)
            .add_f32(self.time)
            .build();

        state.draw_cached(renderer, bounds.size(), key, |frame| {
            // Placeholder visualization - actual implementation uses wgpu WGSL
            match self.shader_id {
                "hyperspace_tunnel" => self.draw_tunnel(frame, bounds),
                "acid_worms" => self.draw_acid(frame, bounds),
                "neon_tunnel" => self.draw_neon_tunnel(frame, bounds),
                "plasma_vortex" => self.draw_plasma(frame, bounds),
                _ => self.draw_placeholder(frame, bounds),
            }
        })
    }
}

impl HyperspaceRenderer<'_> {
    fn draw_tunnel(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let w = bounds.width;
        let h = bounds.height;
        let cx = w * 0.5;
        let cy = h * 0.5;
        let t = self.time;

        // Draw tunnel rings
        let base_radius = w * 0.3 * self.scale;
        for i in 0..20 {
            let ring_t = t - i as f32 * 0.5;
            let radius = (base_radius * (1.0 + ring_t * 0.1)).clamp(10.0, w.min(h) * 0.4);
            let alpha = (1.0 - i as f32 * 0.05).max(0.1);

            let color = Color {
                r: 0.5 + 0.5 * (ring_t * 0.3).sin(),
                g: 0.2 + 0.3 * (ring_t * 0.2).sin(),
                b: 1.0,
                a: alpha * 0.8,
            };

            frame.stroke(
                &Path::circle(Point::new(cx, cy), radius),
                canvas::Stroke::default().with_width(2.0).with_color(color),
            );
        }
    }

    fn draw_acid(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let w = bounds.width;
        let h = bounds.height;
        let cy = h * 0.5;
        let t = self.time;

        // Acid wave effect
        for y in 0..10 {
            let wave_y = cy + (y as f32 - 5.0) * h * 0.08;
            let amplitude = w * 0.2 * self.scale;

            frame.stroke(
                &Path::new(|path| {
                    path.move_to(Point::new(0.0, wave_y));
                    for x in 0..=(w as usize) {
                        let wave = (x as f32 * 0.02 + t * 0.5).sin() * amplitude
                            + (x as f32 * 0.05 + t * 0.3).cos() * amplitude * 0.5;
                        path.line_to(Point::new(x as f32, wave_y + wave));
                    }
                }),
                canvas::Stroke::default().with_width(3.0).with_color(Color {
                    r: 1.0,
                    g: (t * 0.2).sin() * 0.5 + 0.5,
                    b: 0.5,
                    a: 0.7,
                }),
            );
        }
    }

    fn draw_neon_tunnel(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let w = bounds.width;
        let h = bounds.height;
        let cx = w * 0.5;
        let cy = h * 0.5;
        let t = self.time;

        // Neon grid tunnel
        let perspective = 200.0;
        let speed = t * 0.5;

        for i in 0..15 {
            let z = (i as f32 * 30.0 + speed).rem_euclid(450.0);
            let alpha = 1.0 - z / 450.0;
            let scale = perspective / (perspective + z);
            let size = w * 0.4 * scale;

            let color = Color {
                r: 1.0,
                g: (z * 0.01).sin() * 0.5 + 0.5,
                b: 1.0,
                a: alpha,
            };

            let square = (size * 0.7).max(2.0);
            frame.fill(
                &Path::rounded_rectangle(
                    Point::new(cx - square * 0.5, cy - square * 0.5),
                    Size::new(square, square),
                    (2.0 * scale).into(),
                ),
                color,
            );
        }
    }

    fn draw_plasma(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let w = bounds.width;
        let h = bounds.height;
        let t = self.time;

        // Simple plasma effect (low-res for performance)
        let steps = 30;
        for i in 0..steps {
            for j in 0..steps {
                let x = (i as f32 / steps as f32) * w;
                let y = (j as f32 / steps as f32) * h;

                let v = ((x * 0.01 + t * 0.3).sin()
                    + (y * 0.01 + t * 0.2).cos()
                    + ((x + y) * 0.02 + t * 0.1).sin())
                    / 3.0;

                let color = Color {
                    r: (v + 1.0) * 0.5,
                    g: ((v + 0.33) + 1.0) * 0.5,
                    b: ((v + 0.66) + 1.0) * 0.5,
                    a: 0.8,
                };

                let cell_w = w / steps as f32;
                let cell_h = h / steps as f32;
                frame.fill(
                    &Path::rounded_rectangle(
                        Point::new(x, y),
                        Size::new(cell_w - 1.0, cell_h - 1.0),
                        1.0.into(),
                    ),
                    color,
                );
            }
        }
    }

    fn draw_placeholder(&self, frame: &mut canvas::Frame, bounds: Rectangle) {
        let w = bounds.width;
        let h = bounds.height;

        // Generic shader preview
        frame.fill(
            &Path::rectangle(Point::ORIGIN, Size::new(w, h)),
            Color::from_rgb8(32, 32, 48),
        );

        frame.fill_text(canvas::Text {
            content: self.shader_id.to_string(),
            position: Point::new(w * 0.5, h * 0.5),
            color: Color::WHITE,
            size: iced::Pixels(24.0),
            ..Default::default()
        });
    }
}
