//! Versioned, same-user control channel to the separately installed Accessibility app.
//!
//! Window and Accessibility work needs that app, because only it holds TCC
//! access. Reading the display inventory does not: see [`displays`].
#[cfg(target_os = "macos")]
pub mod displays;

use serde::{Deserialize, Serialize};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::PathBuf,
    time::Duration,
};
pub const VERSION: u32 = 2;
pub const MAX_MESSAGE: u64 = 1024 * 1024;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Display {
    pub id: String,
    pub name: String,
    pub frame: Rect,
    pub visible_frame: Rect,
    pub primary: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Window {
    pub id: String,
    pub pid: u32,
    pub app: String,
    pub title: String,
    pub frame: Rect,
    pub movable: bool,
    pub resizable: bool,
    pub minimized: bool,
    pub fullscreen: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowSelector {
    #[serde(default)]
    pub id: String,
    pub pid: u32,
    pub app: String,
    pub title: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Status,
    RequestPermission,
    Snapshot,
    Displays,
    Windows,
    Resolve { selectors: Vec<WindowSelector> },
    SetFrame { id: String, frame: Rect },
    Focus { id: String },
    SetFullscreen { id: String, fullscreen: bool },
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub version: u32,
    pub operation: Operation,
}
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum Response {
    Status {
        trusted: bool,
    },
    Snapshot {
        displays: Vec<Display>,
        windows: Vec<Window>,
    },
    Displays {
        displays: Vec<Display>,
    },
    Windows {
        windows: Vec<Window>,
        warnings: Vec<String>,
    },
    Resolved {
        windows: Vec<Window>,
    },
    Applied {
        window: Window,
    },
    Error {
        message: String,
    },
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Reply {
    pub version: u32,
    pub response: Response,
}
pub fn socket_path() -> Result<PathBuf, String> {
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
            .join("Library/Application Support/Dock/control/control.sock"),
    )
}
pub fn read_message(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut line = Vec::new();
    BufReader::new(reader.take(MAX_MESSAGE + 1))
        .read_until(b'\n', &mut line)
        .map_err(|e| e.to_string())?;
    if line.len() as u64 > MAX_MESSAGE || line.last() != Some(&b'\n') {
        return Err("message exceeds limit or lacks newline".into());
    }
    Ok(line)
}
#[cfg(unix)]
pub fn request(operation: Operation) -> Result<Response, String> {
    use std::os::unix::{fs::MetadataExt, net::UnixStream};
    let path = socket_path()?;
    let meta = std::fs::symlink_metadata(&path).map_err(|e| {
        format!("Dock Control unavailable: {e}; install and launch ~/Applications/Dock Control.app")
    })?;
    if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
        return Err("unsafe helper socket ownership or permissions".into());
    }
    let mut stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
    verify_peer(&stream)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let mut bytes = serde_json::to_vec(&Request {
        version: VERSION,
        operation,
    })
    .map_err(|e| e.to_string())?;
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|e| e.to_string())?;
    let reply: Reply = serde_json::from_slice(&read_message(stream)?).map_err(|e| e.to_string())?;
    if reply.version != VERSION {
        return Err(
            "Dock Control protocol version mismatch; explicit helper upgrade required".into(),
        );
    }
    match reply.response {
        Response::Error { message } => Err(message),
        response => Ok(response),
    }
}
#[cfg(unix)]
pub fn verify_peer(stream: &std::os::unix::net::UnixStream) -> Result<(), String> {
    use std::os::fd::AsRawFd;
    #[cfg(target_os = "macos")]
    let uid = {
        let (mut uid, mut gid) = (0, 0);
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        uid
    };
    #[cfg(target_os = "linux")]
    let uid = {
        let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of_val(&cred) as libc::socklen_t;
        if unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut cred as *mut _ as *mut _,
                &mut len,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        cred.uid
    };
    if uid != unsafe { libc::geteuid() } {
        return Err("helper peer is not the current user".into());
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unbounded_and_unterminated() {
        assert!(read_message(&b"{}"[..]).is_err());
        assert!(read_message(vec![b'x'; MAX_MESSAGE as usize + 1].as_slice()).is_err());
    }
    #[test]
    fn protocol_round_trip() {
        let req = Request {
            version: VERSION,
            operation: Operation::SetFrame {
                id: "instance:42".into(),
                frame: Rect {
                    x: -1920,
                    y: 0,
                    width: 1920,
                    height: 1080,
                },
            },
        };
        let bytes = serde_json::to_vec(&req).unwrap();
        assert!(matches!(
            serde_json::from_slice::<Request>(&bytes).unwrap().operation,
            Operation::SetFrame { .. }
        ));
        assert!(serde_json::from_str::<Operation>(r#"{"op":"shell","command":"ls"}"#).is_err());
        let resolve = Operation::Resolve {
            selectors: vec![WindowSelector {
                id: "helper:1".into(),
                pid: 42,
                app: "Safari".into(),
                title: "Project".into(),
            }],
        };
        let value = serde_json::to_value(resolve).unwrap();
        assert_eq!(value["op"], "resolve");
        assert!(serde_json::from_value::<Operation>(serde_json::json!({
            "op":"resolve",
            "selectors":[{"id":"x","pid":1,"app":"Safari","title":"Page","command":"bad"}]
        }))
        .is_err());
    }
    #[cfg(unix)]
    #[test]
    fn same_user_peer() {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        assert!(verify_peer(&a).is_ok());
        assert!(verify_peer(&b).is_ok());
    }
}
