//! Native imports for producer-owned external surfaces.
//!
//! The portable lifecycle and IPC contract lives in `canvas-core`. This module
//! is deliberately only the renderer adapter: it turns a validated local
//! handle into a texture owned by the same wgpu device iced is rendering with.

use canvas_core::external_surface::{
    ColorSpace, ExternalSurfaceDescriptor, ExternalSurfaceMessage, ExternalSurfaceRegistry,
    PixelFormat, PresentedSurface, RegistryChange, SurfaceTransport,
};
use memmap2::{Mmap, MmapOptions};
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixDatagram;
use std::sync::Arc;

/// One accepted frame with its registration resource kept alive.
///
/// Cloning this value is cheap and lets an iced primitive outlive the registry
/// lock. Shared-memory pixels are mapped once per generation, not once per
/// frame; IOSurface and future DMA-BUF imports retain their native handles in
/// the inbox.
#[derive(Debug, Clone)]
pub struct ExternalSurfaceFrame {
    pub descriptor: ExternalSurfaceDescriptor,
    pub slot: u8,
    pub presented_at_unix_ns: u64,
    shared_memory: Option<Arc<Mmap>>,
}

impl ExternalSurfaceFrame {
    pub fn shared_pixels(&self) -> Result<Option<&[u8]>, String> {
        let SurfaceTransport::SharedMemory {
            byte_len,
            stride,
            slots,
            ..
        } = &self.descriptor.transport
        else {
            return Ok(None);
        };
        if self.slot >= *slots {
            return Err("external surface slot is out of range".into());
        }
        let frame_len = usize::try_from(u64::from(*stride) * u64::from(self.descriptor.height))
            .map_err(|_| "external surface frame is too large")?;
        let offset = frame_len
            .checked_mul(usize::from(self.slot))
            .ok_or("external surface slot offset overflow")?;
        let end = offset
            .checked_add(frame_len)
            .ok_or("external surface slot end overflow")?;
        let mapping = self
            .shared_memory
            .as_ref()
            .ok_or("external surface shared mapping is unavailable")?;
        if end > mapping.len() || end as u64 > *byte_len {
            return Err("external surface slot exceeds its registration".into());
        }
        Ok(Some(&mapping[offset..end]))
    }
}

/// Accepted local registrations and the native resources that keep them alive.
///
/// Socket ownership and scheduling stay with the embedding application. This
/// type provides the deterministic receive/apply boundary without starting an
/// immortal background thread or reaching into Canvas application state.
#[derive(Debug, Default)]
pub struct ExternalSurfaceInbox {
    registry: ExternalSurfaceRegistry,
    registration_handles: BTreeMap<(String, u64), Vec<OwnedFd>>,
    shared_memory: BTreeMap<(String, u64), Arc<Mmap>>,
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
        message.validate(handles.len())?;
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
        let shared_mapping = match &message {
            ExternalSurfaceMessage::Register { surface } => match &surface.transport {
                SurfaceTransport::SharedMemory {
                    handle_index,
                    byte_len,
                    ..
                } => {
                    let handle = handles
                        .get(usize::from(*handle_index))
                        .ok_or("external surface shared-memory handle is missing")?;
                    let length = usize::try_from(*byte_len)
                        .map_err(|_| "external surface shared-memory mapping is too large")?;
                    let file = File::from(
                        handle
                            .try_clone()
                            .map_err(|error| format!("clone external surface handle: {error}"))?,
                    );
                    let mapping = unsafe { MmapOptions::new().len(length).map(&file) }
                        .map_err(|error| format!("map external surface pixels: {error}"))?;
                    Some(Arc::new(mapping))
                }
                _ => None,
            },
            _ => None,
        };
        let change = self.registry.apply(message, handles.len())?;
        if let Some((source_id, generation)) = registration {
            self.registration_handles
                .retain(|(id, _), _| id != &source_id);
            self.shared_memory.retain(|(id, _), _| id != &source_id);
            if let Some(mapping) = shared_mapping {
                self.shared_memory
                    .insert((source_id.clone(), generation), mapping);
            }
            self.registration_handles
                .insert((source_id, generation), handles);
        }
        if let Some(key) = withdrawal {
            self.registration_handles.remove(&key);
            self.shared_memory.remove(&key);
        }
        Ok(change)
    }

    pub fn presented(&self, source_id: &str) -> Option<&PresentedSurface> {
        self.registry.get(source_id)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &PresentedSurface)> {
        self.registry.iter()
    }

    pub fn frame(&self, source_id: &str) -> Option<ExternalSurfaceFrame> {
        let presented = self.registry.get(source_id)?;
        let slot = presented.slot?;
        let presented_at_unix_ns = presented.presented_at_unix_ns?;
        let key = (source_id.to_owned(), presented.descriptor.generation);
        let shared_memory = self.shared_memory.get(&key).cloned();
        Some(ExternalSurfaceFrame {
            descriptor: presented.descriptor.clone(),
            slot,
            presented_at_unix_ns,
            shared_memory,
        })
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

pub(crate) fn wgpu_format(
    format: PixelFormat,
    color_space: ColorSpace,
) -> Result<wgpu::TextureFormat, ImportError> {
    match (format, color_space) {
        (PixelFormat::Rgba8Unorm, ColorSpace::Srgb) => Ok(wgpu::TextureFormat::Rgba8UnormSrgb),
        (PixelFormat::Bgra8Unorm, ColorSpace::Srgb) => Ok(wgpu::TextureFormat::Bgra8UnormSrgb),
        (PixelFormat::Rgba8Unorm, _) => Ok(wgpu::TextureFormat::Rgba8Unorm),
        (PixelFormat::Bgra8Unorm, _) => Ok(wgpu::TextureFormat::Bgra8Unorm),
        (PixelFormat::Rgba16Float, _) => Ok(wgpu::TextureFormat::Rgba16Float),
        // This retained-video pipeline owns a filtering sampler. R32Float and
        // NV12 need their own explicit sampling/conversion pipelines rather
        // than an import that succeeds and later fails WGPU validation.
        (PixelFormat::R32Float | PixelFormat::Nv12, _) => {
            Err(ImportError::UnsupportedFormat(format))
        }
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
        let format = wgpu_format(surface.format, surface.color_space)?;
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
            wgpu_format(PixelFormat::Bgra8Unorm, ColorSpace::Srgb).unwrap(),
            wgpu::TextureFormat::Bgra8UnormSrgb
        );
        assert!(matches!(
            wgpu_format(PixelFormat::Nv12, ColorSpace::Rec709),
            Err(ImportError::UnsupportedFormat(PixelFormat::Nv12))
        ));
    }

    #[test]
    fn shared_memory_is_mapped_once_and_sliced_by_presented_slot() {
        use std::io::{Seek, SeekFrom, Write};
        use std::os::fd::AsFd;

        let mut file = tempfile::tempfile().unwrap();
        file.set_len(32).unwrap();
        file.seek(SeekFrom::Start(16)).unwrap();
        file.write_all(&[9; 16]).unwrap();
        let descriptor = ExternalSurfaceDescriptor {
            schema_version: EXTERNAL_SURFACE_VERSION,
            source_id: "surf/shared".into(),
            generation: 8,
            width: 2,
            height: 2,
            format: PixelFormat::Rgba8Unorm,
            color_space: ColorSpace::Srgb,
            alpha_mode: AlphaMode::Opaque,
            transport: SurfaceTransport::SharedMemory {
                handle_index: 0,
                byte_len: 32,
                stride: 8,
                slots: 2,
            },
        };
        let mut inbox = ExternalSurfaceInbox::default();
        inbox
            .apply(
                ExternalSurfaceMessage::Register {
                    surface: descriptor,
                },
                vec![file.as_fd().try_clone_to_owned().unwrap()],
            )
            .unwrap();
        inbox
            .apply(
                ExternalSurfaceMessage::Present {
                    source_id: "surf/shared".into(),
                    generation: 8,
                    slot: 1,
                    sync: FrameSync::Implicit,
                    presented_at_unix_ns: 456,
                },
                vec![],
            )
            .unwrap();
        let frame = inbox.frame("surf/shared").unwrap();
        assert_eq!(frame.slot, 1);
        assert_eq!(frame.shared_pixels().unwrap().unwrap(), &[9; 16]);
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
