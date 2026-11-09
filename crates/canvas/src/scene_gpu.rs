//! Scene surfaces drawn by Soil's GPU renderer inside iced's render pass.
//!
//! A `Scene` is renderer-neutral geometry. The widget path turns it into one
//! iced container per node, which covers rects and text and silently drops
//! every other node kind. This path hands the same document to
//! [`soil_scene_gpu::Renderer`] — the renderer the native `soil-viz` window
//! uses — and draws its result into iced's pass, so a scene keeps the native
//! antialiasing and the full node vocabulary (polylines, areas, bands,
//! heatmaps, candles, points) without a CPU readback.
//!
//! The renderer draws into an owned texture and the pass samples that texture,
//! because its pipelines declare a depth attachment that iced's pass does not
//! carry. That is the only extra step over a native window: no readback and no
//! upload, and the frame never leaves the GPU.

use std::sync::Arc;

use iced::Length;
use iced::Rectangle;
use iced::widget::shader::{self, Shader};
use soil_scene_gpu::{Renderer, Theme};

/// The renderer's own target. It is sampled by the blit, so it must be
/// filterable and the same format the blit pass writes into.
const SAMPLES: u32 = 1;

/// A scene presented as a GPU surface.
///
/// `generation` invalidates the retained frame: the application bumps it when
/// the document it hands over changes, and a stale generation re-uses the
/// texture the previous frame already drew.
#[derive(Debug, Clone)]
pub struct SceneProgram {
    scene: Arc<soil_scene::Scene>,
    theme: Theme,
    generation: u64,
}

impl SceneProgram {
    pub fn new(scene: Arc<soil_scene::Scene>, theme: Theme, generation: u64) -> Self {
        Self {
            scene,
            theme,
            generation,
        }
    }
}

/// Whether scene surfaces take the GPU path.
///
/// On by default. The older widget renderer draws one iced container per node
/// and only understands rects and text, so every other node kind a producer
/// emits is silently dropped; `CANVAS_SCENE_GPU=0` keeps it available for a
/// host whose iced build has no wgpu backend, where a shader widget draws
/// nothing at all.
pub fn enabled() -> bool {
    gpu_enabled(std::env::var("CANVAS_SCENE_GPU").ok().as_deref())
}

fn gpu_enabled(value: Option<&str>) -> bool {
    value.is_none_or(|value| value.trim() != "0")
}

/// The renderer's colour table, taken from the presented Coherence theme.
///
/// A scene names semantic inks, never literal colours; the host resolves them
/// through this table. Taking it from the presented theme rather than deriving
/// a second palette is what keeps a GPU surface the same colour as the widget
/// beside it.
pub fn theme_from(visual: &canvas_core::VisualTheme) -> Theme {
    let palette = visual.palette;
    Theme {
        background: palette.background,
        surface: palette.surface,
        text: palette.text,
        muted: palette.text_muted,
        primary: palette.primary,
        success: palette.success,
        warning: palette.warning,
        danger: palette.danger,
        info: palette.info,
        body: visual.metrics.body,
    }
}

/// A scene element that fills the space its layout gives it.
pub fn view<'a, Message: 'a>(
    scene: Arc<soil_scene::Scene>,
    theme: Theme,
    generation: u64,
) -> iced::Element<'a, Message> {
    Shader::new(SceneProgram::new(scene, theme, generation))
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[derive(Debug, Clone)]
pub struct ScenePrimitive {
    scene: Arc<soil_scene::Scene>,
    theme: Theme,
    generation: u64,
}

impl<Message> shader::Program<Message> for SceneProgram {
    type State = ();
    type Primitive = ScenePrimitive;

    fn draw(
        &self,
        _state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        ScenePrimitive {
            scene: Arc::clone(&self.scene),
            theme: self.theme,
            generation: self.generation,
        }
    }
}

pub struct ScenePipeline {
    renderer: Renderer,
    format: wgpu::TextureFormat,
    /// The texture the scene is drawn into and the blit samples.
    target: Option<wgpu::Texture>,
    target_size: (u32, u32),
    blit: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_group: Option<wgpu::BindGroup>,
    /// The document already in `target`.
    drawn: Option<u64>,
    drawn_size: (u32, u32),
}

const BLIT_SHADER: &str = r#"
struct VertexOut {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
  var corners = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(3.0, -1.0),
    vec2<f32>(-1.0, 3.0),
  );
  let corner = corners[index];
  var out: VertexOut;
  out.position = vec4<f32>(corner, 0.0, 1.0);
  // Texture rows run top-down, clip space runs bottom-up.
  out.uv = vec2<f32>(corner.x * 0.5 + 0.5, 0.5 - corner.y * 0.5);
  return out;
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
  return textureSample(source, source_sampler, in.uv);
}
"#;

impl ScenePipeline {
    fn bind_group(&mut self, device: &wgpu::Device) {
        let Some(target) = &self.target else {
            return;
        };
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let layout = self.blit.get_bind_group_layout(0);
        self.bind_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("canvas scene blit"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        }));
    }

    /// Size the scene target to the widget, reallocating only on a real change.
    fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        if self.target.is_some() && self.target_size == size {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("canvas scene target"),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: self.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.target = Some(texture);
        self.target_size = size;
        self.bind_group(device);
    }
}

impl shader::Pipeline for ScenePipeline {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut renderer =
            Renderer::from_device_sampled(device.clone(), queue.clone(), format, SAMPLES)
                .expect("scene renderer must build on the host device");
        renderer.set_font(soil_scene_gpu::load_mono_font());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("canvas scene blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("canvas scene blit"),
            bind_group_layouts: &[&device.create_bind_group_layout(
                &wgpu::BindGroupLayoutDescriptor {
                    label: Some("canvas scene blit"),
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                },
            )],
            push_constant_ranges: &[],
        });
        let blit = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("canvas scene blit"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("canvas scene blit"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            renderer,
            format,
            target: None,
            target_size: (0, 0),
            blit,
            sampler,
            bind_group: None,
            drawn: None,
            drawn_size: (0, 0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scene_role_comes_from_the_presented_theme() {
        let visual = canvas_core::ThemeSelection::Coherence {
            mode: canvas_core::ThemeMode::Dark,
            accent: canvas_core::Accent::Brand,
        }
        .visual_theme();
        let theme = theme_from(&visual);
        assert_eq!(theme.background, visual.palette.background);
        assert_eq!(theme.surface, visual.palette.surface);
        assert_eq!(theme.text, visual.palette.text);
        assert_eq!(theme.muted, visual.palette.text_muted);
        assert_eq!(theme.primary, visual.palette.primary);
        assert_eq!(theme.success, visual.palette.success);
        assert_eq!(theme.warning, visual.palette.warning);
        assert_eq!(theme.danger, visual.palette.danger);
        assert_eq!(theme.info, visual.palette.info);
        assert_eq!(theme.body, visual.metrics.body);
    }

    #[test]
    fn the_gpu_path_is_on_unless_explicitly_disabled() {
        assert!(gpu_enabled(None));
        assert!(gpu_enabled(Some("1")));
        assert!(gpu_enabled(Some("")));
        assert!(!gpu_enabled(Some("0")));
        assert!(!gpu_enabled(Some(" 0 ")));
    }
}

impl shader::Primitive for ScenePrimitive {
    type Pipeline = ScenePipeline;
    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        _queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let scale = viewport.scale_factor();
        let size = (
            (bounds.width * scale).round().max(1.0) as u32,
            (bounds.height * scale).round().max(1.0) as u32,
        );
        pipeline.resize(device, size);
        // A retained scene only redraws when the document or its extent moves;
        // iced asks for a prepare on every frame it repaints.
        if pipeline.drawn == Some(self.generation) && pipeline.drawn_size == size {
            return;
        }
        let Some(target) = &pipeline.target else {
            return;
        };
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        if pipeline
            .renderer
            .draw_to(&self.scene, size.0, size.1, self.theme, &view)
            .is_ok()
        {
            pipeline.drawn = Some(self.generation);
            pipeline.drawn_size = size;
        }
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(bind_group) = &pipeline.bind_group else {
            return false;
        };
        render_pass.set_pipeline(&pipeline.blit);
        render_pass.set_bind_group(0, bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}
