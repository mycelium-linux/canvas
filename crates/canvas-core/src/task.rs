//! Generic task lifecycle shared by launchers, bridges, and Canvas views.

use serde::{Deserialize, Serialize};

pub const CONTRACT: &str = "canvas.task.status.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRef {
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskStatus {
    pub task_id: String,
    pub kind: String,
    pub state: String,
    pub target: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
    /// Bounded, human-readable progress emitted by the task owner.
    #[serde(default)]
    pub activity: Vec<String>,
}
