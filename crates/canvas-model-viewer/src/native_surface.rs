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
        pub(crate) fn retain(descriptor: &ExternalSurfaceDescriptor) -> Result<Self, String> {
            let SurfaceTransport::IoSurface { surface_id } = descriptor.transport else {
                return Err("not an IOSurface registration".into());
            };
            let ptr = NonNull::new(unsafe { IOSurfaceLookup(surface_id) })
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
                IOSurfaceGetWidth(ptr.as_ptr()) == descriptor.width as usize
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
