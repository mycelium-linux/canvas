//! Owner-local IOSurface retention, independent of the capture producer.
//! A CF retain keeps the allocation alive; a use-count hold prevents capture
//! pools from recycling its pixels while Canvas/Metal is still reading them.
use canvas_core::external_surface::{ExternalSurfaceDescriptor, PixelFormat, SurfaceTransport};

#[cfg(target_os = "macos")]
#[derive(Debug)]
pub(crate) struct NativeSurface(std::ptr::NonNull<std::ffi::c_void>);

#[cfg(not(target_os = "macos"))]
#[derive(Debug)]
pub(crate) struct NativeSurface;

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use std::{ffi::c_void, ptr::NonNull};
    #[link(name = "IOSurface", kind = "framework")]
    unsafe extern "C" {
        fn IOSurfaceLookup(id: u32) -> *mut c_void;
        fn IOSurfaceLookupFromMachPort(port: u32) -> *mut c_void;
        fn IOSurfaceGetID(surface: *mut c_void) -> u32;
        fn IOSurfaceGetWidth(surface: *mut c_void) -> usize;
        fn IOSurfaceGetHeight(surface: *mut c_void) -> usize;
        fn IOSurfaceGetPixelFormat(surface: *mut c_void) -> u32;
        fn IOSurfaceIncrementUseCount(surface: *mut c_void);
        fn IOSurfaceDecrementUseCount(surface: *mut c_void);
    }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }

    // SAFETY: IOSurface is a thread-safe shared framebuffer. This wrapper never
    // exposes a mutable pixel pointer; registration checks its immutable shape.
    unsafe impl Send for NativeSurface {}
    unsafe impl Sync for NativeSurface {}

    impl NativeSurface {
        pub(crate) fn as_ptr(&self) -> *mut c_void {
            self.0.as_ptr()
        }
        pub(crate) fn retain_with_port(
            descriptor: &ExternalSurfaceDescriptor,
            port: Option<u32>,
        ) -> Result<Self, String> {
            let surface_id = match descriptor.transport {
                SurfaceTransport::IoSurface { surface_id } => surface_id,
                SurfaceTransport::IoSurfaceMachPort { surface_id } if port.is_some() => surface_id,
                _ => return Err("IOSurface registration is missing its native right".into()),
            };
            let ptr = NonNull::new(unsafe {
                match port {
                    Some(port) => IOSurfaceLookupFromMachPort(port),
                    None => IOSurfaceLookup(surface_id),
                }
            })
            .ok_or_else(|| format!("IOSurface {surface_id} is unavailable"))?;
            let expected = match descriptor.format {
                PixelFormat::Bgra8Unorm => u32::from_be_bytes(*b"BGRA"),
                PixelFormat::Rgba8Unorm => u32::from_be_bytes(*b"RGBA"),
                _ => {
                    unsafe {
                        CFRelease(ptr.as_ptr());
                    }
                    return Err("unsupported retained IOSurface format".into());
                }
            };
            let matches = unsafe {
                IOSurfaceGetID(ptr.as_ptr()) == surface_id
                    && IOSurfaceGetWidth(ptr.as_ptr()) == descriptor.width as usize
                    && IOSurfaceGetHeight(ptr.as_ptr()) == descriptor.height as usize
                    && IOSurfaceGetPixelFormat(ptr.as_ptr()) == expected
            };
            if !matches {
                unsafe {
                    CFRelease(ptr.as_ptr());
                }
                return Err("IOSurface native dimensions/format differ from registration".into());
            }
            unsafe {
                IOSurfaceIncrementUseCount(ptr.as_ptr());
            }
            Ok(Self(ptr))
        }
    }
    impl Drop for NativeSurface {
        fn drop(&mut self) {
            unsafe {
                IOSurfaceDecrementUseCount(self.0.as_ptr());
                CFRelease(self.0.as_ptr());
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
impl NativeSurface {
    pub(crate) fn retain(_: &ExternalSurfaceDescriptor) -> Result<Self, String> {
        Err("IOSurface registrations require macOS".into())
    }
}
