//! Retained GPU waterfall for the spectrum instrument.
//!
//! Canvas receives bounded normalized rows. This renderer owns only their
//! presentation: one `R32Float` texture per widget, updated one row at a time.

use iced::widget::shader::{self, Shader};
use iced::{Element, Length, Rectangle};
use iced_wgpu::wgpu;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

const UNIFORM_BYTES: u64 = fpl_gfx::retained_waterfall::UNIFORM_BYTES;

#[derive(Debug)]
struct State {
    ticket: u64,
}

#[derive(Debug, Clone)]
struct Program {
    rows: Vec<Vec<f32>>,
    history: usize,
    stamp: u64,
}

#[derive(Debug, Clone)]
struct Primitive {
    ticket: u64,
    rows: Vec<Vec<f32>>,
    history: u32,
    stamp: u64,
}

struct TextureState {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    width: u32,
    history: u32,
    head: u32,
    filled: u32,
    stamp: u64,
    generation: u64,
}

struct Pipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    textures: BTreeMap<u64, TextureState>,
    generation: u64,
}

pub(super) fn view<'a, Message: 'a>(
    rows: Vec<Vec<f32>>,
    history: usize,
    stamp: u64,
) -> Element<'a, Message> {
    Shader::new(Program {
        rows,
        history,
        stamp,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

impl<Message> shader::Program<Message> for Program {
    type State = State;
    type Primitive = Primitive;

    fn draw(
        &self,
        state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        Primitive {
            ticket: state.ticket,
            rows: self.rows.clone(),
            history: self.history as u32,
            stamp: self.stamp,
        }
    }
}

impl Default for Primitive {
    fn default() -> Self {
        Self {
            ticket: 0,
            rows: Vec::new(),
            history: 8,
            stamp: 0,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            ticket: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let fpl_gfx::retained_waterfall::Pipeline {
            render_pipeline,
            bind_group_layout,
        } = fpl_gfx::retained_waterfall::Pipeline::new(device, format);
        Self {
            render_pipeline,
            bind_group_layout,
            textures: BTreeMap::new(),
            generation: 0,
        }
    }

    fn trim(&mut self) {
        self.generation = self.generation.saturating_add(1);
        let oldest = self.generation.saturating_sub(2);
        self.textures.retain(|_, value| value.generation >= oldest);
    }
}

impl Pipeline {
    fn texture_state(&self, device: &wgpu::Device, width: u32, history: u32) -> TextureState {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Canvas spectral waterfall ring"),
            size: wgpu::Extent3d {
                width,
                height: history,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Canvas spectral waterfall uniforms"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Canvas spectral waterfall bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        TextureState {
            texture,
            bind_group,
            uniform,
            width,
            history,
            head: 0,
            filled: 0,
            stamp: 0,
            generation: self.generation,
        }
    }
}

fn row_bytes(row: &[f32]) -> &[u8] {
    bytemuck::cast_slice(row)
}

fn write_row(queue: &wgpu::Queue, texture: &wgpu::Texture, y: u32, row: &[f32]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d { x: 0, y, z: 0 },
            aspect: wgpu::TextureAspect::All,
        },
        row_bytes(row),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(row.len() as u32 * 4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: row.len() as u32,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
}

fn advance_ring(head: u32, filled: u32, history: u32) -> (u32, u32) {
    ((head + history - 1) % history, (filled + 1).min(history))
}

fn uniform_words(bounds: Rectangle, width: u32, history: u32, head: u32, filled: u32) -> [u32; 8] {
    fpl_gfx::retained_waterfall::Uniforms {
        viewport: [bounds.width, bounds.height],
        texture_size: [width, history],
        head,
        filled,
        waterfall_top: 0.39,
        padding: 0.,
    }
    .words()
}

#[cfg(test)]
fn physical_row(head: u32, logical_row: u32, history: u32) -> u32 {
    (head + logical_row) % history
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &shader::Viewport,
    ) {
        let Some(width) = self.rows.last().map(|row| row.len() as u32) else {
            return;
        };
        let recreate = pipeline
            .textures
            .get(&self.ticket)
            .is_none_or(|state| state.width != width || state.history != self.history);
        if recreate {
            let mut state = pipeline.texture_state(device, width, self.history);
            let retained = self.rows.len().min(self.history as usize);
            for (physical, row) in self.rows.iter().rev().take(retained).enumerate() {
                write_row(queue, &state.texture, physical as u32, row);
            }
            state.filled = retained as u32;
            state.stamp = self.stamp;
            pipeline.textures.insert(self.ticket, state);
        }
        let state = pipeline
            .textures
            .get_mut(&self.ticket)
            .expect("waterfall texture inserted");
        state.generation = pipeline.generation;
        if state.stamp != self.stamp {
            (state.head, state.filled) = advance_ring(state.head, state.filled, state.history);
            write_row(
                queue,
                &state.texture,
                state.head,
                self.rows.last().expect("row exists"),
            );
            state.stamp = self.stamp;
        }
        // WGSL mixes f32 and u32 fields. Packing this as `[f32; 8]` made the
        // texture dimensions arrive as the IEEE-754 bit pattern for `128.0`
        // instead of integer 128, so every textureLoad addressed outside the
        // allocation and the waterfall was blank while the Canvas overlay drew.
        let uniforms = uniform_words(
            *bounds,
            state.width,
            state.history,
            state.head,
            state.filled,
        );
        queue.write_buffer(&state.uniform, 0, bytemuck::cast_slice(&uniforms));
    }

    fn draw(&self, pipeline: &Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(state) = pipeline.textures.get(&self.ticket) else {
            return true;
        };
        render_pass.set_pipeline(&pipeline.render_pipeline);
        render_pass.set_bind_group(0, &state.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{advance_ring, physical_row, uniform_words};
    use iced::{Rectangle, Size};

    #[test]
    fn ring_moves_newest_to_the_top_and_saturates() {
        let history = 4;
        let (head, filled) = advance_ring(0, 2, history);
        assert_eq!(head, 3);
        assert_eq!(filled, 3);
        assert_eq!(physical_row(head, 0, history), 3);
        assert_eq!(physical_row(head, 1, history), 0);
        assert_eq!(advance_ring(head, history, history).1, history);
    }

    #[test]
    fn unfilled_logical_rows_are_detectable_without_texture_clears() {
        let filled = 2;
        assert!(
            (0..4)
                .map(|row| row < filled)
                .eq([true, true, false, false])
        );
    }

    #[test]
    fn mixed_wgsl_uniforms_keep_integer_texture_fields_as_integers() {
        let words = uniform_words(
            Rectangle::with_size(Size::new(1920.0, 1080.0)),
            128,
            360,
            7,
            42,
        );
        assert_eq!(f32::from_bits(words[0]), 1920.0);
        assert_eq!(f32::from_bits(words[1]), 1080.0);
        assert_eq!(&words[2..6], &[128, 360, 7, 42]);
        assert_eq!(f32::from_bits(words[6]), 0.39);
    }
}
