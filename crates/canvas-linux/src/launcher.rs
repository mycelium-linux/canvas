//! App launcher / switcher / search and JSON control socket for the
//! canvas-linux compositor.
//!
//! `desktop-compositor` owns the Wayland server; these helpers own the
//! application model and the request/response protocol that drives it from
//! outside (a shell one-liner, the canvas control socket, or a retained-Lua
//! binder that posts commands back to canvas-linux). Together with the
//! `WindowLifecycle` stream producer this closes the loop: canvas-linux both
//! publishes window lifecycle events AND accepts window/app commands.
//!
//! `Launcher` and the command handlers (`handle_command`, `handle_command_line`,
//! `handle_one`, `send_command`) are platform independent and unit tested on
//! every host (macOS). `send_command` is the client used by the
//! `desktop-compositor-control` action; `handle_one` is the server's per-
//! connection handler (the runner's accept loop is wired in a follow-up Bead).

// In default/non-test builds the control-socket server logic is not reached
// (the runner serves the socket only with the `compositor` feature); tests
// exercise it directly. Keep the bin targets warning-free.
#![cfg_attr(not(test), allow(dead_code))]

use crate::compositor::CanvasCompositor;
use canvas_core::desktop::WindowBackend;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// A launchable application known to the compositor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub exec: String,
}

/// In-process app registry: id -> launchable command. The registry is owned by
/// the compositor and populated from config; `launch` only resolves an entry's
/// exec string — the runner performs the actual `Command::spawn` under the
/// compositor's WAYLAND_DISPLAY so the client maps itself as a managed surface.
pub struct Launcher {
    apps: BTreeMap<String, AppEntry>,
}

impl Default for Launcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Launcher {
    pub fn new() -> Self {
        Self {
            apps: BTreeMap::new(),
        }
    }

    pub fn with_apps(apps: impl IntoIterator<Item = AppEntry>) -> Self {
        let mut launcher = Self::new();
        for app in apps {
            launcher.register(app);
        }
        launcher
    }

    pub fn register(&mut self, app: AppEntry) {
        self.apps.insert(app.id.clone(), app);
    }

    pub fn get(&self, id: &str) -> Option<&AppEntry> {
        self.apps.get(id)
    }

    /// All registered apps, sorted by id.
    pub fn apps(&self) -> Vec<&AppEntry> {
        self.apps.values().collect()
    }

    /// Case-insensitive prefix/substring match on id or name — the "search"
    /// half of an app switcher.
    pub fn search(&self, query: &str) -> Vec<&AppEntry> {
        if query.is_empty() {
            return self.apps();
        }
        let q = query.to_lowercase();
        self.apps
            .values()
            .filter(|app| {
                app.id.to_lowercase().contains(&q) || app.name.to_lowercase().contains(&q)
            })
            .collect()
    }

    /// Resolve a launchable app to its exec string (the runner spawns it).
    pub fn launch(&self, id: &str) -> Result<&AppEntry, String> {
        self.apps.get(id).ok_or_else(|| format!("unknown app {id}"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwitchTarget {
    Window(String),
    App(String),
}

/// Process one control-plane command against the compositor state. Returns a
/// JSON-serializable response envelope: `{ok, result|error}`. Pure with
/// respect to I/O, so it is unit tested directly.
pub fn handle_command(
    command: &Value,
    canvas: &mut CanvasCompositor,
    launcher: &Launcher,
) -> Value {
    let cmd = command["command"].as_str().unwrap_or("");
    match cmd {
        "list" => json!({
            "ok": true,
            "result": {
                "workspace": canvas.workspace(),
                "focused_window": canvas.focused(),
                "mru": canvas.mru(),
                "apps": launcher.apps().iter().map(|a| json!({
                    "id": a.id, "name": a.name, "exec": a.exec,
                })).collect::<Vec<_>>(),
                "windows": canvas
                    .snapshot()
                    .map(|s| {
                        s.windows
                            .iter()
                            .map(|w| json!({ "id": w.id, "app": w.app, "title": w.title }))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default(),
            },
        }),
        "launch" => match command["app"].as_str() {
            Some(id) => match launcher.launch(id) {
                Ok(app) => json!({
                    "ok": true,
                    "result": {
                        "app": app.id,
                        "exec": app.exec,
                        "note": "runner spawns exec under the compositor WAYLAND_DISPLAY",
                    },
                }),
                Err(e) => json!({ "ok": false, "error": e }),
            },
            None => json!({ "ok": false, "error": "app required" }),
        },
        "switch" | "focus" | "app" => {
            let target = match (command["window"].as_str(), command["app"].as_str()) {
                (Some(id), _) => SwitchTarget::Window(id.to_string()),
                (_, Some(app)) => SwitchTarget::App(app.to_string()),
                _ => return json!({ "ok": false, "error": "window or app required" }),
            };
            switch_to(canvas, &target)
        }
        "close" => match command["window"].as_str() {
            Some(id) => {
                canvas.close_window(id);
                json!({ "ok": true, "result": { "closed": id } })
            }
            None => json!({ "ok": false, "error": "window required" }),
        },
        "place" => match command["window"].as_str() {
            Some(id) => {
                let frame = &command["frame"];
                let parsed = (
                    frame["x"].as_i64(),
                    frame["y"].as_i64(),
                    frame["width"].as_u64(),
                    frame["height"].as_u64(),
                );
                match parsed {
                    (Some(x), Some(y), Some(width), Some(height))
                        if width <= u32::MAX as u64 && height <= u32::MAX as u64 =>
                    {
                        let rect = canvas_core::layout::Rect {
                            x: x as i32,
                            y: y as i32,
                            width: width as u32,
                            height: height as u32,
                        };
                        match canvas.place_window(id, rect) {
                            Ok(()) => {
                                json!({ "ok": true, "result": { "placed": id, "frame": frame } })
                            }
                            Err(error) => json!({ "ok": false, "error": error }),
                        }
                    }
                    _ => {
                        json!({ "ok": false, "error": "frame requires integer x, y, width, height" })
                    }
                }
            }
            None => json!({ "ok": false, "error": "window required" }),
        },
        "search" => {
            let query = command["query"].as_str().unwrap_or("");
            json!({
                "ok": true,
                "result": {
                    "query": query,
                    "matches": launcher.search(query).iter().map(|a| json!({
                        "id": a.id, "name": a.name, "exec": a.exec,
                    })).collect::<Vec<_>>(),
                },
            })
        }
        _ => json!({
            "ok": false,
            "error": "unknown command; expected list|launch|switch|focus|close|place|search",
        }),
    }
}

/// Focus a window by id, or (app-switcher) the first mapped window of an app.
fn switch_to(canvas: &mut CanvasCompositor, target: &SwitchTarget) -> Value {
    let snapshot = match canvas.snapshot() {
        Ok(s) => s,
        Err(e) => return json!({ "ok": false, "error": e }),
    };
    let id = match target {
        SwitchTarget::Window(id) => Some(id.as_str()),
        SwitchTarget::App(app) => snapshot
            .windows
            .iter()
            .find(|w| w.app == *app)
            .map(|w| w.id.as_str()),
    };
    match id {
        Some(id) => match canvas.focus(id) {
            Ok(w) => json!({ "ok": true, "result": { "window_id": w.id, "app": w.app } }),
            Err(e) => json!({ "ok": false, "error": e }),
        },
        None => json!({
            "ok": false,
            "error": match target {
                SwitchTarget::Window(_) => "unknown window".into(),
                SwitchTarget::App(app) => format!("no mapped window for app {app}"),
            },
        }),
    }
}

/// Parse one command line, dispatch, and serialize the response. This is the
/// unit of work a control-socket line maps to; tested without any I/O.
pub fn handle_command_line(
    line: &str,
    canvas: &mut CanvasCompositor,
    launcher: &Launcher,
) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return json!({ "ok": false, "error": "empty command" }).to_string();
    }
    let command: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(e) => return json!({ "ok": false, "error": format!("invalid JSON: {e}") }).to_string(),
    };
    handle_command(&command, canvas, launcher).to_string()
}

/// Client side: send one command to a running compositor control socket and
/// return the response text.
pub fn send_command(socket: &Path, command: &Value) -> Result<String, String> {
    use crate::control_transport::{TIMEOUT, read_frame, write_frame};
    let mut stream = std::os::unix::net::UnixStream::connect(socket)
        .map_err(|e| format!("connect {}: {e}", socket.display()))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(TIMEOUT))
        .map_err(|e| e.to_string())?;
    write_frame(&mut stream, command).map_err(|e| format!("write: {e}"))?;
    let mut reader = std::io::BufReader::new(stream);
    let buf = read_frame(&mut reader)
        .map_err(|e| format!("outcome unknown: {e}; refresh before retry"))?;
    Ok(buf.trim().to_string())
}

/// Serve one client connection on an accepted Unix stream: read a command line,
/// dispatch it against the shared compositor state, write a response line.
pub fn handle_one(
    stream: &mut std::os::unix::net::UnixStream,
    canvas: &mut CanvasCompositor,
    launcher: &Launcher,
) -> std::io::Result<()> {
    use crate::control_transport::{TIMEOUT, read_frame, write_frame};
    stream.set_read_timeout(Some(TIMEOUT))?;
    stream.set_write_timeout(Some(TIMEOUT))?;
    let line = read_frame(&mut std::io::BufReader::new(&mut *stream))?;
    let response = handle_command_line(&line, canvas, launcher);
    write_frame(stream, &serde_json::from_str(&response)?)?;
    Ok(())
}

/// One bounded control worker. Clients use one request per connection.
#[cfg(all(feature = "compositor", target_os = "linux"))]
pub fn serve_control(
    listener: std::os::unix::net::UnixListener,
    sender: std::sync::mpsc::SyncSender<(Value, std::sync::mpsc::Sender<Value>)>,
) -> std::io::Result<()> {
    use crate::control_transport::{TIMEOUT, read_frame, write_frame};
    for accepted in listener.incoming() {
        let mut stream = accepted?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let result = (|| -> Result<Value, String> {
            let line =
                read_frame(&mut std::io::BufReader::new(&mut stream)).map_err(|e| e.to_string())?;
            let command = serde_json::from_str(&line).map_err(|e| format!("invalid JSON: {e}"))?;
            let (reply_tx, reply_rx) = std::sync::mpsc::channel();
            sender
                .try_send((command, reply_tx))
                .map_err(|e| format!("control queue unavailable: {e}"))?;
            reply_rx
                .recv_timeout(TIMEOUT)
                .map_err(|e| format!("command outcome unknown: {e}; refresh state before retrying"))
        })();
        let reply = result.unwrap_or_else(|error| json!({"ok":false,"error":error}));
        if let Err(error) = write_frame(&mut stream, &reply) {
            eprintln!("canvas-linux control reply: {error}");
        }
    }
    Err(std::io::Error::other("control listener ended"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::desktop::{WindowSurface, WorkspaceBackend};
    use std::os::unix::net::UnixListener;

    fn win(id: &str, app: &str) -> WindowSurface {
        WindowSurface {
            id: id.into(),
            process_id: None,
            app: app.into(),
            title: String::new(),
            frame: canvas_core::layout::Rect {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
            },
            movable: true,
            resizable: true,
        }
    }

    fn app(id: &str, name: &str) -> AppEntry {
        AppEntry {
            id: id.into(),
            name: name.into(),
            exec: format!("{id}.sh"),
        }
    }

    #[test]
    fn launcher_search_is_case_insensitive_substring() {
        let launcher = Launcher::with_apps([
            app("foot", "Foot Terminal"),
            app("firefox", "Firefox"),
            app("mympd", "mympd kiosk"),
        ]);
        let matches: Vec<&str> = launcher
            .search("FOOT")
            .iter()
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(matches, vec!["foot"]);
        let all: Vec<&str> = launcher.search("").iter().map(|a| a.id.as_str()).collect();
        assert_eq!(all.len(), 3);
        assert!(launcher.search("term").iter().any(|a| a.id == "foot"));
        assert!(launcher.search("mympd").iter().any(|a| a.id == "mympd"));
        assert!(launcher.get("nope").is_none());
        assert!(launcher.launch("foot").is_ok());
        assert!(launcher.launch("nope").is_err());
    }

    #[test]
    fn handle_command_line_lists_searches_and_reports_errors() {
        let mut canvas = CanvasCompositor::new(vec![]);
        canvas.map_window("surface-0".into(), win("surface-0", "foot"));
        let launcher = Launcher::with_apps([app("foot", "Foot Terminal")]);

        let resp: Value = serde_json::from_str(&handle_command_line(
            "{\"command\":\"list\"}",
            &mut canvas,
            &launcher,
        ))
        .unwrap();
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["result"]["workspace"], json!(null));
        assert_eq!(resp["result"]["apps"][0]["id"], "foot");
        assert_eq!(resp["result"]["windows"][0]["id"], "surface-0");

        let resp: Value = serde_json::from_str(&handle_command_line(
            "{\"command\":\"search\",\"query\":\"Foot\"}",
            &mut canvas,
            &launcher,
        ))
        .unwrap();
        assert_eq!(resp["result"]["matches"][0]["name"], "Foot Terminal");
        assert_eq!(
            serde_json::from_str::<Value>(&handle_command_line(
                "{\"command\":\"search\",\"query\":\"zzz\"}",
                &mut canvas,
                &launcher,
            ))
            .unwrap()["result"]["matches"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
        let err: Value = serde_json::from_str(&handle_command_line(
            "{\"command\":\"bogus\"}",
            &mut canvas,
            &launcher,
        ))
        .unwrap();
        assert_eq!(err["ok"], false);
        assert!(handle_command_line("   ", &mut canvas, &launcher).contains("empty"));
        assert!(handle_command_line("not json", &mut canvas, &launcher).contains("invalid JSON"));
    }

    #[test]
    fn switch_focuses_window_or_app() {
        let mut canvas = CanvasCompositor::new(vec![]);
        canvas.map_window("a".into(), win("a", "foot"));
        canvas.map_window("b".into(), win("b", "firefox"));
        let launcher = Launcher::new();

        let r: Value = serde_json::from_str(&handle_command_line(
            "{\"command\":\"switch\",\"window\":\"b\"}",
            &mut canvas,
            &launcher,
        ))
        .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["result"]["window_id"], "b");
        assert_eq!(r["result"]["app"], "firefox");

        let r: Value = serde_json::from_str(&handle_command_line(
            "{\"command\":\"app\",\"app\":\"foot\"}",
            &mut canvas,
            &launcher,
        ))
        .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["result"]["app"], "foot");

        assert_eq!(
            serde_json::from_str::<Value>(&handle_command_line(
                "{\"command\":\"switch\"}",
                &mut canvas,
                &launcher,
            ))
            .unwrap()["ok"],
            false
        );
    }

    #[test]
    fn place_updates_one_window_frame() {
        let mut canvas = CanvasCompositor::new(vec![]);
        canvas.map_window("camera".into(), win("camera", "camera"));
        let response = handle_command(
            &json!({
                "command": "place",
                "window": "camera",
                "frame": { "x": 0, "y": 0, "width": 960, "height": 720 }
            }),
            &mut canvas,
            &Launcher::new(),
        );
        assert_eq!(response["ok"], true);
        let snapshot = canvas.snapshot().unwrap();
        assert_eq!(snapshot.windows[0].frame.width, 960);
        assert_eq!(snapshot.windows[0].frame.height, 720);
    }

    #[test]
    fn launch_resolves_exec_and_close_removes_window() {
        let mut canvas = CanvasCompositor::new(vec![]);
        canvas.map_window("w".into(), win("w", "foot"));
        let launcher = Launcher::with_apps([app("foot", "foot")]);

        let r: Value = serde_json::from_str(&handle_command_line(
            "{\"command\":\"launch\",\"app\":\"foot\"}",
            &mut canvas,
            &launcher,
        ))
        .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(r["result"]["exec"], "foot.sh");

        assert_eq!(
            serde_json::from_str::<Value>(&handle_command_line(
                "{\"command\":\"launch\",\"app\":\"nope\"}",
                &mut canvas,
                &launcher,
            ))
            .unwrap()["ok"],
            false
        );

        let before = canvas.snapshot().unwrap().windows.len();
        let r: Value = serde_json::from_str(&handle_command_line(
            "{\"command\":\"close\",\"window\":\"w\"}",
            &mut canvas,
            &launcher,
        ))
        .unwrap();
        assert_eq!(r["ok"], true);
        assert_eq!(canvas.snapshot().unwrap().windows.len(), before - 1);
    }

    #[test]
    fn send_command_round_trips_via_unix_socket() {
        let mut canvas = CanvasCompositor::new(vec![]);
        canvas.map_window("surface-0".into(), win("surface-0", "foot"));
        canvas.recall_workspace("dock").unwrap();
        let launcher = std::sync::Arc::new(Launcher::with_apps([app("foot", "Foot Terminal")]));
        let canvas = std::sync::Arc::new(std::sync::Mutex::new(canvas));

        let dir = std::env::temp_dir();
        let socket = dir.join(format!(
            "canvas-control-{}-{}.sock",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).unwrap();
        let canvas_for_thread = std::sync::Arc::clone(&canvas);
        let launcher_for_thread = std::sync::Arc::clone(&launcher);
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut g = canvas_for_thread.lock().unwrap();
            let _ = handle_one(&mut stream, &mut g, &launcher_for_thread);
        });

        let resp = send_command(&socket, &json!({"command":"list"})).unwrap();
        let _ = handle.join();
        let _ = std::fs::remove_file(&socket);

        let v: Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["result"]["workspace"], "dock");
        assert_eq!(v["result"]["windows"][0]["id"], "surface-0");
        assert_eq!(v["result"]["apps"][0]["id"], "foot");
    }
}
