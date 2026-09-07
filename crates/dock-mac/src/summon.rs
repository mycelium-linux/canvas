//! Global Ctrl+Option+Space summon, registered with public Carbon hotkey APIs.
//!
//! Registration and destruction belong on the GUI main thread. The iced/AppKit
//! event loop dispatches the native callback; a GUI timer calls `poll_pressed`.
//! This does not observe arbitrary keyboard input or require Accessibility.
//! ABI and constants follow Apple's installed CarbonEvents{,Core}.h/Events.h.
#[cfg(target_os = "macos")]
mod platform {
    use std::{
        ffi::c_void,
        marker::PhantomData,
        ptr,
        rc::Rc,
        sync::atomic::{AtomicBool, Ordering},
    };

    type NativeRef = *mut c_void;
    const SIGNATURE: u32 = u32::from_be_bytes(*b"Dock");
    const HOTKEY_ID: u32 = 1;
    const KEYBOARD: u32 = u32::from_be_bytes(*b"keyb");
    const PRESSED: u32 = 5;
    const NOT_HANDLED: i32 = -9874;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct EventType {
        class: u32,
        kind: u32,
    }
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct HotkeyId {
        signature: u32,
        id: u32,
    }

    #[link(name = "Carbon", kind = "framework")]
    extern "C" {
        fn GetApplicationEventTarget() -> NativeRef;
        fn InstallEventHandler(
            target: NativeRef,
            handler: unsafe extern "C" fn(NativeRef, NativeRef, *mut c_void) -> i32,
            count: usize, // Carbon ItemCount is unsigned long, not UInt32.
            types: *const EventType,
            data: *mut c_void,
            result: *mut NativeRef,
        ) -> i32;
        fn RemoveEventHandler(handler: NativeRef) -> i32;
        fn RegisterEventHotKey(
            keycode: u32,
            modifiers: u32,
            id: HotkeyId,
            target: NativeRef,
            options: u32,
            result: *mut NativeRef,
        ) -> i32;
        fn UnregisterEventHotKey(hotkey: NativeRef) -> i32;
        fn GetEventParameter(
            event: NativeRef,
            name: u32,
            desired_type: u32,
            actual_type: *mut u32,
            buffer_size: usize, // Carbon ByteCount is unsigned long.
            actual_size: *mut usize,
            data: *mut c_void,
        ) -> i32;
    }
    extern "C" {
        fn pthread_main_np() -> i32;
    }

    fn record(pressed: &AtomicBool, id: HotkeyId) -> i32 {
        if id.signature != SIGNATURE || id.id != HOTKEY_ID {
            return NOT_HANDLED;
        }
        pressed.store(true, Ordering::Release);
        0
    }

    unsafe extern "C" fn callback(_next: NativeRef, event: NativeRef, data: *mut c_void) -> i32 {
        if data.is_null() || event.is_null() {
            return NOT_HANDLED;
        }
        let mut id = HotkeyId {
            signature: 0,
            id: 0,
        };
        // SAFETY: Carbon invokes this only for our keyboard/hotkey-pressed
        // registration; it writes a fixed-size EventHotKeyID into our buffer.
        let result = unsafe {
            GetEventParameter(
                event,
                u32::from_be_bytes(*b"----"),
                u32::from_be_bytes(*b"hkid"),
                ptr::null_mut(),
                std::mem::size_of::<HotkeyId>(),
                ptr::null_mut(),
                &mut id as *mut HotkeyId as *mut c_void,
            )
        };
        if result != 0 {
            return NOT_HANDLED;
        }
        // SAFETY: the stable Box remains alive until its handler is removed.
        record(unsafe { &*data.cast::<AtomicBool>() }, id)
    }

    pub struct SummonHotkey {
        hotkey: NativeRef,
        handler: NativeRef,
        pressed: Option<Box<AtomicBool>>,
        // Carbon/AppKit registration and cleanup may not move off the main thread.
        _main_thread: PhantomData<Rc<()>>,
    }

    impl SummonHotkey {
        pub fn register() -> Result<Self, String> {
            // SAFETY: pthread_main_np has no preconditions.
            if unsafe { pthread_main_np() } == 0 {
                return Err("The summon shortcut must be registered on the GUI main thread".into());
            }
            let mut owner = Self {
                hotkey: ptr::null_mut(),
                handler: ptr::null_mut(),
                pressed: Some(Box::new(AtomicBool::new(false))),
                _main_thread: PhantomData,
            };
            let events = EventType {
                class: KEYBOARD,
                kind: PRESSED,
            };
            // SAFETY: all refs/buffers remain valid for these calls. The callback
            // context is heap allocated and remains stable if `owner` moves.
            unsafe {
                let target = GetApplicationEventTarget();
                if target.is_null() {
                    return Err("The macOS application event target is unavailable".into());
                }
                let result = InstallEventHandler(
                    target,
                    callback,
                    1,
                    &events,
                    owner.pressed.as_deref_mut().unwrap() as *mut AtomicBool as *mut c_void,
                    &mut owner.handler,
                );
                if result != 0 {
                    return Err(format!(
                        "Could not install summon callback (macOS error {result})"
                    ));
                }
                let result = RegisterEventHotKey(
                    0x31,                  // kVK_Space
                    (1 << 11) | (1 << 12), // optionKey | controlKey
                    HotkeyId {
                        signature: SIGNATURE,
                        id: HOTKEY_ID,
                    },
                    target,
                    0,
                    &mut owner.hotkey,
                );
                if result != 0 {
                    return Err(format!("Ctrl+Option+Space is unavailable (macOS error {result}); another application or system shortcut may own it"));
                }
            }
            Ok(owner)
        }

        /// Returns true once for one or more native presses since the last poll.
        pub fn poll_pressed(&self) -> bool {
            self.pressed
                .as_ref()
                .is_some_and(|pressed| pressed.swap(false, Ordering::AcqRel))
        }
    }

    impl Drop for SummonHotkey {
        fn drop(&mut self) {
            // SAFETY: this !Send owner can only be dropped on its registering
            // thread. Remove the native handler before freeing its callback data.
            unsafe {
                if !self.hotkey.is_null() {
                    let result = UnregisterEventHotKey(self.hotkey);
                    if result != 0 {
                        eprintln!("Dock: summon shortcut cleanup failed (macOS error {result})");
                    }
                }
                if !self.handler.is_null() {
                    let result = RemoveEventHandler(self.handler);
                    if result != 0 {
                        eprintln!("Dock: summon callback cleanup failed (macOS error {result})");
                        // A still-installed callback must never reference freed
                        // memory. Preserve its tiny context until process exit.
                        if let Some(pressed) = self.pressed.take() {
                            let _ = Box::leak(pressed);
                        }
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn unrelated_hotkeys_do_not_summon_and_presses_coalesce() {
            let pressed = AtomicBool::new(false);
            assert_eq!(
                record(
                    &pressed,
                    HotkeyId {
                        signature: 0,
                        id: HOTKEY_ID
                    }
                ),
                NOT_HANDLED
            );
            assert!(!pressed.load(Ordering::Acquire));
            assert_eq!(
                record(
                    &pressed,
                    HotkeyId {
                        signature: SIGNATURE,
                        id: 2
                    }
                ),
                NOT_HANDLED
            );
            assert_eq!(
                record(
                    &pressed,
                    HotkeyId {
                        signature: SIGNATURE,
                        id: HOTKEY_ID
                    }
                ),
                0
            );
            assert_eq!(
                record(
                    &pressed,
                    HotkeyId {
                        signature: SIGNATURE,
                        id: HOTKEY_ID
                    }
                ),
                0
            );
            assert!(pressed.swap(false, Ordering::AcqRel));
            assert!(!pressed.swap(false, Ordering::AcqRel));
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    pub struct SummonHotkey;
    impl SummonHotkey {
        pub fn register() -> Result<Self, String> {
            Err("The global project summon shortcut requires macOS".into())
        }
        pub fn poll_pressed(&self) -> bool {
            false
        }
    }
}
pub use platform::SummonHotkey;
