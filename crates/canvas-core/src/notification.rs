//! Shared notification contract. Canvas owns visual history; JARVIS owns
//! interruption policy and audible delivery.

use serde::{Deserialize, Serialize};

pub const CONTRACT: &str = "jarvis.notification.v1";
pub const MAX_HISTORY: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Neutral,
    Good,
    Warning,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Notification {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub message: String,
    pub level: Level,
    pub created_at_ms: u64,
    #[serde(default)]
    pub announce: bool,
}

impl Notification {
    pub fn validate(&self) -> Result<(), String> {
        bounded("id", &self.id, 128)?;
        bounded("title", &self.title, 160)?;
        bounded("message", &self.message, 1_000)?;
        if let Some(project_id) = &self.project_id {
            bounded("project_id", project_id, 128)?;
        }
        if self.created_at_ms == 0 {
            return Err("notification timestamp must be nonzero".into());
        }
        Ok(())
    }

    pub fn speech(&self) -> String {
        if self.message.trim().is_empty() {
            self.title.clone()
        } else {
            format!("{}. {}", self.title.trim_end_matches('.'), self.message)
        }
    }
}

fn bounded(label: &str, value: &str, max: usize) -> Result<(), String> {
    if value.trim().is_empty() || value.chars().count() > max || value.chars().any(char::is_control)
    {
        return Err(format!("notification {label} is invalid"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_is_deterministic_and_bounded_input_is_enforced() {
        let notification = Notification {
            id: "build-done".into(),
            project_id: Some("transmog".into()),
            title: "Build complete".into(),
            message: "Revision seven is ready".into(),
            level: Level::Good,
            created_at_ms: 1,
            announce: true,
        };
        notification.validate().unwrap();
        assert_eq!(
            notification.speech(),
            "Build complete. Revision seven is ready"
        );
    }
}
