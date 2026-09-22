//! Public AppKit and Accessibility APIs. Only this installed process owns TCC access.
use canvas_control::{Display, Operation, Rect, Response, Window, WindowSelector};
use core_foundation_sys::{array::*, base::*, dictionary::*, number::*, string::*, uuid::*};
use objc2::{class, msg_send, runtime::AnyObject};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use std::{collections::HashMap, ffi::CStr, ptr};
type Ax = CFTypeRef;
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    static kAXTrustedCheckOptionPrompt: CFStringRef;
    fn AXUIElementCreateApplication(pid: i32) -> Ax;
    fn AXUIElementCopyAttributeValue(e: Ax, a: CFStringRef, v: *mut CFTypeRef) -> i32;
    fn AXUIElementSetAttributeValue(e: Ax, a: CFStringRef, v: CFTypeRef) -> i32;
    fn AXUIElementIsAttributeSettable(e: Ax, a: CFStringRef, v: *mut bool) -> i32;
    fn AXUIElementPerformAction(e: Ax, a: CFStringRef) -> i32;
    fn AXUIElementSetMessagingTimeout(e: Ax, seconds: f32) -> i32;
    fn AXValueCreate(kind: u32, value: *const std::ffi::c_void) -> CFTypeRef;
    fn AXValueGetValue(value: CFTypeRef, kind: u32, out: *mut std::ffi::c_void) -> bool;
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
unsafe fn string(s: &str) -> Owned {
    Owned(CFStringCreateWithBytes(
        ptr::null(),
        s.as_ptr(),
        s.len() as isize,
        kCFStringEncodingUTF8,
        0,
    ) as _)
}
unsafe fn attr_raw(e: Ax, name: &str) -> Result<Owned, i32> {
    let key = string(name);
    let mut value = ptr::null();
    let code = AXUIElementCopyAttributeValue(e, key.0 as _, &mut value);
    if code != 0 || value.is_null() {
        Err(code)
    } else {
        Ok(Owned(value))
    }
}
unsafe fn attr(e: Ax, name: &str) -> Result<Owned, String> {
    attr_raw(e, name).map_err(|code| format!("{name}: Accessibility error {code}"))
}
unsafe fn launch_fingerprint(app: *mut AnyObject) -> Result<u64, String> {
    if app.is_null() {
        return Err("window process exited".into());
    }
    let pid: i32 = msg_send![app, processIdentifier];
    process_fingerprint(pid)
}

// AppKit launchDate is optional, including for the running Finder. Kernel start
// time remains stable for a process lifetime and distinguishes reused PIDs.
fn process_fingerprint(pid: i32) -> Result<u64, String> {
    if pid <= 0 {
        return Err("invalid window process id".into());
    }
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let count = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if count != size {
        return Err("kernel process identity is unavailable".into());
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid as u32 || info.pbi_start_tvsec == 0 || info.pbi_start_tvusec >= 1_000_000
    {
        return Err("invalid kernel process identity".into());
    }
    info.pbi_start_tvsec
        .checked_mul(1_000_000)
        .and_then(|seconds| seconds.checked_add(info.pbi_start_tvusec))
        .ok_or_else(|| "kernel process identity overflow".into())
}

unsafe fn set(e: Ax, name: &str, value: CFTypeRef) -> Result<(), String> {
    let key = string(name);
    let code = AXUIElementSetAttributeValue(e, key.0 as _, value);
    if code == 0 {
        Ok(())
    } else {
        Err(format!("{name}: Accessibility error {code}"))
    }
}
unsafe fn text(v: CFTypeRef) -> String {
    if v.is_null() || CFGetTypeID(v) != CFStringGetTypeID() {
        return String::new();
    }
    let mut bytes = vec![0u8; (CFStringGetLength(v as _) as usize) * 4 + 1];
    if CFStringGetCString(
        v as _,
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
unsafe fn objtext(v: *mut AnyObject) -> String {
    if v.is_null() {
        return String::new();
    }
    let p: *const std::ffi::c_char = msg_send![v, UTF8String];
    if p.is_null() {
        String::new()
    } else {
        CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}
unsafe fn boolean(e: Ax, name: &str) -> Result<bool, String> {
    match attr_raw(e, name) {
        Ok(value) if CFGetTypeID(value.0) == CFBooleanGetTypeID() => {
            Ok(CFBooleanGetValue(value.0 as _))
        }
        Ok(_) => Err(format!("{name}: expected a boolean")),
        Err(-25205 | -25212) => Ok(false), // Attribute unsupported / no value.
        Err(code) => Err(format!("{name}: Accessibility error {code}")),
    }
}
unsafe fn settable(e: Ax, name: &str) -> bool {
    let key = string(name);
    let mut yes = false;
    AXUIElementIsAttributeSettable(e, key.0 as _, &mut yes) == 0 && yes
}
fn choose_window(title: &str, candidates: &[String]) -> Result<usize, String> {
    let exact = candidates
        .iter()
        .enumerate()
        .filter_map(|(index, candidate)| (candidate == title).then_some(index))
        .collect::<Vec<_>>();
    match exact.as_slice() {
        [index] => Ok(*index),
        [] if candidates.len() == 1 => Ok(0),
        [] => Err(format!(
            "Window title changed and {} candidates remain; refresh bindings",
            candidates.len()
        )),
        _ => Err("Window title is ambiguous".into()),
    }
}
unsafe fn window(e: Ax, id: String, pid: u32, app: String) -> Result<Window, String> {
    let pos = attr(e, "AXPosition")?;
    let size = attr(e, "AXSize")?;
    let mut p = NSPoint::new(0., 0.);
    let mut s = NSSize::new(0., 0.);
    if !AXValueGetValue(pos.0, 1, &mut p as *mut _ as _)
        || !AXValueGetValue(size.0, 2, &mut s as *mut _ as _)
    {
        return Err("invalid Accessibility window geometry".into());
    }
    Ok(Window {
        id,
        pid,
        app,
        title: attr(e, "AXTitle").map(|v| text(v.0)).unwrap_or_default(),
        frame: Rect {
            x: p.x.round() as i32,
            y: p.y.round() as i32,
            width: s.width.round() as u32,
            height: s.height.round() as u32,
        },
        movable: settable(e, "AXPosition"),
        resizable: settable(e, "AXSize"),
        minimized: boolean(e, "AXMinimized")?,
        fullscreen: boolean(e, "AXFullScreen")?,
    })
}
pub struct Native {
    entries: HashMap<String, (Owned, u32, String, u64)>,
    nonce: String,
    next: u64,
}
impl Native {
    pub fn new() -> Self {
        unsafe {
            let _: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        }
        Self {
            entries: HashMap::new(),
            nonce: format!(
                "{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ),
            next: 0,
        }
    }
    pub fn execute(&mut self, op: Operation) -> Result<Response, String> {
        objc2::rc::autoreleasepool(|_| unsafe { self.execute_inner(op) })
    }
    unsafe fn execute_inner(&mut self, op: Operation) -> Result<Response, String> {
        if matches!(op, Operation::Status | Operation::RequestPermission) {
            let trusted = if matches!(op, Operation::RequestPermission) {
                let keys = [kAXTrustedCheckOptionPrompt as *const _];
                let values = [kCFBooleanTrue as *const _];
                let d = Owned(CFDictionaryCreate(
                    ptr::null(),
                    keys.as_ptr(),
                    values.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks,
                    &kCFTypeDictionaryValueCallBacks,
                ) as _);
                AXIsProcessTrustedWithOptions(d.0 as _)
            } else {
                AXIsProcessTrustedWithOptions(ptr::null())
            };
            return Ok(Response::Status { trusted });
        }
        if !AXIsProcessTrustedWithOptions(ptr::null()) {
            return Err("Grant Accessibility to Dock Control.app in System Settings → Privacy & Security → Accessibility".into());
        }
        match &op {
            Operation::Snapshot => return self.snapshot(),
            Operation::Displays => return self.displays(),
            Operation::Windows => return self.tolerant_windows(),
            Operation::Resolve { selectors } => return self.resolve(selectors),
            _ => {}
        }
        let id = match &op {
            Operation::Focus { id }
            | Operation::SetFrame { id, .. }
            | Operation::SetFullscreen { id, .. } => id,
            _ => unreachable!(),
        };
        let (e, pid, app, fingerprint) = self
            .entries
            .get(id)
            .ok_or("unknown or stale window id; refresh inventory")?;
        let running: *mut AnyObject = msg_send![class!(NSRunningApplication),runningApplicationWithProcessIdentifier:*pid as i32];
        if launch_fingerprint(running)? != *fingerprint {
            return Err("stale window: process identity changed; refresh inventory".into());
        }
        let before = window(e.0, id.clone(), *pid, app.clone())?;
        match op {
            Operation::SetFrame { frame, .. } => {
                if frame.width == 0
                    || frame.height == 0
                    || frame.width > 100_000
                    || frame.height > 100_000
                {
                    return Err("invalid window dimensions".into());
                }
                if before.fullscreen || before.minimized {
                    return Err("cannot move minimized or fullscreen window".into());
                }
                if !before.movable || !before.resizable {
                    return Err("window does not allow move and resize".into());
                }
                let p = NSPoint::new(frame.x as f64, frame.y as f64);
                let s = NSSize::new(frame.width as f64, frame.height as f64);
                let size = Owned(AXValueCreate(2, &s as *const _ as _));
                let pos = Owned(AXValueCreate(1, &p as *const _ as _));
                set(e.0, "AXSize", size.0)?;
                set(e.0, "AXPosition", pos.0)?;
            }
            Operation::SetFullscreen { fullscreen, .. } => {
                set(
                    e.0,
                    "AXFullScreen",
                    if fullscreen {
                        kCFBooleanTrue as _
                    } else {
                        kCFBooleanFalse as _
                    },
                )?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                while boolean(e.0, "AXFullScreen")? != fullscreen {
                    if std::time::Instant::now() >= deadline {
                        return Err(
                            "fullscreen transition did not complete within 3 seconds".into()
                        );
                    }
                    core_foundation_sys::runloop::CFRunLoopRunInMode(
                        core_foundation_sys::runloop::kCFRunLoopDefaultMode,
                        0.05,
                        1,
                    );
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
            }
            Operation::Focus { .. } => {
                let running: *mut AnyObject = msg_send![class!(NSRunningApplication),runningApplicationWithProcessIdentifier:*pid as i32];
                if running.is_null() {
                    return Err("window process exited".into());
                }
                let activated: bool = msg_send![running,activateWithOptions:2usize];
                if !activated {
                    return Err("application refused activation".into());
                }
                let action = string("AXRaise");
                let code = AXUIElementPerformAction(e.0, action.0 as _);
                if code != 0 {
                    return Err(format!("AXRaise: Accessibility error {code}"));
                }
            }
            _ => unreachable!(),
        }
        Ok(Response::Applied {
            window: window(e.0, before.id, *pid, app.clone())?,
        })
    }

    unsafe fn displays(&mut self) -> Result<Response, String> {
        // Shared with Canvas, which reads the inventory in-process rather than
        // making this app a dependency of it knowing its own screens.
        Ok(Response::Displays {
            displays: canvas_control::displays::enumerate()?,
        })
    }

    unsafe fn tolerant_windows(&mut self) -> Result<Response, String> {
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let apps: *mut AnyObject = msg_send![workspace, runningApplications];
        let count: usize = msg_send![apps, count];
        let mut windows = Vec::new();
        let mut warnings = Vec::new();
        let mut alive = std::collections::HashSet::new();
        let mut unavailable = std::collections::HashSet::new();
        for i in 0..count {
            let app: *mut AnyObject = msg_send![apps,objectAtIndex:i];
            let policy: isize = msg_send![app, activationPolicy];
            if policy != 0 {
                continue;
            }
            let pid: i32 = msg_send![app, processIdentifier];
            let name: *mut AnyObject = msg_send![app, localizedName];
            let name = objtext(name);
            let fingerprint = match launch_fingerprint(app) {
                Ok(value) => value,
                Err(error) => {
                    unavailable.insert(pid as u32);
                    warnings.push(format!("Skipped {name} ({pid}): {error}"));
                    continue;
                }
            };
            let ax = Owned(AXUIElementCreateApplication(pid));
            AXUIElementSetMessagingTimeout(ax.0, 0.3);
            let list = match attr_raw(ax.0, "AXWindows") {
                Ok(list) => list,
                Err(-25212) => continue,
                Err(code) => {
                    unavailable.insert(pid as u32);
                    warnings.push(format!(
                        "Skipped {name} ({pid}): AXWindows Accessibility error {code}"
                    ));
                    continue;
                }
            };
            if CFGetTypeID(list.0) != CFArrayGetTypeID() {
                unavailable.insert(pid as u32);
                warnings.push(format!("Skipped {name} ({pid}): invalid AXWindows value"));
                continue;
            }
            for j in 0..CFArrayGetCount(list.0 as _) {
                let e = CFArrayGetValueAtIndex(list.0 as _, j);
                let id = self
                    .entries
                    .iter()
                    .find(|(_, (value, entry_pid, _, entry_fingerprint))| {
                        *entry_pid == pid as u32
                            && *entry_fingerprint == fingerprint
                            && CFEqual(value.0, e) != 0
                    })
                    .map(|(id, _)| id.clone())
                    .unwrap_or_else(|| {
                        self.next += 1;
                        format!("{}:{}", self.nonce, self.next)
                    });
                match window(e, id.clone(), pid as u32, name.clone()) {
                    Ok(found) => {
                        alive.insert(id.clone());
                        self.entries.entry(id).or_insert_with(|| {
                            (Owned(CFRetain(e)), pid as u32, name.clone(), fingerprint)
                        });
                        windows.push(found);
                    }
                    Err(error) => {
                        unavailable.insert(pid as u32);
                        warnings.push(format!("Skipped {name} ({pid}) window {j}: {error}"));
                    }
                }
            }
        }
        self.entries
            .retain(|id, (_, pid, _, _)| alive.contains(id) || unavailable.contains(pid));
        Ok(Response::Windows { windows, warnings })
    }

    unsafe fn resolve(&mut self, selectors: &[WindowSelector]) -> Result<Response, String> {
        if selectors.is_empty() || selectors.len() > 16 {
            return Err("Resolve requires 1–16 window selectors".into());
        }
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let apps: *mut AnyObject = msg_send![workspace, runningApplications];
        let app_count: usize = msg_send![apps, count];
        let mut resolved = Vec::new();
        let mut resolved_elements: Vec<Owned> = Vec::new();
        for selector in selectors {
            if selector.pid == 0
                || selector.app.is_empty()
                || selector.app.len() > 256
                || selector.title.len() > 4096
                || selector.app.chars().any(char::is_control)
                || selector.title.chars().any(char::is_control)
            {
                return Err("Invalid window selector".into());
            }
            if let Some((e, pid, app, fingerprint)) = self.entries.get(&selector.id) {
                let running: *mut AnyObject = msg_send![class!(NSRunningApplication),runningApplicationWithProcessIdentifier:*pid as i32];
                if app == &selector.app
                    && launch_fingerprint(running).is_ok_and(|value| value == *fingerprint)
                {
                    let found = window(e.0, selector.id.clone(), *pid, app.clone())?;
                    if resolved_elements
                        .iter()
                        .any(|resolved| CFEqual(resolved.0, e.0) != 0)
                    {
                        return Err("Multiple bindings resolve to the same window".into());
                    }
                    resolved_elements.push(Owned(CFRetain(e.0)));
                    resolved.push(found);
                    continue;
                }
            }

            let mut pid_match = Vec::new();
            let mut name_match = Vec::new();
            for i in 0..app_count {
                let app: *mut AnyObject = msg_send![apps,objectAtIndex:i];
                let policy: isize = msg_send![app, activationPolicy];
                if policy != 0 {
                    continue;
                }
                let pid: i32 = msg_send![app, processIdentifier];
                let name: *mut AnyObject = msg_send![app, localizedName];
                if objtext(name) == selector.app {
                    name_match.push(app);
                    if pid as u32 == selector.pid {
                        pid_match.push(app);
                    }
                }
            }
            let candidates = if pid_match.len() == 1 {
                &pid_match
            } else {
                &name_match
            };
            if candidates.len() != 1 {
                return Err(format!(
                    "Window app {} is missing or ambiguous; refresh project bindings",
                    selector.app
                ));
            }
            let app = candidates[0];
            let pid: i32 = msg_send![app, processIdentifier];
            let fingerprint = launch_fingerprint(app)?;
            let ax = Owned(AXUIElementCreateApplication(pid));
            AXUIElementSetMessagingTimeout(ax.0, 0.3);
            let list = attr(ax.0, "AXWindows")?;
            if CFGetTypeID(list.0) != CFArrayGetTypeID() {
                return Err(format!("{} returned invalid AXWindows", selector.app));
            }
            let mut windows = Vec::new();
            for j in 0..CFArrayGetCount(list.0 as _) {
                let e = CFArrayGetValueAtIndex(list.0 as _, j);
                let found = window(e, String::new(), pid as u32, selector.app.clone())?;
                windows.push((e, found));
            }
            let titles = windows
                .iter()
                .map(|(_, found)| found.title.clone())
                .collect::<Vec<_>>();
            let index = choose_window(&selector.title, &titles)
                .map_err(|error| format!("{}: {error}", selector.app))?;
            let (e, mut found) = windows.swap_remove(index);
            if resolved_elements
                .iter()
                .any(|resolved| CFEqual(resolved.0, e) != 0)
            {
                return Err("Multiple bindings resolve to the same window".into());
            }
            self.next += 1;
            let id = format!("{}:{}", self.nonce, self.next);
            found.id = id.clone();
            self.entries.insert(
                id.clone(),
                (
                    Owned(CFRetain(e)),
                    pid as u32,
                    selector.app.clone(),
                    fingerprint,
                ),
            );
            resolved_elements.push(Owned(CFRetain(e)));
            resolved.push(found);
        }
        Ok(Response::Resolved { windows: resolved })
    }

    unsafe fn snapshot(&mut self) -> Result<Response, String> {
        let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        let mut raw = Vec::new();
        for i in 0..count {
            let screen: *mut AnyObject = msg_send![screens,objectAtIndex:i];
            let frame: NSRect = msg_send![screen, frame];
            let visible: NSRect = msg_send![screen, visibleFrame];
            let name: *mut AnyObject = msg_send![screen, localizedName];
            let desc: *mut AnyObject = msg_send![screen, deviceDescription];
            let key = string("NSScreenNumber");
            let number: *mut AnyObject = msg_send![desc,objectForKey:key.0 as *mut AnyObject];
            let number: u32 = msg_send![number, unsignedIntValue];
            let uuid = Owned(CGDisplayCreateUUIDFromDisplayID(number) as _);
            let uuidtext = Owned(CFUUIDCreateString(ptr::null(), uuid.0 as _) as _);
            raw.push((text(uuidtext.0), objtext(name), frame, visible));
        }
        let primary_height = raw.first().map(|(_, _, f, _)| f.size.height).unwrap_or(0.);
        let rect = |r: NSRect| Rect {
            x: r.origin.x.round() as i32,
            y: (primary_height - r.origin.y - r.size.height).round() as i32,
            width: r.size.width.round() as u32,
            height: r.size.height.round() as u32,
        };
        let displays = raw
            .into_iter()
            .enumerate()
            .map(|(i, (id, name, f, v))| Display {
                id,
                name,
                frame: rect(f),
                visible_frame: rect(v),
                primary: i == 0,
            })
            .collect();
        let workspace: *mut AnyObject = msg_send![class!(NSWorkspace), sharedWorkspace];
        let apps: *mut AnyObject = msg_send![workspace, runningApplications];
        let count: usize = msg_send![apps, count];
        let mut windows = Vec::new();
        let mut alive = std::collections::HashSet::new();
        for i in 0..count {
            let app: *mut AnyObject = msg_send![apps,objectAtIndex:i];
            let policy: isize = msg_send![app, activationPolicy];
            if policy != 0 {
                continue;
            }
            let pid: i32 = msg_send![app, processIdentifier];
            let name: *mut AnyObject = msg_send![app, localizedName];
            let name = objtext(name);
            let fingerprint = launch_fingerprint(app)
                .map_err(|e| format!("Incomplete inventory for {name} ({pid}): {e}"))?;
            let ax = Owned(AXUIElementCreateApplication(pid));
            AXUIElementSetMessagingTimeout(ax.0, 0.3);
            let list = match attr_raw(ax.0, "AXWindows") {
                Ok(list) => list,
                Err(-25212) => continue, // A valid application reporting no value has no windows.
                Err(code) => return Err(format!("Incomplete inventory: {name} ({pid}) AXWindows error {code}; retry when the application responds or close it before reserving a display")),
            };
            if CFGetTypeID(list.0) != CFArrayGetTypeID() {
                return Err(format!(
                    "Incomplete inventory: {name} ({pid}) returned invalid AXWindows"
                ));
            }
            for j in 0..CFArrayGetCount(list.0 as _) {
                let e = CFArrayGetValueAtIndex(list.0 as _, j);
                let id = self
                    .entries
                    .iter()
                    .find(|(_, (v, p, _, f))| {
                        *p == pid as u32 && *f == fingerprint && CFEqual(v.0, e) != 0
                    })
                    .map(|(id, _)| id.clone())
                    .unwrap_or_else(|| {
                        self.next += 1;
                        format!("{}:{}", self.nonce, self.next)
                    });
                let w = window(e, id.clone(), pid as u32, name.clone()).map_err(|error| format!("Incomplete inventory: {name} ({pid}) window {j}: {error}; retry before reserving a display"))?;
                alive.insert(id.clone());
                self.entries
                    .entry(id)
                    .or_insert_with(|| (Owned(CFRetain(e)), pid as u32, name.clone(), fingerprint));
                windows.push(w);
            }
        }
        self.entries.retain(|id, _| alive.contains(id));
        Ok(Response::Snapshot { displays, windows })
    }
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[test]
    fn kernel_identity_is_stable_and_rejects_invalid_processes() {
        let pid = std::process::id() as i32;
        let first = process_fingerprint(pid).unwrap();
        assert_eq!(first, process_fingerprint(pid).unwrap());
        assert!(first > 0);
        assert!(process_fingerprint(0).is_err());
        assert!(process_fingerprint(-1).is_err());
    }
    #[test]
    fn targeted_resolution_requires_one_unambiguous_window() {
        assert_eq!(
            choose_window("Project", &["Project".into(), "Other".into()]),
            Ok(0)
        );
        assert_eq!(choose_window("Old title", &["New title".into()]), Ok(0));
        assert!(choose_window("Missing", &["One".into(), "Two".into()]).is_err());
        assert!(choose_window("Same", &["Same".into(), "Same".into()]).is_err());
    }
    #[test]
    #[ignore = "requires a logged-in macOS desktop with Finder running"]
    fn finder_identity_does_not_require_appkit_launch_date() {
        unsafe {
            objc2::rc::autoreleasepool(|_| {
                let finder = string("com.apple.finder");
                // CFString is toll-free bridged to NSString.
                let apps: *mut AnyObject = msg_send![class!(NSRunningApplication), runningApplicationsWithBundleIdentifier: finder.0 as *mut AnyObject];
                let count: usize = msg_send![apps, count];
                assert!(
                    count > 0,
                    "Finder must be running for this native regression test"
                );
                let app: *mut AnyObject = msg_send![apps, objectAtIndex: 0usize];
                assert!(launch_fingerprint(app).unwrap() > 0);
            });
        }
    }
}
