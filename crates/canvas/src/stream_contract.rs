//! The shape of a local stream event, without the machinery that fills it in.
//!
//! Lives outside `stream_binding` because that module needs a Lua VM, and a
//! producer that only wants to hand Canvas one widget snapshot should not have
//! to link an interpreter to describe the envelope. Describing the contract and
//! transforming events into it are different jobs.

use serde::{Deserialize, Serialize};

pub const STREAM_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamEnvelope {
    pub schema_version: u32,
    pub owner: String,
    pub stream: String,
    pub contract: String,
    pub sequence: u64,
    pub emitted_at_ms: u64,
    /// Optional project scope. When present, Canvas only routes the event to
    /// a workspace whose `project_id` matches — preventing project X's activity
    /// from landing in project Y's surface. Falls back to the legacy
    /// owner/stream/contract matching when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub payload: serde_json::Value,
}
