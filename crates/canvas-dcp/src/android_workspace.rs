//! Workspace decisions derived from the physical Android TV owner.
use super::*;

pub(super) fn catalog(app: &AppState) -> Result<Catalog> {
    let reply = android::exchange(
        app,
        json!({"schema_version":1,"request_id":"dcp-workspace-observe","expected_revision":null,"command":{"command":"inspect"}}),
    )?;
    from_inspect(app, &reply)
}

fn from_inspect(app: &AppState, reply: &Value) -> Result<Catalog> {
    let data = &reply["data"];
    let revision = reply["revision"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("Missing TV endpoint revision"))?;
    let owner = data["owner"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing TV owner"))?;
    ensure!(
        data["revision"].as_u64() == Some(revision),
        "TV revision mismatch"
    );
    let projects = data["catalog"]["projects"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Missing TV project catalog"))?;
    let mut ids = projects
        .iter()
        .map(|project| {
            project["id"]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow::anyhow!("Invalid TV project identifier"))
        })
        .collect::<Result<Vec<_>>>()?;
    ids.sort();
    ids.dedup();
    ensure!(!ids.is_empty(), "TV has no installed workspaces");
    let active = data["catalog"]["selected_project"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing selected TV project"))?;
    ensure!(
        ids.iter().any(|id| id == active),
        "Selected TV project is not installed"
    );
    ensure!(
        data["document"]["project_id"].as_str() == Some(active),
        "Selected TV document is not presented"
    );
    let state = BTreeMap::from([
        ("active_project_id".into(), json!(active)),
        ("installed_project_ids".into(), json!(ids)),
        ("canvas_revision".into(), json!(revision)),
        ("owner".into(), json!(owner)),
        ("visible".into(), json!(true)),
    ]);
    let actions = vec![Action {
        id: app.action_id.to_string(),
        domain: "canvas".into(),
        title: "Activate TV workspace".into(),
        description: "Show one installed workspace on the physical Fire TV".into(),
        input_schema: json!({"$schema":dcp::SCHEMA_2020_12,"type":"object","additionalProperties":false,
            "required":["project_id"],"properties":{"project_id":{"type":"string","enum":ids}}}),
        phases: vec![Phase::Commit],
        safety: Safety {
            idempotent: true,
            reversible: true,
            requires_final: false,
            confirmation_required: false,
        },
        availability: Availability {
            available: true,
            reason_code: None,
            reason: None,
        },
    }];
    Ok(Catalog {
        dcp_version: dcp::VERSION.into(),
        provider: provider(app),
        catalog_revision: digest(&actions)?,
        state_revision: workspace_state_revision(&state)?,
        observed_at: Utc::now().to_rfc3339(),
        expires_at: None,
        state: Some(state),
        actions,
        tree: vec![TreeNode {
            id: "workspace".into(),
            label: "TV workspaces".into(),
            action_ids: vec![app.action_id.to_string()],
            children: vec![],
        }],
    })
}

pub(super) fn apply(
    app: &AppState,
    observed: &Catalog,
    request: ExecuteRequest,
    project_id: &str,
) -> Receipt {
    let result = (|| -> Result<Catalog> {
        android::exchange(
            app,
            json!({"schema_version":1,"request_id":request.request_id,
            "expected_revision":observed.state.as_ref().and_then(|state| state.get("canvas_revision")),
            "command":{"command":"workspace_switch","name":project_id}}),
        )?;
        let resulting = catalog(app)?;
        ensure!(
            activation_verified(&resulting, project_id),
            "TV readback did not confirm workspace"
        );
        Ok(resulting)
    })();
    match result {
        Ok(resulting) => receipt(&request, observed, Outcome::Applied, None, Some(&resulting)),
        Err(error) => receipt(
            &request,
            observed,
            Outcome::Rejected,
            Some(Problem {
                code: ErrorCode::InternalError,
                message: format!(
                    "TV activation outcome not verified: {error}; do not replay blindly"
                ),
                retryable: false,
                details: None,
            }),
            None,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repaint_revision_is_observed_without_staling_a_workspace_decision() {
        let (mut app, _) = super::super::tests::sample();
        app.android_serial = Some(Arc::from("test-tv"));
        app.android_mode = AndroidMode::Workspace;
        let mut inspect = json!({"ok":true,"revision":7,"data":{"owner":"firetv-omni","revision":7,
            "catalog":{"selected_project":"security","projects":[{"id":"studio"},{"id":"security"}]},
            "document":{"project_id":"security"}}});
        let before = from_inspect(&app, &inspect).unwrap();
        inspect["revision"] = json!(8);
        inspect["data"]["revision"] = json!(8);
        let repainted = from_inspect(&app, &inspect).unwrap();
        assert_eq!(before.state_revision, repainted.state_revision);
        inspect["data"]["catalog"]["selected_project"] = json!("studio");
        inspect["data"]["document"]["project_id"] = json!("studio");
        assert_ne!(
            before.state_revision,
            from_inspect(&app, &inspect).unwrap().state_revision
        );
    }

    #[test]
    fn catalog_uses_tv_project_ids_and_selected_document() {
        let (mut app, _) = super::super::tests::sample();
        app.android_serial = Some(Arc::from("test-tv"));
        app.android_mode = AndroidMode::Workspace;
        let inspect = json!({"ok":true,"revision":7,"data":{"owner":"firetv-omni","revision":7,
            "catalog":{"selected_project":"security","projects":[{"id":"studio"},{"id":"security"}]},
            "document":{"project_id":"security"}}});
        let catalog = from_inspect(&app, &inspect).unwrap();
        assert_eq!(
            catalog.actions[0].input_schema["properties"]["project_id"]["enum"],
            json!(["security", "studio"])
        );
        assert!(activation_verified(&catalog, "security"));
        assert!(
            from_inspect(
                &app,
                &json!({"ok":true,"revision":8,"data":inspect["data"]})
            )
            .is_err()
        );
    }
}
