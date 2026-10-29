//! The gated `soil:webgpu@1.0.0` linker.
//!
//! The WIT package has no world: a Spore owns its world. This file binds the
//! test world that imports `soil:webgpu/webgpu`, then registers host functions
//! against the generated types. Only a granted `soil:webgpu` is linked;
//! `create-shader` additionally requires `soil:webgpu.shaders`.
//!
//! The host owns the device. Handles returned here are Soil-issued table
//! indices, not wgpu objects or addresses.

use crate::caps::EffectiveCaps;
use crate::webgpu::{
    BindingEntry, BindingKind, BindingLayout, BindingResource, BufferUsage, IndexFormat,
    SamplerAddress, SamplerFilter, TextureFormat, VertexAttribute, VertexBufferLayout,
    VertexFormat, VertexStep, WebGpuHost,
};
use std::collections::BTreeSet;
use std::sync::Arc;
use wasmtime::StoreContextMut;
use wasmtime::component::{Linker, LinkerInstance};

#[allow(clippy::all)]
mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/soil-webgpu.wit",
        world: "canvas:soil-webgpu-test/webgpu-test",
        inline: "
            package canvas:soil-webgpu-test@0.1.0;

            world webgpu-test {
                import soil:webgpu/webgpu@1.0.0;
            }
        ",
    });
}

use bindings::soil::webgpu::types::{
    AdapterInfo, BindingEntry as WitBindingEntry, BindingKind as WitBindingKind,
    BindingLayout as WitBindingLayout, BindingResource as WitBindingResource,
    BufferUsage as WitBufferUsage, Color as WitColor, IndexFormat as WitIndexFormat,
    SamplerAddress as WitSamplerAddress, SamplerFilter as WitSamplerFilter,
    TextureFormat as WitTextureFormat, VertexAttribute as WitVertexAttribute,
    VertexBufferLayout as WitVertexBufferLayout, VertexFormat as WitVertexFormat,
    VertexStep as WitVertexStep,
};
use bindings::soil::webgpu::webgpu::Error;

pub struct WebGpuCtx {
    host: Arc<WebGpuHost>,
    caps: BTreeSet<String>,
}

impl WebGpuCtx {
    pub fn host(&self) -> &Arc<WebGpuHost> {
        &self.host
    }

    fn granted(&self, cap: &str) -> bool {
        self.caps.contains(cap)
    }
}

fn map(error: anyhow::Error) -> Error {
    let text = format!("{error:#}");
    if text.contains("unknown")
        || text.contains("outside")
        || text.contains("needs")
        || text.contains("exceeds")
        || text.contains("shader")
    {
        Error::Invalid
    } else {
        Error::Io
    }
}

fn buffer_usage(usage: WitBufferUsage) -> BufferUsage {
    match usage {
        WitBufferUsage::Vertex => BufferUsage::Vertex,
        WitBufferUsage::Index => BufferUsage::Index,
        WitBufferUsage::Uniform => BufferUsage::Uniform,
        WitBufferUsage::Storage => BufferUsage::Storage,
        WitBufferUsage::CopySrc => BufferUsage::CopySrc,
        WitBufferUsage::CopyDst => BufferUsage::CopyDst,
    }
}

fn vertex_format(format: WitVertexFormat) -> VertexFormat {
    match format {
        WitVertexFormat::Float32x2 => VertexFormat::Float32x2,
        WitVertexFormat::Float32x3 => VertexFormat::Float32x3,
        WitVertexFormat::Float32x4 => VertexFormat::Float32x4,
        WitVertexFormat::Uint32 => VertexFormat::Uint32,
        WitVertexFormat::Uint32x2 => VertexFormat::Uint32x2,
    }
}

fn vertex_layout(layout: WitVertexBufferLayout) -> VertexBufferLayout {
    VertexBufferLayout {
        array_stride: layout.array_stride,
        step_mode: match layout.step_mode {
            WitVertexStep::Vertex => VertexStep::Vertex,
            WitVertexStep::Instance => VertexStep::Instance,
        },
        attributes: layout
            .attributes
            .into_iter()
            .map(|attribute: WitVertexAttribute| VertexAttribute {
                format: vertex_format(attribute.format),
                offset: attribute.offset,
                shader_location: attribute.location,
            })
            .collect(),
    }
}

fn binding_layout(layout: WitBindingLayout) -> BindingLayout {
    BindingLayout {
        binding: layout.binding,
        visibility: layout.visibility,
        kind: match layout.kind {
            WitBindingKind::UniformBuffer => BindingKind::UniformBuffer,
            WitBindingKind::StorageBuffer => BindingKind::StorageBuffer,
            WitBindingKind::Texture => BindingKind::Texture,
            WitBindingKind::Sampler => BindingKind::Sampler,
        },
    }
}

fn binding_entry(entry: WitBindingEntry) -> BindingEntry {
    BindingEntry {
        binding: entry.binding,
        resource: match entry.value {
            WitBindingResource::Buffer(handle) => BindingResource::Buffer(handle),
            WitBindingResource::Texture(handle) => BindingResource::Texture(handle),
            WitBindingResource::Sampler(handle) => BindingResource::Sampler(handle),
        },
    }
}

fn texture_format(format: WitTextureFormat) -> TextureFormat {
    match format {
        WitTextureFormat::Rgba8Unorm => TextureFormat::Rgba8Unorm,
        WitTextureFormat::Rgba8UnormSrgb => TextureFormat::Rgba8UnormSrgb,
        WitTextureFormat::Bgra8Unorm => TextureFormat::Bgra8Unorm,
        WitTextureFormat::Bgra8UnormSrgb => TextureFormat::Bgra8UnormSrgb,
        WitTextureFormat::Depth32Float => TextureFormat::Depth32Float,
    }
}

/// Register the granted WebGPU verbs. `soil:webgpu` is the base authority;
/// shader compilation is a separate authority.
pub fn add_to_linker(
    linker: &mut Linker<WebGpuCtx>,
    caps: &EffectiveCaps,
    host: Arc<WebGpuHost>,
) -> anyhow::Result<WebGpuCtx> {
    let context = WebGpuCtx {
        host: Arc::clone(&host),
        caps: caps.caps.clone(),
    };
    if !caps.caps.contains("soil:webgpu") {
        return Ok(context);
    }
    let mut webgpu = linker.instance("soil:webgpu/webgpu@1.0.0")?;
    register_adapter(&mut webgpu, &host)?;
    register_buffers(&mut webgpu, &host)?;
    register_textures(&mut webgpu, &host)?;
    register_shaders(&mut webgpu, &host)?;
    register_pipelines(&mut webgpu, &host)?;
    register_bind_groups(&mut webgpu, &host)?;
    register_samplers(&mut webgpu, &host)?;
    register_draw_mesh(&mut webgpu, &host)?;
    register_passes(&mut webgpu, &host)?;
    register_probe(&mut webgpu, &host)?;
    Ok(context)
}

fn register_adapter(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    webgpu.func_wrap(
        "adapter",
        move |store: StoreContextMut<'_, WebGpuCtx>, _: ()| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            let (name, backend, device_type) = host.adapter_info();
            Ok((Ok(AdapterInfo {
                name,
                backend,
                device_type,
            }),))
        },
    )
}

fn register_buffers(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let create = Arc::clone(host);
    webgpu.func_wrap(
        "create-buffer",
        move |store: StoreContextMut<'_, WebGpuCtx>, (size, usages): (u64, Vec<WitBufferUsage>)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            let usages: Vec<BufferUsage> = usages.into_iter().map(buffer_usage).collect();
            Ok((create.create_buffer(size, &usages).map_err(map),))
        },
    )?;
    let write = Arc::clone(host);
    webgpu.func_wrap(
        "write-buffer",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (buffer, offset, data): (u32, u64, Vec<u8>)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((write.write_buffer(buffer, offset, &data).map_err(map),))
        },
    )
}

fn register_textures(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let create = Arc::clone(host);
    webgpu.func_wrap(
        "create-texture",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (width, height, format, render_attachment, copy_src): (
            u32,
            u32,
            WitTextureFormat,
            bool,
            bool,
        )| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((create
                .create_texture(
                    width,
                    height,
                    texture_format(format),
                    render_attachment,
                    copy_src,
                )
                .map_err(map),))
        },
    )?;
    let read = Arc::clone(host);
    webgpu.func_wrap(
        "read-pixels",
        move |store: StoreContextMut<'_, WebGpuCtx>, (texture,): (u32,)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((read.read_pixels(texture).map_err(map),))
        },
    )
}

fn register_shaders(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let create = Arc::clone(host);
    webgpu.func_wrap(
        "create-shader",
        move |store: StoreContextMut<'_, WebGpuCtx>, (wgsl,): (String,)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            if !store.data().granted("soil:webgpu.shaders") {
                return Ok((Err(Error::Denied),));
            }
            Ok((create.create_shader(&wgsl).map_err(map),))
        },
    )
}

fn register_pipelines(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let create = Arc::clone(host);
    webgpu.func_wrap(
        "create-pipeline",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (shader, format, vertex_buffers, bind_group_layouts): (
            u32,
            WitTextureFormat,
            Vec<WitVertexBufferLayout>,
            Vec<u32>,
        )| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            if !store.data().granted("soil:webgpu.shaders") {
                return Ok((Err(Error::Denied),));
            }
            let layouts: Vec<VertexBufferLayout> =
                vertex_buffers.into_iter().map(vertex_layout).collect();
            Ok((create
                .create_pipeline(
                    shader,
                    texture_format(format),
                    &layouts,
                    &bind_group_layouts,
                )
                .map_err(map),))
        },
    )?;
    let draw = Arc::clone(host);
    webgpu.func_wrap(
        "draw-triangle",
        move |store: StoreContextMut<'_, WebGpuCtx>, (texture, pipeline): (u32, u32)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((draw.draw_triangle(texture, pipeline).map_err(map),))
        },
    )
}

fn register_bind_groups(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let create_layout = Arc::clone(host);
    webgpu.func_wrap(
        "create-bind-group-layout",
        move |store: StoreContextMut<'_, WebGpuCtx>, (entries,): (Vec<WitBindingLayout>,)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            let entries: Vec<BindingLayout> = entries.into_iter().map(binding_layout).collect();
            Ok((create_layout
                .create_bind_group_layout(&entries)
                .map_err(map),))
        },
    )?;
    let create_group = Arc::clone(host);
    webgpu.func_wrap(
        "create-bind-group",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (layout, entries): (u32, Vec<WitBindingEntry>)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            let entries: Vec<BindingEntry> = entries.into_iter().map(binding_entry).collect();
            Ok((create_group
                .create_bind_group(layout, &entries)
                .map_err(map),))
        },
    )
}

fn register_samplers(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let create = Arc::clone(host);
    webgpu.func_wrap(
        "create-sampler",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (filter, address): (WitSamplerFilter, WitSamplerAddress)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            let filter = match filter {
                WitSamplerFilter::Nearest => SamplerFilter::Nearest,
                WitSamplerFilter::Linear => SamplerFilter::Linear,
            };
            let address = match address {
                WitSamplerAddress::ClampToEdge => SamplerAddress::ClampToEdge,
                WitSamplerAddress::Repeat => SamplerAddress::Repeat,
                WitSamplerAddress::MirrorRepeat => SamplerAddress::MirrorRepeat,
            };
            Ok((create.create_sampler(filter, address).map_err(map),))
        },
    )
}

fn register_draw_mesh(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let draw = Arc::clone(host);
    webgpu.func_wrap(
        "draw-mesh",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (
            texture,
            pipeline,
            vertex_buffer,
            index_buffer,
            index_format,
            index_count,
            instances,
            bind_groups,
        ): (
            u32,
            u32,
            u32,
            Option<u32>,
            WitIndexFormat,
            u32,
            u32,
            Vec<u32>,
        )| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            let index_format = match index_format {
                WitIndexFormat::Uint16 => IndexFormat::Uint16,
                WitIndexFormat::Uint32 => IndexFormat::Uint32,
            };
            Ok((draw
                .draw_mesh(
                    texture,
                    pipeline,
                    vertex_buffer,
                    index_buffer,
                    index_format,
                    index_count,
                    instances,
                    &bind_groups,
                )
                .map_err(map),))
        },
    )
}

fn register_passes(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let create = Arc::clone(host);
    webgpu.func_wrap(
        "create-command-encoder",
        move |store: StoreContextMut<'_, WebGpuCtx>, _: ()| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((create.create_command_encoder().map_err(map),))
        },
    )?;
    let begin = Arc::clone(host);
    webgpu.func_wrap(
        "begin-render-pass",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (encoder, texture, clear): (u32, u32, WitColor)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((begin
                .begin_render_pass(encoder, texture, [clear.r, clear.g, clear.b, clear.a])
                .map_err(map),))
        },
    )?;
    let set_pipeline = Arc::clone(host);
    webgpu.func_wrap(
        "set-pipeline",
        move |store: StoreContextMut<'_, WebGpuCtx>, (pass, pipeline): (u32, u32)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((set_pipeline.set_pipeline(pass, pipeline).map_err(map),))
        },
    )?;
    let set_vertex = Arc::clone(host);
    webgpu.func_wrap(
        "set-vertex-buffer",
        move |store: StoreContextMut<'_, WebGpuCtx>, (pass, slot, buffer): (u32, u32, u32)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((set_vertex
                .set_vertex_buffer(pass, slot, buffer)
                .map_err(map),))
        },
    )?;
    let set_index = Arc::clone(host);
    webgpu.func_wrap(
        "set-index-buffer",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (pass, buffer, format): (u32, u32, WitIndexFormat)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            let format = match format {
                WitIndexFormat::Uint16 => IndexFormat::Uint16,
                WitIndexFormat::Uint32 => IndexFormat::Uint32,
            };
            Ok((set_index
                .set_index_buffer(pass, buffer, format)
                .map_err(map),))
        },
    )?;
    let set_group = Arc::clone(host);
    webgpu.func_wrap(
        "set-bind-group",
        move |store: StoreContextMut<'_, WebGpuCtx>, (pass, index, group): (u32, u32, u32)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((set_group.set_bind_group(pass, index, group).map_err(map),))
        },
    )?;
    let draw = Arc::clone(host);
    webgpu.func_wrap(
        "draw",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (pass, vertices, instances): (u32, u32, u32)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((draw.draw(pass, vertices, instances).map_err(map),))
        },
    )?;
    let draw_indexed = Arc::clone(host);
    webgpu.func_wrap(
        "draw-indexed",
        move |store: StoreContextMut<'_, WebGpuCtx>,
              (pass, indices, base_vertex, instances): (u32, u32, u32, u32)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((draw_indexed
                .draw_indexed(pass, indices, base_vertex, instances)
                .map_err(map),))
        },
    )?;
    let end = Arc::clone(host);
    webgpu.func_wrap(
        "end-pass",
        move |store: StoreContextMut<'_, WebGpuCtx>, (pass,): (u32,)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((end.end_pass(pass).map_err(map),))
        },
    )?;
    let submit = Arc::clone(host);
    webgpu.func_wrap(
        "submit",
        move |store: StoreContextMut<'_, WebGpuCtx>, (encoder,): (u32,)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((submit.submit(encoder).map_err(map),))
        },
    )
}

fn register_probe(
    webgpu: &mut LinkerInstance<'_, WebGpuCtx>,
    host: &Arc<WebGpuHost>,
) -> anyhow::Result<()> {
    let host = Arc::clone(host);
    webgpu.func_wrap(
        "render-probe",
        move |store: StoreContextMut<'_, WebGpuCtx>, (width, height): (u32, u32)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            Ok((host
                .render_probe(width, height)
                .map(|frame| frame.pixels)
                .map_err(map),))
        },
    )
}
