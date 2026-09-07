//! Cross-project presentation MCP. Alfalfa owns project/resource identity;
//! Dock resolves exact IDs and invokes existing Mac and Agora presentation APIs.
use rmcp::{
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ErrorData,
        Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerInfo,
        Tool,
    },
    service::RequestContext,
    transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    },
    RoleServer, ServerHandler, ServiceExt,
};
use serde_json::{json, Value};
use std::{collections::BTreeSet, sync::Arc};

#[derive(Clone, Default)]
struct Orchestration;

static PRESENTATION: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Target {
    #[default]
    Primary,
    Presence,
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Arguments {
    project: Option<String>,
    resource: Option<String>,
    target: Target,
}

fn tools() -> Vec<Tool> {
    [
        (
            "displays_list",
            "List live displays on this Dock host using Dock Control. Read-only; does not list other machines or change visibility.",
            json!({}),
            vec![],
        ),
        (
            "projects_list",
            "List owner-authorized Alfalfa projects. Use this to resolve a spoken project name to an exact project_id before recalling it.",
            json!({}),
            vec![],
        ),
        (
            "workspaces_list",
            "List owner-authorized Alfalfa projects with saved local Mac window counts. Window titles and application details are omitted.",
            json!({}),
            vec![],
        ),
        (
            "presence_route",
            "Inspect whether a fresh, identified JARVIS Presence publication resolves to one configured display. Read-only and returns no identity details.",
            json!({}),
            vec![],
        ),
        (
            "workspace_activate",
            "Arrange and focus the saved local Mac windows for one exact Alfalfa project. target=presence fails when presence is stale, unknown, absent, ambiguous, or mapped to a disconnected display.",
            json!({"project_id":{"type":"string","maxLength":64},"target":{"type":"string","enum":["primary","presence"]}}),
            vec!["project_id"],
        ),
        (
            "project_resources",
            "List safe metadata for one exact Alfalfa project's resources. Source content and resource URIs are omitted.",
            json!({"project_id":{"type":"string","maxLength":64}}),
            vec!["project_id"],
        ),
        (
            "project_recall",
            "Show one exact Alfalfa project workspace on Agora and its saved local Mac windows. May open a viewer, but never starts project applications, opens artifacts, stops background work, migrates compute, or changes MCP bindings. Use resource_open explicitly to open a resource.",
            json!({"project_id":{"type":"string","maxLength":64},"target":{"type":"string","enum":["primary","presence"]}}),
            vec!["project_id"],
        ),
        (
            "resource_open",
            "Open one exact Alfalfa resource in the project workspace or its safe local Mac handler.",
            json!({"project_id":{"type":"string","maxLength":64},"resource_id":{"type":"string","maxLength":128}}),
            vec!["project_id", "resource_id"],
        ),
    ]
    .into_iter()
    .map(|(name, description, properties, required)| {
        Tool::new(
            name,
            description,
            json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
                .as_object()
                .unwrap()
                .clone(),
        )
    })
    .collect()
}

fn arguments(name: &str, args: &Value) -> Result<Arguments, String> {
    let object = args.as_object().ok_or("Tool arguments must be an object")?;
    let tool = tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or("Unknown orchestration tool")?;
    let properties = tool.input_schema["properties"].as_object().unwrap();
    if object.keys().any(|key| !properties.contains_key(key)) {
        return Err("Unexpected orchestration tool argument".into());
    }
    let value = |key: &str, max: usize| -> Result<String, String> {
        let value = args[key]
            .as_str()
            .filter(|value| !value.is_empty() && value.len() <= max)
            .ok_or_else(|| format!("{key} must be nonempty text within {max} bytes"))?;
        if value.chars().any(char::is_control) {
            return Err(format!("{key} contains control characters"));
        }
        Ok(value.into())
    };
    let target = match args.get("target").and_then(Value::as_str) {
        None | Some("primary") => Target::Primary,
        Some("presence") => Target::Presence,
        Some(_) => return Err("target must be primary or presence".into()),
    };
    match name {
        "projects_list" | "displays_list" | "workspaces_list" | "presence_route"
            if object.is_empty() =>
        {
            Ok(Arguments::default())
        }
        "project_resources" => Ok(Arguments {
            project: Some(value("project_id", 64)?),
            ..Arguments::default()
        }),
        "project_recall" | "workspace_activate" => Ok(Arguments {
            project: Some(value("project_id", 64)?),
            target,
            ..Arguments::default()
        }),
        "resource_open" => Ok(Arguments {
            project: Some(value("project_id", 64)?),
            resource: Some(value("resource_id", 128)?),
            ..Arguments::default()
        }),
        _ => Err("Missing or invalid orchestration tool arguments".into()),
    }
}

fn project_summaries(value: &Value) -> Result<Vec<Value>, String> {
    let projects = value
        .as_array()
        .filter(|projects| projects.len() <= 128)
        .ok_or("Alfalfa project catalog has an unsupported size or shape")?;
    let mut ids = BTreeSet::new();
    projects
        .iter()
        .map(|entry| {
            let project = &entry["project"];
            let id = project["id"]
                .as_str()
                .filter(|id| super::session_identifier(id))
                .ok_or("Alfalfa project has an invalid identity")?;
            if !ids.insert(id) {
                return Err("Alfalfa returned duplicate project identities".into());
            }
            let name = project["name"]
                .as_str()
                .filter(|name| public_text(name, 256, false))
                .ok_or("Alfalfa project has an invalid name")?;
            let status = project["status"].as_str().unwrap_or("active");
            let purpose = project["purpose"].as_str().unwrap_or("");
            if !public_text(status, 64, false) || !public_text(purpose, 4096, true) {
                return Err("Alfalfa project has invalid public metadata".into());
            }
            Ok(json!({
                "project_id":id,
                "name":name,
                "status":status,
                "purpose":purpose
            }))
        })
        .collect()
}

fn public_text(value: &str, max: usize, empty: bool) -> bool {
    value.len() <= max
        && (empty || !value.trim().is_empty())
        && !value.chars().any(char::is_control)
}

fn safe_web_uri(entry: &super::resources::Entry) -> Option<&str> {
    let url = url::Url::parse(&entry.uri).ok()?;
    (entry.id != "@home"
        && url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then_some(entry.uri.as_str())
}

fn public_resources(entries: &[super::resources::Entry]) -> Result<Vec<Value>, String> {
    let mut ids = BTreeSet::new();
    entries
        .iter()
        .filter(|entry| entry.id != "@home")
        .map(|entry| {
            if !public_text(&entry.id, 128, false)
                || !public_text(&entry.title, 512, false)
                || !public_text(&entry.kind, 128, false)
                || !ids.insert(entry.id.as_str())
            {
                return Err("Alfalfa resource has invalid or duplicate public metadata".into());
            }
            Ok(json!({
                "resource_id":entry.id,
                "title":entry.title,
                "kind":entry.kind,
                "destination":if safe_web_uri(entry).is_some() {"agora_browser"} else {"mac"}
            }))
        })
        .collect()
}

fn workspace_window_count(value: &Value, project: &str) -> Result<usize, String> {
    fn workspace<'a>(node: &'a Value, project: &str) -> Option<&'a Value> {
        if node["type"] == "workspace" && node["name"] == project {
            return Some(node);
        }
        ["nodes", "floating_nodes"]
            .into_iter()
            .filter_map(|key| node[key].as_array())
            .flatten()
            .find_map(|child| workspace(child, project))
    }
    fn clients(node: &Value) -> usize {
        usize::from(node["app_id"].is_string() || node["window"].is_number())
            + ["nodes", "floating_nodes"]
                .into_iter()
                .filter_map(|key| node[key].as_array())
                .flatten()
                .map(clients)
                .sum::<usize>()
    }
    workspace(&value["tree"], project)
        .map(clients)
        .ok_or_else(|| format!("Recalled project workspace {project} is missing"))
}

fn target_area(target: Target) -> Result<(Option<super::Rect>, Option<Value>), String> {
    match target {
        Target::Primary => Ok((None, None)),
        Target::Presence => {
            let route = super::presence::resolve()?;
            let (display_name, area) = super::display_work_area(&route.display_id)?;
            Ok((
                Some(area),
                Some(json!({
                    "target":"presence",
                    "producer_id":route.producer_id,
                    "zone":route.zone,
                    "display_id":route.display_id,
                    "display_name":display_name,
                    "age_ms":route.age_ms
                })),
            ))
        }
    }
}

fn workspace_summaries() -> Result<Vec<Value>, String> {
    let bindings = super::workspace::load()?;
    project_summaries(&super::alfalfa::list()?)?
        .into_iter()
        .map(|project| {
            let id = project["project_id"]
                .as_str()
                .ok_or("Projected project identity is missing")?;
            Ok(json!({
                "project_id":id,
                "name":project["name"],
                "local_window_count":bindings.windows(id).len()
            }))
        })
        .collect()
}

fn presence_status() -> Value {
    match target_area(Target::Presence) {
        Ok((_, Some(route))) => json!({"available":true,"route":route}),
        Ok(_) => json!({"available":false,"reason":"Presence route did not resolve"}),
        Err(error) => json!({"available":false,"reason":error}),
    }
}

fn activate_workspace(project: &str, target: Target) -> Result<Value, String> {
    // Project/resource lookup is the owner-authorized exact-ID check.
    super::resources::project_entries(project)?;
    let (area, route) = target_area(target)?;
    let mut bindings = super::workspace::load()?;
    let activation = super::workspace::activate(&mut bindings, project, area)?;
    super::workspace::save(&bindings)?;
    Ok(json!({
        "project_id":project,
        "presented":true,
        "local_window_count":activation.windows.len(),
        "target":if target == Target::Presence {"presence"} else {"primary"},
        "presence_route":route
    }))
}

fn recall(project: &str, target: Target) -> Result<Value, String> {
    let entries = super::resources::project_entries(project)?;
    let project_name = entries
        .first()
        .filter(|entry| entry.id == "@home")
        .map(|entry| entry.project_name.clone())
        .ok_or("Alfalfa project home is missing")?;
    let recalled = super::remote::rpc(project, "desktop-recall", &[], None)?;
    let status = super::remote::rpc(project, "desktop-status", &[], None)?;
    let window_count = workspace_window_count(&status, project)?;
    let viewer = super::remote::open_desktop(project)?;
    let (area, route) = target_area(target)?;
    let local = super::workspace::activate_saved_if_bound_at(project, area)?;
    Ok(json!({
        "project_id":project,
        "name":project_name,
        "presented":true,
        "output":recalled["active_workspace"]["output"],
        "window_count":window_count,
        "browser_seeded":false,
        "presentation_only":true,
        "target":if target == Target::Presence {"presence"} else {"primary"},
        "presence_route":route,
        "local_workspace_presented":local.is_some(),
        "local_window_count":local.as_ref().map(|activation| activation.windows.len()).unwrap_or(0),
        "viewer":{"output_index":viewer["output_index"],"reused":viewer["reused"].as_bool().unwrap_or(false)}
    }))
}

pub fn execute(name: &str, args: &Value) -> Result<Value, String> {
    let arguments = arguments(name, args)?;
    // MCP callers share a host's presentation; fail visibly instead of interleaving moves.
    let _presentation = if matches!(
        name,
        "project_recall" | "resource_open" | "workspace_activate"
    ) {
        Some(
            PRESENTATION
                .try_lock()
                .map_err(|_| "Dock presentation is busy; inspect the workspace before retrying")?,
        )
    } else {
        None
    };
    let result = match name {
        "displays_list" => match dock_control::request(dock_control::Operation::Displays)? {
            dock_control::Response::Displays { displays } => {
                json!({"scope":"this_dock_host","displays":displays})
            }
            _ => return Err("Dock Control returned an unexpected display response".into()),
        },
        "projects_list" => json!({"projects":project_summaries(&super::alfalfa::list()?)?}),
        "workspaces_list" => json!({"workspaces":workspace_summaries()?}),
        "presence_route" => presence_status(),
        "project_resources" => {
            let project = arguments.project.as_deref().unwrap();
            let entries = super::resources::project_entries(project)?;
            json!({"project_id":project,"resources":public_resources(&entries)?})
        }
        "workspace_activate" => {
            activate_workspace(arguments.project.as_deref().unwrap(), arguments.target)?
        }
        "project_recall" => recall(arguments.project.as_deref().unwrap(), arguments.target)?,
        "resource_open" => {
            let project = arguments.project.as_deref().unwrap();
            let resource = arguments.resource.as_deref().unwrap();
            let entries = super::resources::project_entries(project)?;
            let entry = entries
                .iter()
                .find(|entry| entry.id == resource && entry.id != "@home")
                .ok_or("Alfalfa resource does not exist in the requested project")?;
            let destination = if let Some(uri) = safe_web_uri(entry) {
                super::remote::rpc(project, "desktop-recall", &[], None)?;
                super::remote::rpc(project, "desktop-web-url", &[uri.into()], None)?;
                super::remote::open_desktop(project)?;
                "agora_browser"
            } else {
                super::resources::open(entry)?;
                "mac"
            };
            json!({"project_id":project,"resource_id":resource,"title":entry.title,"kind":entry.kind,"opened":true,"destination":destination})
        }
        _ => return Err("Unknown orchestration tool".into()),
    };
    Ok(result)
}

fn invoke(name: &str, args: &Value) -> Result<CallToolResult, String> {
    let result = execute(name, args)?;
    Ok(CallToolResult::success(vec![ContentBlock::text(
        result.to_string(),
    )]))
}

impl ServerHandler for Orchestration {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("dock-orchestration", env!("CARGO_PKG_VERSION"));
        info.instructions = Some("Alfalfa owns project and resource identity. List projects before resolving a spoken name; call mutations only with the exact returned ID. Dock presents resources and never returns credentials or private source context.".into());
        info
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult {
            tools: tools(),
            ..Default::default()
        })
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        tools().into_iter().find(|tool| tool.name == name)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = tokio::task::spawn_blocking(move || {
            invoke(
                &request.name,
                &Value::Object(request.arguments.unwrap_or_default()),
            )
        })
        .await
        .map_err(|_| ErrorData::internal_error("Dock orchestration task failed", None))?;
        Ok(match result {
            Ok(result) => result,
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        }
        .into())
    }
}

pub fn serve() -> Result<(), String> {
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let server = Orchestration
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| error.to_string())?;
        server
            .waiting()
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

pub fn token_path() -> Result<std::path::PathBuf, String> {
    Ok(
        std::path::PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
            .join("Library/Application Support/Dock/mcp/token"),
    )
}

fn local_token() -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(token_path()?)
        .map_err(|_| "Private Dock MCP credential unavailable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "Cannot inspect Dock MCP credential")?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("Dock MCP credential must be a private file owned by this user".into());
    }
    let mut token = String::new();
    file.by_ref()
        .take(129)
        .read_to_string(&mut token)
        .map_err(|_| "Cannot read Dock MCP credential")?;
    if token.len() != 64 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid Dock MCP credential".into());
    }
    Ok(token)
}

async fn authorize(
    axum::extract::State(token): axum::extract::State<Arc<String>>,
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or("");
    use subtle::ConstantTimeEq;
    if !bool::from(provided.as_bytes().ct_eq(token.as_bytes())) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// Serve the same tools over a private loopback transport for FPL Desktop
/// Bridge. The bridge reads the bearer from `token_path`; callers never do.
pub fn serve_http(port: u16) -> Result<(), String> {
    if port < 1024 {
        return Err("Dock MCP requires an unprivileged explicit loopback port".into());
    }
    let token = Arc::new(local_token()?);
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    runtime.block_on(async {
        let service = StreamableHttpService::new(
            || Ok(Orchestration),
            Arc::new(LocalSessionManager::default()),
            {
                let mut config = StreamableHttpServerConfig::default();
                config.legacy_session_mode = true;
                config.max_request_body_bytes = 128 * 1024;
                config.allowed_hosts =
                    vec![format!("127.0.0.1:{port}"), format!("localhost:{port}")];
                config.allowed_origins = vec![format!("http://127.0.0.1:{port}")];
                config
            },
        );
        let router = axum::Router::new()
            .nest_service("/mcp", service)
            .layer(axum::middleware::from_fn_with_state(token, authorize));
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|error| error.to_string())?;
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|error| error.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_projection_omits_owner_and_unrelated_fields() {
        let source = json!([{"project":{"id":"pedalkernel","name":"PedalKernel","status":"active","purpose":"Build pedals","owner":"private"},"secret":"hidden"}]);
        let projected = project_summaries(&source).unwrap();
        assert_eq!(projected[0]["project_id"], "pedalkernel");
        assert!(!projected[0].to_string().contains("private"));
        assert!(!projected[0].to_string().contains("hidden"));
        let invalid = json!([{"project":{"id":"dock","name":"Dock","status":"active\nprivate"}}]);
        assert!(project_summaries(&invalid).is_err());
    }

    #[test]
    fn resource_projection_omits_uri_notes_and_context() {
        let entries = vec![super::super::resources::Entry {
            company_id: None,
            project: "dock".into(),
            project_name: "Dock".into(),
            id: "code".into(),
            title: "Repository".into(),
            kind: "repo".into(),
            uri: "https://user:secret@example.test/private".into(),
            note: Some("private source".into()),
            home: "http://127.0.0.1:8791/projects/dock".into(),
            status: "active".into(),
            purpose: String::new(),
        }];
        let projected = public_resources(&entries).unwrap();
        assert_eq!(projected[0]["destination"], "mac");
        assert!(!projected[0].to_string().contains("secret"));
        assert!(!projected[0].to_string().contains("private source"));
        let mut http = entries[0].clone();
        http.uri = "http://example.test/public".into();
        assert_eq!(public_resources(&[http]).unwrap()[0]["destination"], "mac");
        assert!(public_resources(&[entries[0].clone(), entries[0].clone()]).is_err());
    }

    #[test]
    fn tool_arguments_require_exact_bounded_ids() {
        assert!(arguments("displays_list", &json!({})).is_ok());
        assert!(arguments("displays_list", &json!({"host":"other"})).is_err());
        assert!(arguments("workspaces_list", &json!({})).is_ok());
        assert!(arguments("presence_route", &json!({})).is_ok());
        assert!(arguments("projects_list", &json!({})).is_ok());
        assert!(arguments("project_recall", &json!({"project_id":"dock"})).is_ok());
        assert_eq!(
            arguments(
                "workspace_activate",
                &json!({"project_id":"dock","target":"presence"})
            )
            .unwrap()
            .target,
            Target::Presence
        );
        assert!(arguments(
            "workspace_activate",
            &json!({"project_id":"dock","target":"nearby"})
        )
        .is_err());
        assert!(arguments(
            "project_recall",
            &json!({"project_id":"dock","query":"other"})
        )
        .is_err());
        assert!(arguments("resource_open", &json!({"project_id":"dock"})).is_err());
        assert!(arguments("unknown", &json!({})).is_err());
    }

    #[test]
    fn presentation_contention_fails_before_contacting_owners() {
        let _guard = PRESENTATION.lock().unwrap();
        for (name, args) in [
            ("project_recall", json!({"project_id":"dock"})),
            (
                "resource_open",
                json!({"project_id":"dock","resource_id":"code"}),
            ),
            (
                "workspace_activate",
                json!({"project_id":"dock","target":"primary"}),
            ),
        ] {
            assert!(execute(name, &args)
                .unwrap_err()
                .contains("presentation is busy"));
        }
        assert_eq!(tools().len(), 8);
    }

    #[test]
    fn window_count_is_scoped_to_the_exact_workspace() {
        let status = json!({"tree":{"nodes":[
            {"type":"workspace","name":"dock","nodes":[{"type":"con","app_id":"browser","nodes":[]}]},
            {"type":"workspace","name":"other","nodes":[{"type":"con","app_id":"private","nodes":[]}]}
        ]}});
        assert_eq!(workspace_window_count(&status, "dock").unwrap(), 1);
        assert!(workspace_window_count(&status, "missing").is_err());
    }
}
