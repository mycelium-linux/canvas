//! One bounded virtual-pointer transaction on the private project compositor.
use std::{
    os::{fd::AsRawFd, unix::net::UnixStream},
    path::Path,
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{wl_callback, wl_output, wl_pointer, wl_registry, wl_seat},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

#[derive(Default)]
struct State {
    manager: Option<ZwlrVirtualPointerManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    outputs: Vec<(wl_output::WlOutput, String)>,
    done: bool,
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
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
            if interface == "zwlr_virtual_pointer_manager_v1" {
                state.manager = Some(registry.bind(name, version.min(2), qh, ()));
            }
            if interface == "wl_seat" && state.seat.is_none() {
                state.seat = Some(registry.bind(name, version.min(7), qh, ()));
            }
            if interface == "wl_output" && version >= 4 {
                let output = registry.bind(name, 4, qh, ());
                state.outputs.push((output, String::new()));
            }
        }
    }
}
impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(entry) = state.outputs.iter_mut().find(|entry| entry.0 == *output)
        {
            entry.1 = name;
        }
    }
}
impl Dispatch<wl_callback::WlCallback, ()> for State {
    fn event(
        state: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        state.done = true;
    }
}
wayland_client::delegate_noop!(State: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(State: ignore ZwlrVirtualPointerManagerV1);
wayland_client::delegate_noop!(State: ignore ZwlrVirtualPointerV1);

fn sync(
    conn: &Connection,
    queue: &mut wayland_client::EventQueue<State>,
    state: &mut State,
    deadline: Instant,
) -> Result<(), String> {
    state.done = false;
    conn.display().sync(&queue.handle(), ());
    conn.flush().map_err(|e| e.to_string())?;
    loop {
        queue.dispatch_pending(state).map_err(|e| e.to_string())?;
        if state.done {
            return Ok(());
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("virtual pointer timed out")?;
        if let Some(guard) = queue.prepare_read() {
            let mut fd = libc::pollfd {
                fd: conn.backend().poll_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe {
                libc::poll(
                    &mut fd,
                    1,
                    remaining.as_millis().max(1).min(i32::MAX as u128) as i32,
                )
            };
            if result == 0 {
                return Err("virtual pointer timed out".into());
            }
            if result < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            guard.read().map_err(|e| e.to_string())?;
        }
    }
}

pub fn click(
    socket: &Path,
    output_name: &str,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    button: u32,
) -> Result<(), String> {
    if width == 0 || height == 0 || x >= width || y >= height {
        return Err("pointer coordinates out of bounds".into());
    }
    let conn = Connection::from_socket(UnixStream::connect(socket).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let mut queue = conn.new_event_queue::<State>();
    let qh = queue.handle();
    let mut state = State::default();
    let _registry = conn.display().get_registry(&qh, ());
    let deadline = Instant::now() + Duration::from_secs(3);
    sync(&conn, &mut queue, &mut state, deadline)?;
    // Output name events arrive after the registry-created bindings are processed.
    sync(&conn, &mut queue, &mut state, deadline)?;
    let manager = state
        .manager
        .as_ref()
        .ok_or("virtual pointer protocol unavailable")?;
    let seat = state.seat.as_ref().ok_or("Wayland seat unavailable")?;
    if manager.version() < 2 {
        return Err("output-bound virtual pointer unavailable".into());
    }
    let output = &state
        .outputs
        .iter()
        .find(|entry| entry.1 == output_name)
        .ok_or("requested Wayland output unavailable")?
        .0;
    let pointer = manager.create_virtual_pointer_with_output(Some(seat), Some(output), &qh, ());
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    let timestamp =
        ((time.tv_sec as u64).wrapping_mul(1000) + (time.tv_nsec as u64) / 1_000_000) as u32;
    pointer.motion_absolute(timestamp, x, y, width, height);
    pointer.frame();
    sync(&conn, &mut queue, &mut state, deadline)?;
    pointer.button(
        timestamp.wrapping_add(1),
        button,
        wl_pointer::ButtonState::Pressed,
    );
    pointer.frame();
    // Always send release, even if a subsequent compositor acknowledgement fails.
    pointer.button(
        timestamp.wrapping_add(2),
        button,
        wl_pointer::ButtonState::Released,
    );
    pointer.frame();
    let result = sync(&conn, &mut queue, &mut state, deadline);
    pointer.destroy();
    let _ = conn.flush();
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn output_local_bounds_are_checked_before_connecting() {
        let socket = Path::new("/nonexistent/dock-test.sock");
        for (x, y, width, height) in [
            (1600, 0, 1600, 1000),
            (0, 1000, 1600, 1000),
            (0, 0, 0, 1000),
        ] {
            assert_eq!(
                click(socket, "HEADLESS-2", x, y, width, height, 0x110).unwrap_err(),
                "pointer coordinates out of bounds"
            );
        }
    }
    #[test]
    fn unresponsive_compositor_is_bounded() {
        use std::{
            os::unix::net::UnixListener,
            sync::mpsc,
            time::{SystemTime, UNIX_EPOCH},
        };
        let path = std::env::temp_dir().join(format!(
            "dock-pointer-{}-{}.sock",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let listener = UnixListener::bind(&path).unwrap();
        let (send, recv) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_stream, _) = listener.accept().unwrap();
            let _ = recv.recv_timeout(Duration::from_secs(5));
        });
        let begin = Instant::now();
        let result = click(&path, "HEADLESS-1", 1, 1, 10, 10, 0x110);
        let elapsed = begin.elapsed();
        let _ = send.send(());
        server.join().unwrap();
        std::fs::remove_file(path).unwrap();
        assert!(result.unwrap_err().contains("timed out"));
        assert!(elapsed < Duration::from_secs(4));
    }
}
