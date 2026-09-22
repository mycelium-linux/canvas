//! Display inventory read straight from AppKit.
//!
//! This is the one place that answers "what monitors are attached". Canvas used
//! to ask Dock Control for it over a socket, which made a second process a hard
//! dependency of Canvas knowing its own screens: when nothing was listening on
//! the control socket the inventory came back empty, so display shortcuts
//! reported every monitor as disconnected and Canvas could not target one. Any
//! process in the GUI session can read NSScreen itself, so both Canvas and Dock
//! Control call this instead.

use crate::{Display, Rect};
use core_foundation_sys::{base::*, string::*, uuid::*};
use objc2::{class, msg_send, runtime::AnyObject};
use objc2_foundation::NSRect;
use std::{ffi::CStr, ptr};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGDisplayCreateUUIDFromDisplayID(display: u32) -> CFUUIDRef;
}

#[link(name = "AppKit", kind = "framework")]
extern "C" {}

struct Owned(CFTypeRef);

impl Drop for Owned {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                CFRelease(self.0)
            }
        }
    }
}

unsafe fn string(value: &str) -> Owned {
    Owned(CFStringCreateWithBytes(
        ptr::null(),
        value.as_ptr(),
        value.len() as isize,
        kCFStringEncodingUTF8,
        0,
    ) as _)
}

unsafe fn text(value: CFTypeRef) -> String {
    if value.is_null() || CFGetTypeID(value) != CFStringGetTypeID() {
        return String::new();
    }
    let mut bytes = vec![0u8; (CFStringGetLength(value as _) as usize) * 4 + 1];
    if CFStringGetCString(
        value as _,
        bytes.as_mut_ptr() as _,
        bytes.len() as isize,
        kCFStringEncodingUTF8,
    ) != 0
    {
        CStr::from_ptr(bytes.as_ptr() as _)
            .to_string_lossy()
            .into_owned()
    } else {
        String::new()
    }
}

unsafe fn objtext(value: *mut AnyObject) -> String {
    if value.is_null() {
        return String::new();
    }
    let pointer: *const std::ffi::c_char = msg_send![value, UTF8String];
    if pointer.is_null() {
        String::new()
    } else {
        CStr::from_ptr(pointer).to_string_lossy().into_owned()
    }
}

/// Convert AppKit's bottom-left origin into Canvas's top-left origin.
///
/// AppKit measures Y up from the bottom of the primary screen, so a secondary
/// monitor placed above the primary has a positive origin that Canvas would
/// otherwise draw below it. `primary_height` is the height of screen 0, which
/// is the origin of AppKit's global coordinate space.
fn to_canvas_rect(rect: NSRect, primary_height: f64) -> Rect {
    Rect {
        x: rect.origin.x.round() as i32,
        y: (primary_height - rect.origin.y - rect.size.height).round() as i32,
        width: rect.size.width.round() as u32,
        height: rect.size.height.round() as u32,
    }
}

/// Every attached display, screen 0 first. `NSScreen` is main-thread-only, so
/// callers must be on the main thread.
pub fn enumerate() -> Result<Vec<Display>, String> {
    objc2::rc::autoreleasepool(|_| unsafe {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        if screens.is_null() {
            return Err("AppKit returned no screen list".into());
        }
        let count: usize = msg_send![screens, count];
        let mut raw = Vec::with_capacity(count);
        for index in 0..count {
            let screen: *mut AnyObject = msg_send![screens, objectAtIndex: index];
            let frame: NSRect = msg_send![screen, frame];
            let visible: NSRect = msg_send![screen, visibleFrame];
            let name: *mut AnyObject = msg_send![screen, localizedName];
            let description: *mut AnyObject = msg_send![screen, deviceDescription];
            let key = string("NSScreenNumber");
            let number: *mut AnyObject =
                msg_send![description, objectForKey: key.0 as *mut AnyObject];
            let number: u32 = msg_send![number, unsignedIntValue];
            let uuid = Owned(CGDisplayCreateUUIDFromDisplayID(number) as _);
            let uuid_text = Owned(CFUUIDCreateString(ptr::null(), uuid.0 as _) as _);
            raw.push((text(uuid_text.0), objtext(name), frame, visible));
        }
        let primary_height = raw
            .first()
            .map(|(_, _, frame, _)| frame.size.height)
            .unwrap_or(0.0);
        Ok(raw
            .into_iter()
            .enumerate()
            .map(|(index, (id, name, frame, visible))| Display {
                id,
                name,
                frame: to_canvas_rect(frame, primary_height),
                visible_frame: to_canvas_rect(visible, primary_height),
                primary: index == 0,
            })
            .collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSPoint, NSSize};

    fn rect(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn the_primary_screen_sits_at_the_origin() {
        let converted = to_canvas_rect(rect(0.0, 0.0, 1920.0, 1200.0), 1200.0);
        assert_eq!((converted.x, converted.y), (0, 0));
        assert_eq!((converted.width, converted.height), (1920, 1200));
    }

    /// A monitor to the right shares the primary's top edge, so only x moves.
    #[test]
    fn a_side_by_side_screen_keeps_the_same_top_edge() {
        let converted = to_canvas_rect(rect(1920.0, 0.0, 1920.0, 1200.0), 1200.0);
        assert_eq!((converted.x, converted.y), (1920, 0));
    }

    /// AppKit measures Y upward, so a screen physically above the primary has a
    /// positive origin. Treating that as a top-left Y would place it below.
    #[test]
    fn a_screen_above_the_primary_gets_a_negative_y() {
        let converted = to_canvas_rect(rect(0.0, 1200.0, 1920.0, 1080.0), 1200.0);
        assert_eq!(converted.y, -1080);
    }

    #[test]
    fn a_shorter_screen_below_the_primary_gets_a_positive_y() {
        let converted = to_canvas_rect(rect(0.0, -1080.0, 1920.0, 1080.0), 1200.0);
        assert_eq!(converted.y, 1200);
    }
}
