//! Opt-in proof against actual Wayland clients, never a user's live seat.
use serde_json::{Value, json};
use std::{
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn wait_for(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(Instant::now() < deadline, "native probe timed out");
        std::thread::sleep(Duration::from_millis(20));
    }
}
fn request(socket: &Path, command: Value) -> Value {
    let response = crate::launcher::send_command(socket, &command).unwrap();
    let response: Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["ok"], true, "{response}");
    response["result"].clone()
}

#[test]
#[ignore = "requires Linux, foot, and CANVAS_COMPOSITOR_TEST_BIN pointing to a matching built binary"]
fn native_focus_close_and_alt_tab_reach_real_clients() {
    use std::os::unix::fs::DirBuilderExt;
    let binary = std::env::var_os("CANVAS_COMPOSITOR_TEST_BIN").expect("test binary required");
    let directory =
        std::path::PathBuf::from(format!("/tmp/canvas-seat-proof-{}", std::process::id()));
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&directory)
        .unwrap();
    eprintln!("native proof artifacts: {}", directory.display());
    let log = std::fs::File::create(directory.join("server.log")).unwrap();
    let _server = OwnedChild(
        Command::new(binary)
            .args(["desktop-compositor", "headless"])
            .env("XDG_RUNTIME_DIR", &directory)
            .env_remove("DOCK_CANVAS_STREAM")
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap(),
    );
    let socket = directory.join("native-control/compositor-control.sock");
    wait_for(|| socket.exists() && directory.join("wayland-5").exists());
    let mut clients = Vec::new();
    let mut ids = Vec::new();
    for index in 0..3 {
        let log = std::fs::File::create(directory.join(format!("client-{index}.log"))).unwrap();
        clients.push(OwnedChild(
            Command::new("/usr/bin/foot")
                .args(["--title=Canvas native proof", "/usr/bin/sleep", "60"])
                .env("XDG_RUNTIME_DIR", &directory)
                .env("WAYLAND_DISPLAY", "wayland-5")
                .env("WAYLAND_DEBUG", "1")
                .stdout(Stdio::null())
                .stderr(log)
                .spawn()
                .unwrap(),
        ));
        wait_for(|| {
            request(&socket, json!({"command":"list"}))["windows"]
                .as_array()
                .unwrap()
                .len()
                == index + 1
        });
        ids = request(&socket, json!({"command":"list"}))["windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w["id"].as_str().unwrap().to_owned())
            .collect();
    }
    for id in &ids {
        let focused = request(&socket, json!({"command":"focus","window":id}));
        assert_eq!(focused["seat_focused"], true);
    }
    assert_eq!(
        request(&socket, json!({"command":"list"}))["focused_window"],
        ids[2]
    );
    let inventory = request(&socket, json!({"command":"list"}));
    assert!(
        inventory["windows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|w| w["title"] == "Canvas native proof" && w["app"] == "foot")
    );
    let overlay_log = std::fs::File::create(directory.join("overlay.log")).unwrap();
    let _overlay = OwnedChild(
        Command::new("/usr/bin/foot")
            .args([
                "--app-id=dev.fpl.canvas.switcher",
                "--title=Switcher role proof",
                "/usr/bin/sleep",
                "60",
            ])
            .env("XDG_RUNTIME_DIR", &directory)
            .env("WAYLAND_DISPLAY", "wayland-5")
            .env("WAYLAND_DEBUG", "1")
            .stdout(Stdio::null())
            .stderr(overlay_log)
            .spawn()
            .unwrap(),
    );
    wait_for(|| {
        request(&socket, json!({"command":"list"}))["shell_overlays"]
            .as_array()
            .unwrap()
            .len()
            == 1
    });
    let with_overlay = request(&socket, json!({"command":"list"}));
    assert_eq!(with_overlay["windows"].as_array().unwrap().len(), 3);
    assert_eq!(with_overlay["focused_window"], ids[2]);
    let overlay_id = with_overlay["shell_overlays"][0]["id"].as_str().unwrap();
    assert!(
        !with_overlay["mru"]
            .as_array()
            .unwrap()
            .iter()
            .any(|id| id == overlay_id)
    );
    let rejected =
        crate::launcher::send_command(&socket, &json!({"command":"focus","window":overlay_id}))
            .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&rejected).unwrap()["ok"],
        false
    );
    // One held Alt gesture: multiple Tab press/release snapshots, no pointer click.
    let input = |sequence, keys| json!({"command":"input","sequence":sequence,"lease_ms":500,"keys":keys,"pointer_x":0,"pointer_y":0,"pointer_buttons":0});
    request(&socket, input(1, vec![43, 226]));
    let switcher = request(&socket, json!({"command":"list"}))["switcher"].clone();
    assert_eq!(switcher["active"], true);
    assert_eq!(switcher["selected_window"], ids[1]);
    assert_eq!(switcher["windows"], json!([ids[2], ids[1], ids[0]]));
    assert_eq!(
        request(&socket, json!({"command":"list"}))["focused_window"],
        ids[1]
    );
    request(&socket, input(2, vec![226]));
    request(&socket, input(3, vec![43, 226]));
    assert_eq!(
        request(&socket, json!({"command":"list"}))["focused_window"],
        ids[0]
    );
    request(&socket, input(4, vec![226]));
    request(&socket, input(5, vec![43, 225, 226]));
    assert_eq!(
        request(&socket, json!({"command":"list"}))["focused_window"],
        ids[1]
    );
    // Releasing Alt before Tab must not type Tab into the chosen app.
    request(&socket, input(6, vec![43]));
    request(&socket, input(7, vec![]));
    assert_eq!(
        request(&socket, json!({"command":"list"}))["switcher"]["active"],
        false
    );
    request(&socket, input(8, vec![4])); // HID A -> evdev 30 -> XKB 38.
    request(&socket, input(9, vec![]));
    wait_for(|| {
        std::fs::read_to_string(directory.join("client-1.log"))
            .unwrap()
            .lines()
            .any(|line| {
                line.contains("wl_keyboard") && line.contains(".key(") && line.ends_with(", 30, 1)")
            })
    });
    let close = request(&socket, json!({"command":"close","window":ids[1]}));
    assert_eq!(close["close_requested"], true);
    assert_eq!(close["closed"], false);
    wait_for(|| {
        request(&socket, json!({"command":"list"}))["windows"]
            .as_array()
            .unwrap()
            .len()
            == 2
    });
    wait_for(|| clients[1].0.try_wait().unwrap().is_some());
    assert_eq!(
        request(&socket, json!({"command":"list"}))["focused_window"],
        ids[0]
    );
    for index in 0..3 {
        let log = std::fs::read_to_string(directory.join(format!("client-{index}.log"))).unwrap();
        assert!(
            log.contains("wl_keyboard") && log.contains(".enter("),
            "client {index} never saw keyboard enter"
        );
        assert!(
            !log.lines().any(|line| line.contains("wl_keyboard")
                && line.contains(".key(")
                && line.ends_with(", 15, 1)")),
            "Alt-Tab leaked a Tab press to client {index}"
        );
    }
    assert!(
        std::fs::read_to_string(directory.join("client-1.log"))
            .unwrap()
            .contains(".close(")
    );
    for index in [0, 2] {
        request(&socket, json!({"command":"close","window":ids[index]}));
        wait_for(|| clients[index].0.try_wait().unwrap().is_some());
    }
    wait_for(|| {
        request(&socket, json!({"command":"list"}))["windows"]
            .as_array()
            .unwrap()
            .is_empty()
    });
    assert!(request(&socket, json!({"command":"list"}))["focused_window"].is_null());
}
