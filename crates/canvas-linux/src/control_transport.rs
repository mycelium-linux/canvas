//! Bounded newline framing shared by compositor clients and server.
use std::io::{self, BufRead, Write};
pub const MAX_FRAME: usize = 64 * 1024;
pub const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
pub const QUEUE_CAPACITY: usize = 16;

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
    fn binding_never_overwrites_an_existing_endpoint() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!(
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
