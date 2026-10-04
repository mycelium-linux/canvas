use std::collections::BTreeMap;
use std::fs;
use std::io::{self, Read};
use std::process::id;

use canvas::control::{
    self, CONTROL_VERSION, Command, ControlReply, ControlRequest, ErrorCode, Outcome,
    ProjectWorkspaceSpec, Receipt, text,
};
use canvas::{
    Axis, Background, Layout, NormalizedRect, Placement, SCHEMA_VERSION, Surface, SurfaceChrome,
    SurfaceContent, SurfaceTitleMode, ThemeRef, WorkspaceDocument, WorkspacePatch,
    compose::{self, LayoutMode},
    layout_tree::{self, SplitSide},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().lock().read_to_string(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        eprintln!("usage: echo 'inspect' | canvasctl");
        eprintln!("       echo 'surfaces' | canvasctl");
        eprintln!("       echo 'workspace create wikiracing' | canvasctl");
        eprintln!("       echo 'surface add-live-video surf-browser surf-tv 1280 720' | canvasctl");
        eprintln!(
            "       echo 'surface add-external studio-mix synesthesia/mix-overview' | canvasctl"
        );
        eprintln!("       echo 'surface center surf-browser 0.70' | canvasctl");
        eprintln!("       echo 'focus\\nbg dock' | canvasctl");
        eprintln!("       echo 'move 0 25 1920 1150' | canvasctl");
        eprintln!("       echo 'indicator status speaking \"Socket live\"' | canvasctl");
        eprintln!("       echo 'nav scene-view next' | canvasctl");
        eprintln!("       echo 'titles focused' | canvasctl");
        eprintln!("       echo 'spacing 12 8' | canvasctl");
        eprintln!("       echo 'manifest PROJECT FILE.json' | canvasctl");
        eprintln!("       echo 'manifest-search PROJECT [QUERY]' | canvasctl");
        eprintln!("       echo 'patch /tmp/patch.json' | canvasctl");
        eprintln!("       echo '{{\"schema_version\":1,...}}' | canvasctl  (JSON mode)");
        std::process::exit(1);
    }

    if input.starts_with('{') {
        handle_json(input)?;
    } else {
        handle_text(input)?;
    }
    Ok(())
}

fn handle_json(input: &str) -> Result<(), String> {
    let request: ControlRequest = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let reply = control::send(&request)?;
    println!(
        "{}",
        serde_json::to_string(&reply).map_err(|e| e.to_string())?
    );
    Ok(())
}

fn handle_text(input: &str) -> Result<(), String> {
    for line in input.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Err(e) = handle_line(line) {
            println!("ERR {e}");
        }
    }
    Ok(())
}

fn handle_line(line: &str) -> Result<(), String> {
    let tokens = text::tokenize(line);
    if tokens.is_empty() {
        return Ok(());
    }

    if tokens[0] == "patch" {
        return handle_patch(&tokens);
    }
    if tokens[0] == "surfaces" {
        return handle_surfaces(&tokens);
    }
    if tokens[0] == "doc" {
        return handle_doc(&tokens);
    }
    if tokens[0] == "tile" {
        return handle_tile(&tokens);
    }
    if tokens[0] == "titles" {
        return handle_titles(&tokens);
    }
    if tokens[0] == "spacing" {
        return handle_spacing(&tokens);
    }
    if tokens[0] == "manifest" {
        return handle_manifest(&tokens);
    }
    if tokens[0] == "manifest-search" {
        return handle_manifest_search(&tokens);
    }
    if tokens[0] == "minimize" || tokens[0] == "maximize" || tokens[0] == "restore" {
        return handle_window_control(&tokens);
    }
    if tokens[0] == "open_terminal" {
        return handle_open_terminal(&tokens);
    }
    if tokens[0] == "workspace" {
        return handle_workspace(&tokens);
    }
    if tokens[0] == "surface" {
        return handle_surface(&tokens);
    }
    if tokens[0] == "catalog" {
        return handle_catalog(&tokens);
    }
    if tokens[0] == "events" {
        // ... existing events handler
        return Ok(());
    }
    if tokens[0] == "displays" {
        let req = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: format!("displays-{}", id()),
            expected_revision: None,
            actor: None,
            command: Command::Inspect,
        };
        let reply = control::send(&req)?;
        if let Outcome::Ok {
            receipt: Receipt::Snapshot { snapshot },
        } = &reply.outcome
        {
            if snapshot.display_frames.is_empty() {
                println!("No displays reported.");
            } else {
                println!("Connected displays:");
                for frame in &snapshot.display_frames {
                    println!(
                        "  {}x{} at {},{}",
                        frame.width, frame.height, frame.x, frame.y
                    );
                }
            }
        } else {
            println!("{}", text::format_reply(&reply));
        }
        return Ok(());
    }
    if tokens[0] == "move" && tokens.len() == 2 && tokens[1] == "all" {
        let request = ControlRequest {
            schema_version: CONTROL_VERSION,
            request_id: format!("move-all-{}", id()),
            expected_revision: None,
            actor: None,
            command: Command::SetDisplayTarget {
                target: canvas::display::DisplayTarget::All,
            },
        };
        println!("{}", text::format_reply(&control::send(&request)?));
        return Ok(());
    }

    let mut request = match text::parse_line(line) {
        Ok(req) => req,
        Err(e) => {
            println!("ERR parse {e}");
            return Ok(());
        }
    };
    if let Err(e) = maybe_auto_revision(&mut request) {
        println!("ERR {e}");
        return Ok(());
    }

    match send_with_retry(&request) {
        Ok(reply) => println!("{}", text::format_reply(&reply)),
        Err(e) => println!("ERR {e}"),
    }
    Ok(())
}

fn handle_surfaces(tokens: &[String]) -> Result<(), String> {
    if tokens.len() != 1 {
        return Err("usage: surfaces".into());
    }
    let request = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("surfaces-{}", id()),
        expected_revision: None,
        actor: None,
        command: Command::ReadDocument,
    };
    let reply = control::send(&request)?;
    let Outcome::Ok {
        receipt: Receipt::DocumentSnapshot { document, snapshot },
    } = &reply.outcome
    else {
        return Err(format_reply_error(&reply));
    };

    println!(
        "Canvas {}x{} at {},{} · {} surfaces · document rev {} · runtime rev {}",
        snapshot.frame.width,
        snapshot.frame.height,
        snapshot.frame.x,
        snapshot.frame.y,
        document.surfaces.len(),
        document.revision,
        snapshot.revision
    );
    for surface in &document.surfaces {
        let bounds = snapshot.surface_bounds.get(&surface.id);
        let frame = bounds.map_or_else(
            || "not placed".to_owned(),
            |value| {
                format!(
                    "{}x{} at {},{}",
                    value.width, value.height, value.x, value.y
                )
            },
        );
        let mut state = Vec::new();
        if document.focused_surface_id.as_deref() == Some(surface.id.as_str()) {
            state.push("focused");
        }
        if snapshot.minimized_surface_ids.contains(&surface.id) {
            state.push("minimized");
        }
        if snapshot.maximized_surface_ids.contains(&surface.id) {
            state.push("maximized");
        }
        let state = if state.is_empty() {
            String::new()
        } else {
            format!(" [{}]", state.join(","))
        };
        println!(
            "{:<24} {:<12} {:<24} {}{}",
            surface.id,
            surface.content.kind_tag(),
            surface_source(&surface.content),
            frame,
            state
        );
    }
    Ok(())
}

fn surface_source(content: &SurfaceContent) -> String {
    match content {
        SurfaceContent::ProjectResource { resource_id }
        | SurfaceContent::Terminal { resource_id } => resource_id.clone(),
        SurfaceContent::LiveImage { source_id }
        | SurfaceContent::LiveVideo { source_id, .. }
        | SurfaceContent::ExternalSurface { source_id } => source_id.clone(),
        SurfaceContent::Stream { url } => url.clone(),
        SurfaceContent::Widget { spec } => spec.id.clone(),
        SurfaceContent::Prompt { target, .. } => target.clone(),
        _ => "-".into(),
    }
}

fn handle_manifest(tokens: &[String]) -> Result<(), String> {
    if tokens.len() != 3 {
        return Err("usage: manifest PROJECT FILE.json".into());
    }
    let project_id = &tokens[1];
    if !canvas_core::is_safe_frame_id(project_id) {
        return Err("manifest project id is invalid".into());
    }
    let bytes = fs::read(&tokens[2]).map_err(|error| format!("read manifest: {error}"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("manifest exceeds 8 MiB".into());
    }
    let manifest: canvas_core::project_browser::ProjectManifest =
        serde_json::from_slice(&bytes).map_err(|error| format!("parse manifest: {error}"))?;
    manifest.validate().map_err(|error| error.to_string())?;
    if &manifest.project_id != project_id {
        return Err("manifest belongs to another project".into());
    }
    let root = canvas_core::runtime_root().ok_or("Canvas runtime root unavailable")?;
    let target = root.join("manifests").join(format!("{project_id}.json"));
    canvas_core::write_atomic(&target, &bytes)
        .map_err(|error| format!("install manifest: {error}"))?;
    println!("OK manifest {project_id} revision {}", manifest.revision);
    Ok(())
}

fn handle_manifest_search(tokens: &[String]) -> Result<(), String> {
    if tokens.len() < 2 {
        return Err("usage: manifest-search PROJECT [QUERY]".into());
    }
    let project_id = &tokens[1];
    if !canvas_core::is_safe_frame_id(project_id) {
        return Err("manifest project id is invalid".into());
    }
    let root = canvas_core::runtime_root().ok_or("Canvas runtime root unavailable")?;
    let path = root.join("manifests").join(format!("{project_id}.json"));
    let bytes = fs::read(&path).map_err(|error| format!("read manifest: {error}"))?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("manifest exceeds 8 MiB".into());
    }
    let manifest: canvas_core::project_browser::ProjectManifest =
        serde_json::from_slice(&bytes).map_err(|error| format!("parse manifest: {error}"))?;
    if &manifest.project_id != project_id {
        return Err("manifest belongs to another project".into());
    }
    let query = tokens[2..].join(" ");
    let results = manifest
        .search(&query, 50)
        .map_err(|error| error.to_string())?;
    let now = canvas_feeds::now_ms();
    let event = serde_json::json!({
        "schema_version": canvas_feeds::SCHEMA_VERSION,
        "owner": "canvas-artifact-index",
        "stream": format!("{project_id}-artifacts"),
        "contract": canvas_core::project_browser::SEARCH_CONTRACT,
        "sequence": now,
        "emitted_at_ms": now,
        "project_id": project_id,
        "payload": results,
    });
    canvas_feeds::send_raw(&event).map_err(|error| error.to_string())?;
    println!("OK manifest-search {project_id} {}", query);
    Ok(())
}

fn handle_catalog(tokens: &[String]) -> Result<(), String> {
    if tokens.len() > 3 {
        return Err("usage: catalog [widget-directory] [page]".into());
    }
    let directory = tokens.get(1).map_or("config/widgets", String::as_str);
    let page = tokens
        .get(2)
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "catalog page must be a positive integer".to_string())
        })
        .transpose()?
        .unwrap_or(1);
    let document = catalog_document(directory, page)?;
    send_mutation(Command::ApplyDocument { document })
}

/// The TV catalog is an index wall: 24 specimens fill a 6x4 grid at 1280x720.
/// Individual widgets can still be focused when they need close inspection.
const CATALOG_PAGE_SIZE: usize = 24;
const CATALOG_COLUMNS: u32 = 6;

fn catalog_document(directory: &str, page: usize) -> Result<WorkspaceDocument, String> {
    if page == 0 {
        return Err("catalog page must be a positive integer".into());
    }
    let mut paths = fs::read_dir(directory)
        .map_err(|error| format!("read widget catalog {directory}: {error}"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read widget catalog entry: {error}"))?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    if paths.is_empty() {
        return Err(format!(
            "widget catalog {directory} contains no JSON fixtures"
        ));
    }
    let page_count = paths.len().div_ceil(CATALOG_PAGE_SIZE);
    if page > page_count {
        return Err(format!(
            "catalog page {page} does not exist; choose 1..={page_count}"
        ));
    }
    let mut surfaces = Vec::with_capacity(paths.len());
    for path in &paths {
        let spec = canvas_widgets::WidgetSpec::from_file(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        surfaces.push(Surface {
            id: spec.id.clone(),
            title: spec.id.clone(),
            content: SurfaceContent::Widget { spec },
            inputs: Vec::new(),
        });
    }
    // Keep every fixture resident so Canvas can switch pages synchronously;
    // only the six visible page members participate in the layout.
    let all_ids = surfaces
        .iter()
        .map(|surface| surface.id.clone())
        .collect::<Vec<_>>();
    let first = (page - 1) * CATALOG_PAGE_SIZE;
    let last = (first + CATALOG_PAGE_SIZE).min(all_ids.len());
    let ids = &all_ids[first..last];
    let columns = (ids.len() as u32).min(CATALOG_COLUMNS);
    let rows = (ids.len() as u32).div_ceil(columns.max(1));
    Ok(WorkspaceDocument {
        pages: Vec::new(),
        active_page: 0,
        schema_version: SCHEMA_VERSION,
        id: "widget-catalog-all".into(),
        project_id: "dock".into(),
        revision: 1,
        theme: ThemeRef::Project,
        background: Background::default(),
        chrome: SurfaceChrome {
            title: SurfaceTitleMode::Hidden,
            canvas_padding: 18,
            surface_gap: 12,
        },
        surfaces,
        layout: Layout::Canvas {
            placements: compose::grid(ids, rows, columns),
        },
        bindings: Vec::new(),
        focused_surface_id: None,
    })
}

fn handle_titles(tokens: &[String]) -> Result<(), String> {
    if tokens.len() != 2 {
        return Err("usage: titles <hidden|presentation|focused|always>".into());
    }
    let title = match tokens[1].as_str() {
        "hidden" => SurfaceTitleMode::Hidden,
        "presentation" => SurfaceTitleMode::Presentation,
        "focused" => SurfaceTitleMode::Focused,
        "always" => SurfaceTitleMode::Always,
        value => return Err(format!("unknown title mode: {value}")),
    };
    let state = read_document_state()?;
    let patch = WorkspacePatch {
        schema_version: SCHEMA_VERSION,
        workspace_id: state.workspace_id,
        base_revision: state.doc_revision,
        revision: state.doc_revision + 1,
        operations: vec![canvas::PatchOperation::SetChrome {
            chrome: SurfaceChrome {
                title,
                ..state.chrome
            },
        }],
    };
    let request = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("titles-{}", id()),
        expected_revision: Some(state.runtime_revision),
        actor: None,
        command: Command::ApplyPatch { patch },
    };
    let reply = send_with_retry(&request)?;
    println!("{}", text::format_reply(&reply));
    Ok(())
}

fn handle_spacing(tokens: &[String]) -> Result<(), String> {
    if tokens.len() != 3 {
        return Err("usage: spacing <canvas_padding> <surface_gap>".into());
    }
    let canvas_padding = tokens[1]
        .parse::<u16>()
        .map_err(|_| "canvas padding must be a nonnegative integer".to_string())?;
    let surface_gap = tokens[2]
        .parse::<u16>()
        .map_err(|_| "surface gap must be a nonnegative integer".to_string())?;
    let state = read_document_state()?;
    let patch = WorkspacePatch {
        schema_version: SCHEMA_VERSION,
        workspace_id: state.workspace_id,
        base_revision: state.doc_revision,
        revision: state.doc_revision + 1,
        operations: vec![canvas::PatchOperation::SetChrome {
            chrome: SurfaceChrome {
                title: state.chrome.title,
                canvas_padding,
                surface_gap,
            },
        }],
    };
    let request = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("spacing-{}", id()),
        expected_revision: Some(state.runtime_revision),
        actor: None,
        command: Command::ApplyPatch { patch },
    };
    let reply = send_with_retry(&request)?;
    println!("{}", text::format_reply(&reply));
    Ok(())
}

fn handle_patch(tokens: &[String]) -> Result<(), String> {
    if tokens.len() < 2 {
        return Err("usage: patch <path> [rev <N>]".into());
    }
    let path = &tokens[1];
    let file_content =
        fs::read_to_string(path).map_err(|e| format!("read patch file {path}: {e}"))?;
    let mut patch: WorkspacePatch =
        serde_json::from_str(&file_content).map_err(|e| format!("parse patch: {e}"))?;

    let (doc_rev, _) = get_document_and_runtime_revisions()?;
    patch.base_revision = doc_rev;
    patch.revision = doc_rev + 1;
    patch.schema_version = SCHEMA_VERSION;
    if let Some(rev) = parse_explicit_rev(tokens) {
        send_single(rev, Command::ApplyPatch { patch })?;
    } else {
        send_mutation(Command::ApplyPatch { patch })?;
    }
    Ok(())
}

fn handle_doc(tokens: &[String]) -> Result<(), String> {
    if tokens.len() < 2 {
        return Err("usage: doc <path> [rev <N>]".into());
    }
    let path = &tokens[1];
    let file_content =
        fs::read_to_string(path).map_err(|e| format!("read document file {path}: {e}"))?;
    let document: WorkspaceDocument =
        serde_json::from_str(&file_content).map_err(|e| format!("parse document: {e}"))?;

    if let Some(rev) = parse_explicit_rev(tokens) {
        send_single(rev, Command::ApplyDocument { document })?;
    } else {
        send_mutation(Command::ApplyDocument { document })?;
    }
    Ok(())
}

fn handle_tile(tokens: &[String]) -> Result<(), String> {
    if tokens.len() < 2 {
        return Err("usage: tile grid <rows> <cols> | tile golden | tile monocle | tile next | tile split <horizontal|vertical> <surface_id> [before|after] [ratio] | tile resize <delta> | tile rotate | tile balance | tile normalize".into());
    }
    let state = read_document_state()?;
    let doc_rev = state.doc_revision;
    let rt_rev = state.runtime_revision;
    let surfaces = state.surfaces;
    let focused = state.focused;
    let workspace_id = state.workspace_id;
    let operation = match tokens[1].as_str() {
        "golden" => canvas::PatchOperation::SetLayout {
            // Same algorithm the daemon uses for a fresh stack: the golden
            // mosaic, sized to a 16:9 wall.
            layout: Layout::Canvas {
                placements: compose::compose(LayoutMode::Golden, &surfaces, 16.0 / 9.0),
            },
        },
        "grid" => {
            let rows = if tokens.len() > 2 {
                text::tokenize(&tokens[2])
                    .first()
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(1)
            } else {
                1
            };
            let cols = if tokens.len() > 3 {
                text::tokenize(&tokens[3])
                    .first()
                    .and_then(|t| t.parse().ok())
                    .unwrap_or(surfaces.len() as u32)
            } else {
                surfaces.len() as u32
            };
            let placements = compose::grid(&surfaces, rows, cols);
            canvas::PatchOperation::SetLayout {
                layout: Layout::Canvas { placements },
            }
        }
        "monocle" => {
            let active = focused.unwrap_or_else(|| surfaces.first().cloned().unwrap_or_default());
            canvas::PatchOperation::SetLayout {
                layout: Layout::Stack {
                    surface_ids: surfaces.clone(),
                    active_surface_id: active,
                },
            }
        }
        "next" => {
            let next = if surfaces.is_empty() {
                return Ok(());
            } else {
                let current_idx = focused
                    .as_ref()
                    .and_then(|f| surfaces.iter().position(|s| s == f))
                    .unwrap_or(0);
                surfaces[(current_idx + 1) % surfaces.len()].clone()
            };
            canvas::PatchOperation::SetFocus {
                surface_id: Some(next),
            }
        }
        "split" => {
            if tokens.len() < 4 {
                return Err(
                    "usage: tile split <horizontal|vertical> <surface_id> [before|after] [ratio]"
                        .into(),
                );
            }
            let axis = match tokens[2].as_str() {
                "horizontal" | "h" => Axis::Horizontal,
                "vertical" | "v" => Axis::Vertical,
                other => return Err(format!("unknown split axis: {other}")),
            };
            let new_surface_id = &tokens[3];
            if !surfaces.iter().any(|surface| surface == new_surface_id) {
                return Err(format!("unknown workspace surface: {new_surface_id}"));
            }
            let side = match tokens.get(4).map(String::as_str).unwrap_or("after") {
                "before" => SplitSide::Before,
                "after" => SplitSide::After,
                other => return Err(format!("unknown split side: {other}")),
            };
            let ratio = tokens
                .get(5)
                .map(|value| value.parse::<f32>())
                .transpose()
                .map_err(|error| format!("invalid split ratio: {error}"))?
                .unwrap_or(0.5);
            let target = focused.ok_or("tile split requires a focused surface")?;
            let mut layout = state.layout;
            layout_tree::split_surface(&mut layout, &target, new_surface_id, axis, ratio, side)
                .map_err(|error| error.to_string())?;
            canvas::PatchOperation::SetLayout { layout }
        }
        "balance" => {
            let mut layout = state.layout;
            layout_tree::balance(&mut layout);
            canvas::PatchOperation::SetLayout { layout }
        }
        "resize" => {
            let delta = tokens
                .get(2)
                .ok_or("usage: tile resize <delta>")?
                .parse::<f32>()
                .map_err(|error| format!("invalid resize delta: {error}"))?;
            let target = focused.ok_or("tile resize requires a focused surface")?;
            let mut layout = state.layout;
            layout_tree::resize_nearest(&mut layout, &target, delta)
                .map_err(|error| error.to_string())?;
            canvas::PatchOperation::SetLayout { layout }
        }
        "rotate" => {
            let target = focused.ok_or("tile rotate requires a focused surface")?;
            let mut layout = state.layout;
            layout_tree::rotate_nearest(&mut layout, &target).map_err(|error| error.to_string())?;
            canvas::PatchOperation::SetLayout { layout }
        }
        "normalize" => {
            let mut layout = state.layout;
            layout_tree::normalize(&mut layout);
            canvas::PatchOperation::SetLayout { layout }
        }
        other => return Err(format!("unknown tile mode: {other}")),
    };
    let patch = WorkspacePatch {
        schema_version: SCHEMA_VERSION,
        workspace_id,
        base_revision: doc_rev,
        revision: doc_rev + 1,
        operations: vec![operation],
    };
    let request = ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id: format!("tile-{}", id()),
        expected_revision: Some(rt_rev),
        actor: None,
        command: Command::ApplyPatch { patch },
    };
    let reply = send_with_retry(&request)?;
    println!("{}", text::format_reply(&reply));
    Ok(())
}

fn handle_window_control(tokens: &[String]) -> Result<(), String> {
    if tokens[0] != "restore" && tokens.len() < 2 {
        return Err("usage: maximize <surface_id> | restore".into());
    }
    let state = read_document_state()?;
    let layout = match tokens[0].as_str() {
        "maximize" => Layout::Surface {
            surface_id: tokens[1].clone(),
        },
        "restore" => {
            let placements = compose::grid(&state.surfaces, 2, 2);
            Layout::Canvas { placements }
        }
        _ => return Err(format!("unknown window control: {}", tokens[0])),
    };
    let patch = WorkspacePatch {
        schema_version: SCHEMA_VERSION,
        workspace_id: state.workspace_id,
        base_revision: state.doc_revision,
        revision: state.doc_revision + 1,
        operations: vec![canvas::PatchOperation::SetLayout { layout }],
    };
    let request = ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id: format!("winctl-{}", id()),
        expected_revision: Some(state.runtime_revision),
        actor: None,
        command: Command::ApplyPatch { patch },
    };
    let reply = send_with_retry(&request)?;
    println!("{}", text::format_reply(&reply));
    Ok(())
}

struct DocumentState {
    doc_revision: u64,
    runtime_revision: u64,
    surfaces: Vec<String>,
    focused: Option<String>,
    layout: Layout,
    workspace_id: String,
    chrome: SurfaceChrome,
    frame: canvas_core::layout::Rect,
    surface_aspects: BTreeMap<String, f32>,
}

fn handle_open_terminal(tokens: &[String]) -> Result<(), String> {
    if tokens.len() < 2 {
        return Err("usage: open_terminal <surface_id> [shell]".into());
    }
    let surface_id = &tokens[1];
    let shell = tokens.get(2).map(|s| s.as_str()).unwrap_or("sh");
    let tty_path = format!("tty://{shell}");
    let request = text::parse_line(&format!("open {surface_id} {tty_path}"))?;
    let reply = send_with_retry(&request)?;
    println!("{}", text::format_reply(&reply));
    Ok(())
}

fn handle_workspace(tokens: &[String]) -> Result<(), String> {
    if tokens.len() < 2 {
        return Err(
            "usage: workspace create <name> | workspace save <name> | workspace switch <name> | workspace summon <name> --output <output> | workspace list [--output <output>]".into(),
        );
    }
    match tokens[1].as_str() {
        "create" => {
            if tokens.len() != 3 {
                return Err("usage: workspace create <project-id>".into());
            }
            let project_id = tokens[2].clone();
            let inspect = send_read(Command::Inspect)?;
            let Outcome::Ok {
                receipt: Receipt::Snapshot { snapshot },
            } = inspect.outcome
            else {
                return Err(format_reply_error(&inspect));
            };
            let mut projects = snapshot
                .workspace_summaries
                .into_iter()
                .map(|workspace| ProjectWorkspaceSpec {
                    project_id: workspace.project_id,
                    name: workspace.name,
                    status: workspace.status,
                    theme: workspace.theme,
                })
                .collect::<Vec<_>>();
            if !projects
                .iter()
                .any(|project| project.project_id == project_id)
            {
                projects.push(ProjectWorkspaceSpec {
                    name: project_id.replace('-', " "),
                    project_id: project_id.clone(),
                    status: "active".into(),
                    theme: canvas_core::ThemeSelection::Coherence {
                        mode: canvas_core::ThemeMode::Dark,
                        accent: canvas_core::Accent::Info,
                    },
                });
            }
            send_mutation(Command::WorkspaceSync { projects })?;
            send_mutation(Command::WorkspaceSwitch { name: project_id })?;
        }
        "save" => {
            if tokens.len() < 3 {
                return Err("usage: workspace save <name>".into());
            }
            let request = text::parse_line(&format!("ws-save {}", tokens[2]))?;
            let reply = send_with_retry(&request)?;
            println!("{}", text::format_reply(&reply));
        }
        "switch" => {
            if tokens.len() < 3 {
                return Err("usage: workspace switch <name>".into());
            }
            let request = text::parse_line(&format!("ws-switch {}", tokens[2]))?;
            let reply = send_with_retry(&request)?;
            println!("{}", text::format_reply(&reply));
        }
        "summon" | "move" => {
            if tokens.len() < 5 || tokens[3] != "--output" {
                return Err(
                    "usage: workspace summon <name> --output <screen-neo|screen-studio|screen-tv>"
                        .into(),
                );
            }
            let name = &tokens[2];
            let output_id = &tokens[4];
            let read = send_read(Command::WorkspaceRead { name: name.clone() })?;
            let Outcome::Ok {
                receipt: Receipt::WorkspaceDocument { document, .. },
            } = read.outcome
            else {
                return Err(format_reply_error(&read));
            };
            let activate = local_output_id().as_deref() == Some(output_id.as_str());
            let local_output = local_output_id();
            send_mutation(Command::WorkspaceImport {
                document: document.clone(),
                assigned_output: Some(output_id.clone()),
                local_output,
                activate,
            })?;
            #[cfg(unix)]
            canvas::workspace_mirror::publish_assignment(&document, output_id)?;
            println!("summoned={name} output={output_id}");
        }
        "list" => {
            let req = ControlRequest {
                schema_version: CONTROL_VERSION,
                request_id: format!("ws-list-{}", id()),
                expected_revision: None,
                actor: None,
                command: Command::Inspect,
            };
            let reply = control::send(&req)?;
            if let Outcome::Ok {
                receipt: Receipt::Snapshot { snapshot },
            } = &reply.outcome
            {
                let output_filter = tokens
                    .windows(2)
                    .find(|pair| pair[0] == "--output")
                    .map(|pair| pair[1].as_str());
                if snapshot.workspace_summaries.is_empty() {
                    println!("No saved workspaces.");
                } else {
                    println!("Saved workspaces:");
                    for workspace in &snapshot.workspace_summaries {
                        if output_filter.is_some()
                            && workspace.assigned_output.as_deref() != output_filter
                        {
                            continue;
                        }
                        println!(
                            "  {} output={}{}",
                            workspace.project_id,
                            workspace.assigned_output.as_deref().unwrap_or("unassigned"),
                            if workspace.active { " active" } else { "" }
                        );
                    }
                }
            } else {
                println!("{}", text::format_reply(&reply));
            }
        }
        other => return Err(format!("unknown workspace subcommand: {other}")),
    }
    Ok(())
}

fn local_output_id() -> Option<String> {
    if let Some(output) = std::env::var_os("CANVAS_OUTPUT_ID") {
        return Some(output.to_string_lossy().into_owned());
    }
    let path = std::env::var_os("CANVAS_EDGE_CONFIG")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .map(|home| home.join(".config/canvas/edge.json"))
        })?;
    let value: serde_json::Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    value
        .pointer("/mirror/output_id")
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            value
                .get("screen_anchors")
                .and_then(serde_json::Value::as_array)
                .and_then(|anchors| anchors.first())
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_owned)
}

fn handle_surface(tokens: &[String]) -> Result<(), String> {
    if tokens.len() < 2 {
        return Err("usage: surface add-live-video <id> <source> <width> <height> | surface add-external <id> <source> | surface center <id> <scale>".into());
    }
    match tokens[1].as_str() {
        "add-live-video" => {
            if tokens.len() != 6 {
                return Err("usage: surface add-live-video <id> <source> <width> <height>".into());
            }
            send_mutation(Command::OpenLiveVideo {
                surface_id: tokens[2].clone(),
                source_id: tokens[3].clone(),
                width: tokens[4].parse().map_err(|_| "width must be an integer")?,
                height: tokens[5].parse().map_err(|_| "height must be an integer")?,
            })?;
        }
        "add-external" => {
            if tokens.len() != 4 {
                return Err("usage: surface add-external <id> <source>".into());
            }
            send_mutation(Command::OpenExternalSurface {
                surface_id: tokens[2].clone(),
                source_id: tokens[3].clone(),
            })?;
        }
        "center" => {
            if tokens.len() != 4 {
                return Err("usage: surface center <id> <scale>".into());
            }
            let scale: f32 = tokens[3]
                .parse()
                .map_err(|_| "scale must be a number between 0.1 and 1.0")?;
            if !(0.1..=1.0).contains(&scale) {
                return Err("scale must be between 0.1 and 1.0".into());
            }
            let state = read_document_state()?;
            if !state.surfaces.contains(&tokens[2]) {
                return Err(format!("unknown surface: {}", tokens[2]));
            }
            let height = scale;
            let width = state
                .surface_aspects
                .get(&tokens[2])
                .map_or(scale, |aspect| {
                    let padding = f32::from(state.chrome.canvas_padding);
                    let gap = f32::from(state.chrome.surface_gap);
                    let inner_width = state.frame.width as f32 - padding * 2.0;
                    let inner_height = state.frame.height as f32 - padding * 2.0;
                    let visible_height = (height * inner_height - gap).max(1.0);
                    ((visible_height * aspect + gap) / inner_width).clamp(0.1, 1.0)
                });
            let x = (1.0 - width) / 2.0;
            let y = (1.0 - height) / 2.0;
            let patch = WorkspacePatch {
                schema_version: SCHEMA_VERSION,
                workspace_id: state.workspace_id,
                base_revision: state.doc_revision,
                revision: state.doc_revision + 1,
                operations: vec![canvas::PatchOperation::SetLayout {
                    layout: Layout::Canvas {
                        placements: vec![Placement {
                            surface_id: tokens[2].clone(),
                            bounds: NormalizedRect {
                                x,
                                y,
                                width,
                                height,
                            },
                            layer: 0,
                        }],
                    },
                }],
            };
            send_mutation(Command::ApplyPatch { patch })?;
        }
        other => return Err(format!("unknown surface subcommand: {other}")),
    }
    Ok(())
}

fn send_read(command: Command) -> Result<ControlReply, String> {
    control::send(&ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("canvasctl-{}", id()),
        expected_revision: None,
        actor: None,
        command,
    })
}

fn read_document_state() -> Result<DocumentState, String> {
    let req = ControlRequest {
        schema_version: control::CONTROL_VERSION,
        request_id: format!("state-{}", id()),
        expected_revision: None,
        actor: None,
        command: Command::ReadDocument,
    };
    let reply = control::send(&req)?;
    match &reply.outcome {
        Outcome::Ok {
            receipt: Receipt::DocumentSnapshot { document, snapshot },
        } => {
            let surfaces: Vec<String> = document.surfaces.iter().map(|s| s.id.clone()).collect();
            let surface_aspects = document
                .surfaces
                .iter()
                .filter_map(|surface| match surface.content {
                    SurfaceContent::LiveVideo { width, height, .. } if height > 0 => {
                        Some((surface.id.clone(), width as f32 / height as f32))
                    }
                    _ => None,
                })
                .collect();
            Ok(DocumentState {
                doc_revision: document.revision,
                runtime_revision: snapshot.revision,
                surfaces,
                focused: document.focused_surface_id.clone(),
                layout: document.active_layout().clone(),
                workspace_id: document.id.clone(),
                chrome: document.chrome,
                frame: snapshot.frame,
                surface_aspects,
            })
        }
        _ => Err(format_reply_error(&reply)),
    }
}

fn parse_explicit_rev(tokens: &[String]) -> Option<u64> {
    let mut i = 2;
    while i < tokens.len() {
        if tokens[i] == "rev"
            && i + 1 < tokens.len()
            && let Ok(n) = tokens[i + 1].parse::<u64>()
        {
            return Some(n);
        }
        i += 1;
    }
    None
}

fn is_mutation(cmd: &Command) -> bool {
    !matches!(cmd, Command::Inspect | Command::ReadDocument)
}

fn maybe_auto_revision(request: &mut ControlRequest) -> Result<(), String> {
    if is_mutation(&request.command) && request.expected_revision.is_none() {
        let rev = get_runtime_revision()?;
        request.expected_revision = Some(rev);
    }
    Ok(())
}

fn get_runtime_revision() -> Result<u64, String> {
    let req = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("rev-{}", id()),
        expected_revision: None,
        actor: None,
        command: Command::Inspect,
    };
    let reply = control::send(&req)?;
    extract_runtime_revision(&reply)
}

fn get_document_and_runtime_revisions() -> Result<(u64, u64), String> {
    let req = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("doc-rev-{}", id()),
        expected_revision: None,
        actor: None,
        command: Command::ReadDocument,
    };
    let reply = control::send(&req)?;
    match &reply.outcome {
        Outcome::Ok {
            receipt: Receipt::DocumentSnapshot { document, snapshot },
        } => Ok((document.revision, snapshot.revision)),
        _ => Err(format_reply_error(&reply)),
    }
}

fn extract_runtime_revision(reply: &ControlReply) -> Result<u64, String> {
    match &reply.outcome {
        Outcome::Ok {
            receipt: Receipt::Snapshot { snapshot },
        } => Ok(snapshot.revision),
        Outcome::Ok {
            receipt: Receipt::DocumentSnapshot { snapshot, .. },
        } => Ok(snapshot.revision),
        _ => Err(format_reply_error(reply)),
    }
}

fn format_reply_error(reply: &ControlReply) -> String {
    text::format_reply(reply)
}

fn send_mutation(command: Command) -> Result<(), String> {
    let rev = get_runtime_revision()?;
    let request = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("mut-{}", id()),
        expected_revision: Some(rev),
        actor: None,
        command,
    };
    let reply = send_with_retry(&request)?;
    println!("{}", text::format_reply(&reply));
    Ok(())
}

fn send_single(expected_revision: u64, command: Command) -> Result<(), String> {
    let request = ControlRequest {
        schema_version: CONTROL_VERSION,
        request_id: format!("mut-{}", id()),
        expected_revision: Some(expected_revision),
        actor: None,
        command,
    };
    let reply = control::send(&request)?;
    println!("{}", text::format_reply(&reply));
    Ok(())
}

fn send_with_retry(request: &ControlRequest) -> Result<ControlReply, String> {
    let mut last_error = None;
    let mut attempt = request.clone();
    let mut last_conflict = None;
    for _ in 0..3 {
        match control::send(&attempt) {
            Ok(reply) => {
                if matches!(reply.outcome, Outcome::Error { ref code, .. } if *code == ErrorCode::Conflict)
                    && attempt.expected_revision.is_some()
                {
                    let fresh = get_runtime_revision()?;
                    attempt.expected_revision = Some(fresh);
                    attempt.request_id = format!("retry-{}-{}", id(), rand_u64());
                    last_conflict = Some(reply);
                    continue;
                }
                return Ok(reply);
            }
            Err(e) => {
                last_error = Some(e);
            }
        }
    }
    if let Some(reply) = last_conflict {
        return Ok(reply);
    }
    Err(last_error.unwrap_or_else(|| "max retries exceeded".into()))
}

fn rand_u64() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_micros() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod catalog_tests {
    use super::*;

    #[test]
    fn catalog_pages_cover_every_checked_in_widget_at_readable_size() {
        let directory = format!("{}/../../config/widgets", env!("CARGO_MANIFEST_DIR"));
        let fixture_count = fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .path()
                    .extension()
                    .is_some_and(|value| value == "json")
            })
            .count();
        let page_count = fixture_count.div_ceil(CATALOG_PAGE_SIZE);
        let mut ids = std::collections::BTreeSet::new();
        let mut kinds = std::collections::BTreeSet::new();
        for entry in fs::read_dir(&directory).unwrap().filter_map(Result::ok) {
            if !entry
                .path()
                .extension()
                .is_some_and(|value| value == "json")
            {
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
            kinds.insert(value["kind"].as_str().unwrap().to_owned());
        }
        for page in 1..=page_count {
            let document = catalog_document(&directory, page).expect("catalog page builds");
            assert_eq!(document.surfaces.len(), fixture_count);
            let Layout::Canvas { placements } = document.layout else {
                panic!("catalog must tile")
            };
            assert!(placements.len() <= CATALOG_PAGE_SIZE);
            for placement in &placements {
                assert!(placement.bounds.width >= 1.0 / CATALOG_COLUMNS as f32);
                assert!(placement.bounds.height >= 0.25);
            }
            ids.extend(placements.into_iter().map(|placement| placement.surface_id));
        }
        assert_eq!(ids.len(), fixture_count);
        assert_eq!(
            kinds,
            canvas_widgets::KNOWN_WIDGET_KINDS
                .iter()
                .map(|kind| (*kind).to_owned())
                .collect(),
            "config/widgets must contain at least one specimen of every widget kind"
        );
        assert!(catalog_document(&directory, 0).is_err());
        assert!(catalog_document(&directory, page_count + 1).is_err());
    }
}
