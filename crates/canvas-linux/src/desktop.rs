//! Compatibility boundary for the retired headless Sway adapter.
use serde_json::Value;
pub fn execute(_args: &[String]) -> Result<Value, String> {
    Err("Headless Sway adapter retired. Use desktop-compositor start|status or desktop-compositor-control SOCKET JSON. Legacy project-scoped capture/input is not mapped to another seat.".into())
}
#[cfg(any(test, feature = "opencode"))]
pub fn open_session(_project: &str, _tmux: &str, _directory: &str) -> Result<Value, String> {
    Err("Sway terminal attachment retired; OpenCode still owns the session. Native terminal attachment is not implemented.".into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn retired_commands_do_not_retarget_another_seat() {
        assert!(
            super::execute(&["desktop-start".into()])
                .unwrap_err()
                .contains("retired")
        );
        assert!(super::open_session("studio", "session", "/tmp").is_err());
    }
}
