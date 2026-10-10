//! Owner-model document codec, independent of any compositor or transport.
use canvas_workspace::WorkspaceDocument;
use serde::Deserialize;
use serde_json::{Value, json};
use std::io::{Read, Write};

const MAX_FRAME: u64 = 60 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    schema_version: u32,
    request_id: String,
    document: WorkspaceDocument,
}

fn normalize(request: Request) -> Result<Value, &'static str> {
    if request.schema_version != 1 || request.request_id.is_empty() || request.request_id.len() > 64
    {
        return Err("invalid document codec request");
    }
    // Normalize using the owning model, not approximate generic JSON numbers.
    // In particular, f32 geometry and typed widget values expand identically
    // through Value regardless of their source platform's direct serializer.
    Ok(json!({
        "schema_version": 1,
        "request_id": request.request_id,
        "revision": request.document.revision,
        "outcome": {"status":"ok","receipt":{
            "kind":"workspace_document", "document":request.document
        }}
    }))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments == ["--help"] || arguments == ["-h"] {
        println!("canvas-workspace-ctl normalize < codec-request.json");
        return Ok(());
    }
    if arguments != ["normalize"] {
        return Err("usage: canvas-workspace-ctl normalize < codec-request.json".into());
    }
    let mut input = Vec::new();
    std::io::stdin()
        .take(MAX_FRAME + 1)
        .read_to_end(&mut input)?;
    if input.len() as u64 > MAX_FRAME {
        return Err("document codec request exceeds 60 KiB".into());
    }
    let reply = normalize(serde_json::from_slice(&input)?)?;
    let output = serde_json::to_vec(&reply)?;
    if output.len() as u64 > MAX_FRAME {
        return Err("document codec reply exceeds 60 KiB".into());
    }
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(&output)?;
    stdout.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_geometry_has_one_canonical_representation_and_preserves_identity() {
        let document: Value = serde_json::from_str(include_str!(
            "../../../../config/workspaces/shared/canvas-sync.json"
        ))
        .unwrap();
        let input = json!({"schema_version":1,"request_id":"normalize-test","document":document});
        let first = normalize(serde_json::from_value(input).unwrap()).unwrap();
        assert_eq!(first["request_id"], "normalize-test");
        let normalized = &first["outcome"]["receipt"]["document"];
        assert_eq!(normalized["project_id"], "canvas-sync");
        assert_eq!(normalized["revision"], 1);
        assert_eq!(
            normalized["layout"]["placements"][0]["bounds"]["x"].as_f64(),
            Some(f64::from(0.02_f32))
        );
        let second = normalize(
            serde_json::from_value(
                json!({"schema_version":1,"request_id":"again","document":normalized}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(second["outcome"]["receipt"]["document"], *normalized);
    }
}
