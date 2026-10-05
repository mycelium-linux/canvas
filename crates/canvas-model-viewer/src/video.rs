//! Retained WGPU video surface for the native model viewer.
//!
//! The portable producer contract remains an atomically replaced RGBA frame.
//! Unlike iced's immutable image handles, this widget allocates one GPU texture
//! and updates it in place. Platform-native imports (DMA-BUF on Linux, IOSurface
//! on macOS, shared handles on Windows) can implement the same source boundary
//! later without changing Canvas documents or layout code.

use iced::widget::shader::{self, Shader};
use iced::{Element, Length, Rectangle};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use canvas_core::external_surface::SurfaceTransport;

const REDRAW_INTERVAL: std::time::Duration = std::time::Duration::from_millis(42);

/// Size of the shader's `Uniforms`: two vec2s, staleness, and scalar padding.
///
/// The WGSL struct, this buffer and the bind group layout's `min_binding_size`
/// must agree. Growing the struct without the layout aborts the process on
/// pipeline creation rather than failing soft.
const UNIFORM_BYTES: u64 = 32;

/// A producer at 24 fps that misses this much is not merely between frames.
const FRESH_SECONDS: f32 = 3.0;
/// Fully drained by here: no feed this quiet is still reporting the present.
const STALE_SECONDS: f32 = 20.0;

/// How dead a feed looks, from its silence.
///
/// Unreadable metadata counts as fully stale rather than fully fresh: a
/// surface that cannot say when it last moved is not evidence that it just
/// did, and this is what a security board is read from.
fn staleness_of(age: std::time::Duration) -> f32 {
    let seconds = age.as_secs_f32();
    ((seconds - FRESH_SECONDS) / (STALE_SECONDS - FRESH_SECONDS)).clamp(0.0, 1.0)
}

const SHADER: &str = r#"
struct Uniforms {
    viewport: vec2<f32>,
    source: vec2<f32>,
    // 0 while the producer is writing frames, rising to 1 once it has stopped
    // for long enough that nothing on screen should be read as current.
    staleness: f32,
    // Scalars, not a vec3: a vec3 here aligns the struct to 16 and grows it to
    // 48 bytes against a 32-byte binding, which wgpu rejects outright.
    _pad0: f32,
    premultiplied: f32,
    native_alpha: f32,
};

@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;
@group(0) @binding(2) var<uniform> u: Uniforms;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var uvs = array<vec2<f32>, 3>(
        vec2<f32>(0.0, 1.0),
        vec2<f32>(2.0, 1.0),
        vec2<f32>(0.0, -1.0),
    );
    var out: VertexOut;
    out.position = vec4<f32>(positions[index], 0.0, 1.0);
    out.uv = uvs[index];
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let viewport_aspect = u.viewport.x / max(u.viewport.y, 1.0);
    let source_aspect = u.source.x / max(u.source.y, 1.0);
    var uv = in.uv;
    var content_size = u.viewport;
    if viewport_aspect > source_aspect {
        let width = source_aspect / viewport_aspect;
        if abs(uv.x - 0.5) > width * 0.5 { return vec4<f32>(0.0); }
        uv.x = (uv.x - 0.5) / width + 0.5;
        content_size.x = u.viewport.y * source_aspect;
    } else {
        let height = viewport_aspect / source_aspect;
        if abs(uv.y - 0.5) > height * 0.5 { return vec4<f32>(0.0); }
        uv.y = (uv.y - 0.5) / height + 0.5;
        content_size.y = u.viewport.x / source_aspect;
    }
    let frame_color = textureSample(frame, frame_sampler, uv);
    // H.264 has no alpha channel. A macOS window's transparent outer corners
    // therefore arrive as black pixels unless the compositor restores the
    // window silhouette. Clip at the fitted content boundary (not the widget
    // boundary) with a resolution-independent, softly antialiased radius.
    // The same restrained treatment makes camera/video surfaces sit cleanly
    // in Canvas without baking chrome into any producer.
    let radius = clamp(min(content_size.x, content_size.y) * 0.06, 8.0, 30.0);
    let point = uv * content_size;
    let q = abs(point - content_size * 0.5) - (content_size * 0.5 - vec2<f32>(radius));
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
    let mask = select(1.0 - smoothstep(-1.25, 1.25, distance), 1.0, u.native_alpha > 0.5);
    // A camera that has stopped keeps its last frame on the texture forever, so
    // a dead feed has to be marked. NOT by draining colour: these cameras
    // switch to infrared at night and produce legitimately black-and-white
    // video, so a greyscale picture is normal exactly when it matters most.
    // Diagonal banding cannot come out of a camera, so it cannot be mistaken
    // for the scene. Computed in framebuffer space to keep the bands an even
    // width whatever size the surface is.
    let band = step(0.5, fract((in.position.x + in.position.y) / 24.0));
    let rgb = select(frame_color.rgb, frame_color.rgb / max(frame_color.a, 0.00001), u.premultiplied > 0.5);
    let dimmed = rgb * (1.0 - 0.45 * u.staleness);
    let barred = dimmed * (1.0 - 0.35 * u.staleness * band);
    return vec4<f32>(barred, frame_color.a * mask);
}
"#;

/// What the surface saw, in surface coordinates. The widget reports gestures
/// and refuses to interpret them: where the camera ends up is decided by the
/// orbit controller, which is testable without a window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrbitEvent {
    Press(iced::Point),
    Drag(iced::Point, iced::Size),
    Release,
    Zoom(f32),
}

/// Emits an application message for a gesture. `Arc` rather than a plain
/// closure because iced rebuilds the widget tree constantly and the hook is
/// cloned with it.
type OrbitHook<Message> = std::sync::Arc<dyn Fn(OrbitEvent) -> Message + Send + Sync>;

struct Program<Message> {
    source: FrameSource,
    /// `None` for a surface that is only being watched, such as a camera.
    on_orbit: Option<OrbitHook<Message>>,
}

impl<Message> std::fmt::Debug for Program<Message> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Program")
            .field("source", &self.source)
            .field("orbitable", &self.on_orbit.is_some())
            .finish()
    }
}

impl<Message> Clone for Program<Message> {
    fn clone(&self) -> Self {
        Self {
            source: self.source.clone(),
            on_orbit: self.on_orbit.clone(),
        }
    }
}

#[derive(Debug)]
struct State {
    ticket: u64,
    /// Whether the press that began a drag landed on this surface.
    dragging: bool,
}

impl Default for State {
    fn default() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            ticket: NEXT.fetch_add(1, Ordering::Relaxed),
            dragging: false,
        }
    }
}

#[derive(Debug, Clone)]
struct Primitive {
    ticket: u64,
    source: FrameSource,
}

#[derive(Debug, Clone)]
enum FrameSource {
    File {
        path: PathBuf,
        width: u32,
        height: u32,
    },
    External(crate::external_surface::ExternalSurfaceFrame),
}

struct TextureState {
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    width: u32,
    height: u32,
    stamp: Option<(SystemTime, u64)>,
    external_stamp: Option<u64>,
    generation: u64,
    /// Last (expected, actual) byte count reported, so a persistent mismatch
    /// is one line rather than one per frame.
    reported_mismatch: Option<(usize, usize)>,
    source_generation: u64,
    native_hold: Option<DeferredNativeHold>,
}

struct DeferredNativeHold {
    surface: Option<std::sync::Arc<crate::native_surface::NativeSurface>>,
    queue: wgpu::Queue,
}

impl Drop for DeferredNativeHold {
    fn drop(&mut self) {
        if let Some(surface) = self.surface.take() {
            // Replacement/trim occurs in prepare, before this frame's draw.
            // Earlier uses of this texture are in already-submitted work.
            self.queue.on_submitted_work_done(move || drop(surface));
        }
    }
}

struct Pipeline {
    render_pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: BTreeMap<u64, TextureState>,
    generation: u64,
}

impl Pipeline {
    fn texture_state(
        &self,
        device: &wgpu::Device,
        width: u32,
        height: u32,
        source_generation: u64,
        format: wgpu::TextureFormat,
        texture: Option<wgpu::Texture>,
    ) -> TextureState {
        let texture = texture.unwrap_or_else(|| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Canvas retained video frame"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Canvas retained video uniforms"),
            size: UNIFORM_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Canvas retained video bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        TextureState {
            texture,
            bind_group,
            uniform,
            width,
            height,
            stamp: None,
            external_stamp: None,
            generation: self.generation,
            reported_mismatch: None,
            source_generation,
            native_hold: None,
        }
    }
}

impl<Message> shader::Program<Message> for Program<Message> {
    type State = State;
    type Primitive = Primitive;

    fn update(
        &self,
        _state: &mut Self::State,
        event: &iced::Event,
        _bounds: Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Option<iced::widget::Action<Message>> {
        // A file changing does not itself wake iced. Let the presentation
        // primitive own its redraw cadence, just like the animated background,
        // without mutating the workspace revision on every video frame.
        if let iced::Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            let interval = match &self.source {
                FrameSource::External(frame) if frame.native_surface.is_some() => {
                    std::time::Duration::from_millis(33)
                }
                _ => REDRAW_INTERVAL,
            };
            return Some(iced::widget::Action::request_redraw_at(
                *now + interval,
            ));
        }
        let Some(hook) = self.on_orbit.as_ref() else {
            return None;
        };
        let surface = iced::Size::new(_bounds.width, _bounds.height);
        // Only gestures that started on this surface are followed. A press
        // elsewhere that happens to drag across a model must not seize its
        // camera, and the button release that ends such a drag arrives here
        // too -- releasing a grab this surface never took would be harmless
        // but releasing one it *did* take, because another surface saw the
        // press, would strand the camera mid-turn.
        match event {
            iced::Event::Mouse(iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left)) => {
                let position = _cursor.position_in(_bounds)?;
                _state.dragging = true;
                Some(iced::widget::Action::publish(hook(OrbitEvent::Press(
                    position,
                ))))
            }
            iced::Event::Mouse(iced::mouse::Event::CursorMoved { .. }) if _state.dragging => {
                // Deliberately not `position_in`: a drag that leaves the
                // surface keeps turning the model until the button comes up,
                // which is what makes a long sweep possible on a small tile.
                let position = _cursor.position()?;
                let local = iced::Point::new(position.x - _bounds.x, position.y - _bounds.y);
                Some(iced::widget::Action::publish(hook(OrbitEvent::Drag(
                    local, surface,
                ))))
            }
            iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left))
                if _state.dragging =>
            {
                _state.dragging = false;
                Some(iced::widget::Action::publish(hook(OrbitEvent::Release)))
            }
            iced::Event::Mouse(iced::mouse::Event::WheelScrolled { delta }) => {
                _cursor.position_in(_bounds)?;
                let steps = match delta {
                    iced::mouse::ScrollDelta::Lines { y, .. } => *y,
                    // Pixel deltas are far finer grained than a notched wheel;
                    // scaled so a trackpad swipe is a comparable amount of zoom.
                    iced::mouse::ScrollDelta::Pixels { y, .. } => *y / 50.0,
                };
                (steps != 0.0).then(|| iced::widget::Action::publish(hook(OrbitEvent::Zoom(steps))))
            }
            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        _cursor: iced::mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        Primitive {
            ticket: state.ticket,
            source: self.source.clone(),
        }
    }
}

impl shader::Pipeline for Pipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Canvas retained video layout"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        // Must match the uniform struct and the buffer exactly; wgpu
                        // refuses the pipeline outright if the layout promises less.
                        min_binding_size: wgpu::BufferSize::new(UNIFORM_BYTES),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Canvas retained video pipeline layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Canvas retained video shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let render_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Canvas retained video pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Canvas retained video linear sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            min_filter: wgpu::FilterMode::Linear,
            mag_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            render_pipeline,
            bind_group_layout,
            sampler,
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

impl Primitive {
    /// The size to read frames at: what the decoder published, or what the
    /// document guessed if it has not published yet.
    ///
    /// The decoder wins because it is the process that chose the size. The
    /// document's value is only a starting guess, and when the two disagreed
    /// the old behaviour was to keep believing the document and discard every
    /// frame — a surface frozen on its last good picture with nothing logged.
    fn resolved_size(&self) -> (u32, u32) {
        match &self.source {
            FrameSource::File {
                path,
                width,
                height,
            } => canvas_core::video_frame_size_file_for(path)
                .and_then(|path| std::fs::read_to_string(path).ok())
                .and_then(|text| canvas_core::parse_frame_size(&text))
                .unwrap_or((*width, *height)),
            FrameSource::External(frame) => (frame.descriptor.width, frame.descriptor.height),
        }
    }

    fn source_generation(&self) -> u64 {
        match &self.source {
            FrameSource::File { .. } => 0,
            FrameSource::External(frame) => frame.descriptor.generation,
        }
    }
}

impl shader::Primitive for Primitive {
    type Pipeline = Pipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        _viewport: &shader::Viewport,
    ) {
        let (source_width, source_height) = self.resolved_size();
        let needs_texture = pipeline.textures.get(&self.ticket).is_none_or(|state| {
            state.width != source_width
                || state.height != source_height
                || state.source_generation != self.source_generation()
        });
        // iced may preserve shader widget state when a workspace inserts or
        // replaces a surface at the same tree position. The retained ticket is
        // therefore stable across a source-size change; recreate the texture
        // before uploading instead of asking wgpu to overrun the old extent.
        if needs_texture {
            let (format, imported) = match &self.source {
                FrameSource::File { .. } => (wgpu::TextureFormat::Rgba8UnormSrgb, None),
                FrameSource::External(frame) => {
                    let descriptor = &frame.descriptor;
                    let format = match crate::external_surface::wgpu_format(
                        descriptor.format,
                        descriptor.color_space,
                    ) {
                        Ok(format) => format,
                        Err(error) => {
                            eprintln!(
                                "Canvas external surface {} generation {}: {error}",
                                descriptor.source_id, descriptor.generation
                            );
                            return;
                        }
                    };
                    let imported = match &descriptor.transport {
                        SurfaceTransport::SharedMemory { .. } => None,
                        SurfaceTransport::IoSurface { .. } => {
                            match crate::external_surface::import_iosurface(device, descriptor) {
                                Ok(texture) => Some(texture),
                                Err(error) => {
                                    eprintln!(
                                        "Canvas external surface {} generation {}: {error}",
                                        descriptor.source_id, descriptor.generation
                                    );
                                    return;
                                }
                            }
                        }
                        SurfaceTransport::DmaBuf { .. } => {
                            eprintln!(
                                "Canvas external surface {} generation {}: DMA-BUF import is not available on this renderer",
                                descriptor.source_id, descriptor.generation
                            );
                            return;
                        }
                    };
                    (format, imported)
                }
            };
            let mut state = pipeline.texture_state(
                device,
                source_width,
                source_height,
                self.source_generation(),
                format,
                imported,
            );
            if let FrameSource::External(frame) = &self.source {
                state.native_hold = frame.native_surface.clone().map(|surface| DeferredNativeHold {
                    surface: Some(surface), queue: queue.clone(),
                });
            }
            pipeline.textures.insert(self.ticket, state);
        }
        let state = pipeline
            .textures
            .get_mut(&self.ticket)
            .expect("retained video texture was inserted");
        state.generation = pipeline.generation;
        let metadata = match &self.source {
            FrameSource::File { path, .. } => std::fs::metadata(path).ok(),
            FrameSource::External(_) => None,
        };
        // Age the picture by how long the producer has been silent, not by how
        // long this surface has been open: a feed that died an hour ago must
        // not look the same as one delivering frames right now.
        let staleness = match &self.source {
            FrameSource::File { .. } => metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| SystemTime::now().duration_since(modified).ok())
                .map_or(1.0, staleness_of),
            FrameSource::External(frame) => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|now| {
                    now.as_nanos()
                        .checked_sub(u128::from(frame.presented_at_unix_ns))
                })
                .and_then(|age| u64::try_from(age).ok())
                .map(std::time::Duration::from_nanos)
                .map_or(1.0, staleness_of),
        };
        let uniforms = [
            bounds.width,
            bounds.height,
            source_width as f32,
            source_height as f32,
            staleness,
            0.0,
            if matches!(&self.source, FrameSource::External(frame) if frame.descriptor.alpha_mode == canvas_core::external_surface::AlphaMode::Premultiplied) { 1.0 } else { 0.0 },
            if matches!(&self.source, FrameSource::External(frame) if frame.descriptor.alpha_mode != canvas_core::external_surface::AlphaMode::Opaque) { 1.0 } else { 0.0 },
        ];
        queue.write_buffer(&state.uniform, 0, unsafe {
            std::slice::from_raw_parts(uniforms.as_ptr().cast::<u8>(), UNIFORM_BYTES as usize)
        });

        if let FrameSource::External(frame) = &self.source {
            if state.external_stamp == Some(frame.presented_at_unix_ns) {
                return;
            }
            match frame.shared_pixels() {
                Ok(Some(bytes)) => {
                    let SurfaceTransport::SharedMemory { stride, .. } = &frame.descriptor.transport
                    else {
                        unreachable!("shared pixels require shared-memory transport")
                    };
                    queue.write_texture(
                        wgpu::TexelCopyTextureInfo {
                            texture: &state.texture,
                            mip_level: 0,
                            origin: wgpu::Origin3d::ZERO,
                            aspect: wgpu::TextureAspect::All,
                        },
                        bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(*stride),
                            rows_per_image: Some(frame.descriptor.height),
                        },
                        wgpu::Extent3d {
                            width: frame.descriptor.width,
                            height: frame.descriptor.height,
                            depth_or_array_layers: 1,
                        },
                    );
                }
                Ok(None) => {}
                Err(error) => {
                    eprintln!(
                        "Canvas external surface {} generation {}: {error}",
                        frame.descriptor.source_id, frame.descriptor.generation
                    );
                    return;
                }
            }
            state.external_stamp = Some(frame.presented_at_unix_ns);
            return;
        }

        let FrameSource::File { path, .. } = &self.source else {
            return;
        };

        let Some(metadata) = metadata else {
            return;
        };
        let stamp = (metadata.modified().unwrap_or(UNIX_EPOCH), metadata.len());
        if state.stamp == Some(stamp) {
            return;
        }
        let expected = source_width as usize * source_height as usize * 4;
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        if bytes.len() != expected {
            // Reported once per distinct mismatch rather than per frame: at 24
            // frames a second a per-frame line is a log flood, and staying
            // quiet is what let a 720p stream sit invisible behind a 320x180
            // declaration.
            if state.reported_mismatch != Some((expected, bytes.len())) {
                state.reported_mismatch = Some((expected, bytes.len()));
                eprintln!(
                    "Canvas retained video {}: frame is {} bytes, expected {} for {source_width}x{source_height} — dropping frames until the sizes agree",
                    path.display(),
                    bytes.len(),
                    expected
                );
            }
            return;
        }
        state.reported_mismatch = None;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &state.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(source_width * 4),
                rows_per_image: Some(source_height),
            },
            wgpu::Extent3d {
                width: source_width,
                height: source_height,
                depth_or_array_layers: 1,
            },
        );
        state.stamp = Some(stamp);
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(state) = pipeline.textures.get(&self.ticket) else {
            return true;
        };
        render_pass.set_pipeline(&pipeline.render_pipeline);
        render_pass.set_bind_group(0, &state.bind_group, &[]);
        render_pass.draw(0..3, 0..1);
        true
    }
}

pub fn view<'a, Message: 'a>(
    path: PathBuf,
    width: u32,
    height: u32,
    on_orbit: Option<OrbitHook<Message>>,
) -> Element<'a, Message> {
    Shader::new(Program {
        source: FrameSource::File {
            path,
            width,
            height,
        },
        on_orbit,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

/// Compose a producer-owned external frame through the same retained shader
/// used by decoded video. The caller owns registry freshness and rebuilds the
/// widget when a newer presentation timestamp or generation is accepted.
pub fn external_view<'a, Message: 'a>(
    frame: crate::external_surface::ExternalSurfaceFrame,
) -> Element<'a, Message> {
    Shader::new(Program {
        source: FrameSource::External(frame),
        on_orbit: None,
    })
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

#[cfg(test)]
mod staleness_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_live_feed_is_untouched() {
        assert_eq!(staleness_of(Duration::from_millis(40)), 0.0);
        assert_eq!(staleness_of(Duration::from_secs_f32(FRESH_SECONDS)), 0.0);
    }

    /// A brief hitch must not make a working camera look broken.
    #[test]
    fn a_stutter_is_not_a_failure() {
        assert!(staleness_of(Duration::from_secs(4)) < 0.15);
    }

    /// The failure this exists for: cameras that stopped hours ago were still
    /// being drawn at full colour, indistinguishable from live.
    #[test]
    fn a_feed_that_died_hours_ago_is_fully_drained() {
        assert_eq!(staleness_of(Duration::from_secs(38_724)), 1.0);
        assert_eq!(staleness_of(Duration::from_secs_f32(STALE_SECONDS)), 1.0);
    }

    #[test]
    fn decay_is_monotonic() {
        let curve: Vec<f32> = (0..30)
            .map(|s| staleness_of(Duration::from_secs(s)))
            .collect();
        assert!(curve.windows(2).all(|pair| pair[1] >= pair[0]), "{curve:?}");
    }
}
