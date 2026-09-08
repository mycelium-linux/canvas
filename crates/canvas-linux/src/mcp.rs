//! Project-bound MCP adapter. FPL Desktop Bridge owns cloud identity and routing;
//! existing desktop control owns permissions and compositor operations.
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
    RoleServer, ServerHandler,
};
use serde_json::{json, Value};
use std::sync::Arc;

#[derive(Clone)]
struct Desktop {
    project: String,
}

fn tools() -> Vec<Tool> {
    [
        ("desktop_status", "Inspect only this project's windows, output and human/agent ownership.", json!({}), vec![]),
        ("desktop_capture", "Capture the active project output as a PNG. Fails if a different project is focused.", json!({}), vec![]),
        ("desktop_recall", "Recall this project's workspace after human delegation. The project is fixed by this connector.", json!({}), vec![]),
        ("desktop_focus", "Focus this project's window after human delegation. Fails after takeover.", json!({"window_id":{"type":"integer","minimum":1}}),vec!["window_id"]),
        ("desktop_text", "Type text after human delegation. Never grants control.", json!({"text":{"type":"string","maxLength":16384}}),vec!["text"]),
        ("desktop_key", "Press a key chord after human delegation, e.g. CTRL+l.", json!({"key":{"type":"string","maxLength":128}}),vec!["key"]),
        ("desktop_click", "Click project-output-local coordinates after human delegation.", json!({"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"button":{"type":"string","enum":["left","middle","right"]}}),vec!["x","y","button"]),
    ].into_iter().map(|(name,description,properties,required)| {
        Tool::new(name,description,json!({"type":"object","properties":properties,"required":required,"additionalProperties":false}).as_object().unwrap().clone())
    }).collect()
}

fn arguments(project: &str, name: &str, args: &Value) -> Result<Vec<String>, String> {
    crate::unit(project)?;
    let object = args.as_object().ok_or("Tool arguments must be an object")?;
    let tool = tools()
        .into_iter()
        .find(|tool| tool.name == name)
        .ok_or("Unknown desktop tool")?;
    let properties = tool.input_schema["properties"].as_object().unwrap();
    if object.keys().any(|key| !properties.contains_key(key)) {
        return Err("Unexpected desktop tool argument".into());
    }
    let text = |key: &str, max: usize| -> Result<String, String> {
        let value = args[key]
            .as_str()
            .ok_or_else(|| format!("{key} must be text"))?;
        if value.len() > max {
            return Err(format!("{key} exceeds limit"));
        }
        Ok(value.into())
    };
    let number = |key: &str| -> Result<String, String> {
        args[key]
            .as_u64()
            .filter(|n| *n <= u32::MAX as u64)
            .map(|n| n.to_string())
            .ok_or_else(|| format!("{key} must be an unsigned 32-bit integer"))
    };
    let action = if name == "desktop_recall" {
        "desktop-recall-agent".into()
    } else {
        name.replace('_', "-")
    };
    let mut command = vec![action, project.into()];
    match name {
        "desktop_status" | "desktop_capture" | "desktop_recall" => {}
        "desktop_focus" => command.push(number("window_id")?),
        "desktop_text" => command.push(text("text", 16384)?),
        "desktop_key" => command.push(text("key", 128)?),
        "desktop_click" => {
            command.push(number("x")?);
            command.push(number("y")?);
            let button = text("button", 6)?;
            if !["left", "middle", "right"].contains(&button.as_str()) {
                return Err("Invalid mouse button".into());
            }
            command.push(button);
        }
        _ => return Err("Unknown desktop tool".into()),
    }
    Ok(command)
}

fn workspace<'a>(tree: &'a Value, project: &str) -> Option<&'a Value> {
    if tree["type"] == "workspace" && tree["name"] == project {
        return Some(tree);
    }
    ["nodes", "floating_nodes"]
        .into_iter()
        .filter_map(|key| tree[key].as_array())
        .flatten()
        .find_map(|node| workspace(node, project))
}

fn project_status(value: Value, project: &str) -> Result<Value, String> {
    let tree = workspace(&value["tree"], project).ok_or("Project workspace is unavailable")?;
    let output_name = tree["output"]
        .as_str()
        .ok_or("Project output is unavailable")?;
    let output = value["outputs"]
        .as_array()
        .and_then(|outputs| outputs.iter().find(|o| o["name"] == output_name))
        .ok_or("Project output is unavailable")?;
    // Output nodes contain unrelated workspace titles: expose geometry only.
    Ok(
        json!({"project":project,"ownership":value["ownership"],"compositor":value["compositor"],"workspace":tree,"output":{"name":output["name"],"rect":output["rect"],"scale":output["scale"],"active":output["active"]}}),
    )
}

fn invoke(project: &str, name: &str, args: &Value) -> Result<CallToolResult, String> {
    let command = arguments(project, name, args)?;
    let mut result = crate::desktop::execute(&command)?;
    if name == "desktop_status" {
        result = project_status(result, project)?;
    }
    if name == "desktop_capture" {
        let png = result["png_base64"]
            .as_str()
            .ok_or("Capture omitted image")?
            .to_owned();
        result.as_object_mut().unwrap().remove("png_base64");
        result.as_object_mut().unwrap().remove("path");
        return Ok(CallToolResult::success(vec![
            ContentBlock::text(result.to_string()),
            ContentBlock::image(png, "image/png"),
        ]));
    }
    Ok(CallToolResult::success(vec![ContentBlock::text(
        result.to_string(),
    )]))
}

impl ServerHandler for Desktop {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new("dock-desktop", env!("CARGO_PKG_VERSION"));
        info.instructions = Some(format!("Project {} only. Human controls delegation outside MCP. Input refuses human ownership and inactive projects. Never request a sign-in secret through tools.", self.project));
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
        let project = self.project.clone();
        let result = tokio::task::spawn_blocking(move || {
            invoke(
                &project,
                &request.name,
                &Value::Object(request.arguments.unwrap_or_default()),
            )
        })
        .await
        .map_err(|_| ErrorData::internal_error("Desktop task failed", None))?;
        Ok(match result {
            Ok(result) => result,
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        }
        .into())
    }
}

fn local_token(project: &str) -> Result<String, String> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let path = std::path::PathBuf::from(std::env::var("HOME").map_err(|_| "HOME missing")?)
        .join(format!(".local/state/dock/mcp/{project}.token"));
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| "Private MCP credential unavailable")?;
    let metadata = file
        .metadata()
        .map_err(|_| "Cannot inspect MCP credential")?;
    if !metadata.is_file()
        || metadata.mode() & 0o077 != 0
        || metadata.uid() != unsafe { libc::geteuid() }
    {
        return Err("MCP credential must be a private file owned by this user".into());
    }
    let mut token = String::new();
    file.by_ref()
        .take(129)
        .read_to_string(&mut token)
        .map_err(|_| "Cannot read MCP credential")?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid MCP credential".into());
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
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    use subtle::ConstantTimeEq;
    let equal = bool::from(provided.as_bytes().ct_eq(token.as_bytes()));
    if !equal {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// Complete the owner-managed route only after real SSO verification succeeds.
pub fn ready(project: &str) -> Result<Value, String> {
    crate::unit(project)?;
    let owner = std::path::PathBuf::from(std::env::var("HOME").map_err(|_| "HOME missing")?)
        .join(".local/bin/fpl-desktop-bridge");
    let owner = owner.to_str().ok_or("Owner executable path is invalid")?;
    let status: Value = serde_json::from_str(&crate::command(owner, &["status"])?)
        .map_err(|_| "Owner status is invalid")?;
    if status["session"] != "authenticated" {
        return Err(
            "Enroll Agora One through the owner browser pairing before starting cloud MCP".into(),
        );
    }
    let unit = format!("dock-mcp-{project}.service");
    crate::command(
        "systemctl",
        &["--user", "start", &unit, "fpl-desktop-bridge-dock.service"],
    )?;
    let connector = format!("desktop.dock.desktop.{project}");
    let verified = crate::command(
        owner,
        &["verify", "--connector", &connector, "--desktop-read"],
    )?;
    let result: Value =
        serde_json::from_str(&verified).map_err(|_| "Owner MCP verification is invalid")?;
    if result["authenticated_mcp"] != true || result["desktop_read_verified"] != true {
        return Err("Owner did not verify the cloud MCP route".into());
    }
    Ok(result)
}

pub fn serve(project: &str, port: u16) -> Result<(), String> {
    crate::unit(project)?;
    if port < 1024 {
        return Err("MCP requires an unprivileged explicit loopback port".into());
    }
    let token = Arc::new(local_token(project)?);
    let runtime = tokio::runtime::Runtime::new().map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let desktop = Desktop {
            project: project.into(),
        };
        let service = StreamableHttpService::new(
            move || Ok(desktop.clone()),
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
            .map_err(|e| e.to_string())?;
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn caller_cannot_change_project_or_grant_control() {
        assert!(arguments(
            "dock",
            "desktop_text",
            &json!({"text":"x","project":"studio"})
        )
        .is_err());
        assert!(arguments("dock", "desktop_control", &json!({"mode":"agent"})).is_err());
        assert!(arguments("dock", "desktop_recall", &json!({"project":"studio"})).is_err());
        assert!(arguments("dock", "desktop_key", &json!({"key":"x".repeat(129)})).is_err());
        assert_eq!(
            arguments("studio", "desktop_recall", &json!({})).unwrap(),
            ["desktop-recall-agent", "studio"]
        );
        assert_eq!(
            arguments(
                "studio",
                "desktop_click",
                &json!({"x":1,"y":2,"button":"left"})
            )
            .unwrap(),
            ["desktop-click", "studio", "1", "2", "left"]
        );
    }
    #[test]
    fn inventory_does_not_leak_other_workspace_titles() {
        let result=project_status(json!({"tree":{"nodes":[{"type":"workspace","name":"dock","output":"HEADLESS-1","nodes":[]},{"type":"workspace","name":"studio","title":"private"}]},"outputs":[{"name":"HEADLESS-1","nodes":[{"title":"private"}],"rect":{}}]}),"dock").unwrap();
        assert!(!result.to_string().contains("private"));
        assert_eq!(result["project"], "dock");
    }
}
