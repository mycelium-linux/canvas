//! Smithay Wayland server that makes canvas-linux the WM itself.
//!
//! Compiled only with the `compositor` feature. A server-side Wayland
//! compositor: `wl_compositor` + `xdg-shell` + `wl_seat` + `wl_shm`, a dispatch
//! loop, and a [`CanvasCompositor`] state model that raises [`WindowLifecycle`]
//! events. `canvas-linux desktop-compositor start` runs it, replacing the
//! headless-Sway desktop with Canvas owning the seat.
//!
//! KMS/DRM output: when a DRM card with a connected connector is available,
//! an EGL/GLES renderer on a GBM swapchain drives the first CRTC through a
//! `DrmCompositor` (self-managing device; pageflip completion is drained via
//! `frame_submitted` on the same 16ms tick). Mapped xdg toplevels composite
//! at their [`CanvasCompositor`] frame rectangles. Keyboard and pointer state
//! can be injected through the owner-only control socket. Without a usable DRM card the compositor runs headless
//! (control socket only), unchanged from the previous behavior.

use crate::compositor::CanvasCompositor;
use canvas_core::desktop::{Display as CanvasDisplay, WindowBackend, WindowSurface};
use canvas_core::layout::Rect;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::{
    backend::input::{ButtonState, KeyState, Keycode},
    delegate_compositor, delegate_data_device, delegate_dmabuf, delegate_output, delegate_seat,
    delegate_shm, delegate_xdg_shell,
    desktop::utils::send_frames_surface_tree,
    input::{
        Seat, SeatHandler, SeatState,
        keyboard::{FilterResult, KeyboardHandle},
        pointer::{ButtonEvent, MotionEvent, PointerHandle},
    },
    output::{Mode as WlMode, Output, PhysicalProperties, Subpixel},
    reexports::wayland_server::{
        Client, Display, DisplayHandle, ListeningSocket,
        backend::{ClientData, ClientId, DisconnectReason},
        protocol::wl_buffer,
        protocol::wl_seat,
        protocol::wl_surface::WlSurface,
    },
    utils::{Logical, Physical, Point, SERIAL_COUNTER, Serial, Size, Transform},
    wayland::{
        buffer::BufferHandler,
        compositor::{CompositorClientState, CompositorHandler, CompositorState},
        dmabuf::{DmabufFeedbackBuilder, DmabufGlobal, DmabufHandler, DmabufState, ImportNotifier},
        output::{OutputHandler, OutputManagerState},
        selection::SelectionHandler,
        selection::data_device::{
            ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler,
        },
        shell::xdg::{
            PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState,
        },
        shm::{ShmHandler, ShmState},
    },
};

use smithay::backend::{
    allocator::{
        Format, Fourcc,
        dmabuf::Dmabuf,
        gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
    },
    drm::{
        DrmDevice, DrmDeviceFd, DrmNode, NodeType,
        compositor::{DrmCompositor, FrameFlags},
        exporter::gbm::GbmFramebufferExporter,
    },
    egl::{EGLContext, EGLDisplay},
    renderer::{
        Color32F, ImportDma,
        element::{
            Kind,
            surface::{WaylandSurfaceRenderElement, render_elements_from_surface_tree},
        },
        gles::GlesRenderer,
    },
};
use smithay::reexports::calloop::EventLoop as DrmEventLoop;
use smithay::reexports::drm::control::{Device as ControlDevice, ModeTypeFlags, connector, crtc};
use smithay::utils::DeviceFd as SmsDeviceFd;

pub struct RunnerStatus {
    pub workspace: Option<String>,
    pub window_count: u32,
    pub wayland_display: Option<String>,
}

fn state_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".local/state/dock/compositor.json"))
}

fn write_state(path: &PathBuf, workspace: Option<&str>, window_count: u32, display: Option<&str>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(
        path,
        json!({
            "workspace": workspace,
            "window_count": window_count,
            "wayland_display": display,
        })
        .to_string(),
    );
}

pub fn status() -> Option<RunnerStatus> {
    let path = state_path()?;
    let raw = std::fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(RunnerStatus {
        workspace: v["workspace"].as_str().map(|s| s.to_string()),
        window_count: v["window_count"].as_u64().map(|n| n as u32).unwrap_or(0),
        wayland_display: v["wayland_display"].as_str().map(|s| s.to_string()),
    })
}

pub fn run() -> Result<Value, String> {
    run_with_output(false)
}

pub fn run_headless() -> Result<Value, String> {
    run_with_output(true)
}

fn run_with_output(headless: bool) -> Result<Value, String> {
    let path = if headless {
        PathBuf::from(
            std::env::var_os("XDG_RUNTIME_DIR")
                .ok_or("headless mode requires a private XDG_RUNTIME_DIR")?,
        )
        .join("compositor.json")
    } else {
        state_path().ok_or("HOME not set; cannot locate compositor state file")?
    };
    let mut display: Display<App> = Display::new().map_err(|e| format!("wayland Display: {e}"))?;
    let dh = display.handle();

    let compositor_state = CompositorState::new::<App>(&dh);
    let shm_state = ShmState::new::<App>(&dh, Vec::new());
    let mut seat_state = SeatState::new();
    let mut seat = seat_state.new_wl_seat(&dh, "canvas");
    let output_manager = OutputManagerState::new_with_xdg_output::<App>(&dh);
    let data_device_state = DataDeviceState::new::<App>(&dh);

    let displays = vec![CanvasDisplay {
        id: "HEADLESS-1".into(),
        name: "HEADLESS-1".into(),
        frame: Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        },
        work_area: Rect {
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
        },
        primary: true,
    }];
    // When DOCK_CANVAS_STREAM points at a stream.sock path, window
    // map/focus/close events are published as datagrams for retained-Lua
    // binders. Otherwise the compositor runs open-loop (no producer).
    let mut canvas = match std::env::var("DOCK_CANVAS_STREAM") {
        Ok(path) => CanvasCompositor::with_lifecycle(
            displays,
            Box::new(crate::compositor::StreamLifecycle::new(Some(
                std::path::PathBuf::from(path),
            ))),
        ),
        Err(_) => CanvasCompositor::new(displays),
    };

    let keyboard = seat
        .add_keyboard(Default::default(), 200, 200)
        .map_err(|e| format!("add_keyboard: {e}"))?;
    let pointer = seat.add_pointer();

    let control_directory = path
        .parent()
        .unwrap_or(Path::new("/tmp"))
        .join("native-control");
    use std::os::unix::fs::DirBuilderExt;
    match std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&control_directory)
    {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("create private control directory: {error}")),
    }
    let control_socket = control_directory.join("compositor-control.sock");
    let listener = crate::control_transport::bind(&control_socket)
        .map_err(|e| format!("bind control socket: {e}"))?;
    let (cmd_tx, cmd_rx) = std::sync::mpsc::sync_channel::<(Value, std::sync::mpsc::Sender<Value>)>(
        crate::control_transport::QUEUE_CAPACITY,
    );
    let launcher = crate::launcher::Launcher::new();

    let mut seat_access = if headless {
        None
    } else {
        Some(crate::seat_access::SeatAccess::open()?)
    };
    let drm = if headless {
        None
    } else {
        Some(
            DrmRender::try_new(
                &dh,
                &mut seat_access.as_mut().expect("hardware seat").session,
            )
            .ok_or("seat acquired but no usable DRM output; refusing invisible hardware desktop")?,
        )
    };
    let mut dmabuf_state = DmabufState::new();
    let dmabuf_global = drm.as_ref().and_then(|drm| {
        let formats = drm.renderer.dmabuf_formats();
        if formats.iter().next().is_none() {
            eprintln!("canvas-linux: GLES renderer exposes no dmabuf formats; wl_shm only");
            return None;
        }
        let feedback = match DmabufFeedbackBuilder::new(drm.render_node.dev_id(), formats).build() {
            Ok(feedback) => feedback,
            Err(error) => {
                eprintln!("canvas-linux: cannot build dmabuf feedback: {error}");
                return None;
            }
        };
        let global = dmabuf_state.create_global_with_default_feedback::<App>(&dh, &feedback);
        eprintln!("canvas-linux: linux-dmabuf enabled on {}", drm.render_node);
        Some(global)
    });
    if let Some(d) = &drm {
        // The canvas state model tracks the real panel geometry once known.
        let size = d.size;
        let frame = Rect {
            x: 0,
            y: 0,
            width: size.w.max(0) as u32,
            height: size.h.max(0) as u32,
        };
        canvas.set_outputs(vec![CanvasDisplay {
            id: "HDMI-1".into(),
            name: "HDMI-1".into(),
            frame,
            work_area: frame,
            primary: true,
        }]);
    } else {
        eprintln!("canvas-linux: no DRM output available; running headless");
    }

    let virtual_output = if headless {
        let output = Output::new(
            "canvas-test-output".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Canvas".into(),
                model: "isolated protocol test".into(),
            },
        );
        output.create_global::<App>(&dh);
        output.change_current_state(
            Some(WlMode {
                size: (1280, 720).into(),
                refresh: 60000,
            }),
            Some(Transform::Normal),
            None,
            Some((0, 0).into()),
        );
        Some(output)
    } else {
        None
    };
    let started = Instant::now();
    let mut state = App {
        virtual_output,
        canvas,
        compositor_state,
        xdg_shell_state: XdgShellState::new::<App>(&dh),
        shm_state,
        seat_state,
        data_device_state,
        output_manager,
        seat,
        keyboard,
        pointer,
        remote_input: RemoteInput::default(),
        seat_active: true,
        switcher: crate::window_switcher::WindowSwitcher::default(),
        toplevel_map: Vec::new(),
        next_id: 0u64,
        window_count: 0,
        state_path: path,
        cmd_rx,
        launcher,
        drm,
        dmabuf_state,
        dmabuf_global,
    };

    // Worker thread: accept control-socket clients and forward commands to the
    // compositor loop via the channel above. The loop drains cmd_rx and applies
    // commands to its (single-threaded) CanvasCompositor.
    let control_worker =
        std::thread::spawn(move || crate::launcher::serve_control(listener, cmd_tx));

    let wayland_display = "wayland-5".to_string();
    unsafe { std::env::set_var("WAYLAND_DISPLAY", &wayland_display) };
    write_state(
        &state.state_path,
        state.canvas.workspace(),
        0,
        Some(&wayland_display),
    );

    let listener =
        ListeningSocket::bind(&wayland_display).map_err(|e| format!("bind wayland socket: {e}"))?;
    let mut clients = Vec::new();
    // Headless tests must never read the workstation's physical seat.
    let mut physical_input = if headless {
        None
    } else {
        Some(crate::physical_input::open(
            seat_access.as_ref().expect("hardware seat").session.clone(),
        )?)
    };
    let mut physical_keys = Vec::<u16>::new();
    let mut physical_buttons = 0u8;

    loop {
        if let Some(access) = &mut seat_access {
            for event in access.dispatch()? {
                use smithay::backend::session::Event;
                match event {
                    Event::PauseSession => {
                        state.release_input();
                        state.seat_active = false;
                        physical_keys.clear();
                        physical_buttons = 0;
                        if let Some(input) = &mut physical_input {
                            input.suspend();
                        }
                        if let Some(drm) = &mut state.drm {
                            drm.device.pause();
                        }
                    }
                    Event::ActivateSession => {
                        if let Some(drm) = &mut state.drm {
                            drm.device
                                .activate(false)
                                .map_err(|error| format!("DRM seat resume: {error}"))?;
                            drm.compositor
                                .reset_state()
                                .map_err(|error| format!("DRM resume state: {error}"))?;
                            drm.flip_pending = false;
                        }
                        if let Some(input) = &mut physical_input {
                            input.resume().map_err(|()| "libinput seat resume failed")?;
                        }
                        state.seat_active = true;
                        if let Some(id) = state.canvas.focused().map(str::to_owned) {
                            state.focus_native(&id)?;
                        }
                    }
                }
            }
        }
        if control_worker.is_finished() {
            return Err("control worker stopped; refusing to run an uncontrollable desktop".into());
        }
        if let Ok(Some(stream)) = listener.accept()
            && let Ok(client) = display
                .handle()
                .insert_client(stream, Arc::new(ClientState::default()))
        {
            clients.push(client);
        }

        display
            .dispatch_clients(&mut state)
            .map_err(|e| format!("dispatch: {e}"))?;
        display.flush_clients().map_err(|e| format!("flush: {e}"))?;

        // Drain control-socket commands from the accept-loop worker and apply
        // them to this thread's CanvasCompositor. Single-threaded; no locking.
        for _ in 0..crate::control_transport::QUEUE_CAPACITY {
            let Ok((cmd, reply_tx)) = state.cmd_rx.try_recv() else {
                break;
            };
            let resp = state.control(&cmd);
            let _ = reply_tx.send(resp);
        }
        state.expire_remote_input();
        if let Some(input) = &mut physical_input
            && state.seat_active
        {
            input
                .dispatch()
                .map_err(|error| format!("physical input dispatch: {error}"))?;
            for event in input.by_ref().take(128) {
                use input::event::keyboard::KeyboardEventTrait;
                use input::event::{KeyboardEvent, PointerEvent};
                let mut position = state.remote_input.pointer;
                match event {
                    input::Event::Device(input::event::DeviceEvent::Removed(_)) => {
                        // Never strand held state when a keyboard or mouse disappears.
                        physical_keys.clear();
                        physical_buttons = 0;
                        if !state.remote_input.local_owner {
                            continue;
                        }
                    }
                    input::Event::Keyboard(KeyboardEvent::Key(event)) => {
                        let Some(usage) =
                            (0..=255).find(|usage| hid_to_evdev(*usage) == Some(event.key()))
                        else {
                            eprintln!("physical input: unsupported evdev key {}", event.key());
                            continue;
                        };
                        if event.key_state() == input::event::keyboard::KeyState::Pressed {
                            if !physical_keys.contains(&usage) {
                                physical_keys.push(usage);
                            }
                        } else {
                            physical_keys.retain(|key| *key != usage);
                        }
                    }
                    input::Event::Pointer(PointerEvent::Motion(event)) => {
                        let area = state
                            .canvas
                            .displays()
                            .first()
                            .map(|display| display.work_area);
                        let Some(area) = area else {
                            continue;
                        };
                        position.0 = (f64::from(position.0)
                            + event.dx() * 65535.0 / f64::from(area.width.max(1)))
                        .clamp(0.0, 65535.0) as u16;
                        position.1 = (f64::from(position.1)
                            + event.dy() * 65535.0 / f64::from(area.height.max(1)))
                        .clamp(0.0, 65535.0) as u16;
                    }
                    input::Event::Pointer(PointerEvent::MotionAbsolute(event)) => {
                        position = (
                            event.absolute_x_transformed(65535).clamp(0.0, 65535.0) as u16,
                            event.absolute_y_transformed(65535).clamp(0.0, 65535.0) as u16,
                        );
                    }
                    input::Event::Pointer(PointerEvent::ScrollWheel(event)) => {
                        use input::event::pointer::PointerScrollEvent;
                        state.local_input(&physical_keys, physical_buttons, position);
                        let pointer = state.pointer.clone();
                        let mut frame = smithay::input::pointer::AxisFrame::new(monotonic_ms())
                            .source(smithay::backend::input::AxisSource::Wheel);
                        for (axis, target) in [
                            (
                                input::event::pointer::Axis::Vertical,
                                smithay::backend::input::Axis::Vertical,
                            ),
                            (
                                input::event::pointer::Axis::Horizontal,
                                smithay::backend::input::Axis::Horizontal,
                            ),
                        ] {
                            if event.has_axis(axis) {
                                frame = frame.value(target, event.scroll_value(axis));
                            }
                        }
                        pointer.axis(&mut state, frame);
                        pointer.frame(&mut state);
                        continue;
                    }
                    input::Event::Pointer(PointerEvent::Button(event)) => {
                        let bit = match event.button() {
                            0x110 => 1,
                            0x111 => 2,
                            0x112 => 4,
                            0x116 => 8,
                            0x115 => 16,
                            _ => continue,
                        };
                        if event.button_state() == input::event::pointer::ButtonState::Pressed {
                            physical_buttons |= bit;
                        } else {
                            physical_buttons &= !bit;
                        }
                    }
                    _ => continue,
                }
                state.local_input(&physical_keys, physical_buttons, position);
            }
        }

        let count = state
            .canvas
            .snapshot()
            .map(|s| s.windows.len() as u32)
            .unwrap_or(0);
        if count != state.window_count {
            state.window_count = count;
            write_state(
                &state.state_path,
                state.canvas.workspace(),
                count,
                Some(&wayland_display),
            );
        }

        state.configure_all();
        if let Some(drm) = &mut state.drm
            && state.seat_active
        {
            let mut toplevels: Vec<(WlSurface, String)> = state
                .toplevel_map
                .iter()
                .filter(|(_, id)| !state.canvas.is_shell_overlay(id) || state.switcher.active())
                .map(|(t, id)| (t.wl_surface().clone(), id.clone()))
                .collect();
            toplevels.sort_by_key(|(_, id)| !state.canvas.is_shell_overlay(id));
            let canvas = &mut state.canvas;
            drm.redraw(&toplevels, canvas);
        }
        if let Some(output) = &state.virtual_output {
            for (surface, _) in &state.toplevel_map {
                send_frames_surface_tree(
                    surface.wl_surface(),
                    output,
                    started.elapsed(),
                    Some(Duration::from_millis(16)),
                    |_, _| Some(output.clone()),
                );
            }
        }

        std::thread::sleep(Duration::from_millis(16));
    }
}

/// One static DRM output: renderer + swapchain compositor. The DrmDevice is
/// self-managing, so flip completion is polled through `frame_submitted`.
struct DrmRender {
    renderer: GlesRenderer,
    #[allow(dead_code)]
    device: DrmDevice,
    drm_loop: DrmEventLoop<'static, bool>,
    flip_pending: bool,
    compositor: DrmCompositor<
        GbmAllocator<DrmDeviceFd>,
        GbmFramebufferExporter<DrmDeviceFd>,
        (),
        DrmDeviceFd,
    >,
    /// Owned so the wl_output global and OutputModeSource stay alive.
    #[allow(dead_code)]
    output: Output,
    size: Size<i32, Physical>,
    render_node: DrmNode,
    started_at: std::time::Instant,
    // Last: all DRM/renderer wrappers drop before the broker descriptor closes.
    _seat_device: crate::seat_access::SeatDevice,
}

impl DrmRender {
    fn try_new(
        dh: &DisplayHandle,
        session: &mut smithay::backend::session::libseat::LibSeatSession,
    ) -> Option<Self> {
        let mut failed_last: Option<String> = None;
        for card in 0..8u32 {
            let path = format!("/dev/dri/card{card}");
            let device = match crate::seat_access::SeatDevice::open(session, Path::new(&path)) {
                Ok(device) => device,
                Err(error) => {
                    eprintln!("{error}");
                    continue;
                }
            };
            let duplicate = match device.duplicate() {
                Ok(fd) => fd,
                Err(error) => {
                    eprintln!("DRM descriptor: {error}");
                    continue;
                }
            };
            let fd = DrmDeviceFd::new(SmsDeviceFd::from(duplicate));
            match Self::bring_up(dh, fd, &path, device) {
                Ok(Some(out)) => {
                    eprintln!(
                        "canvas-linux: DRM output active on {path} {}x{}",
                        out.size.w, out.size.h
                    );
                    return Some(out);
                }
                Ok(None) => continue,
                Err(e) => {
                    failed_last = Some(format!("{path}: {e}"));
                    continue;
                }
            }
        }
        if let Some(err) = failed_last {
            eprintln!("canvas-linux: DRM init failed ({err})");
        }
        None
    }

    fn bring_up(
        dh: &DisplayHandle,
        fd: DrmDeviceFd,
        path: &str,
        seat_device: crate::seat_access::SeatDevice,
    ) -> Result<Option<Self>, String> {
        let (mut device, notifier) =
            DrmDevice::new(fd.clone(), true).map_err(|e| format!("DrmDevice: {e}"))?;
        // Dedicated loop pumping the notifier source so pageflip completions
        // reach DrmCompositor::frame_submitted; wayland traffic stays on the
        // main polling loop.
        let drm_loop: DrmEventLoop<'static, bool> =
            DrmEventLoop::try_new().map_err(|e| format!("drm event loop: {e}"))?;
        drm_loop
            .handle()
            .insert_source(notifier, |event, _meta, vblank| match event {
                smithay::backend::drm::DrmEvent::VBlank(_) => *vblank = true,
                smithay::backend::drm::DrmEvent::Error(err) => {
                    eprintln!("canvas-linux: drm event error: {err}");
                }
            })
            .map_err(|e| format!("insert notifier: {e}"))?;

        let res = device
            .resource_handles()
            .map_err(|e| format!("resource_handles: {e}"))?;
        // Candidate (connector, crtc, mode) pairs: every connected connector's
        // preferred mode crossed with every crtc; the atomic commit in
        // create_surface/DrmCompositor filters invalid pairings.
        let mut candidates: Vec<(
            connector::Handle,
            crtc::Handle,
            smithay::reexports::drm::control::Mode,
        )> = Vec::new();
        for conn in res.connectors() {
            let info = match device.get_connector(*conn, true) {
                Ok(i) => i,
                Err(_) => continue,
            };
            if info.state() != connector::State::Connected || info.modes().is_empty() {
                continue;
            }
            let idx = info
                .modes()
                .iter()
                .position(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED))
                .unwrap_or(0);
            let mode = info.modes()[idx];
            for crtc in res.crtcs() {
                candidates.push((*conn, *crtc, mode));
            }
        }
        if candidates.is_empty() {
            return Ok(None);
        }

        let gbm = GbmDevice::new(fd.clone()).map_err(|e| format!("gbm: {e}"))?;
        let egl =
            unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|e| format!("EGLDisplay: {e}"))?;
        let context = EGLContext::new(&egl).map_err(|e| format!("EGLContext: {e}"))?;
        let mut renderer =
            unsafe { GlesRenderer::new(context) }.map_err(|e| format!("GlesRenderer: {e}"))?;

        let render_formats: Vec<Format> = egl.dmabuf_render_formats().iter().copied().collect();

        let node = DrmNode::from_path(path).map_err(|e| format!("DRM node: {e}"))?;
        let render_node = node
            .node_with_type(NodeType::Render)
            .and_then(Result::ok)
            .unwrap_or(node);
        let allocator = GbmAllocator::new(
            gbm.clone(),
            GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT,
        );
        let exporter = GbmFramebufferExporter::new(gbm.clone(), Some(node));

        let mut last_err = String::from("no usable connector/crtc pairing");
        for (conn, crtc, mode) in candidates {
            let (w, h) = mode.size();
            let wl_mode = WlMode {
                size: (w as i32, h as i32).into(),
                refresh: (mode.vrefresh() * 1000) as i32,
            };
            let output = Output::new(
                "canvas-hdmi".to_string(),
                PhysicalProperties {
                    size: (0i32, 0i32).into(),
                    subpixel: Subpixel::Unknown,
                    make: "canvas-linux".into(),
                    model: "HDMI".into(),
                },
            );
            output.create_global::<App>(dh);
            output.change_current_state(
                Some(wl_mode),
                Some(Transform::Normal),
                None,
                Some((0, 0).into()),
            );

            let surface = match device.create_surface(crtc, mode, &[conn]) {
                Ok(s) => s,
                Err(e) => {
                    last_err = format!("create_surface: {e}");
                    continue;
                }
            };
            let mut compositor = match DrmCompositor::new(
                &output,
                surface,
                None,
                allocator.clone(),
                exporter.clone(),
                [Fourcc::Xrgb8888].into_iter(),
                render_formats.iter().copied(),
                device.cursor_size(),
                Some(gbm.clone()),
            ) {
                Ok(c) => c,
                Err(e) => {
                    last_err = format!("DrmCompositor: {e:?}");
                    continue;
                }
            };
            match compositor.render_frame(
                &mut renderer,
                &[] as &[WaylandSurfaceRenderElement<GlesRenderer>],
                Color32F::new(0.02, 0.02, 0.03, 1.0),
                FrameFlags::empty(),
            ) {
                Ok(_) => {}
                Err(e) => {
                    last_err = format!("first frame: {e:?}");
                    continue;
                }
            }
            if let Err(e) = compositor.queue_frame(()) {
                last_err = format!("queue first frame: {e}");
                continue;
            }

            return Ok(Some(Self {
                renderer,
                device,
                drm_loop,
                compositor,
                output,
                size: wl_mode.size,
                flip_pending: true,
                render_node,
                started_at: std::time::Instant::now(),
                _seat_device: seat_device,
            }));
        }
        Err(last_err)
    }

    fn redraw(&mut self, surfaces: &[(WlSurface, String)], canvas: &mut CanvasCompositor) {
        // Only retire a pending frame after its actual vblank event. Calling
        // frame_submitted on every polling iteration releases the swapchain
        // buffer before KMS is done with it and causes EBUSY retry storms.
        let mut vblank = false;
        let _ = self
            .drm_loop
            .dispatch(Some(std::time::Duration::ZERO), &mut vblank);
        if self.flip_pending {
            if !vblank {
                return;
            }
            match self.compositor.frame_submitted() {
                Ok(Some(())) => {
                    self.flip_pending = false;
                    let elapsed = self.started_at.elapsed();
                    for (surface, _) in surfaces {
                        send_frames_surface_tree(
                            surface,
                            &self.output,
                            elapsed,
                            Some(Duration::from_millis(16)),
                            |_, _| Some(self.output.clone()),
                        );
                    }
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("canvas-linux: frame_submitted: {e}");
                    self.flip_pending = false;
                }
            }
            if self.flip_pending {
                return;
            }
        }

        let mut elements: Vec<WaylandSurfaceRenderElement<GlesRenderer>> = Vec::new();
        let snapshot = canvas.snapshot().ok();
        for (surface, id) in surfaces {
            if !<WlSurface as smithay::reexports::wayland_server::Resource>::is_alive(surface) {
                continue;
            }
            let frame = snapshot
                .as_ref()
                .and_then(|s| s.windows.iter().find(|w| w.id == *id))
                .map(|w| w.frame)
                .unwrap_or(Rect {
                    x: 0,
                    y: 0,
                    width: self.size.w.max(0) as u32,
                    height: self.size.h.max(0) as u32,
                });
            let location: Point<i32, smithay::utils::Physical> = (frame.x, frame.y).into();
            elements.extend(render_elements_from_surface_tree(
                &mut self.renderer,
                surface,
                location,
                1.0,
                1.0,
                Kind::Unspecified,
            ));
        }

        match self.compositor.render_frame(
            &mut self.renderer,
            &elements,
            Color32F::new(0.20, 0.22, 0.28, 1.0),
            FrameFlags::empty(),
        ) {
            Ok(_) => match self.compositor.queue_frame(()) {
                Ok(()) => self.flip_pending = true,
                Err(e) => {
                    let msg = format!("{e}");
                    if !msg.contains("No frame has been prepared") {
                        eprintln!("canvas-linux: queue_frame: {e}");
                    }
                    self.flip_pending = false;
                }
            },
            Err(e) => eprintln!("canvas-linux: render_frame: {e:?}"),
        }
    }
}

struct App {
    virtual_output: Option<Output>,
    canvas: CanvasCompositor,
    compositor_state: CompositorState,
    xdg_shell_state: XdgShellState,
    shm_state: ShmState,
    seat_state: SeatState<App>,
    data_device_state: DataDeviceState,
    /// Owns the wl_output global state registered on the display.
    #[allow(dead_code)]
    output_manager: OutputManagerState,
    #[allow(dead_code)]
    seat: Seat<App>,
    #[allow(dead_code)]
    keyboard: KeyboardHandle<App>,
    pointer: PointerHandle<App>,
    remote_input: RemoteInput,
    seat_active: bool,
    switcher: crate::window_switcher::WindowSwitcher,
    /// Toplevel handle -> canvas window id, for resolving focus/destroy,
    /// configure sync, and the per-frame render element walk.
    toplevel_map: Vec<(ToplevelSurface, String)>,
    next_id: u64,
    window_count: u32,
    state_path: PathBuf,
    #[allow(dead_code)]
    cmd_rx: std::sync::mpsc::Receiver<(Value, std::sync::mpsc::Sender<Value>)>,
    launcher: crate::launcher::Launcher,
    drm: Option<DrmRender>,
    dmabuf_state: DmabufState,
    /// Keeps the advertised linux-dmabuf global alive with the display.
    #[allow(dead_code)]
    dmabuf_global: Option<DmabufGlobal>,
}

impl App {
    fn release_input(&mut self) {
        self.inject_keys(&[]);
        let (x, y) = self.remote_input.pointer;
        self.inject_pointer(x, y, 0);
        self.remote_input = RemoteInput {
            local_owner: true,
            pointer: (x, y),
            ..Default::default()
        };
        self.switcher.finish();
        let keyboard = self.keyboard.clone();
        keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
        let pointer = self.pointer.clone();
        pointer.motion(
            self,
            None,
            &smithay::input::pointer::MotionEvent {
                location: (x as f64, y as f64).into(),
                serial: SERIAL_COUNTER.next_serial(),
                time: 0,
            },
        );
        pointer.frame(self);
    }

    fn focus_native(&mut self, id: &str) -> Result<Value, String> {
        if !self.seat_active {
            return Err("Linux seat is inactive".into());
        }
        if self.canvas.is_shell_overlay(id) {
            return Err("shell overlay cannot take keyboard focus".into());
        }
        let (surface, _) = self
            .toplevel_map
            .iter()
            .find(|(_, window)| window == id)
            .ok_or_else(|| format!("unknown native window {id}"))?;
        let surface = surface.wl_surface().clone();
        self.inject_keys(&[]);
        let pointer = self.pointer.clone();
        for (bit, code) in [(1, 0x110), (2, 0x111), (4, 0x112), (8, 0x116), (16, 0x115)] {
            if self.remote_input.buttons & bit != 0 {
                pointer.button(
                    self,
                    &ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: monotonic_ms(),
                        button: code,
                        state: ButtonState::Released,
                    },
                );
            }
        }
        pointer.frame(self);
        self.remote_input.buttons = 0;
        let keyboard = self.keyboard.clone();
        keyboard.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
        if keyboard.current_focus().as_ref() != Some(&surface) {
            return Err("keyboard seat did not acknowledge focus".into());
        }
        self.canvas.focus_window(id)?;
        // Smithay render elements are front-to-back; list order also drives hit testing.
        if let Some(index) = self
            .toplevel_map
            .iter()
            .position(|(_, window)| window == id)
        {
            let entry = self.toplevel_map.remove(index);
            self.toplevel_map.insert(0, entry);
        }
        self.configure_all();
        Ok(json!({"window_id":id,"seat_focused":true}))
    }

    fn cycle_native(&mut self, reverse: bool) -> Result<Value, String> {
        let id = self
            .switcher
            .next(self.canvas.mru(), self.canvas.focused(), reverse)
            .ok_or("no native windows to switch")?;
        self.focus_native(&id)
    }

    fn control(&mut self, command: &Value) -> Value {
        if !self.seat_active && command["command"] != "list" {
            return json!({"ok":false,"error":"Linux seat is inactive"});
        }
        let result = match command["command"].as_str().unwrap_or("") {
            "list" => {
                let mut response = crate::launcher::handle_command(command, &mut self.canvas, &self.launcher);
                response["result"]["switcher"] = self.switcher.snapshot();
                response["result"]["seat_active"] = json!(self.seat_active);
                if let Some(display) = self.canvas.displays().first() {
                    response["result"]["switcher"]["frame"] = json!(crate::control_transport::switcher_frame(display.work_area));
                }
                response["result"]["input_owner"] = json!(if self.remote_input.local_owner { "local" } else { "remote" });
                response["result"]["shell_overlays"] = self.canvas.snapshot().map(|snapshot| snapshot.windows.into_iter()
                    .filter(|window| self.canvas.is_shell_overlay(&window.id)).collect::<Vec<_>>()).map_or(json!([]), |windows| json!(windows));
                if let Some(windows) = response["result"]["windows"].as_array_mut() {
                    windows.retain(|window| !self.canvas.is_shell_overlay(window["id"].as_str().unwrap_or("")));
                }
                return response;
            }
            "input" => return self.apply_remote_input(command),
            "input_takeover" => {
                if self.remote_input.local_held {
                    return json!({"ok":false,"error":"release local held keys/buttons before remote takeover"});
                }
                self.remote_input.local_owner = false;
                Ok(json!({"owner":"remote"}))
            }
            "launch" => Err("native app spawning is not implemented; use the host launcher and observe mapped windows".into()),
            "cycle_finish" => {
                self.switcher.finish();
                Ok(json!({"finished":true}))
            }
            "cycle" => self.cycle_native(command["reverse"].as_bool().unwrap_or(false)),
            "focus" | "switch" | "app" => {
                self.switcher.finish();
                let id = command["window"].as_str().map(str::to_owned).or_else(|| {
                    let app = command["app"].as_str()?;
                    self.canvas
                        .snapshot()
                        .ok()?
                        .windows
                        .into_iter()
                        .find(|w| w.app == app)
                        .map(|w| w.id)
                });
                match id {
                    Some(id) => self.focus_native(&id),
                    None => Err("known window or app required".into()),
                }
            }
            "close" => {
                let id = command["window"].as_str().unwrap_or("");
                match self.toplevel_map.iter().find(|(_, window)| window == id) {
                    Some((surface, _)) => {
                        surface.send_close();
                        Ok(json!({"window_id":id,"close_requested":true,"closed":false}))
                    }
                    None => Err(format!("unknown native window {id}")),
                }
            }
            _ => return crate::launcher::handle_command(command, &mut self.canvas, &self.launcher),
        };
        match result {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error}),
        }
    }

    fn local_input(&mut self, keys: &[u16], buttons: u8, position: (u16, u16)) {
        if !self.remote_input.local_owner {
            self.inject_keys(&[]);
            let (x, y) = self.remote_input.pointer;
            self.inject_pointer(x, y, 0);
            self.switcher.finish();
            self.remote_input.tab_down = false;
            self.remote_input.shortcut_tab_owned = false;
            self.remote_input.shortcut_escape_owned = false;
            self.remote_input.gesture_cancelled = false;
            self.remote_input.cycling = false;
        }
        let mut keys = keys.to_vec();
        keys.sort_unstable();
        self.remote_input.local_owner = false;
        self.remote_input.deadline = None;
        let response = self.apply_remote_input(&json!({
            "sequence":0,"lease_ms":500,"keys":keys,
            "pointer_buttons":buttons,"pointer_x":position.0,"pointer_y":position.1,
        }));
        self.remote_input.deadline = None;
        self.remote_input.local_owner = true;
        self.remote_input.local_held = !keys.is_empty() || buttons != 0;
        if response["ok"] != true {
            eprintln!("physical input rejected: {response}");
        }
    }

    fn apply_remote_input(&mut self, command: &Value) -> Value {
        if self.remote_input.local_owner {
            return json!({"ok":false,"error":"physical seat owns input; explicit input_takeover required"});
        }
        let lease_active = self
            .remote_input
            .deadline
            .is_some_and(|deadline| Instant::now() < deadline);
        let sequence = match command["sequence"].as_u64() {
            Some(value) if !lease_active || value > self.remote_input.sequence => value,
            Some(_) => return json!({"ok": true, "result": {"ignored": "stale sequence"}}),
            None => return json!({"ok": false, "error": "sequence required"}),
        };
        let lease_ms = command["lease_ms"].as_u64().unwrap_or(250);
        if !(1..=500).contains(&lease_ms) {
            return json!({"ok": false, "error": "lease_ms must be 1..=500"});
        }
        let Some(keys) = command["keys"].as_array().and_then(|values| {
            values
                .iter()
                .map(|value| value.as_u64().and_then(|v| u16::try_from(v).ok()))
                .collect::<Option<Vec<_>>>()
        }) else {
            return json!({"ok": false, "error": "keys must be an array of HID usages"});
        };
        if keys.len() > 32 || keys.windows(2).any(|pair| pair[0] >= pair[1]) {
            return json!({"ok": false, "error": "keys must be sorted and unique (at most 32)"});
        }
        let Some(x) = command["pointer_x"]
            .as_u64()
            .and_then(|v| u16::try_from(v).ok())
        else {
            return json!({"ok": false, "error": "pointer_x required"});
        };
        let Some(y) = command["pointer_y"]
            .as_u64()
            .and_then(|v| u16::try_from(v).ok())
        else {
            return json!({"ok": false, "error": "pointer_y required"});
        };
        let Some(buttons) = command["pointer_buttons"]
            .as_u64()
            .and_then(|v| u8::try_from(v).ok())
        else {
            return json!({"ok": false, "error": "pointer_buttons required"});
        };
        if buttons & !0x1f != 0 {
            return json!({"ok": false, "error": "unknown pointer button"});
        }

        self.remote_input.sequence = sequence;
        self.remote_input.deadline = Some(Instant::now() + Duration::from_millis(lease_ms));
        self.inject_pointer(x, y, buttons);
        let alt = keys.contains(&0xe2) || keys.contains(&0xe6);
        let tab = keys.contains(&0x2b);
        let escape = keys.contains(&0x29);
        if escape && self.switcher.active() {
            self.remote_input.shortcut_escape_owned = true;
            self.remote_input.gesture_cancelled = true;
            self.remote_input.cycling = false;
            if let Some(origin) = self.switcher.cancel(self.canvas.mru()) {
                if let Err(error) = self.focus_native(&origin) {
                    return json!({"ok":false,"error":error});
                }
            }
        }
        if self.remote_input.gesture_cancelled && tab {
            self.remote_input.shortcut_tab_owned = true;
        }
        if alt && tab && !self.remote_input.tab_down && !self.remote_input.gesture_cancelled {
            self.remote_input.cycling = true;
            self.remote_input.shortcut_tab_owned = true;
            if let Err(error) = self.cycle_native(keys.contains(&0xe1) || keys.contains(&0xe5)) {
                return json!({"ok":false,"error":error});
            }
        }
        self.remote_input.tab_down = tab;
        if !alt {
            self.remote_input.cycling = false;
            self.switcher.finish();
            self.remote_input.gesture_cancelled = false;
        }
        if !tab {
            self.remote_input.shortcut_tab_owned = false;
        }
        if !escape {
            self.remote_input.shortcut_escape_owned = false;
        }
        if self.remote_input.cycling || self.remote_input.gesture_cancelled {
            self.inject_keys(&[]);
        } else {
            let forwarded: Vec<_> = keys
                .iter()
                .copied()
                .filter(|key| *key != 0x2b || !self.remote_input.shortcut_tab_owned)
                .filter(|key| *key != 0x29 || !self.remote_input.shortcut_escape_owned)
                .collect();
            self.inject_keys(&forwarded);
        }
        json!({"ok": true, "result": {"sequence": sequence}})
    }

    fn inject_keys(&mut self, keys: &[u16]) {
        let previous = self.remote_input.keys.clone();
        let keyboard = self.keyboard.clone();
        for usage in previous.iter().filter(|usage| !keys.contains(usage)) {
            if let Some(code) = hid_to_evdev(*usage) {
                keyboard.input(
                    self,
                    // Smithay accepts XKB keycodes, not Linux evdev codes.
                    Keycode::from(code + 8),
                    KeyState::Released,
                    SERIAL_COUNTER.next_serial(),
                    monotonic_ms(),
                    |_, _, _| FilterResult::<()>::Forward,
                );
            }
        }
        for usage in keys.iter().filter(|usage| !previous.contains(usage)) {
            if let Some(code) = hid_to_evdev(*usage) {
                keyboard.input(
                    self,
                    Keycode::from(code + 8),
                    KeyState::Pressed,
                    SERIAL_COUNTER.next_serial(),
                    monotonic_ms(),
                    |_, _, _| FilterResult::<()>::Forward,
                );
            }
        }
        self.remote_input.keys = keys.to_vec();
    }

    fn inject_pointer(&mut self, x: u16, y: u16, buttons: u8) {
        let display = self
            .canvas
            .displays()
            .first()
            .map(|d| d.work_area)
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
            });
        let px = display.x as f64
            + f64::from(x) * f64::from(display.width.saturating_sub(1)) / f64::from(u16::MAX);
        let py = display.y as f64
            + f64::from(y) * f64::from(display.height.saturating_sub(1)) / f64::from(u16::MAX);
        let location: Point<f64, Logical> = (px, py).into();
        let snapshot = self.canvas.snapshot().ok();
        let focus = self.toplevel_map.iter().find_map(|(surface, id)| {
            if self.canvas.is_shell_overlay(id) {
                return None;
            }
            let window = snapshot.as_ref()?.windows.iter().find(|w| &w.id == id)?;
            (px >= f64::from(window.frame.x)
                && py >= f64::from(window.frame.y)
                && px < f64::from(window.frame.x) + f64::from(window.frame.width)
                && py < f64::from(window.frame.y) + f64::from(window.frame.height))
            .then(|| {
                (
                    surface.wl_surface().clone(),
                    (f64::from(window.frame.x), f64::from(window.frame.y)).into(),
                )
            })
        });
        let pointer = self.pointer.clone();
        pointer.motion(
            self,
            focus.clone(),
            &MotionEvent {
                location,
                serial: SERIAL_COUNTER.next_serial(),
                time: monotonic_ms(),
            },
        );
        if buttons & !self.remote_input.buttons != 0
            && let Some((surface, _)) = focus
        {
            if let Some(id) = self
                .toplevel_map
                .iter()
                .find(|(top, _)| *top.wl_surface() == surface)
                .map(|(_, id)| id.clone())
            {
                self.switcher.finish();
                if let Err(error) = self.focus_native(&id) {
                    eprintln!("pointer focus: {error}");
                }
            }
        }
        let previous = self.remote_input.buttons;
        for (bit, code) in [(1, 0x110), (2, 0x111), (4, 0x112), (8, 0x116), (16, 0x115)] {
            if previous & bit != buttons & bit {
                pointer.button(
                    self,
                    &ButtonEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time: monotonic_ms(),
                        button: code,
                        state: if buttons & bit != 0 {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        },
                    },
                );
            }
        }
        pointer.frame(self);
        self.remote_input.buttons = buttons;
        self.remote_input.pointer = (x, y);
    }

    fn expire_remote_input(&mut self) {
        if self
            .remote_input
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.inject_keys(&[]);
            let (x, y) = self.remote_input.pointer;
            self.inject_pointer(x, y, 0);
            self.remote_input.deadline = None;
            self.remote_input.tab_down = false;
            self.remote_input.shortcut_tab_owned = false;
            self.remote_input.shortcut_escape_owned = false;
            self.remote_input.gesture_cancelled = false;
            self.remote_input.cycling = false;
            self.switcher.finish();
        }
    }
    /// Push each window's frame rect to its client via xdg configure. The
    /// canvas state model is the layout authority; sizes follow the frame.
    fn configure_all(&mut self) {
        if let Some(area) = self
            .canvas
            .displays()
            .first()
            .map(|display| display.work_area)
        {
            let frame = crate::control_transport::switcher_frame(area);
            for (_, id) in &self.toplevel_map {
                if self.canvas.is_shell_overlay(id) {
                    if let Err(error) = self.canvas.place_window(id, frame) {
                        eprintln!("switcher placement: {error}");
                    }
                }
            }
        }
        let frames: Vec<(String, Rect)> = self
            .canvas
            .snapshot()
            .map(|s| s.windows.into_iter().map(|w| (w.id, w.frame)).collect())
            .unwrap_or_default();
        for (toplevel, id) in &self.toplevel_map {
            let Some(frame) = frames.iter().find(|(f_id, _)| f_id == id).map(|(_, f)| *f) else {
                continue;
            };
            let wanted: smithay::utils::Size<i32, smithay::utils::Logical> =
                (frame.width as i32, frame.height as i32).into();
            let activated = self.canvas.focused() == Some(id.as_str());
            let changed = toplevel.with_pending_state(|state| {
                use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State;
                let changed = state.size != Some(wanted)
                    || state.states.contains(State::Activated) != activated;
                state.size = Some(wanted);
                if activated {
                    state.states.set(State::Activated);
                } else {
                    state.states.unset(State::Activated);
                }
                changed
            });
            if changed {
                toplevel.send_configure();
            }
        }
    }

    fn write_state(&mut self, display: Option<&str>) {
        let count = self
            .canvas
            .snapshot()
            .map(|s| s.windows.len() as u32)
            .unwrap_or(0);
        write_state(&self.state_path, self.canvas.workspace(), count, display);
    }
}

#[derive(Default)]
struct RemoteInput {
    local_owner: bool,
    local_held: bool,
    shortcut_tab_owned: bool,
    shortcut_escape_owned: bool,
    gesture_cancelled: bool,
    tab_down: bool,
    cycling: bool,
    sequence: u64,
    deadline: Option<Instant>,
    keys: Vec<u16>,
    buttons: u8,
    pointer: (u16, u16),
}

fn monotonic_ms() -> u32 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u32
}

fn hid_to_evdev(usage: u16) -> Option<u32> {
    const TABLE: &[(u16, u32)] = &[
        (4, 30),
        (5, 48),
        (6, 46),
        (7, 32),
        (8, 18),
        (9, 33),
        (10, 34),
        (11, 35),
        (12, 23),
        (13, 36),
        (14, 37),
        (15, 38),
        (16, 50),
        (17, 49),
        (18, 24),
        (19, 25),
        (20, 16),
        (21, 19),
        (22, 31),
        (23, 20),
        (24, 22),
        (25, 47),
        (26, 17),
        (27, 45),
        (28, 21),
        (29, 44),
        (30, 2),
        (31, 3),
        (32, 4),
        (33, 5),
        (34, 6),
        (35, 7),
        (36, 8),
        (37, 9),
        (38, 10),
        (39, 11),
        (40, 28),
        (41, 1),
        (42, 14),
        (43, 15),
        (44, 57),
        (45, 12),
        (46, 13),
        (47, 26),
        (48, 27),
        (49, 43),
        (51, 39),
        (52, 40),
        (53, 41),
        (54, 51),
        (55, 52),
        (56, 53),
        (57, 58),
        (58, 59),
        (59, 60),
        (60, 61),
        (61, 62),
        (62, 63),
        (63, 64),
        (64, 65),
        (65, 66),
        (66, 67),
        (67, 68),
        (68, 87),
        (69, 88),
        (73, 110),
        (74, 102),
        (75, 104),
        (76, 111),
        (77, 107),
        (78, 109),
        (79, 106),
        (80, 105),
        (81, 108),
        (82, 103),
        (224, 29),
        (225, 42),
        (226, 56),
        (227, 125),
        (228, 97),
        (229, 54),
        (230, 100),
        (231, 126),
    ];
    TABLE
        .iter()
        .find_map(|(hid, code)| (*hid == usage).then_some(*code))
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn local_activity_revokes_remote_lease_and_requires_explicit_takeover() {
        let display = Display::<App>::new().unwrap();
        let dh = display.handle();
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(&dh, "test");
        let keyboard = seat.add_keyboard(Default::default(), 200, 200).unwrap();
        let pointer = seat.add_pointer();
        let (_, cmd_rx) = std::sync::mpsc::channel();
        let mut app = App {
            virtual_output: None,
            canvas: CanvasCompositor::new(Vec::<CanvasDisplay>::new()),
            compositor_state: CompositorState::new::<App>(&dh),
            xdg_shell_state: XdgShellState::new::<App>(&dh),
            shm_state: ShmState::new::<App>(&dh, Vec::new()),
            seat_state,
            seat,
            keyboard,
            pointer,
            data_device_state: DataDeviceState::new::<App>(&dh),
            output_manager: OutputManagerState::new_with_xdg_output::<App>(&dh),
            remote_input: RemoteInput::default(),
            seat_active: true,
            switcher: crate::window_switcher::WindowSwitcher::default(),
            toplevel_map: Vec::new(),
            next_id: 0,
            window_count: 0,
            state_path: PathBuf::new(),
            cmd_rx,
            launcher: crate::launcher::Launcher::new(),
            drm: None,
            dmabuf_state: DmabufState::new(),
            dmabuf_global: None,
        };
        let remote = json!({"command":"input","sequence":1,"keys":[4],"pointer_x":0,"pointer_y":0,"pointer_buttons":1});
        assert_eq!(app.control(&remote)["ok"], true);
        app.local_input(&[5], 0, (10, 20));
        assert_eq!(app.remote_input.keys, vec![5]);
        assert_eq!(app.remote_input.buttons, 0);
        assert!(app.remote_input.deadline.is_none());
        assert_eq!(app.control(&remote)["ok"], false);
        assert_eq!(
            app.control(&json!({"command":"input_takeover"}))["ok"],
            false
        );
        app.local_input(&[], 0, (10, 20));
        assert_eq!(app.control(&remote)["ok"], false);
        assert_eq!(
            app.control(&json!({"command":"input_takeover"}))["ok"],
            true
        );
        assert_eq!(app.control(&remote)["ok"], true);
        app.release_input();
        app.seat_active = false;
        assert!(app.remote_input.keys.is_empty());
        assert_eq!(app.remote_input.buttons, 0);
        assert!(app.remote_input.deadline.is_none());
        assert_eq!(app.control(&remote)["ok"], false);
        assert_eq!(
            app.control(&json!({"command":"input_takeover"}))["ok"],
            false
        );
        assert_eq!(
            app.control(&json!({"command":"list"}))["result"]["seat_active"],
            false
        );
        assert!(
            app.focus_native("missing")
                .unwrap_err()
                .contains("inactive")
        );
        app.seat_active = true;
        assert_eq!(app.control(&remote)["ok"], false);
        assert_eq!(
            app.control(&json!({"command":"input_takeover"}))["ok"],
            true
        );
        assert_eq!(app.control(&remote)["ok"], true);
    }
}

impl BufferHandler for App {
    fn buffer_destroyed(&mut self, _buffer: &wl_buffer::WlBuffer) {}
}

impl DmabufHandler for App {
    fn dmabuf_state(&mut self) -> &mut DmabufState {
        &mut self.dmabuf_state
    }

    fn dmabuf_imported(
        &mut self,
        _global: &DmabufGlobal,
        dmabuf: Dmabuf,
        notifier: ImportNotifier,
    ) {
        let Some(drm) = self.drm.as_mut() else {
            notifier.failed();
            return;
        };
        match drm.renderer.import_dmabuf(&dmabuf, None) {
            Ok(_) => {
                if dmabuf.node().is_none() {
                    dmabuf.set_node(drm.render_node);
                }
                if let Err(error) = notifier.successful::<App>() {
                    eprintln!("canvas-linux: dmabuf client disappeared during import: {error}");
                }
            }
            Err(error) => {
                eprintln!("canvas-linux: rejected client dmabuf: {error}");
                notifier.failed();
            }
        }
    }
}

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // Stable window identity is separate from mutable client app_id/title.
        let id = format!("surface-{}", self.next_id);
        self.next_id += 1;
        self.toplevel_map.push((surface.clone(), id.clone()));
        if let Some(output) = &self.virtual_output {
            output.enter(surface.wl_surface());
        }
        let display_frame = self
            .canvas
            .displays()
            .first()
            .map(|d| d.work_area)
            .unwrap_or(Rect {
                x: 0,
                y: 0,
                width: 1280,
                height: 720,
            });
        let win = WindowSurface {
            id: id.clone(),
            process_id: None,
            app: "surface".into(),
            title: String::new(),
            frame: display_frame,
            movable: true,
            resizable: true,
        };
        self.canvas.map_window(id, win);
        self.write_state(Some(&std::env::var("WAYLAND_DISPLAY").unwrap_or_default()));
        self.configure_all();
    }

    fn new_popup(&mut self, _surface: PopupSurface, _positioner: PositionerState) {}
    fn grab(&mut self, _surface: PopupSurface, _seat: wl_seat::WlSeat, _serial: Serial) {}
    fn reposition_request(
        &mut self,
        _surface: PopupSurface,
        _positioner: PositionerState,
        _token: u32,
    ) {
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        let wl = surface.wl_surface();
        if let Some(pos) = self
            .toplevel_map
            .iter()
            .position(|(s, _)| *s.wl_surface() == *wl)
        {
            let id = self.toplevel_map.remove(pos).1;
            self.canvas.close_window(&id);
            if self.canvas.focused().is_none() {
                if let Some(next) = self.canvas.mru().first().cloned() {
                    if let Err(error) = self.focus_native(&next) {
                        eprintln!("restore focus after close: {error}");
                    }
                } else {
                    let keyboard = self.keyboard.clone();
                    keyboard.set_focus(self, None, SERIAL_COUNTER.next_serial());
                }
            }
            self.write_state(Some(&std::env::var("WAYLAND_DISPLAY").unwrap_or_default()));
            self.switcher
                .reconcile(self.canvas.mru(), self.canvas.focused());
        }
    }
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }

    fn commit(&mut self, surface: &WlSurface) {
        smithay::backend::renderer::utils::on_commit_buffer_handler::<App>(surface);
        if let Some(id) = self
            .toplevel_map
            .iter()
            .find(|(top, _)| top.wl_surface() == surface)
            .map(|(_, id)| id.clone())
        {
            let identity = smithay::wayland::compositor::with_states(surface, |states| {
                let attributes = states
                    .data_map
                    .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()?;
                let attributes = attributes.lock().ok()?;
                Some((attributes.app_id.clone(), attributes.title.clone()))
            });
            if let Some((app, title)) = identity {
                let switcher_overlay =
                    app.as_deref() == Some(crate::control_transport::SWITCHER_APP_ID);
                self.canvas.update_identity(&id, app, title);
                if switcher_overlay && !self.canvas.is_shell_overlay(&id) {
                    if let Err(error) = self.canvas.mark_shell_overlay(&id) {
                        eprintln!("switcher role rejected: {error}");
                    }
                }
            }
        }
    }
}

impl ShmHandler for App {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}

impl OutputHandler for App {}

impl SeatHandler for App {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;

    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, _seat: &Seat<Self>, focused: Option<&WlSurface>) {
        if let Some(srf) = focused
            && let Some((_, id)) = self
                .toplevel_map
                .iter()
                .find(|(s, _)| *s.wl_surface() == *srf)
        {
            let _ = self.canvas.focus_window(id);
        }
    }

    fn cursor_image(
        &mut self,
        _seat: &Seat<Self>,
        _image: smithay::input::pointer::CursorImageStatus,
    ) {
    }
}

impl SelectionHandler for App {
    type SelectionUserData = ();
}

impl DataDeviceHandler for App {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}

impl ClientDndGrabHandler for App {}
impl ServerDndGrabHandler for App {
    fn send(&mut self, _mime_type: String, _fd: std::os::unix::io::OwnedFd, _seat: Seat<Self>) {}
}

impl AsMut<CompositorState> for App {
    fn as_mut(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
}

#[derive(Default)]
struct ClientState {
    compositor_state: CompositorClientState,
}

impl ClientData for ClientState {
    fn initialized(&self, _client_id: ClientId) {}
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

delegate_xdg_shell!(App);
delegate_compositor!(App);
delegate_shm!(App);
delegate_dmabuf!(App);
delegate_seat!(App);
delegate_data_device!(App);
delegate_output!(App);
