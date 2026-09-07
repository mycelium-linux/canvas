#[cfg(target_os = "macos")]
mod native;
#[cfg(target_os = "macos")]
fn run() -> Result<(), String> {
    use dock_control::*;
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        os::{
            fd::AsRawFd,
            unix::{
                fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
                net::UnixListener,
            },
        },
        time::Duration,
    };
    // This app is an independent LaunchServices process, never a child Accessibility script.
    unsafe {
        libc::umask(0o077);
    }
    let socket = socket_path()?;
    let dir = socket.parent().unwrap();
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let meta = fs::symlink_metadata(dir).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("control directory must be owned by current user with mode 0700".into());
    }
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(dir.join("server.lock"))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("Dock Control is already running".into());
    }
    match fs::remove_file(&socket) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => return Err(e.to_string()),
    }
    let listener = UnixListener::bind(&socket).map_err(|e| e.to_string())?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let mut native = native::Native::new();
    loop {
        // Service AppKit's main run loop between commands.
        unsafe {
            core_foundation_sys::runloop::CFRunLoopRunInMode(
                core_foundation_sys::runloop::kCFRunLoopDefaultMode,
                0.05,
                1,
            );
        }
        let (mut stream, _) = match listener.accept() {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        if let Err(e) = verify_peer(&stream) {
            eprintln!("rejected peer: {e}");
            continue;
        }
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .map_err(|e| e.to_string())?;
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|e| e.to_string())?;
        let result = (|| {
            let request: Request =
                serde_json::from_slice(&read_message(&stream)?).map_err(|e| e.to_string())?;
            if request.version != VERSION {
                return Err("unsupported protocol version".into());
            }
            native.execute(request.operation)
        })();
        let reply = Reply {
            version: VERSION,
            response: result.unwrap_or_else(|message| Response::Error { message }),
        };
        let mut bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        if bytes.len() as u64 > MAX_MESSAGE {
            bytes = serde_json::to_vec(&Reply {
                version: VERSION,
                response: Response::Error {
                    message: "inventory exceeds protocol limit".into(),
                },
            })
            .unwrap();
            bytes.push(b'\n')
        }
        if let Err(e) = stream.write_all(&bytes) {
            eprintln!("control reply failed: {e}")
        }
    }
}
fn main() {
    #[cfg(target_os = "macos")]
    if let Err(e) = run() {
        eprintln!("Dock Control: {e}");
        std::process::exit(1)
    }
    #[cfg(not(target_os = "macos"))]
    {
        eprintln!("Dock Control requires macOS");
        std::process::exit(1)
    }
}
