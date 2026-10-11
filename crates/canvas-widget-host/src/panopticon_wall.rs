//! Canvas-owned local presentation of transport-neutral Panopticon observations.
use serde_json::{json, Value};
use std::io::{Read, Write};
type Error = Box<dyn std::error::Error>;

pub fn select(projection: &mut Value, selected: Option<&str>) -> Result<(), String> {
    let Some(id) = selected else {
        projection["selection"] = json!({"state":"none"});
        projection["map"]["selected"] = Value::Null;
        return Ok(());
    };
    if id.is_empty() || id.len() > 128 {
        return Err("invalid selected owner ID".into());
    }
    if projection["observations"].get(id).is_some() {
        projection["selection"] =
            json!({"state":"ready","id":id,"observation":projection["observations"][id]});
        projection["map"]["selected"] = json!(id);
    } else {
        projection["selection"] = json!({"state":"lost","id":id});
        projection["map"]["selected"] = Value::Null;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selection_is_recipient_local_and_missing_is_not_autoselected() {
        let mut projection = json!({"observations":{"adsb:one":{"callsign":"REAL"}},"map":{}});
        select(&mut projection, Some("adsb:one")).unwrap();
        assert_eq!(projection["map"]["selected"], "adsb:one");
        select(&mut projection, Some("adsb:gone")).unwrap();
        assert_eq!(projection["selection"]["state"], "lost");
        assert!(projection["map"]["selected"].is_null());
        select(&mut projection, None).unwrap();
        assert_eq!(projection["selection"]["state"], "none");
    }
}

pub fn compose(host: &str, binder: &str, projection: &Value) -> Result<Value, Error> {
    let bytes = serde_json::to_vec(projection)?;
    if bytes.len() > 64 * 1024 {
        return Err("projection exceeds Lua input budget".into());
    }
    let mut child = std::process::Command::new(host)
        .arg(binder)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("missing binder input")?
        .write_all(&bytes)?;
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .ok_or("missing binder output")?
        .take(64 * 1024 + 1)
        .read_to_end(&mut output)?;
    if !child.wait()?.success() || output.len() > 64 * 1024 {
        return Err("bounded binder rejected projection".into());
    }
    Ok(serde_json::from_slice(&output)?)
}

/// Never changes workspace selection. Document mutations precede live frames.
pub fn present(
    mut projection: Value,
    host: &str,
    binder: &str,
    tvctl: &str,
    endpoint: &str,
    lost: &mut Option<String>,
) -> Result<(), Error> {
    let call = |request: &Value| crate::presentation::call(tvctl, endpoint, request);
    let current = call(
        &json!({"schema_version":1,"request_id":"panopticon-document","command":{"command":"read_document"}}),
    )?;
    if current["ok"] != true {
        return Err("owner document read rejected".into());
    }
    if current["data"]["id"] != "tv-panopticon-wall" {
        return Ok(());
    }
    let state = call(
        &json!({"schema_version":1,"request_id":"panopticon-selection","command":{"command":"read_presentation"}}),
    )?;
    if state["ok"] != true {
        return Err("owner selection read rejected".into());
    }
    let requested = state["data"]["surfaces"]["panopticon-map"]["frame"]["map"]["selected"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| lost.clone());
    select(&mut projection, requested.as_deref())?;
    *lost = if projection["selection"]["state"] == "lost" {
        requested
    } else {
        None
    };
    let composed = compose(host, binder, &projection)?;
    let revision = current["data"]["revision"]
        .as_u64()
        .ok_or("missing workspace revision")?;
    let next = revision
        .checked_add(1)
        .ok_or("workspace revision exhausted")?;
    let operations:Vec<_>=[("panopticon-source","Source health","evidence"),("panopticon-detail","Selected contact","detail")].iter().map(|(id,title,key)|json!({"op":"upsert_surface","surface":{"id":id,"title":title,"content":{"kind":"widget","spec":composed[*key]}}})).collect();
    let receipt = call(
        &json!({"schema_version":1,"request_id":"panopticon-health","expected_revision":current["revision"],"command":{"command":"apply_patch","patch":{"schema_version":1,"workspace_id":"tv-panopticon-wall","base_revision":revision,"revision":next,"operations":operations}}}),
    )?;
    if receipt["ok"] != true {
        return Err(format!("health patch rejected: {receipt}").into());
    }
    for (surface, key) in [("panopticon-map", "map"), ("panopticon-chart", "chart")] {
        let receipt = call(
            &json!({"schema_version":1,"request_id":format!("panopticon-{key}"),"command":{"command":"surface_snapshot","workspace_id":"tv-panopticon-wall","surface_id":surface,"frame":composed[key]["frame"],"data":composed[key]["data"]}}),
        )?;
        if receipt["ok"] != true {
            return Err(format!("{key} snapshot rejected: {receipt}").into());
        }
    }
    Ok(())
}
