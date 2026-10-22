//! Android audio uses the app's authenticated owner endpoint, never UI taps.
use super::*;
fn exchange(app: &AppState, request: Value) -> Result<Value> {
    let reply = android::exchange(app, request.clone())?;
    ensure!(
        reply["schema_version"] == 1 && reply["request_id"] == request["request_id"],
        "Android audio receipt does not match request"
    );
    Ok(reply["audio"].clone())
}

pub(super) fn catalog(app: &AppState) -> Result<Catalog> {
    let snapshot = exchange(
        app,
        json!({"schema_version":1,"request_id":"dcp-audio-observe","expected_revision":null,"command":{"command":"audio_inspect"}}),
    )?;
    from_snapshot(app, snapshot)
}

fn from_snapshot(app: &AppState, snapshot: Value) -> Result<Catalog> {
    ensure!(
        snapshot["revision"].as_u64().is_some() && snapshot["muted"].as_bool().is_some(),
        "Invalid owner audio snapshot"
    );
    let mut actions = Vec::new();
    if snapshot["ready"] == true && snapshot["pending"].is_null() {
        let source = &snapshot["source"];
        let surface = source["surface_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing audio surface"))?;
        let stream = source["stream_id"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing audio stream"))?;
        let title = source["title"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing audio title"))?;
        actions.push(Action {
            id: app.action_id.to_string(), domain:"canvas".into(),
            title:format!("Set audio mute for {title}"),
            description:format!("Mute or unmute the currently rendered {title} stream on {}. Source-local gain only, not system volume.", app.instance),
            input_schema:json!({"$schema":dcp::SCHEMA_2020_12,"type":"object","additionalProperties":false,
                "required":["surface_id","stream_id","muted"],"properties":{
                    "surface_id":{"type":"string","enum":[surface]},"stream_id":{"type":"string","enum":[stream]},"muted":{"type":"boolean"}}}),
            phases:vec![Phase::Commit], safety:Safety { idempotent:true,reversible:true,requires_final:true,confirmation_required:false },
            availability:Availability { available:true,reason_code:None,reason:None },
        });
    }
    let state = BTreeMap::from([("audio".into(), snapshot)]);
    Ok(Catalog {
        dcp_version: dcp::VERSION.into(),
        provider: provider(app),
        catalog_revision: digest(&actions)?,
        state_revision: digest(&state)?,
        observed_at: Utc::now().to_rfc3339(),
        expires_at: None,
        state: Some(state),
        tree: vec![TreeNode {
            id: "audio".into(),
            label: "Source audio".into(),
            action_ids: actions.iter().map(|a| a.id.clone()).collect(),
            children: vec![],
        }],
        actions,
    })
}

pub(super) fn apply(app: &AppState, observed: &Catalog, request: ExecuteRequest) -> Receipt {
    let reject = |code, message: &str| {
        receipt(
            &request,
            observed,
            Outcome::Rejected,
            Some(Problem {
                code,
                message: message.into(),
                retryable: false,
                details: None,
            }),
            None,
        )
    };
    if !request.evidence.final_ {
        return reject(
            ErrorCode::UnsafePhase,
            "Audio gain changes require final transcript evidence",
        );
    }
    if observed.actions.is_empty() {
        return reject(
            ErrorCode::ActionNotFound,
            "Audio renderer unavailable or request pending",
        );
    }
    let snapshot = &observed.state.as_ref().unwrap()["audio"];
    let source = &snapshot["source"];
    if request.arguments.len() != 3
        || request.arguments.get("surface_id") != Some(&source["surface_id"])
        || request.arguments.get("stream_id") != Some(&source["stream_id"])
    {
        return reject(
            ErrorCode::InvalidRequest,
            "Only the observed surface and stream may be controlled",
        );
    }
    let Some(muted) = request.arguments.get("muted").and_then(Value::as_bool) else {
        return reject(ErrorCode::InvalidRequest, "muted must be boolean");
    };
    let result = (|| -> Result<Catalog> {
        exchange(
            app,
            json!({"schema_version":1,"request_id":request.request_id,"expected_revision":snapshot["revision"],
            "command":{"command":"audio_set_muted","source":source,"muted":muted}}),
        )?;
        let resulting = catalog(app)?;
        let audio = &resulting.state.as_ref().unwrap()["audio"];
        ensure!(
            audio["source"] == *source
                && audio["ready"] == true
                && audio["pending"].is_null()
                && audio["muted"] == muted,
            "Fresh renderer readback did not confirm mute state; do not replay"
        );
        Ok(resulting)
    })();
    match result {
        Ok(resulting) => receipt(&request, observed, Outcome::Applied, None, Some(&resulting)),
        Err(_) => reject(
            ErrorCode::InternalError,
            "Audio outcome not verified; do not replay automatically",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_action_without_confirmed_renderer_and_pending_requests_are_hidden() {
        let (app, _) = super::super::tests::sample();
        for (ready, pending) in [
            (false, Value::Null),
            (true, json!({"request_id":"pending"})),
        ] {
            let catalog = from_snapshot(
                &app,
                json!({"revision":1,"muted":false,"ready":ready,"pending":pending}),
            )
            .unwrap();
            assert!(catalog.actions.is_empty());
        }
    }
    #[test]
    fn options_and_semantics_come_from_renderer_not_aliases() {
        let (app, _) = super::super::tests::sample();
        let catalog=from_snapshot(&app,json!({"revision":3,"muted":true,"ready":true,"pending":null,
            "source":{"surface_id":"video-a","stream_id":"opaque-stream","title":"An arbitrary channel"}})).unwrap();
        assert!(catalog.actions[0].title.contains("An arbitrary channel"));
        assert_eq!(
            catalog.actions[0].input_schema["properties"]["surface_id"]["enum"],
            json!(["video-a"])
        );
    }

    #[test]
    fn invalid_or_partial_requests_reject_without_touching_owner_transport() {
        let (mut app, _) = super::super::tests::sample();
        app.android_serial = Some(Arc::from("test-no-device"));
        app.action_id = Arc::from("canvas-firetv.audio.set_muted");
        let observed = from_snapshot(
            &app,
            json!({"revision":3,"muted":true,"ready":true,"pending":null,
            "source":{"surface_id":"video-a","stream_id":"opaque-stream","title":"Video"}}),
        )
        .unwrap();
        let base = ExecuteRequest {
            dcp_version: dcp::VERSION.into(),
            request_id: "test".into(),
            decision_id: "decision".into(),
            action_id: app.action_id.to_string(),
            arguments: BTreeMap::from([
                ("surface_id".into(), json!("video-a")),
                ("stream_id".into(), json!("opaque-stream")),
                ("muted".into(), json!(false)),
            ]),
            phase: Phase::Commit,
            expected_catalog_revision: observed.catalog_revision.clone(),
            expected_state_revision: observed.state_revision.clone(),
            prepared_receipt_id: None,
            confirmed: false,
            evidence: dcp::Evidence {
                utterance_id: "test".into(),
                transcript_revision: 1,
                final_: true,
            },
        };
        let mut partial = base.clone();
        partial.evidence.final_ = false;
        assert_eq!(
            super::super::apply(&app, &observed, partial)
                .error
                .unwrap()
                .code,
            ErrorCode::UnsafePhase
        );
        let mut stale = base.clone();
        stale.expected_state_revision = "old".into();
        assert_eq!(
            super::super::apply(&app, &observed, stale)
                .error
                .unwrap()
                .code,
            ErrorCode::StaleState
        );
        for (field, value) in [
            ("surface_id", json!("other")),
            ("stream_id", json!("other")),
            ("muted", json!("false")),
            ("shell", json!("never")),
        ] {
            let mut invalid = base.clone();
            invalid.arguments.insert(field.into(), value);
            assert_eq!(
                super::super::apply(&app, &observed, invalid)
                    .error
                    .unwrap()
                    .code,
                ErrorCode::InvalidRequest
            );
        }
    }
}
