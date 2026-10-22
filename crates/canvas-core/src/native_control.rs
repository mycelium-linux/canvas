//! Bounded newline framing shared by compositor clients and server.
use std::io::{self, BufRead, Write};
pub const MAX_FRAME: usize = 64 * 1024;
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub const QUEUE_CAPACITY: usize = 16;
pub const SWITCHER_APP_ID: &str = "dev.fpl.canvas.switcher";

/// One geometry source for the native output and its Lua presentation window.
pub fn switcher_frame(area: crate::layout::Rect) -> crate::layout::Rect {
    let width = area.width.saturating_sub(48).clamp(1, 900);
    let height = area.height.saturating_sub(48).clamp(1, 180);
    crate::layout::Rect {
        x: area.x + ((area.width.saturating_sub(width)) / 2) as i32,
        y: area.y + ((area.height.saturating_sub(height)) / 2) as i32,
        width,
        height,
    }
}

/// One bounded owner-local request. A timed-out mutation has unknown outcome.
pub fn send_command(
    socket: &std::path::Path,
    command: &serde_json::Value,
) -> Result<String, String> {
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
    Ok(buf.trim().to_owned())
}

pub fn read_frame(reader: &mut impl BufRead) -> io::Result<String> {
    let mut bytes = Vec::new();
    let deadline = std::time::Instant::now() + TIMEOUT;
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control frame deadline exceeded",
            ));
        }
        let available = reader.fill_buf()?;
        if available.is_empty() {
            break;
        }
        let length = available
            .iter()
            .position(|b| *b == b'\n')
            .map_or(available.len(), |i| i + 1);
        if bytes.len() + length > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "oversized control frame",
            ));
        }
        let complete = available[length - 1] == b'\n';
        bytes.extend_from_slice(&available[..length]);
        reader.consume(length);
        if complete {
            break;
        }
    }
    if bytes.len() > MAX_FRAME || bytes.last() != Some(&b'\n') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "oversized or unterminated control frame",
        ));
    }
    String::from_utf8(bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
pub fn write_frame(writer: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() >= MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "control response exceeds limit",
        ));
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")
}

pub fn bind(path: &std::path::Path) -> io::Result<std::os::unix::net::UnixListener> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("socket needs private parent"))?;
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "control socket parent must be owner-only",
        ));
    }
    // Never unlink an unknown path or steal an existing compositor's socket.
    let listener = std::os::unix::net::UnixListener::bind(path)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn switcher_geometry_is_centered_and_contained_even_on_small_outputs() {
        for (width, height) in [(1920, 1080), (1280, 720), (320, 120), (1, 1)] {
            let area = crate::layout::Rect {
                x: -100,
                y: 50,
                width,
                height,
            };
            let frame = switcher_frame(area);
            assert!(frame.width > 0 && frame.height > 0);
            assert!(frame.x >= area.x && frame.y >= area.y);
            assert!(frame.x + frame.width as i32 <= area.x + width as i32);
            assert!(frame.y + frame.height as i32 <= area.y + height as i32);
        }
    }
    #[test]
    fn binding_never_overwrites_an_existing_endpoint() {
        use std::os::unix::fs::PermissionsExt;
        // macOS TMPDIR can itself consume most of sockaddr_un's 104 bytes.
        let dir = std::path::PathBuf::from("/tmp").join(format!(
            "canvas-control-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("control.sock");
        let listener = bind(&path).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(bind(&path).is_err());
        drop(listener);
        std::fs::remove_file(&path).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(bind(&path).is_err());
        std::fs::remove_dir(&dir).unwrap();
    }
    #[test]
    fn frames_are_bounded_complete_and_utf8() {
        assert_eq!(read_frame(&mut &b"{}\n"[..]).unwrap(), "{}\n");
        assert!(read_frame(&mut &b"{}"[..]).is_err());
        assert!(read_frame(&mut &b"\xff\n"[..]).is_err());
        assert!(read_frame(&mut vec![b'x'; MAX_FRAME + 1].as_slice()).is_err());
        let mut output = Vec::new();
        assert!(write_frame(&mut output, &serde_json::json!("x".repeat(MAX_FRAME))).is_err());
        assert!(output.is_empty());
    }
}
