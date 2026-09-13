use std::fs;
use std::io::{self, Read};
use std::process::id;

use canvas::control::{
    self, CONTROL_VERSION, Command, ControlReply, ControlRequest, ErrorCode, Outcome, Receipt, text,
};
use canvas::{
    Layout, NormalizedRect, Placement, SCHEMA_VERSION, SurfaceChrome, SurfaceTitleMode,
    WorkspaceDocument, WorkspacePatch,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().lock().read_to_string(&mut input)?;
    let input = input.trim();

    if input.is_empty() {
        eprintln!("usage: echo 'inspect' | canvasctl");
        eprintln!("       echo 'focus\\nbg dock' | canvasctl");
        eprintln!("       echo 'move 0 25 1920 1150' | canvasctl");
        eprintln!("       echo 'indicator status speaking \"Socket live\"' | canvasctl");
        eprintln!("       echo 'nav scene-view next' | canvasctl");
        eprintln!("       echo 'titles focused' | canvasctl");
        eprintln!("       echo 'spacing 12 8' | canvasctl");
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
    if tokens[0] == "minimize" || tokens[0] == "maximize" || tokens[0] == "restore" {
        return handle_window_control(&tokens);
    }
    if tokens[0] == "open_terminal" {
        return handle_open_terminal(&tokens);
    }
    if tokens[0] == "workspace" {
        return handle_workspace(&tokens);
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

fn handle_titles(tokens: &[String]) -> Result<(), String> {
    if tokens.len() != 2 {
        return Err("usage: titles <hidden|focused|always>".into());
    }
    let title = match tokens[1].as_str() {
        "hidden" => SurfaceTitleMode::Hidden,
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
        return Err("usage: tile grid <rows> <cols> | tile monocle | tile next".into());
    }
    let state = read_document_state()?;
    let doc_rev = state.doc_revision;
    let rt_rev = state.runtime_revision;
    let surfaces = state.surfaces;
    let focused = state.focused;
    let workspace_id = state.workspace_id;
    let operation = match tokens[1].as_str() {
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
            let placements = tile_grid(&surfaces, rows, cols);
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
            let placements = tile_grid(&state.surfaces, 2, 2);
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

fn tile_grid(surfaces: &[String], rows: u32, cols: u32) -> Vec<Placement> {
    let cols = cols.max(1);
    let total = surfaces.len() as u32;
    let rows = rows.max(1).max(total.div_ceil(cols));
    let width = 1.0 / cols as f32;
    let height = 1.0 / rows as f32;
    surfaces
        .iter()
        .enumerate()
        .map(|(i, surface_id)| {
            let row = i as u32 / cols;
            let col = i as u32 % cols;
            Placement {
                surface_id: surface_id.clone(),
                bounds: NormalizedRect {
                    x: col as f32 * width,
                    y: row as f32 * height,
                    width,
                    height,
                },
                layer: i as i32,
            }
        })
        .collect()
}

struct DocumentState {
    doc_revision: u64,
    runtime_revision: u64,
    surfaces: Vec<String>,
    focused: Option<String>,
    workspace_id: String,
    chrome: SurfaceChrome,
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
            "usage: workspace save <name> | workspace switch <name> | workspace list".into(),
        );
    }
    match tokens[1].as_str() {
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
                if snapshot.workspaces.is_empty() {
                    println!("No saved workspaces.");
                } else {
                    println!("Saved workspaces:");
                    for name in &snapshot.workspaces {
                        println!("  {name}");
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
            Ok(DocumentState {
                doc_revision: document.revision,
                runtime_revision: snapshot.revision,
                surfaces,
                focused: document.focused_surface_id.clone(),
                workspace_id: document.id.clone(),
                chrome: document.chrome,
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
