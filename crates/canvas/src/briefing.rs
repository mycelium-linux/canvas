//! Briefing runner: paced, seekable presentations over the whole Canvas.
//!
//! A briefing is a sequence of beats authored as Lua scripts in
//! `config/briefings/`. Each beat drives Canvas state (background, theme,
//! surfaces, layout, focus) and optionally requests TTS narration through
//! JARVIS.
//!
//! Producers emit beat events via the stream socket; the binder renders each
//! beat into a [`BriefingBeat`] containing workspace patch operations.
//! Canvas applies the patches atomically to the active workspace document —
//! the same `WorkspacePatch` path that `ApplyDocument` and `ApplyPatch` use.
//!
//! Pacing is a property of the producer, not of Canvas. Canvas renders beats
//! as they arrive and applies them immediately, keeping the frame loop free of
//! scheduler logic.

use crate::stream_binding::{self, StreamEnvelope};
use canvas_widgets::BriefingBeat;
use std::path::PathBuf;

/// Load a briefing Lua binder from runtime config or the compiled-in fallback.
///
/// Checks `runtime_config::read("briefings/<name>.lua")` first, then falls
/// back to `include_str!` for built-in briefings. This makes a briefing editable
/// at runtime without a rebuild.
pub fn load_briefing(name: &str) -> Result<Vec<u8>, String> {
    let relative = PathBuf::from("briefings").join(format!("{name}.lua"));
    if let Some(bytes) =
        crate::runtime_config::read(&relative, stream_binding::MAX_ADAPTER_BYTES as u64)
            .map_err(|error| error.to_string())?
    {
        return Ok(bytes);
    }
    let built_in = match name {
        "intro-v1" => include_str!("../../../config/briefings/intro-v1.lua"),
        "speedy-layout" => include_str!("../../../config/briefings/speedy-layout.lua"),
        _ => {
            return Err(format!("unknown briefing: {name}"));
        }
    };
    Ok(built_in.as_bytes().to_vec())
}

/// Render one beat event through the briefing binder.
///
/// The event must carry `payload.briefing` (the briefing name) and
/// `payload.beat` (the zero-based index). The binder source is loaded from
/// runtime config or the built-in path, then executed in the same sandbox
/// as widget adapters.
pub fn render_beat(event: &StreamEnvelope) -> Result<BriefingBeat, String> {
    let payload = &event.payload;
    let name = payload
        .get("briefing")
        .and_then(|v| v.as_str())
        .ok_or("briefing beat event must include payload.briefing")?;
    let source = load_briefing(name)?;
    let source_str = std::str::from_utf8(&source)
        .map_err(|error| format!("Canvas briefing is not UTF-8: {error}"))?;
    stream_binding::apply_briefing_binder(source_str, event).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::SystemTime;

    fn envelope(beat: u32, briefing: &str) -> StreamEnvelope {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        StreamEnvelope {
            schema_version: stream_binding::STREAM_SCHEMA_VERSION,
            owner: "machine".into(),
            stream: "briefing".into(),
            contract: stream_binding::BRIEFING_CONTRACT.into(),
            project_id: None,
            sequence: 1,
            emitted_at_ms: now,
            payload: json!({ "beat": beat, "briefing": briefing }),
        }
    }

    #[test]
    fn intro_briefing_beats_render_correctly() {
        for beat in 0..6u32 {
            let env = envelope(beat, "intro-v1");
            let result = render_beat(&env).unwrap_or_else(|e| {
                panic!("beat {beat} failed: {e}");
            });
            assert_eq!(result.beat, beat);
            assert!(
                !result.patches.is_empty(),
                "beat {beat} should have patches"
            );
        }
    }

    #[test]
    fn past_end_beat_reports_done_with_message() {
        let env = envelope(99, "intro-v1");
        let result = render_beat(&env).unwrap();
        assert!(result.done);
        assert!(result.message.is_some());
        assert!(result.patches.is_empty());
    }

    #[test]
    fn missing_briefing_name_is_rejected() {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        let env = StreamEnvelope {
            schema_version: stream_binding::STREAM_SCHEMA_VERSION,
            owner: "machine".into(),
            stream: "briefing".into(),
            contract: stream_binding::BRIEFING_CONTRACT.into(),
            project_id: None,
            sequence: 1,
            emitted_at_ms: now,
            payload: json!({ "beat": 0 }),
        };
        assert!(render_beat(&env).is_err());
    }

    #[test]
    fn load_briefing_finds_built_in() {
        let bytes = load_briefing("intro-v1").unwrap();
        assert!(!bytes.is_empty(), "built-in briefing must not be empty");
        assert!(
            std::str::from_utf8(&bytes).unwrap().contains("narrate"),
            "briefing must contain narrate field"
        );
    }

    #[test]
    fn load_briefing_unknown_name_is_rejected() {
        assert!(load_briefing("nope").is_err());
    }
}
