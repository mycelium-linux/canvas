//! Render a named workspace in an isolated headless compositor.
//!
//! The live Canvas process, active workspace, and display are never touched.

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("canvas-workspace-capture currently requires Linux and Weston headless");
    std::process::exit(1);
}

#[cfg(target_os = "linux")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    linux::run()
}

#[cfg(target_os = "linux")]
mod linux {
    use canvas::control::{
        CONTROL_VERSION, Command, ControlReply, ControlRequest, Outcome, Receipt,
    };
    use std::{
        fs,
        io::{BufRead, BufReader, Write},
        os::unix::{fs::PermissionsExt, net::UnixStream},
        path::{Path, PathBuf},
        process::{Child, Command as ProcessCommand, Stdio},
        thread,
        time::{Duration, Instant, SystemTime, UNIX_EPOCH},
    };

    struct Children(Vec<Child>);

    impl Drop for Children {
        fn drop(&mut self) {
            for child in self.0.iter_mut().rev() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    pub fn run() -> Result<(), Box<dyn std::error::Error>> {
        let mut args = std::env::args().skip(1);
        let project = args
            .next()
            .ok_or("usage: canvas-workspace-capture <workspace> <output.png> [WIDTHxHEIGHT]")?;
        let output = PathBuf::from(args.next().ok_or("missing output PNG path")?);
        let size = args.next().unwrap_or_else(|| "1280x720".into());
        let (width, height) = parse_size(&size)?;
        if args.next().is_some() {
            return Err("too many arguments".into());
        }

        let nonce = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let runtime =
            std::env::temp_dir().join(format!("canvas-capture-{}-{nonce}", std::process::id()));
        fs::create_dir(&runtime)?;
        fs::set_permissions(&runtime, fs::Permissions::from_mode(0o700))?;
        let _runtime = RuntimeDir(runtime.clone());
        let wayland = "wayland-capture";
        let mut children = Children(Vec::new());

        children.0.push(
            ProcessCommand::new("weston")
                .args([
                    "--backend=headless-backend.so",
                    &format!("--socket={wayland}"),
                    &format!("--width={width}"),
                    &format!("--height={height}"),
                    "--idle-time=0",
                ])
                .env("XDG_RUNTIME_DIR", &runtime)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        );
        wait_for(&runtime.join(wayland), Duration::from_secs(5))?;

        let current = std::env::current_exe()?;
        let canvas = current
            .parent()
            .ok_or("capture binary has no parent directory")?
            .join("canvas");
        let store = canvas::workspace_store::default_path()?;
        children.0.push(
            ProcessCommand::new(canvas)
                .env("XDG_RUNTIME_DIR", &runtime)
                .env("WAYLAND_DISPLAY", wayland)
                .env("CANVAS_WORKSPACE_STORE_PATH", &store)
                .env("CANVAS_BOOT_PROJECT", &project)
                .env("CANVAS_CAPTURE_FRAME", &size)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        );
        let socket = runtime.join("dock/canvas/control.sock");
        wait_for(&socket, Duration::from_secs(10))?;
        thread::sleep(Duration::from_secs(2));

        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: "workspace-capture".into(),
            expected_revision: None,
            actor: None,
            command: Command::Screenshot {
                surface_id: None,
                region: None,
            },
        };
        let reply = send(&socket, &request)?;
        let Receipt::ScreenshotCaptured {
            path,
            width: actual_width,
            height: actual_height,
            ..
        } = (match reply.outcome {
            Outcome::Ok { receipt } => receipt,
            Outcome::Error { message, .. } => return Err(message.into()),
        })
        else {
            return Err("Canvas returned an unexpected capture receipt".into());
        };
        if let Some(parent) = output.parent().filter(|path| !path.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        fs::copy(path, &output)?;
        println!("{} {}x{}", output.display(), actual_width, actual_height);
        Ok(())
    }

    struct RuntimeDir(PathBuf);
    impl Drop for RuntimeDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn parse_size(value: &str) -> Result<(u32, u32), Box<dyn std::error::Error>> {
        let (width, height) = value.split_once('x').ok_or("size must be WIDTHxHEIGHT")?;
        let pair = (width.parse()?, height.parse()?);
        if !(64..=7680).contains(&pair.0) || !(64..=4320).contains(&pair.1) {
            return Err("capture size is out of range".into());
        }
        Ok(pair)
    }

    fn wait_for(path: &Path, timeout: Duration) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = Instant::now() + timeout;
        while !path.exists() {
            if Instant::now() >= deadline {
                return Err(format!("timed out waiting for {}", path.display()).into());
            }
            thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    fn send(
        path: &Path,
        request: &ControlRequest,
    ) -> Result<ControlReply, Box<dyn std::error::Error>> {
        let mut stream = UnixStream::connect(path)?;
        serde_json::to_writer(&mut stream, request)?;
        stream.write_all(b"\n")?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line)?;
        Ok(serde_json::from_str(&line)?)
    }
}
