//! Local, transport-neutral workspace document publication.
//!
//! Canvas sends complete documents because they are small, independently
//! validated snapshots. The edge may collapse superseded snapshots under
//! backpressure without reconstructing a patch chain.

use crate::WorkspaceDocument;
use serde::{Deserialize, Serialize};

pub const MIRROR_SCHEMA_VERSION: u32 = 1;
pub const MIRROR_KIND: &str = "canvas.workspace_state.v1";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalWorkspaceSnapshot {
    pub schema_version: u32,
    pub document: WorkspaceDocument,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceState {
    pub schema_version: u32,
    pub workspace_key: String,
    pub origin: String,
    pub sequence: u64,
    pub document: WorkspaceDocument,
}

#[cfg(unix)]
pub struct Publisher {
    stream: Option<std::os::unix::net::UnixStream>,
    path: Option<std::path::PathBuf>,
    last_sent: Vec<u8>,
}

#[cfg(unix)]
impl Publisher {
    pub fn new() -> Self {
        Self {
            stream: None,
            path: canvas_core::workspace_mirror_socket_path(),
            last_sent: Vec::new(),
        }
    }

    /// Best-effort local publication. Absence of an edge is a valid disabled
    /// state; serialization failures remain visible because a valid Canvas
    /// document must always serialize.
    pub fn publish_if_changed(&mut self, document: &WorkspaceDocument) {
        let snapshot = LocalWorkspaceSnapshot {
            schema_version: MIRROR_SCHEMA_VERSION,
            document: document.clone(),
        };
        let bytes = serde_json::to_vec(&snapshot)
            .expect("a validated Canvas workspace document must serialize");
        if bytes == self.last_sent {
            return;
        }
        let Some(path) = &self.path else { return };
        use std::io::Write;
        let mut frame = bytes.clone();
        frame.push(b'\n');
        let send = |stream: &mut std::os::unix::net::UnixStream| stream.write_all(&frame);
        if self.stream.is_none() {
            self.stream = std::os::unix::net::UnixStream::connect(path).ok();
        }
        let mut sent = self
            .stream
            .as_mut()
            .is_some_and(|stream| send(stream).is_ok());
        if !sent {
            self.stream = std::os::unix::net::UnixStream::connect(path).ok();
            sent = self
                .stream
                .as_mut()
                .is_some_and(|stream| send(stream).is_ok());
        }
        if sent {
            self.last_sent = bytes;
        } else {
            self.stream = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_state_round_trips_without_losing_the_document() {
        let document = WorkspaceDocument {
            schema_version: crate::SCHEMA_VERSION,
            id: "demo-home".into(),
            project_id: "demo".into(),
            revision: 1,
            theme: crate::ThemeRef::Project,
            background: crate::Background::Theme,
            chrome: crate::SurfaceChrome::default(),
            surfaces: Vec::new(),
            layout: crate::Layout::Canvas {
                placements: Vec::new(),
            },
            bindings: Vec::new(),
            focused_surface_id: None,
        };
        let state = WorkspaceState {
            schema_version: MIRROR_SCHEMA_VERSION,
            workspace_key: document.id.clone(),
            origin: "screen-a".into(),
            sequence: 7,
            document,
        };
        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: WorkspaceState = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, state);
    }
}
