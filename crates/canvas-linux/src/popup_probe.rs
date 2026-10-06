//! Real xdg-popup protocol proof on an isolated headless server; never a live seat.
use serde_json::json;
use smithay::reexports::wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use std::{
    os::{
        fd::AsFd,
        unix::{fs::DirBuilderExt, net::UnixStream},
    },
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    protocol::{
        wl_buffer, wl_callback, wl_compositor, wl_keyboard, wl_pointer, wl_registry, wl_seat,
        wl_shm, wl_shm_pool, wl_surface,
    },
};

#[derive(Default)]
struct Probe {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    pressed: Option<u32>,
    key_serial: Option<u32>,
    keyboard_surface: Option<wl_surface::WlSurface>,
    popup_configures: usize,
    repositioned: bool,
    frame: bool,
    done: bool,
    pointer_surface: Option<wl_surface::WlSurface>,
}
impl Dispatch<wl_registry::WlRegistry, ()> for Probe {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_compositor" => {
                    state.compositor = Some(registry.bind(name, version.min(4), qh, ()))
                }
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => state.wm = Some(registry.bind(name, version.min(3), qh, ())),
                "wl_seat" => state.seat = Some(registry.bind(name, version.min(5), qh, ())),
                _ => {}
            }
        }
    }
}
impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Probe {
    fn event(
        _: &mut Self,
        wm: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}
impl Dispatch<xdg_surface::XdgSurface, ()> for Probe {
    fn event(
        _: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
        }
    }
}
impl Dispatch<wl_seat::WlSeat, ()> for Probe {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: wayland_client::WEnum::Value(caps),
        } = event
        {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Keyboard) && state.keyboard.is_none() {
                state.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}
impl Dispatch<wl_pointer::WlPointer, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Button {
                serial,
                state: wayland_client::WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => state.pressed = Some(serial),
            wl_pointer::Event::Enter { surface, .. } => state.pointer_surface = Some(surface),
            _ => {}
        }
    }
}
impl Dispatch<xdg_popup::XdgPopup, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &xdg_popup::XdgPopup,
        event: xdg_popup::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_popup::Event::Configure { width, height, .. } => {
                assert_eq!((width, height), (80, 50));
                state.popup_configures += 1;
            }
            xdg_popup::Event::Repositioned { token } => {
                assert_eq!(token, 7);
                state.repositioned = true;
            }
            xdg_popup::Event::PopupDone => state.done = true,
            _ => {}
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.frame = true;
    }
}
delegate_noop!(Probe: ignore wl_compositor::WlCompositor);
delegate_noop!(Probe: ignore wl_surface::WlSurface);
delegate_noop!(Probe: ignore wl_shm::WlShm);
delegate_noop!(Probe: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Probe: ignore wl_buffer::WlBuffer);
impl Dispatch<wl_keyboard::WlKeyboard, ()> for Probe {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::Key {
                serial,
                state: wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => state.key_serial = Some(serial),
            wl_keyboard::Event::Enter { surface, .. } => state.keyboard_surface = Some(surface),
            _ => {}
        }
    }
}
delegate_noop!(Probe: ignore xdg_toplevel::XdgToplevel);
delegate_noop!(Probe: ignore xdg_positioner::XdgPositioner);

struct OwnedServer(std::process::Child);
impl Drop for OwnedServer {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires matching Linux CANVAS_COMPOSITOR_TEST_BIN"]
fn real_popup_configures_repositions_receives_input_and_dismisses() {
    let binary = std::env::var_os("CANVAS_COMPOSITOR_TEST_BIN").expect("matching binary");
    let dir = std::path::PathBuf::from(format!("/tmp/canvas-popup-proof-{}", std::process::id()));
    std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
    eprintln!("popup proof artifacts: {}", dir.display());
    let log = std::fs::File::create(dir.join("server.log")).unwrap();
    let _server = OwnedServer(
        Command::new(binary)
            .args(["desktop-compositor", "headless"])
            .env("XDG_RUNTIME_DIR", &dir)
            .env_remove("DOCK_CANVAS_STREAM")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while !dir.join("wayland-5").exists() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    let connection =
        Connection::from_socket(UnixStream::connect(dir.join("wayland-5")).unwrap()).unwrap();
    let mut queue = connection.new_event_queue::<Probe>();
    let qh = queue.handle();
    connection.display().get_registry(&qh, ());
    let mut state = Probe::default();
    queue.roundtrip(&mut state).unwrap();
    queue.roundtrip(&mut state).unwrap();
    let compositor = state.compositor.clone().unwrap();
    let wm = state.wm.clone().unwrap();
    let root = compositor.create_surface(&qh, ());
    let root_xdg = wm.get_xdg_surface(&root, &qh, ());
    let top = root_xdg.get_toplevel(&qh, ());
    top.set_app_id("canvas-popup-proof".into());
    root.commit();
    queue.roundtrip(&mut state).unwrap();
    let pixels = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(dir.join("pixels.shm"))
        .unwrap();
    pixels.set_len(640 * 480 * 4).unwrap();
    let pool = state
        .shm
        .as_ref()
        .unwrap()
        .create_pool(pixels.as_fd(), 640 * 480 * 4, &qh, ());
    let root_buffer = pool.create_buffer(0, 640, 480, 640 * 4, wl_shm::Format::Argb8888, &qh, ());
    root_xdg.set_window_geometry(10, 20, 300, 200);
    root.attach(Some(&root_buffer), 0, 0);
    root.commit();
    queue.roundtrip(&mut state).unwrap();
    let socket = dir.join("native-control/compositor-control.sock");
    let request = |value| {
        let reply = crate::launcher::send_command(&socket, &value).unwrap();
        let reply: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply["ok"], true, "{reply}");
        reply["result"].clone()
    };
    let id = request(json!({"command":"list"}))["windows"][0]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    request(json!({"command":"focus","window":id}));
    let input = |sequence, x, y, buttons| json!({"command":"input","sequence":sequence,"keys":[],"lease_ms":500,"pointer_x":x,"pointer_y":y,"pointer_buttons":buttons});
    request(input(1, 1025, 2735, 1)); // Approx (20, 30) in geometry-local coordinates.
    queue.roundtrip(&mut state).unwrap();
    let serial = state.pressed.expect("pointer click serial");
    let positioner = wm.create_positioner(&qh, ());
    positioner.set_size(80, 50);
    positioner.set_anchor_rect(20, 20, 1, 1);
    let popup_surface = compositor.create_surface(&qh, ());
    let popup_xdg = wm.get_xdg_surface(&popup_surface, &qh, ());
    let popup = popup_xdg.get_popup(Some(&root_xdg), &positioner, &qh, ());
    popup.grab(state.seat.as_ref().unwrap(), serial);
    popup_surface.commit();
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.popup_configures, 1);
    assert!(!state.done);
    let popup_buffer = pool.create_buffer(0, 80, 50, 80 * 4, wl_shm::Format::Argb8888, &qh, ());
    popup_xdg.set_window_geometry(0, 0, 80, 50);
    popup_surface.attach(Some(&popup_buffer), 0, 0);
    popup_surface.frame(&qh, ());
    popup_surface.commit();
    let frame_deadline = Instant::now() + Duration::from_secs(2);
    while !state.frame {
        assert!(
            Instant::now() < frame_deadline,
            "popup frame callback missing"
        );
        std::thread::sleep(Duration::from_millis(5));
        queue.roundtrip(&mut state).unwrap();
    }
    popup.reposition(&positioner, 7);
    queue.roundtrip(&mut state).unwrap();
    assert!(state.repositioned);
    assert_eq!(state.popup_configures, 2);
    request(input(2, 1025, 2735, 0));
    request(input(3, 1281, 3646, 0)); // Approx (25, 40), inside popup.
    queue.roundtrip(&mut state).unwrap();
    assert_eq!(state.pointer_surface.as_ref(), Some(&popup_surface));
    request(input(4, 60000, 60000, 1)); // Outside the parent and popup.
    request(input(5, 60000, 60000, 0));
    queue.roundtrip(&mut state).unwrap();
    assert!(state.done, "outside click did not dismiss the popup");
    popup.destroy();
    popup_xdg.destroy();
    popup_surface.destroy();
    queue.roundtrip(&mut state).unwrap();
    state.done = false;
    let invalid_surface = compositor.create_surface(&qh, ());
    let invalid_xdg = wm.get_xdg_surface(&invalid_surface, &qh, ());
    let invalid_popup = invalid_xdg.get_popup(Some(&root_xdg), &positioner, &qh, ());
    invalid_popup.grab(state.seat.as_ref().unwrap(), 0);
    invalid_surface.commit();
    queue.roundtrip(&mut state).unwrap();
    assert!(state.done, "unproven serial was granted a popup grab");
    invalid_popup.destroy();
    invalid_xdg.destroy();
    invalid_surface.destroy();
    queue.roundtrip(&mut state).unwrap();
    request(json!({"command":"focus","window":id}));
    let mut key_input = input(6, 0, 0, 0);
    key_input["keys"] = json!([4]);
    request(key_input);
    request(input(7, 0, 0, 0));
    queue.roundtrip(&mut state).unwrap();
    let key_serial = state.key_serial.expect("keyboard press serial");
    state.done = false;
    let key_surface = compositor.create_surface(&qh, ());
    let key_xdg = wm.get_xdg_surface(&key_surface, &qh, ());
    let key_popup = key_xdg.get_popup(Some(&root_xdg), &positioner, &qh, ());
    key_popup.grab(state.seat.as_ref().unwrap(), key_serial);
    key_surface.commit();
    queue.roundtrip(&mut state).unwrap();
    assert!(!state.done, "valid keyboard grab was rejected");
    assert_eq!(state.keyboard_surface.as_ref(), Some(&key_surface));
    request(json!({"command":"focus","window":id}));
    queue.roundtrip(&mut state).unwrap();
    assert!(state.done, "explicit focus did not dismiss popup grab");
    assert_eq!(state.keyboard_surface.as_ref(), Some(&root));
}
