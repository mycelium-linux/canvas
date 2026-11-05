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

    unsafe {
        let flags = libc::fcntl(stdout_fd, libc::F_GETFL);
        if flags >= 0 {
            libc::fcntl(stdout_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
        }
    }

    let pid = child.id();
    std::mem::forget(child);

    Ok((stdout_fd, stdin_fd, pid))
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
