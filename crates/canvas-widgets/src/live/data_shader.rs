//! A shader a Spore wrote.
//!
//! The whole point: a GPU component that does not need a native build. The WGSL
//! arrives as data, the host compiles it once, caches the pipeline by source
//! hash, and draws it inside the node's rectangle with the uniforms the producer
//! supplied.
//!
//! The contract is deliberately small, because it is the part every producer has
//! to get right: a producer writes **only the fragment stage**. The host supplies
//! the vertex stage, the uniform block and its binding, so the producer cannot
//! disagree with the host about them -- and a redeclaration is a compile error
//! rather than a silently different layout.
//!
//! ```wgsl
//! // Provided by the host, available to your shader:
//! //   struct Uniforms {
//! //     resolution: vec4<f32>,   // xy = px, z = time, w = scale factor
//! //     p0: vec4<f32>, p1: vec4<f32>, p2: vec4<f32>, p3: vec4<f32>, p4: vec4<f32>,
//! //   }
//! //   @group(0) @binding(0) var<uniform> u: Uniforms;
//!
//! @fragment
//! fn fs_main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
//!   return vec4<f32>(uv, u.resolution.z, 1.0);
//! }
//! ```

use iced::widget::shader::{self, Shader};
use iced::{Element, Length, Rectangle};
use iced_wgpu::wgpu;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Floats a producer may supply, packed after the header.
pub const PARAM_FLOATS: usize = 20;
/// Header (resolution.xy, time, scale) plus the parameter block.
pub const UNIFORM_FLOATS: usize = 4 + PARAM_FLOATS;

const VERTEX_STAGE: &str = r#"
struct Uniforms {
  resolution: vec4<f32>,
  p0: vec4<f32>,
  p1: vec4<f32>,
  p2: vec4<f32>,
  p3: vec4<f32>,
  p4: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VertexOut {
  @builtin(position) position: vec4<f32>,
  @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
  // One triangle covering the node: no vertex buffers, no geometry to upload.
  var positions = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(3.0, -1.0),
    vec2<f32>(-1.0, 3.0),
  );
  let p = positions[index];
  var out: VertexOut;
  out.position = vec4<f32>(p, 0.0, 1.0);
  // uv is 0..1 across the node, y down, so a producer can think in pixels.
  out.uv = vec2<f32>((p.x + 1.0) * 0.5, (p.y + 1.0) * 0.5);
  return out;
}
"#;

#[derive(Debug)]
struct State(u64);
impl Default for State {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

#[derive(Debug, Clone)]
struct Program {
    wgsl: String,
    uniforms: Vec<f32>,
}

#[derive(Debug, Clone)]
struct Primitive {
    /// Identifies this node's slot in the pipeline's cache.
    ticket: u64,
    wgsl: String,
    uniforms: Vec<f32>,
}

struct Entry {
    key: u64,
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    generation: u64,
    prepared_size: [u32; 2],
}

struct Pipeline {
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    entries: BTreeMap<u64, Entry>,
    generation: u64,
    failures: BTreeMap<u64, String>,
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spore shader uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("spore shader"),
            bind_group_layouts: &[&layout],
            push_constant_ranges: &[],
        });
        Self {
            format,
            layout,
            pipeline_layout,
            entries: BTreeMap::new(),
            generation: 0,
            failures: BTreeMap::new(),
        }
    }

    fn trim(&mut self) {
        // Called at the end of a frame. Entries touched during the frame carry the
        // current generation; anything older was not drawn and is dropped. The
        // increment has to come *after* the retain, or every frame would evict
        // the pipelines it just built and recompile them -- which is what a
        // 25 ms frame with a 26-iteration shader looked like.
        self.entries
            .retain(|_, entry| entry.generation == self.generation);
        self.generation = self.generation.wrapping_add(1);
    }
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &shader::Viewport,
    ) {
        let key = hash(&self.wgsl);
        let size = [
            (bounds.width * viewport.scale_factor()).round().max(1.0) as u32,
            (bounds.height * viewport.scale_factor()).round().max(1.0) as u32,
        ];
        let mut floats = [0f32; UNIFORM_FLOATS];
        floats[0] = size[0] as f32;
        floats[1] = size[1] as f32;
        // z is the producer's clock if it sends one; w is the scale factor.
        floats[2] = self.uniforms.first().copied().unwrap_or(0.0);
        floats[3] = viewport.scale_factor();
        for (index, value) in self.uniforms.iter().skip(1).take(PARAM_FLOATS).enumerate() {
            floats[4 + index] = *value;
        }
        let bytes = bytemuck::cast_slice(&floats).to_vec();

        match pipeline.entries.get_mut(&self.ticket) {
            Some(entry) if entry.key == key => {
                entry.generation = pipeline.generation;
                if entry.prepared_size != size {
                    // The uniform holds the extent, so a resize is a write, not a
                    // recompile.
                    queue.write_buffer(&entry.uniforms, 0, &bytes);
                    entry.prepared_size = size;
                } else {
                    queue.write_buffer(&entry.uniforms, 0, &bytes);
                }
                return;
            }
            _ => {}
        }

        if pipeline.failures.contains_key(&key) {
            return;
        }
        let source = format!("{VERTEX_STAGE}\n{}", self.wgsl);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spore shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let descriptor = wgpu::RenderPipelineDescriptor {
            label: Some("spore shader"),
            layout: Some(&pipeline.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                // The producer's entry point is fixed so a fragment shader is
                // the only thing it has to get right.
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: pipeline.format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        };
        // wgpu reports a bad shader on the device's error scope; a scene with a
        // broken shader must not take the frame down with it, so a failure is
        // recorded and the node simply draws nothing.
        let created = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            device.create_render_pipeline(&descriptor)
        }));
        let Ok(render_pipeline) = created else {
            pipeline.failures.insert(key, "pipeline creation panicked".into());
            return;
        };

        use wgpu::util::DeviceExt;
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("spore shader uniforms"),
            contents: &bytes,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("spore shader uniforms"),
            layout: &pipeline.layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        pipeline.entries.insert(
            self.ticket,
            Entry {
                key,
                pipeline: render_pipeline,
                uniforms,
                bind,
                generation: pipeline.generation,
                prepared_size: size,
            },
        );
    }

    fn draw(&self, pipeline: &Pipeline, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(entry) = pipeline.entries.get(&self.ticket) else {
            return false;
        };
        pass.set_pipeline(&entry.pipeline);
        pass.set_bind_group(0, &entry.bind, &[]);
        pass.draw(0..3, 0..1);
        true
    }
}

impl<Message> shader::Program<Message> for Program {
    type State = State;
    type Primitive = Primitive;

    fn draw(&self, state: &State, _cursor: iced::mouse::Cursor, _bounds: Rectangle) -> Primitive {
        Primitive {
            ticket: state.0,
            wgsl: self.wgsl.clone(),
            uniforms: self.uniforms.clone(),
        }
    }
}

/// A GPU component: WGSL plus the floats it reads. Fills its parent, so the
/// caller decides the rectangle.
pub fn view<'a, Message: 'a>(wgsl: String, uniforms: Vec<f32>) -> Element<'a, Message> {
    Shader::new(Program { wgsl, uniforms })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn hash(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vertex_stage_covers_the_node_and_hands_over_uv() {
        assert!(VERTEX_STAGE.contains("vs_main"));
        assert!(VERTEX_STAGE.contains("uv"));
        // One triangle, no vertex buffers: nothing to upload per node.
        assert!(VERTEX_STAGE.contains("array<vec2<f32>, 3>"));
    }

    #[test]
    fn the_uniform_block_matches_the_header_and_parameter_count() {
        // 4 header floats + 5 vec4 parameters, and the WGSL must declare
        // exactly what the host will write.
        assert_eq!(UNIFORM_FLOATS, 24);
        for field in ["resolution", "p0", "p1", "p2", "p3", "p4"] {
            assert!(VERTEX_STAGE.contains(field), "missing {field}");
        }
    }

    #[test]
    fn a_shader_is_cached_by_its_source() {
        assert_eq!(hash("a"), hash("a"));
        assert_ne!(hash("a"), hash("b"));
    }
}
