//! A small cross-platform owner for one application-wide shortcut.
//!
//! macOS uses the public Carbon hotkey API, which does not require Accessibility
//! permission. Other platforms return an explicit unsupported error so their
//! compositor-specific implementations can satisfy the same interface later.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HotkeySpec {
    pub signature: [u8; 4],
    pub id: u32,
    pub key_code: u32,
    pub modifiers: u32,
    pub label: &'static str,
}

#[cfg(target_os = "macos")]
mod platform {
    use super::HotkeySpec;
    use std::{
        ffi::c_void,
        marker::PhantomData,
        ptr,
        rc::Rc,
        sync::atomic::{AtomicBool, Ordering},
    };

    type NativeRef = *mut c_void;
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

    struct CallbackState {
        pressed: AtomicBool,
        expected: HotkeyId,
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn GetApplicationEventTarget() -> NativeRef;
        fn InstallEventHandler(
            target: NativeRef,
            handler: unsafe extern "C" fn(NativeRef, NativeRef, *mut c_void) -> i32,
            count: usize,
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
            buffer_size: usize,
            actual_size: *mut usize,
            data: *mut c_void,
        ) -> i32;
        fn pthread_main_np() -> i32;
    }

    fn record(state: &CallbackState, id: HotkeyId) -> i32 {
        if id.signature != state.expected.signature || id.id != state.expected.id {
            return NOT_HANDLED;
        }
        state.pressed.store(true, Ordering::Release);
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
        // SAFETY: Carbon invokes this callback for the registered keyboard
        // event and writes one fixed-size EventHotKeyID into `id`.
        let result = unsafe {
            GetEventParameter(
                event,
                u32::from_be_bytes(*b"----"),
                u32::from_be_bytes(*b"hkid"),
                ptr::null_mut(),
                std::mem::size_of::<HotkeyId>(),
                ptr::null_mut(),
                (&mut id as *mut HotkeyId).cast(),
            )
        };
        if result != 0 {
            return NOT_HANDLED;
        }
        // SAFETY: the boxed callback state is retained until the handler is removed.
        record(unsafe { &*data.cast::<CallbackState>() }, id)
    }

    pub struct GlobalHotkey {
        hotkey: NativeRef,
        handler: NativeRef,
        state: Option<Box<CallbackState>>,
        label: &'static str,
        _main_thread: PhantomData<Rc<()>>,
    }

    impl GlobalHotkey {
        pub fn register(spec: HotkeySpec) -> Result<Self, String> {
            if spec.id == 0 {
                return Err("global hotkey id must be nonzero".into());
            }
            // SAFETY: pthread_main_np has no preconditions.
            if unsafe { pthread_main_np() } == 0 {
                return Err(format!(
                    "{} must be registered on the GUI main thread",
                    spec.label
                ));
            }
            let expected = HotkeyId {
                signature: u32::from_be_bytes(spec.signature),
                id: spec.id,
            };
            let mut owner = Self {
                hotkey: ptr::null_mut(),
                handler: ptr::null_mut(),
                state: Some(Box::new(CallbackState {
                    pressed: AtomicBool::new(false),
                    expected,
                })),
                label: spec.label,
                _main_thread: PhantomData,
            };
            let event = EventType {
                class: KEYBOARD,
                kind: PRESSED,
            };
            // SAFETY: native refs and the boxed callback state remain valid for
            // the registration lifetime and cleanup occurs on this thread.
            unsafe {
                let target = GetApplicationEventTarget();
                if target.is_null() {
                    return Err("the macOS application event target is unavailable".into());
                }
                let result = InstallEventHandler(
                    target,
                    callback,
                    1,
                    &event,
                    owner.state.as_deref_mut().unwrap() as *mut CallbackState as *mut c_void,
                    &mut owner.handler,
                );
                if result != 0 {
                    return Err(format!(
                        "could not install {} callback (macOS error {result})",
                        spec.label
                    ));
                }
                let result = RegisterEventHotKey(
                    spec.key_code,
                    spec.modifiers,
                    expected,
                    target,
                    0,
                    &mut owner.hotkey,
                );
                if result != 0 {
                    return Err(format!(
                        "{} is unavailable (macOS error {result})",
                        spec.label
                    ));
                }
            }
            Ok(owner)
        }

        pub fn poll_pressed(&self) -> bool {
            self.state
                .as_ref()
                .is_some_and(|state| state.pressed.swap(false, Ordering::AcqRel))
        }
    }

    impl Drop for GlobalHotkey {
        fn drop(&mut self) {
            // SAFETY: this !Send owner is dropped on the registration thread.
            unsafe {
                if !self.hotkey.is_null() {
                    let _ = UnregisterEventHotKey(self.hotkey);
                }
                if !self.handler.is_null() && RemoveEventHandler(self.handler) != 0 {
                    eprintln!("{} callback cleanup failed", self.label);
                    if let Some(state) = self.state.take() {
                        let _ = Box::leak(state);
                    }
                }
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_the_registered_identity_records_a_press() {
            let state = CallbackState {
                pressed: AtomicBool::new(false),
                expected: HotkeyId {
                    signature: 7,
                    id: 2,
                },
            };
            assert_eq!(
                record(
                    &state,
                    HotkeyId {
                        signature: 7,
                        id: 1
                    }
                ),
                NOT_HANDLED
            );
            assert!(!state.pressed.load(Ordering::Acquire));
            assert_eq!(record(&state, state.expected), 0);
            assert!(state.pressed.swap(false, Ordering::AcqRel));
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::HotkeySpec;

    pub struct GlobalHotkey;

    impl GlobalHotkey {
        pub fn register(spec: HotkeySpec) -> Result<Self, String> {
            Err(format!(
                "{} is not implemented on this platform",
                spec.label
            ))
        }

        pub fn poll_pressed(&self) -> bool {
            false
        }
    }
}

pub use platform::GlobalHotkey;
