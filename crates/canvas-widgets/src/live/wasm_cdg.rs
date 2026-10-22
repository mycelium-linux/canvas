//! WASM-based CDG renderer using kroak's decoder.
//! Integrates with kroak-pipeline's CDG command parser.
//!
//! Allow: the packet parser (`init_from_frame`, presets, tile blitting) is the
//! API surface the kroak decode phase will call once a CDG source is loadable;
//! it is staged, not abandoned.
#![allow(dead_code)]

use crate::live::{CacheState, Key, ThemePalette, color};
use iced::mouse;
use iced::widget::canvas::{self, Canvas, Geometry, Path};
use iced::{Element, Length, Point, Rectangle, Renderer, Theme};

/// CDG framebuffer dimensions
const CDG_WIDTH: usize = 300;
const CDG_HEIGHT: usize = 216;
const TILE_WIDTH: usize = 6;
const TILE_HEIGHT: usize = 12;

/// 16-color palette
type Palette = [[u8; 3]; 16];

/// CDG renderer state for iced canvas
pub struct CdgRenderer<'a> {
    pixels: Vec<Vec<u8>>,
    clut: Palette,
    border_color: u8,
    palette: ThemePalette,
    title: &'a str,
}

impl<'a> CdgRenderer<'a> {
    pub fn new(title: &'a str, palette: ThemePalette) -> Self {
        Self {
            pixels: vec![vec![1; CDG_WIDTH]; CDG_HEIGHT],
            clut: [[255; 3]; 16],
            border_color: 0,
            palette,
            title,
        }
    }

    /// Initialize CDG from frame data.
    ///
    /// Walks the raw CDG packet stream and applies CLUT (packet 0) and 6x12
    /// tile-set (packet 3) packets to the framebuffer. Other packet types
    /// (title, 2x2/8x16 tile sets) are currently ignored.
    pub fn init_from_frame(&mut self, frame_data: &[u8]) {
        let mut pos = 0;
        while pos < frame_data.len() {
            let header = frame_data[pos];
            pos += 1;
            let packet_type = (header >> 2) & 0x3;

            match packet_type {
                0 => {
                    if pos + 32 > frame_data.len() {
                        break;
                    }
                    let clut = &frame_data[pos..pos + 32];
                    pos += 32;
                    let low: [u8; 16] = clut[..16].try_into().unwrap();
                    self.load_palette(&low, false);
                    let high: [u8; 16] = clut[16..].try_into().unwrap();
                    self.load_palette(&high, true);
                }
                3 => {
                    if pos + 864 > frame_data.len() {
                        break;
                    }
                    let tiles = &frame_data[pos..pos + 864];
                    pos += 864;
                    for i in 0..72 {
                        let col = (i % 6) as u8;
                        let row = (i / 6) as u8;
                        let base = i * 12;
                        let pixels: [u8; 12] = tiles[base..base + 12].try_into().unwrap();
                        self.draw_tile_block(0, 1, row, col, &pixels);
                    }
                }
                _ => {}
            }
        }
    }

    /// Apply border preset
    fn border_preset(&mut self, color_idx: u8) {
        self.border_color = color_idx;
        for y in 0..CDG_HEIGHT {
            for x in 0..CDG_WIDTH {
                if !(6..CDG_WIDTH - 6).contains(&x) || !(12..CDG_HEIGHT - 12).contains(&y) {
                    self.pixels[y][x] = color_idx;
                }
            }
        }
    }

    /// Apply memory preset (fill screen)
    fn memory_preset(&mut self, color_idx: u8) {
        for y in 0..CDG_HEIGHT {
            for x in 0..CDG_WIDTH {
                self.pixels[y][x] = color_idx;
            }
        }
    }

    /// Draw tile block (6x12 tile)
    fn draw_tile_block(&mut self, color_1: u8, color_2: u8, row: u8, col: u8, pixels: &[u8; 12]) {
        let x = (col as usize) * TILE_WIDTH;
        let y = (row as usize) * TILE_HEIGHT;

        for (i, &pixel_data) in pixels.iter().enumerate() {
            let row_offset = y + i;

            for j in 0..6 {
                let col_offset = x + j;
                let color_idx = if pixel_data & (1 << (5 - j)) != 0 {
                    color_2
                } else {
                    color_1
                };

                if col_offset < CDG_WIDTH && row_offset < CDG_HEIGHT {
                    self.pixels[row_offset][col_offset] = color_idx;
                }
            }
        }
    }

    /// Load color palette from CDG packet
    fn load_palette(&mut self, payload: &[u8; 16], high: bool) {
        for i in 0..8 {
            let color_data =
                ((payload[i * 2] as u16 & 0x3F) << 6) | (payload[i * 2 + 1] as u16 & 0x3F);

            let r = ((color_data >> 8) & 0xF) * 17;
            let g = ((color_data >> 4) & 0xF) * 17;
            let b = (color_data & 0xF) * 17;

            let idx = if high { i + 8 } else { i };
            self.clut[idx] = [r as u8, g as u8, b as u8];
        }
    }

    /// Convert pixel data to an owned iced Element
    pub fn into_element<Message: 'a>(self) -> Element<'a, Message> {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl<Message> canvas::Program<Message> for CdgRenderer<'_> {
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

        let key = self
            .pixels
            .iter()
            .fold(Key::new().add_str(self.title), |key, row| {
                row.iter().fold(key, |k, &p| k.add_u64(p as u64))
            });

        state.draw_cached(renderer, bounds.size(), key.build(), |frame| {
            let scale_x = bounds.width / CDG_WIDTH as f32;
            let scale_y = bounds.height / CDG_HEIGHT as f32;
            let scale = scale_x.min(scale_y);

            let margin_x = (bounds.width - CDG_WIDTH as f32 * scale) * 0.5;
            let margin_y = (bounds.height - CDG_HEIGHT as f32 * scale) * 0.5;

            let transform = |x: usize, y: usize| {
                Point::new(margin_x + x as f32 * scale, margin_y + y as f32 * scale)
            };

            // Draw border first
            if self.border_color != 0 {
                let border_color = color(self.clut[self.border_color as usize]);
                for y in 0..CDG_HEIGHT {
                    for x in 0..CDG_WIDTH {
                        if self.pixels[y][x] == self.border_color {
                            let pos = transform(x, y);
                            frame.fill(&Path::circle(pos, 1.0), border_color);
                        }
                    }
                }
            }

            // Draw content
            for y in 0..CDG_HEIGHT {
                for x in 0..CDG_WIDTH {
                    let color_idx = self.pixels[y][x];
                    if color_idx != self.border_color && color_idx != 1 {
                        let color = color(self.clut[color_idx as usize]);
                        let pos = transform(x, y);
                        frame.fill(&Path::circle(pos, 1.0), color);
                    }
                }
            }

            // Draw title overlay in the theme's text colour, so a stale CDG
            // panel drains exactly like the rows above it.
            frame.fill_text(canvas::Text {
                content: self.title.to_string(),
                position: Point::new(4.0, 14.0),
                color: color(self.palette.text),
                size: iced::Pixels(12.0),
                ..Default::default()
            });
        })
    }
}
