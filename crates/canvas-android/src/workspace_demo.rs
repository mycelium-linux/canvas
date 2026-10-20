//! Android owns EGL and event delivery; the first-party Rust shell owns all mock behavior.
use canvas_workspace_ui::shell::Shell;
use egui_glow::{Painter, glow};
use jni::{
    JNIEnv,
    objects::{JClass, JString},
    sys::{jdouble, jint, jlong, jstring},
};
use std::{
    cell::RefCell,
    ffi::{CString, c_char, c_void},
    sync::{
        Arc,
        atomic::{AtomicI64, Ordering},
    },
};

#[link(name = "EGL")]
unsafe extern "C" {
    fn eglGetProcAddress(name: *const c_char) -> *const c_void;
    fn eglGetCurrentContext() -> *mut c_void;
}
#[link(name = "GLESv3")]
unsafe extern "C" {
    fn glGetString(name: u32) -> *const u8;
}
#[link(name = "dl")]
unsafe extern "C" {
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *const c_void;
}
struct Renderer {
    id: i64,
    egl_context: usize,
    context: egui::Context,
    painter: Painter,
    shell: Shell,
}
thread_local! { static RENDERER: RefCell<Option<Renderer>> = const { RefCell::new(None) }; }
static GENERATION: AtomicI64 = AtomicI64::new(1);
fn fail(env: &mut JNIEnv<'_>, error: String) {
    let _ = env.throw_new("java/lang/IllegalStateException", error);
}
fn string(env: &JNIEnv<'_>, text: String) -> Result<jstring, String> {
    env.new_string(text)
        .map(|s| s.into_raw())
        .map_err(|e| e.to_string())
}
fn with_renderer<T>(
    id: i64,
    f: impl FnOnce(&mut Renderer) -> Result<T, String>,
) -> Result<T, String> {
    RENDERER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let renderer = slot
            .as_mut()
            .filter(|r| r.id == id)
            .ok_or("Stale workspace GPU generation")?;
        f(renderer)
    })
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_WorkspaceDemoView_nativeCreate(
    mut env: JNIEnv<'_>,
    _: JClass<'_>,
    saved: JString<'_>,
) -> jlong {
    let result = (|| -> Result<i64, String> {
        let saved: String = env.get_string(&saved).map_err(|e| e.to_string())?.into();
        let mut shell = if saved.is_empty() {
            Shell::demo()?
        } else {
            Shell::restore_preview(&saved)?
        };
        shell.input_hint = Some("TV: Up/Down conversations | OK open | Left/Right focus | FF archive | RW undo | Play select | CH+/- scroll | Back unfocus | MENU workspaces".into());
        // SAFETY: GLSurfaceView calls this only with its own current GLES3 context.
        // No GL pointer, painter or context crosses its GL thread or generation.
        let gl = unsafe {
            if glGetString(glow::VERSION).is_null() {
                return Err("No current GLES context".into());
            }
            glow::Context::from_loader_function(|name| {
                let Ok(name) = CString::new(name) else {
                    return std::ptr::null();
                };
                let pointer = eglGetProcAddress(name.as_ptr());
                if pointer.is_null() {
                    dlsym(std::ptr::null_mut(), name.as_ptr())
                } else {
                    pointer
                }
            })
        };
        let painter = Painter::new(Arc::new(gl), "", None, false).map_err(|e| e.to_string())?;
        let id = GENERATION.fetch_add(1, Ordering::Relaxed);
        RENDERER.with(|slot| {
            *slot.borrow_mut() = Some(Renderer {
                id,
                // SAFETY: creation is called on GLSurfaceView's current GL thread.
                egl_context: unsafe { eglGetCurrentContext() } as usize,
                context: egui::Context::default(),
                painter,
                shell,
            });
        });
        Ok(id)
    })();
    match result {
        Ok(id) => id,
        Err(error) => {
            fail(&mut env, error);
            0
        }
    }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_WorkspaceDemoView_nativeFrame(
    mut env: JNIEnv<'_>,
    _: JClass<'_>,
    id: jlong,
    width: jint,
    height: jint,
    time: jdouble,
    now: jlong,
    events: JString<'_>,
) -> jstring {
    let result = (|| -> Result<String, String> {
        if !(1..=4096).contains(&width)
            || !(1..=2160).contains(&height)
            || !time.is_finite()
            || now < 0
        {
            return Err("Invalid workspace frame geometry or clock".into());
        }
        let events: String = env.get_string(&events).map_err(|e| e.to_string())?.into();
        if events.len() > 16384 {
            return Err("Workspace input exceeds bound".into());
        }
        let events: Vec<serde_json::Value> =
            serde_json::from_str(&events).map_err(|e| e.to_string())?;
        if events.len() > 128 {
            return Err("Too many workspace inputs".into());
        }
        with_renderer(id, |r| {
            // SAFETY: querying EGL's thread-local current handle does not transfer ownership.
            if unsafe { eglGetCurrentContext() } as usize != r.egl_context {
                return Err("Workspace EGL context changed".into());
            }
            let scale = (width as f32 / 1600.0).clamp(0.75, 2.5);
            r.context.set_pixels_per_point(scale);
            r.shell.owner.advance_to(now as u64);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width as f32 / scale, height as f32 / scale),
                )),
                time: Some(time),
                focused: true,
                ..Default::default()
            };
            for event in events {
                if let Some(name) = event.get("key").and_then(|v| v.as_str()) {
                    let key = if name == "Next" || name == "Previous" {
                        // egui reserves raw arrows for focus traversal before widgets run.
                        // Browse mail with J/K until a control explicitly owns focus.
                        if r.context.memory(|m| m.focused().is_none()) {
                            if name == "Next" {
                                egui::Key::J
                            } else {
                                egui::Key::K
                            }
                        } else if name == "Next" {
                            egui::Key::ArrowDown
                        } else {
                            egui::Key::ArrowUp
                        }
                    } else if name == "Activate" {
                        if r.context.memory(|m| m.focused().is_some()) {
                            egui::Key::Space
                        } else {
                            egui::Key::Enter
                        }
                    } else {
                        egui::Key::from_name(name).ok_or("Unknown workspace key")?
                    };
                    let modifiers = egui::Modifiers {
                        shift: event
                            .get("shift")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        command: event
                            .get("command")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false),
                        ..Default::default()
                    };
                    for pressed in [true, false] {
                        input.events.push(egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed,
                            repeat: false,
                            modifiers,
                        });
                    }
                } else if let Some(text) = event.get("text").and_then(|v| v.as_str()) {
                    if text.len() > 256 {
                        return Err("Workspace text exceeds bound".into());
                    }
                    input.events.push(egui::Event::Text(text.into()));
                } else if let Some(scroll) = event.get("scroll").and_then(|v| v.as_f64()) {
                    if !scroll.is_finite() || scroll.abs() > 1000.0 {
                        return Err("Invalid workspace scroll".into());
                    }
                    input.events.push(egui::Event::PointerMoved(egui::pos2(
                        width as f32 / scale * 0.8,
                        height as f32 / scale * 0.5,
                    )));
                    input.events.push(egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, scroll as f32),
                        modifiers: egui::Modifiers::NONE,
                    });
                } else if let (Some(x), Some(y), Some(pressed)) = (
                    event.get("x").and_then(|v| v.as_f64()),
                    event.get("y").and_then(|v| v.as_f64()),
                    event.get("pressed").and_then(|v| v.as_bool()),
                ) {
                    if !x.is_finite() || !y.is_finite() {
                        return Err("Invalid workspace pointer".into());
                    }
                    let pos = egui::pos2(x as f32 / scale, y as f32 / scale);
                    input.events.push(egui::Event::PointerMoved(pos));
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                } else {
                    return Err("Unknown workspace input".into());
                }
            }
            let output = r.context.run(input, |ctx| r.shell.show(ctx));
            let primitives = r.context.tessellate(output.shapes, output.pixels_per_point);
            r.painter
                .clear([width as u32, height as u32], [0.0, 0.0, 0.0, 1.0]);
            r.painter.paint_and_update_textures(
                [width as u32, height as u32],
                output.pixels_per_point,
                &primitives,
                &output.textures_delta,
            );
            let selected = r
                .shell
                .mail
                .selected
                .as_ref()
                .and_then(|key| r.shell.owner.threads.iter().find(|t| &t.id == key))
                .map(|t| t.subject.as_str())
                .unwrap_or("none");
            Ok(format!(
                "Canvas mock workspace; native egui GLES; selected: {selected}; unread: {}; drafts: {}; notice: {}",
                r.shell
                    .owner
                    .threads
                    .iter()
                    .filter(|t| t.unread && !t.archived && !t.snoozed)
                    .count(),
                r.shell.owner.drafts.len(),
                r.shell.mail.notice.as_deref().unwrap_or("none")
            ))
        })
    })();
    match result.and_then(|text| string(&env, text)) {
        Ok(text) => text,
        Err(error) => {
            fail(&mut env, error);
            std::ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_WorkspaceDemoView_nativeCheckpoint(
    mut env: JNIEnv<'_>,
    _: JClass<'_>,
    id: jlong,
) -> jstring {
    match with_renderer(id, |r| r.shell.save_preview()).and_then(|text| string(&env, text)) {
        Ok(text) => text,
        Err(error) => {
            fail(&mut env, error);
            std::ptr::null_mut()
        }
    }
}
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_WorkspaceDemoView_nativeDestroy(
    mut env: JNIEnv<'_>,
    _: JClass<'_>,
    id: jlong,
) {
    let result = with_renderer(id, |r| {
        // A queued detach can run after Android has unbound EGL. Only delete
        // GL objects in their own context; Android reclaims them with that context otherwise.
        if unsafe { eglGetCurrentContext() } as usize == r.egl_context {
            r.painter.destroy();
        }
        Ok(())
    });
    match result {
        Ok(()) => RENDERER.with(|slot| *slot.borrow_mut() = None),
        Err(error) => fail(&mut env, error),
    }
}
