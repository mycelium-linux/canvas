//! Canvas's view of the shared bus (`FuturePresentLabs/unibus`).
//!
//! The contract itself lives in exactly one place; this module re-exports it
//! so no Canvas crate names the dependency directly, adds the canvas-domain
//! message kinds, and pins parity against the golden JARVIS v1 frames we must
//! keep accepting. Envelope builders emit the v1 kind spellings
//! (`canvas_command`) until every deployed hub route table carries the
//! namespaced v2 aliases (`canvas.command`).

pub use ::unibus::{
    AuthError, Control, DEFAULT_TTL_MS, Envelope, EnvelopeError, FrameError, MAX_FRAME,
    MAX_FUTURE_SKEW_MS, Message, NodeClaims, NodeGrant, OfflineVerifier, Qos, Role,
    SCHEMA_VERSION_V1, SCHEMA_VERSION_V2, envelope, framing, grants, kinds,
};

/// v1 tag every deployed hub routes today.
pub const COMMAND_KIND_V1: &str = "canvas_command";
/// v2 namespaced alias.
pub const COMMAND_KIND: &str = "canvas.command";
/// Execution receipt an edge sends back to the producer that commanded it.
pub const RESULT_KIND: &str = "canvas.result";
/// Edge attention state: which surfaces are actually on screen.
pub const VISIBILITY_KIND: &str = "canvas.visibility";

/// The receipt frame: `{request_id, ok, error?}` correlated to the
/// `ControlRequest` the edge executed. The hub routes this by producer back
/// to the central that sent the command.
pub fn result_message(request_id: &str, ok: bool, error: Option<String>) -> Message {
    Message::new(
        RESULT_KIND,
        serde_json::json!({
            "request_id": request_id,
            "ok": ok,
            "error": error,
        }),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultFrame {
    pub request_id: String,
    pub ok: bool,
    pub error: Option<String>,
}

/// Parse a receipt body that an edge emitted for a canvas_command envelope.
pub fn parse_result(message: &Message) -> Option<ResultFrame> {
    if message.kind != RESULT_KIND {
        return None;
    }
    Some(ResultFrame {
        request_id: message.body.get("request_id")?.as_str()?.to_owned(),
        ok: message.body.get("ok")?.as_bool()?,
        error: message
            .body
            .get("error")
            .and_then(|v| v.as_str())
            .map(str::to_owned),
    })
}
/// v1 capability tag; the v2 namespaced spelling for new grants.
pub const DISPLAY_CAPABILITY_V1: &str = "display";
pub const DISPLAY_CAPABILITY: &str = "canvas.display";

/// A canvas command envelope addressed to one screen anchor.
pub fn command_for_screen(
    installation_id: &str,
    producer_id: &str,
    token: &str,
    screen_anchor: &str,
    command: serde_json::Value,
) -> Envelope {
    Envelope::for_screen(
        installation_id,
        producer_id,
        token,
        screen_anchor,
        command_message(command),
    )
}

/// A canvas command envelope addressed to every display in a zone.
pub fn command_for_zone(
    installation_id: &str,
    producer_id: &str,
    token: &str,
    zone: &str,
    command: serde_json::Value,
) -> Envelope {
    Envelope::new(
        installation_id,
        producer_id,
        Some(zone),
        token,
        command_message(command),
    )
}

fn command_message(command: serde_json::Value) -> Message {
    Message::new(COMMAND_KIND_V1, serde_json::json!({ "command": command }))
}

/// The register body an edge Canvas must send first, claiming its screens.
pub fn register_message(capabilities: &[&str], zones: &[&str], screen_anchors: &[&str]) -> Message {
    Message::new(
        kinds::V1_REGISTER,
        serde_json::json!({
            "capabilities": capabilities,
            "zones": zones,
            "screen_anchors": screen_anchors,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// The exact frame `jarvis-core::edge_transport` emits today (its
    /// `canvas_command_round_trips` test vector). Canvas must keep parsing it.
    const GOLDEN_JARVIS_V1_CANVAS_COMMAND: &str = r#"{"schema_version":1,"installation_id":"home","producer_id":"mac-studio-brain","destination":null,"target_screen":null,"request_id":"router-7-8-9","issued_at_unix_ms":1700000000000,"ttl_ms":30000,"payload_bytes":0,"token":"t","message":{"message_type":"canvas_command","command":{"schema_version":1,"request_id":"test-1","command":{"upsert_indicator":{"id":"jarvis-voice","title":"Jarvis","state":"speaking"}}}}}"#;

    #[test]
    fn golden_jarvis_frame_parses_through_the_bus_codec() {
        let envelope: Envelope = serde_json::from_str(GOLDEN_JARVIS_V1_CANVAS_COMMAND).unwrap();
        assert_eq!(envelope.schema_version, SCHEMA_VERSION_V1);
        assert_eq!(envelope.message.kind, COMMAND_KIND_V1);
        assert_eq!(Qos::of_kind(&envelope.message.kind), Qos::Command);
        assert_eq!(
            envelope.message.body["command"]["command"]["upsert_indicator"]["state"],
            "speaking"
        );
    }

    #[test]
    fn golden_frame_survives_the_framing_round_trip() {
        let bytes = GOLDEN_JARVIS_V1_CANVAS_COMMAND.as_bytes();
        let mut wire = Vec::new();
        let envelope: Envelope = serde_json::from_slice(bytes).unwrap();
        framing::write_frame(&mut wire, &envelope, &[]).unwrap();
        let (read, payload) = framing::read_frame(&mut Cursor::new(wire)).unwrap();
        assert!(payload.is_empty());
        assert_eq!(read, envelope);
    }

    #[test]
    fn built_envelopes_keep_the_jarvis_wire_shape() {
        let built = command_for_screen(
            "home",
            "canvas-studio",
            "t",
            "screen-tv",
            serde_json::json!({"upsert_indicator": {"id": "voice", "state": "listening"}}),
        );
        let value = serde_json::to_value(&built).unwrap();
        assert_eq!(value["message"]["message_type"], COMMAND_KIND_V1);
        assert_eq!(value["target_screen"], "screen-tv");
        assert_eq!(
            value["message"]["command"]["upsert_indicator"]["state"],
            "listening"
        );
        assert_eq!(value["schema_version"], SCHEMA_VERSION_V2);
        assert_eq!(built.ttl_ms, Some(DEFAULT_TTL_MS));
    }

    #[test]
    fn zone_command_and_register_bodies_route_like_v1_expects() {
        let zone = command_for_zone(
            "home",
            "canvas-studio",
            "t",
            "living-room",
            serde_json::json!({}),
        );
        assert_eq!(zone.destination.as_deref(), Some("living-room"));
        assert!(zone.target_screen.is_none());

        let register = register_message(&[DISPLAY_CAPABILITY], &["living-room"], &["screen-tv"]);
        let envelope = Envelope::new("home", "home-pi-tv", None, "t", register);
        assert_eq!(kinds::classify(&envelope.message.kind), Control::Register);
        assert_eq!(envelope.message.body["capabilities"][0], DISPLAY_CAPABILITY);
        assert_eq!(envelope.message.body["screen_anchors"][0], "screen-tv");
    }

    #[test]
    fn payloads_declared_and_written_must_match() {
        let rgba = vec![0u8; 64 * 64 * 4];
        let mut envelope = command_for_zone(
            "home",
            "canvas-studio",
            "t",
            "default",
            serde_json::json!({"frame": true}),
        );
        envelope.message.kind = "canvas.image".into();
        envelope.payload_bytes = rgba.len();
        let mut wire = Vec::new();
        framing::write_frame(&mut wire, &envelope, &rgba).unwrap();
        let (read, payload) = framing::read_frame(&mut Cursor::new(wire)).unwrap();
        assert_eq!(read, envelope);
        assert_eq!(payload, rgba);
    }

    #[test]
    fn canvas_display_capability_validates_in_v2_grants() {
        let grant = NodeGrant {
            producer_id: "home-pi-tv".into(),
            auth_subject: "avery/home-pi".into(),
            zones: vec!["living-room".into()],
            capabilities: vec![DISPLAY_CAPABILITY.into()],
            publishes: Vec::new(),
            publish_zones: Vec::new(),
            publish_screen_anchors: Vec::new(),
            publish_producers: Vec::new(),
            screen_anchors: vec!["screen-tv".into()],
            site: "home-lan".into(),
            role: Role::Edge,
        };
        assert!(grant.validate().is_ok());
        assert!(grant.serves("living-room", DISPLAY_CAPABILITY));
    }
}
