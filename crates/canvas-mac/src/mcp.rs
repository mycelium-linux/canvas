//! Cross-project presentation MCP. Alfalfa owns project/resource identity;
//! Dock resolves exact IDs and invokes existing Mac and Agora presentation APIs.
// `canvas_socket`, not `canvas_control`: the crate `canvas-control` is the Mac
// Accessibility helper and every other module here calls it `canvas_control`.
// Aliasing this module to that name shadowed the crate, so `Operation`,
// `Response` and `request` stopped resolving. Two different things had the same
// spelling only after the rename — before it they were `canvas_control` and
// `dock_control`.
use canvas::control::{
    self as canvas_socket, Command as CanvasCommand, ControlRequest as CanvasRequest,
    Outcome as CanvasOutcome, ProjectWorkspaceSpec, Receipt as CanvasReceipt, RuntimeSnapshot,
    CONTROL_VERSION as CANVAS_CONTROL_VERSION, MAX_PROJECT_WORKSPACES,
};
use canvas_core::{Accent, ThemeMode, ThemeSelection};
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
    name: Option<String>,
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
            "Synchronize Canvas with owner-authorized Alfalfa projects, then list full-desktop workspaces with project labels, surface counts, appearance, and saved local Mac window counts.",
            json!({}),
            vec![],
        ),
        (
            "canvas_workspaces_sync",
            "Create missing Canvas desktops from the owner-authorized Alfalfa catalog and refresh their public labels, status, and project themes. Existing layouts and backgrounds are preserved.",
            json!({}),
            vec![],
        ),
        (
            "canvas_workspace_create",
            "Create an ad hoc Canvas desktop that is not backed by an Alfalfa portfolio folder, for scratch or one-off work. Portfolio synchronization never removes it. Creating one that already exists is refused rather than resetting its layout.",
            json!({
                "workspace_id":{"type":"string","maxLength":64},
                "name":{"type":"string","maxLength":96}
            }),
            vec!["workspace_id", "name"],
        ),
        (
            "canvas_workspace_switch",
            "Synchronize Canvas and switch its complete desktop to one exact Alfalfa project_id. The outgoing desktop remains saved for later recall.",
            json!({"project_id":{"type":"string","maxLength":64}}),
            vec!["project_id"],
        ),
        (
            "canvas_workspace_reset",
            "Reset one Alfalfa project's Canvas desktop to the current default metrics view. Other project desktops are unchanged.",
            json!({"project_id":{"type":"string","maxLength":64}}),
            vec!["project_id"],
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
        "projects_list"
        | "displays_list"
        | "workspaces_list"
        | "canvas_workspaces_sync"
        | "presence_route"
            if object.is_empty() =>
        {
            Ok(Arguments::default())
        }
        "project_resources" => Ok(Arguments {
            project: Some(value("project_id", 64)?),
            ..Arguments::default()
        }),
        "project_recall"
        | "workspace_activate"
        | "canvas_workspace_switch"
        | "canvas_workspace_reset" => Ok(Arguments {
            project: Some(value("project_id", 64)?),
            target,
            ..Arguments::default()
        }),
        "canvas_workspace_create" => Ok(Arguments {
            project: Some(value("workspace_id", 64)?),
            name: Some(value("name", 96)?),
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
        .filter(|projects| projects.len() <= MAX_PROJECT_WORKSPACES)
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

/// Desktops for the portfolio's workspaces, not for every repository.
///
/// One desktop per repository gave 380 of them, 367 named `gh-<number>`. Nobody
/// keeps 380 workspaces; they keep the nine folders they can name. A repository
/// stops being a destination and becomes what is *inside* one, which is also
/// what makes a per-workspace background mean something — it is the identity of
/// Ohm the place, not of one paused repo that happens to be called ohmOS.
fn portfolio_workspace_specs(value: &Value) -> Result<Vec<ProjectWorkspaceSpec>, String> {
    let entries = value
        .as_array()
        .ok_or("Alfalfa workspace list has an unsupported shape")?;
    if entries.is_empty() {
        // Syncing an empty set would delete every desktop the human has set up
        // on the strength of one bad reply.
        return Err("Alfalfa returned no portfolio workspaces".into());
    }
    entries
        .iter()
        .map(|entry| {
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .ok_or("Portfolio workspace identity is missing")?;
            let name = entry
                .get("name")
                .and_then(Value::as_str)
                .ok_or("Portfolio workspace name is missing")?;
            if !public_text(id, 64, false) || !public_text(name, 96, false) {
                return Err("Portfolio workspace identity is not presentable".into());
            }
            let spec = ProjectWorkspaceSpec {
                project_id: id.into(),
                name: name.into(),
                // A folder is a place, not a task; the portfolio tracks status
                // per project and has none to report for the folder itself.
                status: "active".into(),
                theme: workspace_theme(id),
            };
            spec.validate()?;
            Ok(spec)
        })
        .collect()
}

/// A stable accent per workspace, so nine folders are not nine identical rooms.
///
/// Derived from the id rather than stored, so a workspace added to the portfolio
/// arrives already distinguishable instead of defaulting into the same look and
/// waiting for someone to theme it by hand.
fn workspace_theme(id: &str) -> ThemeSelection {
    let hash = id.bytes().fold(0u32, |acc, b| {
        acc.wrapping_mul(31).wrapping_add(u32::from(b))
    });
    let accent = match hash % 3 {
        0 => Accent::Brand,
        1 => Accent::Info,
        _ => Accent::Success,
    };
    ThemeSelection::Coherence {
        mode: ThemeMode::Dark,
        accent,
    }
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

fn canvas_snapshot() -> Result<RuntimeSnapshot, String> {
    let reply = canvas_socket::send(&CanvasRequest {
        schema_version: CANVAS_CONTROL_VERSION,
        request_id: "dock-workspaces-inspect".into(),
        expected_revision: None,
        actor: None,
        command: CanvasCommand::Inspect,
    })?;
    match reply.outcome {
        CanvasOutcome::Ok {
            receipt: CanvasReceipt::Snapshot { snapshot },
        } => Ok(snapshot),
        CanvasOutcome::Error { message, .. } => Err(message),
        _ => Err("Canvas returned an unexpected inspection receipt".into()),
    }
}

fn sync_canvas_projects(source: &Value) -> Result<Value, String> {
    sync_canvas_projects_from_specs(portfolio_workspace_specs(source)?)
}

fn sync_canvas_projects_from_specs(projects: Vec<ProjectWorkspaceSpec>) -> Result<Value, String> {
    let revision = canvas_snapshot()?.revision;
    let reply = canvas_socket::send(&CanvasRequest {
        schema_version: CANVAS_CONTROL_VERSION,
        request_id: "dock-workspaces-sync".into(),
        expected_revision: Some(revision),
        actor: None,
        command: CanvasCommand::WorkspaceSync { projects },
    })?;
    match reply.outcome {
        CanvasOutcome::Ok {
            receipt:
                CanvasReceipt::WorkspacesSynced {
                    created,
                    updated,
                    preserved,
                    total,
                },
        } => Ok(json!({
            "created":created,
            "updated":updated,
            "preserved":preserved,
            "total":total,
            "revision":reply.revision
        })),
        CanvasOutcome::Error { message, .. } => Err(message),
        _ => Err("Canvas returned an unexpected workspace sync receipt".into()),
    }
}

fn reset_canvas_workspace(project_id: &str) -> Result<Value, String> {
    let revision = canvas_snapshot()?.revision;
    let reply = canvas_socket::send(&CanvasRequest {
        schema_version: CANVAS_CONTROL_VERSION,
        request_id: "dock-workspace-reset".into(),
        expected_revision: Some(revision),
        actor: None,
        command: CanvasCommand::WorkspaceReset {
            project_id: project_id.into(),
        },
    })?;
    match reply.outcome {
        CanvasOutcome::Ok {
            receipt: CanvasReceipt::WorkspaceReset { project_id },
        } => Ok(json!({"project_id": project_id, "revision": reply.revision})),
        CanvasOutcome::Error { message, .. } => Err(message),
        _ => Err("Canvas returned an unexpected workspace reset receipt".into()),
    }
}

/// A desktop that is not a portfolio folder.
///
/// Not everything worth having a desktop for is a company or a repository —
/// scratch space, a one-off investigation, the plain desktop someone works on
/// between projects. Those have no Alfalfa folder and should not need one
/// invented just to get a place to put windows.
///
/// This reuses `WorkspaceSync` rather than adding a command, because sync is
/// already additive: it inserts definitions and creates missing documents and
/// never deletes, which is exactly the guarantee an ad hoc desktop needs to
/// survive every later portfolio sync. The portfolio specs are sent alongside so
/// one reply cannot make the folders look absent.
fn create_adhoc_workspace(id: &str, name: &str) -> Result<Value, String> {
    if !public_text(id, 64, false) || !public_text(name, 96, false) {
        return Err("Ad hoc workspace identity is not presentable".into());
    }
    let source = super::alfalfa::workspaces()?;
    let mut specs = portfolio_workspace_specs(&source)?;
    if specs.iter().any(|spec| spec.project_id == id) {
        return Err("That id already names an Alfalfa portfolio folder".into());
    }
    // Refuse rather than re-create: sync would preserve the existing document,
    // but silently succeeding would read as "created" for a desktop that
    // already holds someone's layout.
    if canvas_snapshot()?
        .workspace_summaries
        .iter()
        .any(|workspace| workspace.project_id == id)
    {
        return Err("That Canvas desktop already exists".into());
    }
    let spec = ProjectWorkspaceSpec {
        project_id: id.into(),
        name: name.into(),
        status: "active".into(),
        theme: workspace_theme(id),
    };
    spec.validate()?;
    specs.push(spec);
    sync_canvas_projects_from_specs(specs)
}

fn switch_canvas_workspace(source: &Value, project: &str) -> Result<Value, String> {
    if !portfolio_workspace_specs(source)?
        .iter()
        .any(|candidate| candidate.project_id == project)
    {
        return Err("Alfalfa workspace does not exist for Canvas recall".into());
    }
    let sync = sync_canvas_projects(source)?;
    let revision = sync["revision"]
        .as_u64()
        .ok_or("Canvas sync omitted its revision")?;
    let reply = canvas_socket::send(&CanvasRequest {
        schema_version: CANVAS_CONTROL_VERSION,
        request_id: "dock-workspace-switch".into(),
        expected_revision: Some(revision),
        actor: None,
        command: CanvasCommand::WorkspaceSwitch {
            name: project.into(),
        },
    })?;
    match reply.outcome {
        CanvasOutcome::Ok {
            receipt: CanvasReceipt::WorkspaceSwitched { name },
        } if name == project => {
            let snapshot = canvas_snapshot()?;
            let workspace = snapshot
                .workspace_summaries
                .into_iter()
                .find(|workspace| workspace.project_id == project)
                .ok_or("Canvas switched workspace is missing from its snapshot")?;
            Ok(json!({
                "project_id":workspace.project_id,
                "name":workspace.name,
                "status":workspace.status,
                "surface_count":workspace.surface_count,
                "theme":workspace.theme,
                "background":workspace.background,
                "active":workspace.active,
                "revision":reply.revision,
                "sync":sync
            }))
        }
        CanvasOutcome::Error { message, .. } => Err(message),
        _ => Err("Canvas returned an unexpected workspace switch receipt".into()),
    }
}

/// The desktops a person can actually go to: one per portfolio folder.
///
/// Listed from the portfolio rather than the repository catalog, matching what
/// `sync_canvas_projects` now creates. Feeding the project list in here while
/// the sync consumed workspaces was the shape mismatch that made this list
/// unusable in the first place.
fn workspace_summaries() -> Result<Vec<Value>, String> {
    let source = super::alfalfa::workspaces()?;
    sync_canvas_projects(&source)?;
    let canvas = canvas_snapshot()?;
    let canvas_by_project = canvas
        .workspace_summaries
        .into_iter()
        .map(|workspace| (workspace.project_id.clone(), workspace))
        .collect::<std::collections::BTreeMap<_, _>>();
    let bindings = super::workspace::load()?;
    portfolio_workspace_specs(&source)?
        .into_iter()
        .map(|spec| {
            let canvas = canvas_by_project
                .get(&spec.project_id)
                .ok_or("Synchronized Canvas workspace is missing")?;
            Ok(json!({
                "project_id":spec.project_id,
                "name":spec.name,
                "status":spec.status,
                "local_window_count":bindings.windows(&spec.project_id).len(),
                "canvas_surface_count":canvas.surface_count,
                "canvas_active":canvas.active,
                "theme":canvas.theme,
                "background":canvas.background
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
    let canvas = switch_canvas_workspace(&super::alfalfa::list()?, project)?;
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
        "canvas":canvas,
        "viewer":{"output_index":viewer["output_index"],"reused":viewer["reused"].as_bool().unwrap_or(false)}
    }))
}

pub fn execute(name: &str, args: &Value) -> Result<Value, String> {
    let arguments = arguments(name, args)?;
    // MCP callers share a host's presentation; fail visibly instead of interleaving moves.
    let _presentation = if matches!(
        name,
        "project_recall"
            | "resource_open"
            | "workspace_activate"
            | "workspaces_list"
            | "canvas_workspaces_sync"
            | "canvas_workspace_create"
            | "canvas_workspace_switch"
            | "canvas_workspace_reset"
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
        "displays_list" => match canvas_control::request(canvas_control::Operation::Displays)? {
            canvas_control::Response::Displays { displays } => {
                json!({"scope":"this_dock_host","displays":displays})
            }
            _ => return Err("Dock Control returned an unexpected display response".into()),
        },
        "projects_list" => json!({"projects":project_summaries(&super::alfalfa::list()?)?}),
        "workspaces_list" => json!({"workspaces":workspace_summaries()?}),
        "canvas_workspaces_sync" => {
            let source = super::alfalfa::workspaces()?;
            json!({"canvas":sync_canvas_projects(&source)?})
        }
        "canvas_workspace_create" => {
            let id = arguments.project.as_deref().unwrap();
            let label = arguments.name.as_deref().unwrap();
            json!({"canvas":create_adhoc_workspace(id, label)?})
        }
        "canvas_workspace_switch" => {
            let project = arguments.project.as_deref().unwrap();
            let source = super::alfalfa::workspaces()?;
            json!({"canvas":switch_canvas_workspace(&source, project)?})
        }
        "canvas_workspace_reset" => {
            json!({"canvas":reset_canvas_workspace(arguments.project.as_deref().unwrap())?})
        }
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

use canvas::loopback::local_token;
/// The shared loopback credential, owned by `canvas::loopback`.
///
/// Re-exported because the installer and this server must name one file; when
/// the checks lived here too, only one of the two MCP servers would have been
/// updated the first time they changed.
pub use canvas::loopback::token_path;

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
    if !canvas::loopback::token_matches(provided, &token) {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

/// Serve the same tools over a private loopback transport for FPL Desktop
/// Bridge. The bridge reads the bearer from `token_path`; callers never do.
pub fn serve_http(port: u16) -> Result<(), String> {
    if port < 1024 {
        return Err("Canvas MCP requires an unprivileged explicit loopback port".into());
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
    fn an_ad_hoc_workspace_needs_an_id_and_a_presentable_name() {
        assert!(
            arguments(
                "canvas_workspace_create",
                &json!({"workspace_id":"desktop","name":"Desktop"})
            )
            .is_ok()
        );
        // A missing half is not a usable desktop.
        assert!(
            arguments(
                "canvas_workspace_create",
                &json!({"workspace_id":"desktop"})
            )
            .is_err()
        );
        assert!(arguments("canvas_workspace_create", &json!({"name":"Desktop"})).is_err());
        // Control characters would carry into a presented label.
        assert!(
            arguments(
                "canvas_workspace_create",
                &json!({"workspace_id":"desktop","name":"Desk\ntop"})
            )
            .is_err()
        );
        // Unknown arguments are refused rather than ignored.
        assert!(
            arguments(
                "canvas_workspace_create",
                &json!({"workspace_id":"d","name":"D","extra":1})
            )
            .is_err()
        );
    }

    #[test]
    fn an_ad_hoc_desktop_keeps_a_stable_accent_like_a_folder_does() {
        // It is a place among the others, so it must not be the one room that
        // looks different every time it is opened.
        assert_eq!(workspace_theme("desktop"), workspace_theme("desktop"));
    }

    #[test]
    fn a_desktop_is_made_for_each_portfolio_folder_not_each_repository() {
        // The nine a person names, not the 380 they never open. `ohm` is the
        // workspace someone means; `gh-1288853134` is a repo inside it.
        let source = json!([
            {"id":"fpl","name":"FPL"},
            {"id":"ohm","name":"Ohm"},
            {"id":"player-aerospace","name":"Player Aerospace"}
        ]);
        let specs = portfolio_workspace_specs(&source).unwrap();
        assert_eq!(specs.len(), 3);
        assert_eq!(specs[1].project_id, "ohm");
        assert_eq!(specs[1].name, "Ohm");
        assert_eq!(specs[1].status, "active");
    }

    #[test]
    fn each_folder_gets_a_stable_accent_of_its_own() {
        // Nine identical rooms would defeat the point of naming them; and the
        // accent must not move under a workspace between syncs.
        let ids = ["fpl", "ohm", "puget-audio", "agora", "personal"];
        let once: Vec<_> = ids.iter().map(|id| workspace_theme(id)).collect();
        let twice: Vec<_> = ids.iter().map(|id| workspace_theme(id)).collect();
        assert_eq!(once, twice, "a folder's accent must be stable across syncs");
        // Not a set: `ThemeSelection` is deliberately only `PartialEq`, and what
        // this asserts is that the folders differ, not that they can be ordered.
        assert!(
            once.iter().any(|theme| *theme != once[0]),
            "the folders must not all look the same"
        );
    }

    #[test]
    fn an_empty_or_malformed_portfolio_is_refused_rather_than_emptying_the_desktop() {
        // Syncing an empty set would delete every desktop the human set up on
        // the strength of one bad reply.
        assert!(portfolio_workspace_specs(&json!([])).is_err());
        assert!(portfolio_workspace_specs(&json!({})).is_err());
        assert!(portfolio_workspace_specs(&json!([{"name":"No identity"}])).is_err());
        assert!(portfolio_workspace_specs(&json!([{"id":"ohm"}])).is_err());
        // Control characters would carry into a presented label.
        assert!(portfolio_workspace_specs(&json!([{"id":"ohm","name":"O\nhm"}])).is_err());
    }

    #[test]
    fn a_large_repository_catalog_no_longer_becomes_a_large_desktop_count() {
        // The portfolio is what is synced now, so 380 repositories produce as
        // many desktops as there are folders — nine, not 380.
        let source = Value::Array(
            (0..380)
                .map(|index| json!({"id":format!("gh-{index}"),"name":format!("Project {index}")}))
                .collect(),
        );
        // Shape is still honoured if someone does hand it a large portfolio,
        // but the real one has nine entries.
        assert_eq!(portfolio_workspace_specs(&source).unwrap().len(), 380);

        let real = json!([
            {"id":"fpl","name":"FPL"},{"id":"puget-audio","name":"Puget Audio"},
            {"id":"ohm","name":"Ohm"},{"id":"agora","name":"Agora"},
            {"id":"personal","name":"Personal"},{"id":"zipwhip","name":"Zipwhip · Archive"},
            {"id":"gsgmfg","name":"GSG Mfg"},{"id":"boosst","name":"Boosst"},
            {"id":"player-aerospace","name":"Player Aerospace"}
        ]);
        assert_eq!(portfolio_workspace_specs(&real).unwrap().len(), 9);
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
        assert!(arguments("canvas_workspaces_sync", &json!({})).is_ok());
        assert!(
            arguments(
                "canvas_workspace_switch",
                &json!({"project_id":"gh-1288853134"})
            )
            .is_ok()
        );
        assert!(arguments("canvas_workspace_reset", &json!({"project_id":"ohm"})).is_ok());
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
        assert!(
            arguments(
                "workspace_activate",
                &json!({"project_id":"dock","target":"nearby"})
            )
            .is_err()
        );
        assert!(
            arguments(
                "project_recall",
                &json!({"project_id":"dock","query":"other"})
            )
            .is_err()
        );
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
            // Creating a desktop syncs, so it contends for presentation like
            // the rest and must refuse before it writes anything.
            (
                "canvas_workspace_create",
                json!({"workspace_id":"scratch","name":"Scratch"}),
            ),
        ] {
            assert!(
                execute(name, &args)
                    .unwrap_err()
                    .contains("presentation is busy")
            );
        }
        // Pinned so a new tool is a decision, not an accident: anything added
        // here has to be considered against the presentation lock above.
        assert_eq!(tools().len(), 11);
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
