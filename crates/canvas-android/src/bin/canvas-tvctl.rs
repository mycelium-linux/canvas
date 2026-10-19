//! Explicit, ADB-authorized transport to the TV's presentation endpoint.
use canvas_workspace::endpoint::{Command as PresentationCommand, MAX_FRAME, Request};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpStream,
    process::Command,
};

#[path = "tvctl/launch.rs"]
mod launch;

fn adb(serial: &str, arguments: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new("adb")
        .args(["-s", serial])
        .args(arguments)
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(output.stdout)
}

struct Forward {
    serial: String,
    local: String,
}
impl Drop for Forward {
    fn drop(&mut self) {
        let _ = adb(&self.serial, &["forward", "--remove", &self.local]);
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().collect();
    if arguments.len() == 3 && arguments[2] == "observer-stream" {
        let (port, token, _guard) = connect(&arguments[1])?;
        let mut input = std::io::stdin().lock();
        loop {
            let mut line = String::new();
            let n = std::io::Read::by_ref(&mut input)
                .take(4097)
                .read_line(&mut line)?;
            if n == 0 {
                break;
            }
            if n > 4096 || !line.ends_with('\n') {
                return Err("Invalid observer stream frame".into());
            }
            let request: Request = serde_json::from_str(&line)?;
            if !matches!(request.command, PresentationCommand::ObserverFrame { .. }) {
                return Err("Observer stream accepts display observations only".into());
            }
            let receipt = exchange(port, &token, &request)?;
            println!("{}", serde_json::to_string(&receipt)?);
            std::io::stdout().flush()?;
            if receipt["ok"] != true {
                return Err("TV rejected observation".into());
            }
        }
        return Ok(());
    }
    // Explicit loopback-only development tunnel. Credential remains operator-
    // local; this never grants LAN access or bypasses the app's authentication.
    if arguments.len() == 6 && arguments[1] == "--tunnel" && arguments[4] == "signal-stream" {
        let port = arguments[2].parse::<u16>()?;
        let token = std::fs::read(&arguments[3])?;
        if token.len() != 64 || !token.iter().all(u8::is_ascii_hexdigit) {
            return Err("Invalid private app credential".into());
        }
        return stream_at(port, &token, &arguments[5]);
    }
    if arguments.len() == 4 && arguments[2] == "signal-stream" {
        return signal_stream(&arguments[1], &arguments[3]);
    }
    if arguments.get(2).is_some_and(|arg| arg == "widget-watch") {
        if arguments.len() < 6 {
            return Err(
                "usage: canvas-tvctl SERIAL widget-watch WORKSPACE SURFACE SOURCE [ARGS]".into(),
            );
        }
        return watch_widget(&arguments[1], &arguments[3], &arguments[4], &arguments[5..]);
    }
    let serial = std::env::args()
        .nth(1)
        .ok_or("usage: canvas-tvctl ADB_SERIAL < request.json")?;
    if std::env::args().count() != 2 {
        return Err("Expected exactly one explicit ADB target".into());
    }
    let mut raw = String::new();
    std::io::stdin()
        .take(MAX_FRAME as u64 + 1)
        .read_to_string(&mut raw)?;
    if raw.len() > MAX_FRAME {
        return Err("Request exceeds 4 MiB".into());
    }
    if raw.trim_start().starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        if value["command"]["command"] == "launch" {
            let request: launch::Request = serde_json::from_value(value)?;
            let receipt = launch::execute(&request, |args| adb(&serial, args));
            println!("{}", serde_json::to_string(&receipt)?);
            if receipt["outcome"]["status"] != "ok" {
                return Err("TV launch failed; see receipt".into());
            }
            return Ok(());
        }
    }
    if raw.trim_start().starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        if value["command"]["command"]
            .as_str()
            .is_some_and(|command| command == "audio_inspect" || command == "audio_set_muted")
        {
            let _: canvas_workspace::audio::Request = serde_json::from_value(value.clone())?;
            let (port, token, guard) = connect(&serial)?;
            let receipt = exchange_value(port, &token, &value)?;
            println!("{}", serde_json::to_string(&receipt)?);
            drop(guard);
            if receipt["ok"] != true {
                return Err("TV rejected audio request".into());
            }
            return Ok(());
        }
    }
    let text_mode = !raw.trim_start().starts_with('{');
    let mut request: Request = if text_mode {
        let words: Vec<&str> = raw.split_whitespace().collect();
        let command =
            match words.as_slice() {
                ["inspect"] | ["workspace", "list"] => PresentationCommand::Inspect,
                ["doc"] => PresentationCommand::ReadDocument,
                ["workspace", "switch", name] => PresentationCommand::WorkspaceSwitch {
                    name: (*name).into(),
                },
                ["workspace", "read", name] => PresentationCommand::WorkspaceRead {
                    name: (*name).into(),
                },
                ["workspace", "presentation", name] => PresentationCommand::WorkspacePresentation {
                    name: (*name).into(),
                },
                ["workspace", "install", path] => {
                    let file = std::fs::File::open(path)?;
                    let mut input = String::new();
                    file.take(MAX_FRAME as u64 + 1).read_to_string(&mut input)?;
                    if input.len() > MAX_FRAME {
                        return Err("Install file exceeds limit".into());
                    }
                    serde_json::from_str::<Request>(&input)?.command
                }
                _ => return Err(
                    "Use inspect, doc, workspace list/read/switch/install, or presentation JSON"
                        .into(),
                ),
            };
        Request {
            schema_version: 1,
            request_id: "tvctl".into(),
            expected_revision: None,
            command,
        }
    } else {
        serde_json::from_str(&raw)?
    };
    let (port, token, guard) = connect(&serial)?;
    if text_mode
        && matches!(
            request.command,
            PresentationCommand::Install { .. } | PresentationCommand::WorkspaceSwitch { .. }
        )
    {
        let observed = exchange(
            port,
            &token,
            &Request {
                schema_version: 1,
                request_id: "tvctl-observe".into(),
                expected_revision: None,
                command: PresentationCommand::Inspect,
            },
        )?;
        request.expected_revision = observed["revision"].as_u64();
    }
    let receipt = exchange(port, &token, &request)?;
    println!("{}", serde_json::to_string(&receipt)?);
    drop(guard);
    if receipt["ok"] != true {
        return Err("TV rejected presentation request".into());
    }
    Ok(())
}

/// One authenticated connection/forward per feed, not one ADB process per note.
fn connect(serial: &str) -> Result<(u16, Vec<u8>, Forward), Box<dyn std::error::Error>> {
    let token = adb(
        &serial,
        &[
            "shell",
            "run-as",
            "dev.fpl.canvas.tv",
            "cat",
            "files/control-token",
        ],
    )?;
    if token.len() != 64 || !token.iter().all(u8::is_ascii_hexdigit) {
        return Err(
            "Private TV credential unavailable; start the authorized development app".into(),
        );
    }
    let forwarded = adb(
        &serial,
        &[
            "forward",
            "tcp:0",
            &format!("tcp:{}", canvas_android::CONTROL_PORT),
        ],
    )?;
    let port: u16 = String::from_utf8(forwarded)?.trim().parse()?;
    let guard = Forward {
        serial: serial.into(),
        local: format!("tcp:{port}"),
    };
    Ok((port, token, guard))
}

fn signal_stream(serial: &str, workspace_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let (port, token, _guard) = connect(serial)?;
    stream_at(port, &token, workspace_id)
}

fn stream_at(
    port: u16,
    token: &[u8],
    workspace_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let inspect = Request {
        schema_version: 1,
        request_id: "signal-stream-inspect".into(),
        expected_revision: None,
        command: PresentationCommand::Inspect,
    };
    let observed = exchange(port, token, &inspect)?;
    let mut revision = observed["revision"]
        .as_u64()
        .ok_or("Missing target revision")?;
    // Latest value per surface: a slow or hidden TV must not queue old meters.
    #[derive(Default)]
    struct Pending {
        frames: std::collections::BTreeMap<String, serde_json::Value>,
        done: bool,
        error: Option<String>,
    }
    let pending = std::sync::Arc::new((
        std::sync::Mutex::new(Pending::default()),
        std::sync::Condvar::new(),
    ));
    let reader_pending = pending.clone();
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut input = std::io::stdin().lock();
            loop {
                let mut line = Vec::new();
                if input
                    .by_ref()
                    .take(512 * 1024 + 1)
                    .read_until(b'\n', &mut line)
                    .map_err(|e| e.to_string())?
                    == 0
                {
                    return Ok(());
                }
                if line.len() > 512 * 1024 || line.last() != Some(&b'\n') {
                    return Err("Invalid bounded snapshot line".into());
                }
                let value: serde_json::Value =
                    serde_json::from_slice(&line).map_err(|e| e.to_string())?;
                let id = value["surface_id"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 128)
                    .ok_or("Invalid surface_id")?
                    .to_owned();
                let mut queue = reader_pending
                    .0
                    .lock()
                    .map_err(|_| "Snapshot mailbox poisoned")?;
                if queue.frames.len() >= 64 && !queue.frames.contains_key(&id) {
                    return Err("Too many snapshot surfaces".into());
                }
                queue.frames.insert(id, value);
                reader_pending.1.notify_one();
            }
        })();
        if let Ok(mut queue) = reader_pending.0.lock() {
            queue.error = result.err();
            queue.done = true;
            reader_pending.1.notify_one();
        }
    });
    let mut last_wait_log = std::time::Instant::now() - std::time::Duration::from_secs(10);
    loop {
        let batch = {
            let mut queue = pending.0.lock().map_err(|_| "Snapshot mailbox poisoned")?;
            while queue.frames.is_empty() && !queue.done {
                queue = pending
                    .1
                    .wait(queue)
                    .map_err(|_| "Snapshot mailbox poisoned")?;
            }
            if let Some(error) = queue.error.take() {
                return Err(error.into());
            }
            if queue.frames.is_empty() && queue.done {
                break;
            }
            std::mem::take(&mut queue.frames)
        };
        for value in batch.into_values() {
            let request = Request {
                schema_version: 1,
                request_id: "signal-stream".into(),
                expected_revision: Some(revision),
                command: PresentationCommand::SurfaceSnapshot {
                    workspace_id: workspace_id.into(),
                    surface_id: value["surface_id"]
                        .as_str()
                        .ok_or("Missing surface_id")?
                        .into(),
                    frame: value["frame"].clone(),
                    data: serde_json::from_value(value["data"].clone())?,
                },
            };
            let receipt = exchange(port, token, &request)?;
            revision = receipt["revision"]
                .as_u64()
                .ok_or("Missing target revision")?;
            if receipt["ok"] != true {
                let error = receipt["error"].as_str().unwrap_or("Invalid target error");
                if error == "Snapshot target is not active" {
                    if last_wait_log.elapsed() >= std::time::Duration::from_secs(5) {
                        eprintln!("canvas-tvctl: waiting for target workspace");
                        last_wait_log = std::time::Instant::now();
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    break;
                }
                if error != "Stale or missing endpoint revision; inspect this target first" {
                    return Err(format!("Snapshot rejected: {error}").into());
                }
            }
        }
    }
    Ok(())
}

/// Generic host source -> existing revision-bound widget patch. No agent protocol here.
fn watch_widget(
    serial: &str,
    workspace: &str,
    surface_id: &str,
    source: &[String],
) -> Result<(), Box<dyn std::error::Error>> {
    let invoke = |request: &str| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg(serial)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or("No TV request pipe")?
            .write_all(request.as_bytes())?;
        let output = child.wait_with_output()?;
        if !output.status.success() {
            return Err("TV control request failed".into());
        }
        Ok(serde_json::from_slice(&output.stdout)?)
    };
    loop {
        let update = (|| -> Result<(), Box<dyn std::error::Error>> {
            let observed = invoke("inspect\n")?;
            if observed["data"]["catalog"]["selected_project"] != workspace {
                return Ok(());
            }
            let mut document: canvas_workspace::WorkspaceDocument =
                serde_json::from_value(observed["data"]["document"].clone())?;
            let Some(surface) = document.surfaces.iter_mut().find(|s| s.id == surface_id) else {
                return Err("Named widget surface is not installed".into());
            };
            let canvas_workspace::SurfaceContent::Widget { spec } = &mut surface.content else {
                return Err("Named surface is not a widget".into());
            };
            let output = Command::new(&source[0]).args(&source[1..]).output()?;
            if output.stdout.len() > MAX_FRAME {
                return Err("Source snapshot exceeds limit".into());
            }
            if output.status.success() {
                let payload: serde_json::Value = serde_json::from_slice(&output.stdout)?;
                let mut encoded = serde_json::to_value(&*spec)?;
                let kind = payload["widget"].as_object().ok_or("Missing widget kind")?;
                // Replace kind body, preserving surface/spec identity and theme.
                let common = [
                    "schema_version",
                    "id",
                    "chrome",
                    "data",
                    "refresh_ms",
                    "theme",
                ];
                encoded
                    .as_object_mut()
                    .ok_or("Invalid widget spec")?
                    .retain(|key, _| common.contains(&key.as_str()));
                for (key, value) in kind {
                    encoded[key] = value.clone();
                }
                encoded["data"] = payload["data"].clone();
                *spec = serde_json::from_value(encoded)?;
            } else {
                // Preserve last values, but do not pretend a failed read is live.
                let mut encoded = serde_json::to_value(&*spec)?;
                encoded["data"]["state"] = serde_json::json!("disconnected");
                encoded["data"]["detail"] = serde_json::json!("Owner telemetry unavailable");
                *spec = serde_json::from_value(encoded)?;
                eprintln!("canvas-tvctl: widget source failed; marking disconnected");
            }
            let patch = canvas_workspace::WorkspacePatch {
                schema_version: 1,
                workspace_id: document.id.clone(),
                base_revision: document.revision,
                revision: document
                    .revision
                    .checked_add(1)
                    .ok_or("Revision overflow")?,
                operations: vec![canvas_workspace::PatchOperation::UpsertSurface {
                    surface: surface.clone(),
                }],
            };
            let request = Request {
                schema_version: 1,
                request_id: "widget-watch".into(),
                expected_revision: observed["revision"].as_u64(),
                command: PresentationCommand::ApplyPatch { patch },
            };
            invoke(&serde_json::to_string(&request)?)?;
            Ok(())
        })();
        if let Err(error) = update {
            eprintln!("canvas-tvctl: widget-watch: {error}");
        }
        std::thread::sleep(std::time::Duration::from_secs(10));
    }
}

fn exchange(
    port: u16,
    token: &[u8],
    request: &Request,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    exchange_value(port, token, &serde_json::to_value(request)?)
}

fn exchange_value(
    port: u16,
    token: &[u8],
    request: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_secs(3),
    )?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(std::time::Duration::from_secs(5)))?;
    stream.write_all(token)?;
    stream.write_all(b"\n")?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let mut bytes = Vec::new();
    BufReader::new(stream.take(MAX_FRAME as u64 + 1)).read_until(b'\n', &mut bytes)?;
    if bytes.len() > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err("Incomplete or oversized TV receipt".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
