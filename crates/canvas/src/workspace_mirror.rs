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
    /// None preserves legacy same-workspace mirroring. Some(output) makes this
    /// a portable assignment that every peer retains and only that output
    /// presents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_output: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceState {
    pub schema_version: u32,
    pub workspace_key: String,
    pub origin: String,
    pub sequence: u64,
    pub document: WorkspaceDocument,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assigned_output: Option<String>,
}

#[cfg(unix)]
pub struct Publisher {
    stream: Option<std::os::unix::net::UnixStream>,
    path: Option<std::path::PathBuf>,
    last_sent: Option<LocalWorkspaceSnapshot>,
}

#[cfg(unix)]
impl Publisher {
    pub fn new() -> Self {
        Self {
            stream: None,
            path: canvas_core::workspace_mirror_socket_path(),
            last_sent: None,
        }
    }

    /// Best-effort local publication. Absence of an edge is a valid disabled
    /// state; serialization failures remain visible because a valid Canvas
    /// document must always serialize.
    pub fn publish_if_changed(
        &mut self,
        document: &WorkspaceDocument,
        assigned_output: Option<&str>,
    ) {
        let snapshot = LocalWorkspaceSnapshot {
            schema_version: MIRROR_SCHEMA_VERSION,
            document: document.clone(),
            assigned_output: assigned_output.map(str::to_owned),
        };
        if self.last_sent.as_ref().is_some_and(|previous| {
            previous.assigned_output == snapshot.assigned_output
                && same_representation(&previous.document, document)
        }) {
            return;
        }
        let bytes = serde_json::to_vec(&snapshot)
            .expect("a validated Canvas workspace document must serialize");
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
            self.last_sent = Some(snapshot);
        } else {
            self.stream = None;
        }
    }
}

/// Publish one complete workspace assignment through the local edge.
///
/// This is transport-neutral from the CLI's perspective: the edge owns the
/// Unibus route, credentials, retry policy, and remote output discovery.
#[cfg(unix)]
pub fn publish_assignment(
    document: &WorkspaceDocument,
    assigned_output: &str,
) -> Result<(), String> {
    use std::io::Write;
    let path = canvas_core::workspace_mirror_socket_path()
        .ok_or("Canvas workspace mirror socket is unavailable")?;
    let snapshot = LocalWorkspaceSnapshot {
        schema_version: MIRROR_SCHEMA_VERSION,
        document: document.clone(),
        assigned_output: Some(assigned_output.to_owned()),
    };
    let mut bytes = serde_json::to_vec(&snapshot).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    let mut stream = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|error| format!("connect {}: {error}", path.display()))?;
    stream
        .write_all(&bytes)
        .map_err(|error| format!("publish workspace assignment: {error}"))
}

/// Whether two documents describe the same board.
///
/// `revision` is a per-canvas counter rather than a property of what is on
/// screen, so it must stay out of the comparison. Applying a document always
/// bumps it, so any echo check that treats the result as a *different*
/// document will send it straight back to the peer it came from, and the two
/// canvases will escalate revisions against each other forever.
pub fn same_representation(left: &WorkspaceDocument, right: &WorkspaceDocument) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    left.revision = 0;
    right.revision = 0;
    left == right
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demo_document() -> WorkspaceDocument {
        WorkspaceDocument {
            pages: Vec::new(),
            active_page: 0,
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
        }
    }

    /// The loop that ran two canvases to revision 112,000 against each other:
    /// the echo check compared serialized snapshots, but applying a document
    /// bumps its revision, so the snapshot coming back never matched the bytes
    /// that went out. Every apply published an echo, and each echo was "newer"
    /// than the peer's copy, so both sides escalated without end.
    #[test]
    fn a_document_is_the_same_board_after_its_revision_is_bumped() {
        let document = demo_document();
        let mut applied = document.clone();
        applied.revision += 1;
        assert!(
            same_representation(&document, &applied),
            "a bumped revision is the same board and must not echo back"
        );
        assert_ne!(
            serde_json::to_vec(&document).unwrap(),
            serde_json::to_vec(&applied).unwrap(),
            "byte equality is exactly what failed here"
        );
    }

    #[test]
    fn a_real_edit_is_a_different_board() {
        let document = demo_document();
        let mut edited = document.clone();
        edited.revision += 1;
        edited.chrome.canvas_padding += 2;
        assert!(!same_representation(&document, &edited));
    }

    #[test]
    fn wire_state_round_trips_without_losing_the_document() {
        let document = demo_document();
        let state = WorkspaceState {
            schema_version: MIRROR_SCHEMA_VERSION,
            workspace_key: document.id.clone(),
            origin: "screen-a".into(),
            sequence: 7,
            document,
            assigned_output: Some("screen-tv".into()),
        };
        let encoded = serde_json::to_vec(&state).unwrap();
        let decoded: WorkspaceState = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, state);
    }

    #[test]
    fn revision_only_changes_do_not_republish_the_same_representation() {
        let document = WorkspaceDocument {
            pages: Vec::new(),
            active_page: 0,
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
        let mut later = document.clone();
        later.revision = 99;
        assert!(same_representation(&document, &later));
        later.id = "changed".into();
        assert!(!same_representation(&document, &later));
    }
}
