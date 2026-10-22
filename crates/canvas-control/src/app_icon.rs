//! Read-only AppKit icon lookup. This does not call the Accessibility helper.
use objc2::{class, msg_send, rc::autoreleasepool, runtime::AnyObject};
use objc2_foundation::NSString;
use std::path::Path;

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

/// Copy an application icon as bounded PNG bytes. All borrowed Objective-C
/// objects stay inside the autorelease pool; only owned Rust bytes escape.
pub fn png(path: &Path) -> Result<Vec<u8>, String> {
    if !path.is_absolute() || path.extension().is_none_or(|e| e != "app") || !path.is_dir() {
        return Err("icon lookup requires an installed application bundle".into());
    }
    let path = path.to_str().ok_or("application path is not UTF-8")?;
    autoreleasepool(|_| unsafe {
        // SAFETY: selectors and ABI match the public AppKit bindings. Every
        // nullable result is checked before use; NSData is copied before pool exit.
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace.is_null() {
            return Err("NSWorkspace unavailable".into());
        }
        let name = NSString::from_str(path);
        let icon: *mut AnyObject = msg_send![workspace, iconForFile: &*name];
        if icon.is_null() {
            return Err("application icon unavailable".into());
        }
        let tiff: *mut AnyObject = msg_send![icon, TIFFRepresentation];
        if tiff.is_null() {
            return Err("application icon has no bitmap representation".into());
        }
        let bitmap: *mut AnyObject = msg_send![class!(NSBitmapImageRep), imageRepWithData: tiff];
        if bitmap.is_null() {
            return Err("application icon bitmap unavailable".into());
        }
        let properties: *mut AnyObject = msg_send![class!(NSDictionary), dictionary];
        // NSBitmapImageFileType::PNG = 4 (NSUInteger).
        let data: *mut AnyObject =
            msg_send![bitmap, representationUsingType: 4usize properties: properties];
        if data.is_null() {
            return Err("application icon PNG encoding failed".into());
        }
        let length: usize = msg_send![data, length];
        if length == 0 || length > 4 * 1024 * 1024 {
            return Err("application icon exceeds byte limit".into());
        }
        // NSData subclasses (notably OS_dispatch_data) can encode `bytes`
        // as a C string pointer, tripping objc2's checked return ABI. Copy
        // through NSData's public void-pointer method instead of borrowing
        // a subclass-specific pointer. Rust owns the initialized output.
        let mut bytes = vec![0_u8; length];
        let _: () = msg_send![data, getBytes: bytes.as_mut_ptr().cast::<std::ffi::c_void>(), length: length];
        Ok(bytes)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_non_bundle_paths() {
        assert!(png(Path::new("relative.app")).is_err());
        assert!(png(Path::new("/etc/passwd")).is_err());
    }
    #[test]
    fn installed_app_icon_is_a_png() {
        let bytes = png(Path::new("/System/Applications/Calculator.app")).unwrap();
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
