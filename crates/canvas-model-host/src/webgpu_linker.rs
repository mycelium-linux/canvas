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
use crate::webgpu::{BufferUsage, TextureFormat, WebGpuHost};
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
    AdapterInfo, BufferUsage as WitBufferUsage, TextureFormat as WitTextureFormat,
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
        move |store: StoreContextMut<'_, WebGpuCtx>, (shader, format): (u32, WitTextureFormat)| {
            if !store.data().granted("soil:webgpu") {
                return Ok((Err(Error::Denied),));
            }
            if !store.data().granted("soil:webgpu.shaders") {
                return Ok((Err(Error::Denied),));
            }
            Ok((create
                .create_pipeline(shader, texture_format(format))
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
