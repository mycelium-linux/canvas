use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Result, ensure};
use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use canvas::control::{
    Command, ControlRequest, Outcome as CanvasOutcome, Receipt as CanvasReceipt, RuntimeSnapshot,
};
use chrono::Utc;
use dcp::{
    Action, Authorization, Availability, Catalog, Discovery, Endpoints, ErrorCode, ExecuteRequest,
    Outcome, Phase, Problem, Provider, Receipt, Safety, TreeNode,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
mod android;
mod android_workspace;
mod audio;

#[derive(Clone, Copy, PartialEq, Eq)]
enum AndroidMode {
    Audio,
    Workspace,
}

#[derive(Clone)]
struct AppState {
    token: Arc<str>,
    provider_id: Arc<str>,
    instance: Arc<str>,
    action_id: Arc<str>,
    android_serial: Option<Arc<str>>,
    android_mode: AndroidMode,
}

#[tokio::main]
async fn main() -> Result<()> {
    let listen = std::env::var("CANVAS_DCP_LISTEN").unwrap_or_else(|_| "127.0.0.1:7895".into());
    // The DCP edge uses the same audited loopback credential as local Canvas
    // control. Android execution uses canvas-tvctl's app-private endpoint.
    let token = canvas::loopback::local_token().map_err(anyhow::Error::msg)?;
    ensure!(!token.trim().is_empty(), "Canvas DCP bearer token is empty");
    let provider_id = std::env::var("CANVAS_DCP_PROVIDER_ID").unwrap_or_else(|_| "canvas".into());
    ensure!(
        !provider_id.trim().is_empty(),
        "Canvas DCP provider id is empty"
    );
    let instance = std::env::var("CANVAS_DCP_INSTANCE")
        .unwrap_or_else(|_| format!("urn:fpl:canvas:{provider_id}"));
    let android_serial = std::env::var("CANVAS_DCP_ANDROID_SERIAL").ok();
    if let Some(serial) = &android_serial {
        ensure!(!serial.trim().is_empty(), "Android target is empty");
    }
    let android_mode = match std::env::var("CANVAS_DCP_ANDROID_MODE").as_deref() {
        Ok("workspace") => AndroidMode::Workspace,
        Ok("audio") | Err(_) => AndroidMode::Audio,
        Ok(other) => anyhow::bail!("unsupported CANVAS_DCP_ANDROID_MODE: {other}"),
    };
    ensure!(
        android_mode != AndroidMode::Workspace || android_serial.is_some(),
        "Android workspace requires CANVAS_DCP_ANDROID_SERIAL"
    );
    let verb = if android_serial.is_some() && android_mode == AndroidMode::Audio {
        "audio.set_muted"
    } else {
        "workspace.activate"
    };
    let state = AppState {
        token: Arc::from(token.trim()),
        action_id: Arc::from(format!("{provider_id}.{verb}")),
        provider_id: Arc::from(provider_id),
        instance: Arc::from(instance),
        android_serial: android_serial.map(Arc::from),
        android_mode,
    };
    let app = router(state);
    let listener = tokio::net::TcpListener::bind(&listen).await?;
    eprintln!("canvas-dcp listening on http://{listen}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn router(state: AppState) -> Router {
    Router::new()
        .route("/.well-known/dcp", get(discovery))
        .route("/dcp/v0.1/catalog", get(decisions))
        .route("/dcp/v0.1/execute", post(execute))
        // Compatibility aliases for clients deployed before DCP gained
        // versioned canonical routes.
        .route("/v1/decisions", get(decisions))
        .route("/v1/decisions/execute", post(execute))
        .with_state(state)
}

async fn discovery(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    dcp_json(
        StatusCode::OK,
        Discovery {
            dcp: dcp::SPEC.into(),
            provider: provider(&state),
            versions: vec![dcp::VERSION.into()],
            endpoints: Endpoints {
                catalog: "/dcp/v0.1/catalog".into(),
                execute: Some("/dcp/v0.1/execute".into()),
                stream: None,
            },
            authorization: vec![Authorization {
                scheme: "bearer".into(),
                issuer: None,
                scopes: vec![if state.android_serial.is_some()
                    && state.android_mode == AndroidMode::Audio
                {
                    "canvas:audio".into()
                } else {
                    "canvas:workspace".into()
                }],
            }],
        },
    )
}

async fn decisions(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    match observe(state).await {
        Ok(catalog) => dcp_json(StatusCode::OK, catalog),
        Err(error) => problem_response(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::InternalError,
            &error.to_string(),
            true,
        ),
    }
}

async fn execute(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<ExecuteRequest>,
) -> Response {
    if let Err(response) = authorize(&state, &headers) {
        return response;
    }
    let observed = match observe(state.clone()).await {
        Ok(catalog) => catalog,
        Err(error) => {
            return problem_response(
                StatusCode::SERVICE_UNAVAILABLE,
                ErrorCode::InternalError,
                &error.to_string(),
                true,
            );
        }
    };
    let receipt = match tokio::task::spawn_blocking(move || apply(&state, &observed, request)).await
    {
        Ok(receipt) => receipt,
        Err(_) => {
            return problem_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorCode::InternalError,
                "Canvas execution task failed; outcome unknown; do not replay",
                false,
            );
        }
    };
    let status = if receipt.outcome == Outcome::Rejected {
        StatusCode::CONFLICT
    } else {
        StatusCode::OK
    };
    dcp_json(status, receipt)
}

async fn observe(app: AppState) -> Result<Catalog> {
    tokio::task::spawn_blocking(move || catalog(&app)).await?
}

fn catalog(app: &AppState) -> Result<Catalog> {
    if app.android_serial.is_some() && app.android_mode == AndroidMode::Audio {
        return audio::catalog(app);
    }
    if app.android_serial.is_some() {
        return android_workspace::catalog(app);
    }
    let request = ControlRequest {
        schema_version: canvas::control::CONTROL_VERSION,
        request_id: format!(
            "dcp-observe-{}",
            Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ),
        expected_revision: None,
        actor: None,
        command: Command::Inspect,
    };
    let reply = canvas::control::send(&request).map_err(anyhow::Error::msg)?;
    let CanvasOutcome::Ok {
        receipt: CanvasReceipt::Snapshot { snapshot },
    } = reply.outcome
    else {
        anyhow::bail!("Canvas control did not return a runtime snapshot");
    };
    catalog_from_snapshot(app, &snapshot)
}

fn catalog_from_snapshot(app: &AppState, snapshot: &RuntimeSnapshot) -> Result<Catalog> {
    // Persistence is an owner implementation detail, not this adapter's API.
    // Inspect is authoritative for what is installed and actually presented.
    let mut projects = snapshot
        .workspace_summaries
        .iter()
        .map(|row| row.project_id.clone())
        .collect::<Vec<_>>();
    projects.sort();
    projects.dedup();
    ensure!(!projects.is_empty(), "Canvas has no installed workspaces");
    let state = BTreeMap::from([
        (
            "active_project_id".into(),
            Value::String(snapshot.project_id.clone()),
        ),
        ("installed_project_ids".into(), json!(projects)),
        ("canvas_revision".into(), json!(snapshot.revision)),
        ("visible".into(), json!(snapshot.visible)),
        (
            "focused_surface_id".into(),
            json!(snapshot.focused_surface_id),
        ),
        (
            "surface_ids".into(),
            json!(
                snapshot
                    .surfaces
                    .iter()
                    .map(|surface| &surface.id)
                    .collect::<Vec<_>>()
            ),
        ),
    ]);
    let input_schema = json!({
        "$schema": dcp::SCHEMA_2020_12,
        "type": "object",
        "additionalProperties": false,
        "required": ["project_id"],
        "properties": {"project_id": {"type": "string", "enum": projects}}
    });
    let actions = vec![Action {
        id: app.action_id.to_string(),
        domain: "canvas".into(),
        title: "Activate Canvas workspace".into(),
        description: "Show one exact installed Canvas workspace on this Canvas surface".into(),
        input_schema,
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
    // Workspace activation depends on workspace identity, visibility and its
    // available targets, not on widget repaint ticks. Keep the renderer's
    // revision observable without making every live update stale a decision.
    let state_revision = workspace_state_revision(&state)?;
    Ok(Catalog {
        dcp_version: dcp::VERSION.into(),
        provider: provider(app),
        catalog_revision: digest(&actions)?,
        state_revision,
        observed_at: Utc::now().to_rfc3339(),
        expires_at: None,
        state: Some(state),
        actions,
        tree: vec![TreeNode {
            id: "workspace".into(),
            label: "Workspaces".into(),
            action_ids: vec![app.action_id.to_string()],
            children: vec![],
        }],
    })
}

fn apply(state: &AppState, observed: &Catalog, request: ExecuteRequest) -> Receipt {
    let reject = |code, message: &str, retryable| {
        receipt(
            &request,
            observed,
            Outcome::Rejected,
            Some(Problem {
                code,
                message: message.into(),
                retryable,
                details: None,
            }),
            None,
        )
    };
    if request.dcp_version != dcp::VERSION {
        return reject(
            ErrorCode::UnsupportedVersion,
            "unsupported DCP version",
            false,
        );
    }
    if request.phase != Phase::Commit {
        return reject(
            ErrorCode::UnsafePhase,
            "Canvas actions are commit-only",
            false,
        );
    }
    if request.expected_catalog_revision != observed.catalog_revision {
        return reject(ErrorCode::StaleCatalog, "Canvas catalog changed", true);
    }
    if request.expected_state_revision != observed.state_revision {
        return reject(ErrorCode::StaleState, "Canvas state changed", true);
    }
    if request.action_id != state.action_id.as_ref() {
        return reject(ErrorCode::ActionNotFound, "action is not advertised", false);
    }
    if state.android_serial.is_some() && state.android_mode == AndroidMode::Audio {
        return audio::apply(state, observed, request);
    }
    if request.arguments.len() != 1 {
        return reject(
            ErrorCode::InvalidRequest,
            "only project_id is supported",
            false,
        );
    }
    let Some(project_id) = request.arguments.get("project_id").and_then(Value::as_str) else {
        return reject(ErrorCode::InvalidRequest, "project_id is required", false);
    };
    let installed = observed
        .state
        .as_ref()
        .and_then(|state| state.get("installed_project_ids"))
        .and_then(Value::as_array)
        .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(project_id)));
    if !installed {
        return reject(
            ErrorCode::InvalidRequest,
            "project_id is not installed",
            false,
        );
    }
    if state.android_serial.is_some() {
        let project_id = project_id.to_owned();
        return android_workspace::apply(state, observed, request, &project_id);
    }
    let control = ControlRequest {
        schema_version: canvas::control::CONTROL_VERSION,
        request_id: request.request_id.clone(),
        // The DCP workspace guard was checked above. A global renderer tick
        // is not a precondition for selecting an independently named project.
        expected_revision: None,
        actor: None,
        command: Command::WorkspaceSwitch {
            name: project_id.into(),
        },
    };
    match canvas::control::send(&control) {
        Ok(reply)
            if matches!(
                reply.outcome,
                CanvasOutcome::Ok {
                    receipt: CanvasReceipt::WorkspaceSwitched { .. },
                    ..
                }
            ) =>
        {
            match catalog(state) {
                Ok(resulting) => {
                    if activation_verified(&resulting, project_id) {
                        receipt(&request, observed, Outcome::Applied, None, Some(&resulting))
                    } else {
                        reject(
                            ErrorCode::InternalError,
                            "Canvas activation did not verify visible project state; do not replay blindly",
                            false,
                        )
                    }
                }
                Err(error) => reject(
                    ErrorCode::InternalError,
                    &format!(
                        "Activation outcome could not be observed: {error}; do not replay blindly"
                    ),
                    false,
                ),
            }
        }
        Ok(reply) => reject(
            ErrorCode::InternalError,
            &format!("Canvas rejected workspace switch: {:?}", reply.outcome),
            true,
        ),
        Err(error) => reject(
            ErrorCode::InternalError,
            &format!("Activation dispatch outcome unknown: {error}; do not replay blindly"),
            false,
        ),
    }
}

fn activation_verified(catalog: &Catalog, project_id: &str) -> bool {
    catalog.state.as_ref().is_some_and(|state| {
        state.get("active_project_id").and_then(Value::as_str) == Some(project_id)
            && state.get("visible").and_then(Value::as_bool) == Some(true)
    })
}

fn receipt(
    request: &ExecuteRequest,
    observed: &Catalog,
    outcome: Outcome,
    error: Option<Problem>,
    resulting: Option<&Catalog>,
) -> Receipt {
    Receipt {
        dcp_version: dcp::VERSION.into(),
        receipt_id: format!("canvas-{}", request.request_id),
        request_id: request.request_id.clone(),
        provider_id: observed.provider.id.clone(),
        action_id: request.action_id.clone(),
        phase: request.phase.clone(),
        outcome,
        expected_catalog_revision: request.expected_catalog_revision.clone(),
        expected_state_revision: request.expected_state_revision.clone(),
        observed_catalog_revision: observed.catalog_revision.clone(),
        observed_state_revision: observed.state_revision.clone(),
        resulting_catalog_revision: resulting.map(|value| value.catalog_revision.clone()),
        resulting_state_revision: resulting.map(|value| value.state_revision.clone()),
        observed_at: Utc::now().to_rfc3339(),
        error,
    }
}

fn provider(state: &AppState) -> Provider {
    Provider {
        id: state.provider_id.to_string(),
        name: "Canvas".into(),
        instance: state.instance.to_string(),
    }
}

fn workspace_state_revision(state: &BTreeMap<String, Value>) -> Result<String> {
    let execution_state = state
        .iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "active_project_id" | "installed_project_ids" | "visible" | "owner"
            )
        })
        .collect::<BTreeMap<_, _>>();
    digest(&execution_state)
}

fn digest(value: &impl serde::Serialize) -> Result<String> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(serde_json::to_vec(value)?)
    ))
}

fn authorize(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let version_ok = headers
        .get("dcp-version")
        .and_then(|value| value.to_str().ok())
        == Some(dcp::VERSION);
    if !version_ok {
        return Err(problem_response(
            StatusCode::NOT_ACCEPTABLE,
            ErrorCode::UnsupportedVersion,
            "DCP-Version must be exactly 0.1",
            false,
        ));
    }
    let supplied = headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !supplied.is_some_and(|value| canvas::loopback::token_matches(value, &state.token)) {
        return Err(problem_response(
            StatusCode::UNAUTHORIZED,
            ErrorCode::Unauthorized,
            "valid bearer authorization is required",
            false,
        ));
    }
    Ok(())
}

fn dcp_json(status: StatusCode, value: impl serde::Serialize) -> Response {
    (status, [("dcp-version", dcp::VERSION)], Json(value)).into_response()
}
fn problem_response(
    status: StatusCode,
    code: ErrorCode,
    message: &str,
    retryable: bool,
) -> Response {
    dcp_json(
        status,
        Problem {
            code,
            message: message.into(),
            retryable,
            details: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn revisions_are_deterministic() {
        assert_eq!(
            digest(&json!({"b": 2, "a": 1})).unwrap(),
            digest(&json!({"b": 2, "a": 1})).unwrap()
        );
    }
    pub(super) fn sample() -> (AppState, RuntimeSnapshot) {
        let app = AppState {
            token: Arc::from("test"),
            provider_id: Arc::from("canvas-tv"),
            instance: Arc::from("urn:fpl:canvas:home-pi"),
            action_id: Arc::from("canvas-tv.workspace.activate"),
            android_serial: None,
            android_mode: AndroidMode::Audio,
        };
        let snapshot = serde_json::from_value(json!({
            "workspace_id":"smoke-workspace", "project_id":"smoke", "revision":32,
            "frame":{"x":0,"y":0,"width":1920,"height":1080}, "focused_surface_id":null,
            "surfaces":[], "visible":true,
            "workspace_summaries":[{"project_id":"smoke","name":"Smoke","status":"active","surface_count":0,"active":true,
                "theme":{"source":"coherence","mode":"dark","accent":"info"},"background":{"kind":"theme"}}]
        })).unwrap();
        (app, snapshot)
    }
    #[test]
    fn catalog_projects_live_snapshot_without_loading_store() {
        let (app, snapshot) = sample();
        let catalog = catalog_from_snapshot(&app, &snapshot).unwrap();
        assert_eq!(
            catalog.actions[0].input_schema["properties"]["project_id"]["enum"],
            json!(["smoke"])
        );
        assert!(activation_verified(&catalog, "smoke"));
        assert!(!activation_verified(&catalog, "television"));
    }
    #[test]
    fn hidden_workspace_is_not_verified_and_changes_state_revision() {
        let (app, mut snapshot) = sample();
        let before = catalog_from_snapshot(&app, &snapshot).unwrap();
        snapshot.visible = false;
        let after = catalog_from_snapshot(&app, &snapshot).unwrap();
        assert!(!activation_verified(&after, "smoke"));
        assert_eq!(before.catalog_revision, after.catalog_revision);
        assert_ne!(before.state_revision, after.state_revision);
    }
    #[test]
    fn renderer_ticks_do_not_invalidate_workspace_activation() {
        let (app, mut snapshot) = sample();
        let before = catalog_from_snapshot(&app, &snapshot).unwrap();
        snapshot.revision += 1;
        snapshot.focused_surface_id = Some("a-new-widget".into());
        let after = catalog_from_snapshot(&app, &snapshot).unwrap();
        assert_eq!(before.state_revision, after.state_revision);
        assert_ne!(
            before.state.as_ref().unwrap()["canvas_revision"],
            after.state.as_ref().unwrap()["canvas_revision"]
        );
        snapshot.project_id = "another-workspace".into();
        assert_ne!(
            before.state_revision,
            catalog_from_snapshot(&app, &snapshot)
                .unwrap()
                .state_revision
        );
    }
    #[test]
    fn stale_state_and_unadvertised_arguments_reject_before_owner_dispatch() {
        let (app, snapshot) = sample();
        let catalog = catalog_from_snapshot(&app, &snapshot).unwrap();
        let request = ExecuteRequest {
            dcp_version: dcp::VERSION.into(),
            request_id: "test-reject".into(),
            decision_id: "test-decision".into(),
            action_id: app.action_id.to_string(),
            arguments: BTreeMap::from([("project_id".into(), json!("smoke"))]),
            phase: Phase::Commit,
            expected_catalog_revision: catalog.catalog_revision.clone(),
            expected_state_revision: "stale".into(),
            prepared_receipt_id: None,
            confirmed: false,
            evidence: dcp::Evidence {
                utterance_id: "test".into(),
                transcript_revision: 1,
                final_: true,
            },
        };
        let rejected = apply(&app, &catalog, request.clone());
        assert_eq!(rejected.outcome, Outcome::Rejected);
        assert_eq!(rejected.error.unwrap().code, ErrorCode::StaleState);
        let mut request = request;
        request.expected_state_revision = catalog.state_revision.clone();
        request
            .arguments
            .insert("shell".into(), json!("never dispatched"));
        assert_eq!(
            apply(&app, &catalog, request).error.unwrap().code,
            ErrorCode::InvalidRequest
        );
    }
}
