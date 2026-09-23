//! The shape of a local stream event, without the machinery that fills it in.
//!
//! Lives outside `stream_binding` because that module needs a Lua VM, and a
//! producer that only wants to hand Canvas one widget snapshot should not have
//! to link an interpreter to describe the envelope. Describing the contract and
//! transforming events into it are different jobs.

use serde::{Deserialize, Serialize};

use crate::{Error, Result};
// The canonical registry lives in canvas-core; stream_binding re-exports it
// for its own callers, and validation needs it here.
use canvas_core::is_declared_contract;

pub const STREAM_SCHEMA_VERSION: u32 = 1;

/// Contract a briefing beat is published under.
pub const BRIEFING_CONTRACT: &str = "canvas.briefing.beat.v1";


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

impl StreamEnvelope {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != STREAM_SCHEMA_VERSION {
            return Err(Error("unsupported stream schema version".into()));
        }
        for (name, value) in [("owner", &self.owner), ("stream", &self.stream)] {
            if value.is_empty()
                || value.len() > 64
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            {
                return Err(Error(format!("invalid stream {name}")));
            }
        }
        if self.contract.is_empty() || self.contract.len() > 128 {
            return Err(Error("invalid stream contract".into()));
        }
        if !is_declared_contract(&self.contract) {
            return Err(Error(format!(
                "undeclared stream contract: {}; add it to DECLARED_CONTRACTS in stream_binding.rs",
                self.contract
            )));
        }
        Ok(())
    }

    pub fn age_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.emitted_at_ms)
    }
}
