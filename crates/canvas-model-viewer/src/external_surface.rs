//! Native imports for producer-owned external surfaces.
//!
//! The portable lifecycle and IPC contract lives in `canvas-core`. This module
//! is deliberately only the renderer adapter: it turns a validated local
//! handle into a texture owned by the same wgpu device iced is rendering with.

use canvas_core::external_surface::{
    ExternalSurfaceDescriptor, ExternalSurfaceMessage, ExternalSurfaceRegistry, PixelFormat,
    PresentedSurface, RegistryChange, SurfaceTransport,
};
use std::collections::BTreeMap;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixDatagram;

/// Accepted local registrations and the native resources that keep them alive.
///
/// Socket ownership and scheduling stay with the embedding application. This
/// type provides the deterministic receive/apply boundary without starting an
/// immortal background thread or reaching into Canvas application state.
#[derive(Debug, Default)]
pub struct ExternalSurfaceInbox {
    registry: ExternalSurfaceRegistry,
    registration_handles: BTreeMap<(String, u64), Vec<OwnedFd>>,
}

impl ExternalSurfaceInbox {
    pub fn receive(&mut self, socket: &UnixDatagram) -> io::Result<RegistryChange> {
        let (message, handles) = canvas_core::external_surface_ipc::receive(socket)?;
        self.apply(message, handles).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("external surface: {error}"),
            )
        })
    }

    pub fn apply(
        &mut self,
        message: ExternalSurfaceMessage,
        handles: Vec<OwnedFd>,
    ) -> Result<RegistryChange, String> {
        let registration = match &message {
            ExternalSurfaceMessage::Register { surface } => {
                Some((surface.source_id.clone(), surface.generation))
            }
            _ => None,
        };
        let withdrawal = match &message {
            ExternalSurfaceMessage::Withdraw {
                source_id,
                generation,
            } => Some((source_id.clone(), *generation)),
            _ => None,
        };
        let change = self.registry.apply(message, handles.len())?;
        if let Some((source_id, generation)) = registration {
            self.registration_handles
                .retain(|(id, _), _| id != &source_id);
            self.registration_handles
                .insert((source_id, generation), handles);
        }
        if let Some(key) = withdrawal {
            self.registration_handles.remove(&key);
        }
        Ok(change)
    }

    pub fn presented(&self, source_id: &str) -> Option<&PresentedSurface> {
        self.registry.get(source_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &PresentedSurface)> {
        self.registry.iter()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    WrongTransport,
    UnsupportedFormat(PixelFormat),
    UnsupportedBackend,
    SurfaceUnavailable(u32),
    NativeTexture,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongTransport => formatter.write_str("external surface is not an IOSurface"),
            Self::UnsupportedFormat(format) => {
                write!(formatter, "unsupported IOSurface pixel format: {format:?}")
            }
            Self::UnsupportedBackend => {
                formatter.write_str("Canvas renderer is not using the Metal backend")
            }
            Self::SurfaceUnavailable(id) => write!(formatter, "IOSurface {id} is unavailable"),
            Self::NativeTexture => formatter.write_str("Metal refused the IOSurface texture"),
        }
    }
}

impl std::error::Error for ImportError {}

/// Import an IOSurface as a sampled wgpu texture without copying its pixels.
///
/// The producer retains the IOSurface until it withdraws the registered
/// generation. The returned wgpu texture retains the Metal texture, which in
/// turn retains the IOSurface for the duration of GPU use.
#[cfg(target_os = "macos")]
pub fn import_iosurface(
    device: &wgpu::Device,
    surface: &ExternalSurfaceDescriptor,
) -> Result<wgpu::Texture, ImportError> {
    macos::import(device, surface)
}

#[cfg(not(target_os = "macos"))]
pub fn import_iosurface(
    _device: &wgpu::Device,
    surface: &ExternalSurfaceDescriptor,
) -> Result<wgpu::Texture, ImportError> {
    if !matches!(surface.transport, SurfaceTransport::IoSurface { .. }) {
        return Err(ImportError::WrongTransport);
    }
    Err(ImportError::UnsupportedBackend)
}

fn wgpu_format(format: PixelFormat) -> Result<wgpu::TextureFormat, ImportError> {
    match format {
        PixelFormat::Rgba8Unorm => Ok(wgpu::TextureFormat::Rgba8Unorm),
        PixelFormat::Bgra8Unorm => Ok(wgpu::TextureFormat::Bgra8Unorm),
        PixelFormat::Rgba16Float => Ok(wgpu::TextureFormat::Rgba16Float),
        PixelFormat::R32Float => Ok(wgpu::TextureFormat::R32Float),
        PixelFormat::Nv12 => Err(ImportError::UnsupportedFormat(format)),
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc 0.2's selector macro probes its historical cargo-clippy cfg.
mod macos {
    use super::*;
    use metal::{
        MTLPixelFormat, MTLStorageMode, MTLTextureType, MTLTextureUsage, TextureDescriptor,
        foreign_types::ForeignType, objc::runtime::Object,
    };
    use objc::{msg_send, sel, sel_impl};
    use std::ffi::c_void;
    use std::ptr::NonNull;

    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        fn IOSurfaceLookup(csid: u32) -> *mut c_void;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    struct SurfaceRef(NonNull<c_void>);

    impl Drop for SurfaceRef {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0.as_ptr()) };
        }
    }

    fn metal_format(format: PixelFormat) -> Result<MTLPixelFormat, ImportError> {
        match format {
            PixelFormat::Rgba8Unorm => Ok(MTLPixelFormat::RGBA8Unorm),
            PixelFormat::Bgra8Unorm => Ok(MTLPixelFormat::BGRA8Unorm),
            PixelFormat::Rgba16Float => Ok(MTLPixelFormat::RGBA16Float),
            PixelFormat::R32Float => Ok(MTLPixelFormat::R32Float),
            PixelFormat::Nv12 => Err(ImportError::UnsupportedFormat(format)),
        }
    }

    pub(super) fn import(
        device: &wgpu::Device,
        surface: &ExternalSurfaceDescriptor,
    ) -> Result<wgpu::Texture, ImportError> {
        let SurfaceTransport::IoSurface { surface_id } = surface.transport else {
            return Err(ImportError::WrongTransport);
        };
        let format = wgpu_format(surface.format)?;
        let metal_format = metal_format(surface.format)?;
        let io_surface = NonNull::new(unsafe { IOSurfaceLookup(surface_id) })
            .map(SurfaceRef)
            .ok_or(ImportError::SurfaceUnavailable(surface_id))?;

        let descriptor = TextureDescriptor::new();
        descriptor.set_texture_type(MTLTextureType::D2);
        descriptor.set_pixel_format(metal_format);
        descriptor.set_width(u64::from(surface.width));
        descriptor.set_height(u64::from(surface.height));
        descriptor.set_depth(1);
        descriptor.set_array_length(1);
        descriptor.set_mipmap_level_count(1);
        descriptor.set_sample_count(1);
        descriptor.set_storage_mode(MTLStorageMode::Shared);
        descriptor.set_usage(MTLTextureUsage::ShaderRead);

        let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Metal>() }
            .ok_or(ImportError::UnsupportedBackend)?;
        let raw_device = hal_device.raw_device().lock();
        let raw_texture: *mut Object = unsafe {
            msg_send![
                raw_device.as_ptr(),
                newTextureWithDescriptor: descriptor.as_ptr()
                iosurface: io_surface.0.as_ptr()
                plane: 0_u64
            ]
        };
        if raw_texture.is_null() {
            return Err(ImportError::NativeTexture);
        }
        let raw_texture = unsafe { metal::Texture::from_ptr(raw_texture.cast()) };
        let hal_texture = unsafe {
            wgpu_hal::metal::Device::texture_from_raw(
                raw_texture,
                format,
                MTLTextureType::D2,
                1,
                1,
                wgpu_hal::CopyExtent {
                    width: surface.width,
                    height: surface.height,
                    depth: 1,
                },
            )
        };
        let descriptor = wgpu::TextureDescriptor {
            label: Some("Canvas IOSurface external surface"),
            size: wgpu::Extent3d {
                width: surface.width,
                height: surface.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        Ok(unsafe {
            device.create_texture_from_hal::<wgpu_hal::api::Metal>(hal_texture, &descriptor)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::external_surface::{
        AlphaMode, ColorSpace, EXTERNAL_SURFACE_VERSION, FrameSync,
    };

    #[test]
    fn sampled_formats_are_explicit() {
        assert_eq!(
            wgpu_format(PixelFormat::Bgra8Unorm).unwrap(),
            wgpu::TextureFormat::Bgra8Unorm
        );
        assert!(matches!(
            wgpu_format(PixelFormat::Nv12),
            Err(ImportError::UnsupportedFormat(PixelFormat::Nv12))
        ));
    }

    #[test]
    fn inbox_publishes_only_presented_registered_generations() {
        let (sender, receiver) = UnixDatagram::pair().unwrap();
        let descriptor = ExternalSurfaceDescriptor {
            schema_version: EXTERNAL_SURFACE_VERSION,
            source_id: "surf/local".into(),
            generation: 7,
            width: 1280,
            height: 720,
            format: PixelFormat::Bgra8Unorm,
            color_space: ColorSpace::DisplayP3,
            alpha_mode: AlphaMode::Opaque,
            transport: SurfaceTransport::IoSurface { surface_id: 42 },
        };
        canvas_core::external_surface_ipc::send(
            &sender,
            &ExternalSurfaceMessage::Register {
                surface: descriptor,
            },
            &[],
        )
        .unwrap();
        canvas_core::external_surface_ipc::send(
            &sender,
            &ExternalSurfaceMessage::Present {
                source_id: "surf/local".into(),
                generation: 7,
                slot: 0,
                sync: FrameSync::Implicit,
                presented_at_unix_ns: 123,
            },
            &[],
        )
        .unwrap();

        let mut inbox = ExternalSurfaceInbox::default();
        assert!(matches!(
            inbox.receive(&receiver).unwrap(),
            RegistryChange::Registered { generation: 7, .. }
        ));
        assert_eq!(inbox.presented("surf/local").unwrap().slot, None);
        assert!(matches!(
            inbox.receive(&receiver).unwrap(),
            RegistryChange::Presented { generation: 7, .. }
        ));
        let presented = inbox.presented("surf/local").unwrap();
        assert_eq!(presented.slot, Some(0));
        assert_eq!(presented.presented_at_unix_ns, Some(123));
    }
}
