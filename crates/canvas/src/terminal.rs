use std::os::unix::io::{AsRawFd, IntoRawFd};
use std::os::unix::io::RawFd;
use std::process::{Command, Stdio};

pub fn spawn_terminal(shell: &str) -> Result<(i32, i32, u32), String> {
    // `script` allocates a pty for the child, which is the difference between an
    // interactive program and one that thinks it is talking to a file: a shell on
    // pipes gets no prompt, no echo, and no job control. This is the smallest way
    // to get a real terminal without a pty abstraction in this crate.
    let mut child = Command::new("/usr/bin/script");
    // A GUI app inherits no terminal environment. Without TERM every curses
    // program refuses to start, and the shell's own line editor degrades; the
    // surface looks broken for a reason that has nothing to do with the surface.
    let mut child = child
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .env("TERM_PROGRAM", "canvas")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Wide on purpose: the pty's size is fixed for the life of the process
        // (there is no descriptor here to ioctl), and a shell that thinks it has
        // 80 columns wraps its prompt at 80 even when the surface is wider. A
        // generous width lets the *emulator's* grid decide where lines break.
        .args([
            "-q",
            "/dev/null",
            "/bin/sh",
            "-c",
            &format!("stty cols 200 rows 50 2>/dev/null; exec {shell}"),
        ])
        .spawn()
        .map_err(|e| format!("spawn {shell} failed: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "no stdout from shell".to_string())?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "no stdin to shell".to_string())?;

    // Take ownership of the descriptors. Dropping the `File`s here closed both
    // ends of the pipe immediately, so the surface never received a byte and
    // typing went into a closed descriptor: a terminal that was connected to
    // nothing, which looked exactly like a terminal that had crashed.
    let stdout_fd = stdout.into_raw_fd();
    let stdin_fd = stdin.into_raw_fd();

    // Deliberately left *blocking*: the read lives on its own thread and is
    // woken by the kernel when the program writes. Non-blocking here would have
    // meant a poll loop, which is latency added to every keystroke's echo.
    let pid = child.id();
    std::mem::forget(child);

    Ok((stdout_fd, stdin_fd, pid))
}

/// Read a terminal's output forever, handing each chunk to `sink`.
///
/// Blocking, on its own thread: a poll loop adds its interval to every keystroke's
/// echo, and there is no reason for a terminal to be polled when the kernel will
/// wake a thread the moment bytes exist.
pub fn pump(
    fd: RawFd,
    surface_id: String,
    sink: tokio::sync::mpsc::UnboundedSender<(String, Vec<u8>)>,
) {
    std::thread::Builder::new()
        .name(format!("terminal-{surface_id}"))
        .spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
                if read > 0 {
                    let bytes = buffer[..read as usize].to_vec();
                    if sink.send((surface_id.clone(), bytes)).is_err() {
                        return;
                    }
                } else if read == 0 {
                    return;
                } else {
                    let error = std::io::Error::last_os_error();
                    match error.kind() {
                        std::io::ErrorKind::Interrupted => continue,
                        std::io::ErrorKind::WouldBlock => {
                            // Still non-blocking from an earlier life: wait a
                            // millisecond rather than spin, and let the reader
                            // settle into blocking mode on the next round.
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        _ => return,
                    }
                }
            }
        })
        .ok();
}

pub fn read_fd(fd: RawFd) -> Vec<u8> {
    let mut buf = [0u8; 4096];
    unsafe {
        let ret = libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len());
        if ret <= 0 {
            return Vec::new();
        }
        buf[..ret as usize].to_vec()
    }
}

pub fn write_fd(fd: RawFd, data: &[u8]) -> Result<(), String> {
    unsafe {
        let ret = libc::write(fd, data.as_ptr() as *const _, data.len());
        if ret < 0 {
            return Err(format!(
                "write to terminal failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub fn kill_pid(pid: u32) {
    unsafe {
        libc::kill(pid as i32, libc::SIGTERM);
    }
}

#[allow(dead_code)]
pub fn close_fd(fd: RawFd) {
    unsafe {
        libc::close(fd);
    }
}
