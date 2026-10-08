//! Read-only presentation mirroring over operator-authorized private transports.
//! Layout is imported only when changed; live frames never enter saved state.
use serde_json::{Value, json};
use std::{
    io::{BufRead, Read, Write},
    process::{Command, Stdio},
    time::Duration,
};

const LIMIT: usize = 256 * 1024;
const CONTRACT: &str = "canvas.widget.snapshot.v1";

fn call(program: &str, arguments: &[&str], request: &Value) -> Result<Value, String> {
    let mut child = Command::new(program)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|e| e.to_string())?;
    child
        .stdin
        .take()
        .ok_or("missing control input")?
        .write_all(request.to_string().as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() || output.stdout.len() > LIMIT {
        return Err("presentation control failed or exceeded bound".into());
    }
    let value: Value = serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    if value["ok"] == false || value["outcome"]["status"] == "error" {
        return Err("presentation endpoint rejected request".into());
    }
    Ok(value)
}

fn request(command: Value, revision: Option<u64>) -> Value {
    json!({"schema_version":1,"request_id":"presentation-mirror","expected_revision":revision,"command":command})
}

/// Convert the declared layout into an explicitly bound mirror, not live data.
fn layout(presentation: &Value) -> Result<Value, String> {
    if presentation["schema_version"] != 1 {
        return Err("unsupported presentation schema".into());
    }
    let mut document = presentation["document"].clone();
    let surfaces = document["surfaces"]
        .as_array_mut()
        .ok_or("missing surfaces")?;
    let mut bindings = Vec::new();
    for surface in surfaces {
        if surface["content"]["kind"] != "widget" {
            continue;
        }
        let id = surface["id"]
            .as_str()
            .ok_or("missing surface identity")?
            .to_owned();
        let spec = &mut surface["content"]["spec"];
        if spec["kind"] != "signal_view" && spec["kind"] != "status_list" {
            continue;
        }
        spec["data"] = json!({"state":"no_data"});
        // Documents carry the owner's declared placeholder, never its live frame.
        surface["inputs"] = json!([{"id":"mirror","contract":CONTRACT}]);
        bindings.push(json!({"id":format!("mirror-{id}"),"source":{"owner":"canvas-tv","stream":id,"contract":CONTRACT},"target":{"surface_id":id,"input_id":"mirror"},"adapter":"widget-snapshot-v1","delivery":{"mode":"latest"}}));
    }
    document["bindings"] = json!(bindings);
    document["revision"] = json!(1);
    Ok(document)
}

struct Media(std::collections::BTreeMap<String, (String, Value, std::process::Child)>);
impl Drop for Media {
    fn drop(&mut self) {
        for (_, _, child) in self.0.values_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
impl Media {
    fn prepare(&mut self, document: &mut Value) -> Result<(), String> {
        let mut wanted = std::collections::BTreeSet::new();
        for surface in document["surfaces"]
            .as_array_mut()
            .ok_or("missing surfaces")?
        {
            if surface["content"]["kind"] != "stream" {
                continue;
            }
            let id = format!(
                "mirror-{}",
                surface["id"].as_str().ok_or("missing media identity")?
            );
            let mut url = reqwest::Url::parse(
                surface["content"]["url"]
                    .as_str()
                    .ok_or("missing stream URL")?,
            )
            .map_err(|_| "invalid media URL")?;
            if let Ok(origin) = std::env::var("CANVAS_MIRROR_MEDIA_ORIGIN") {
                let origin =
                    reqwest::Url::parse(&origin).map_err(|_| "invalid private media origin")?;
                url.set_host(origin.host_str())
                    .map_err(|_| "invalid media host")?;
                url.set_port(origin.port())
                    .map_err(|_| "invalid media port")?;
            }
            wanted.insert(id.clone());
            let endpoint = url.to_string();
            let restart = match self.0.get_mut(&id) {
                Some((previous, _, child)) => {
                    *previous != endpoint || child.try_wait().map_err(|e| e.to_string())?.is_some()
                }
                None => true,
            };
            if restart {
                let geometry = if let Some((_, geometry, _)) = self
                    .0
                    .get(&id)
                    .filter(|(previous, _, _)| *previous == endpoint)
                {
                    geometry.clone()
                } else {
                    let output = Command::new("canvas-tv")
                        .args(["describe", &endpoint])
                        .output()
                        .map_err(|e| e.to_string())?;
                    if !output.status.success() {
                        return Err("media geometry unavailable".into());
                    }
                    serde_json::from_slice::<Value>(&output.stdout)
                        .map_err(|_| "invalid media geometry")?
                };
                if let Some((_, _, mut child)) = self.0.remove(&id) {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                let child = Command::new("canvas-tv")
                    .args([&id, &endpoint])
                    .arg(geometry["width"].to_string())
                    .arg(geometry["height"].to_string())
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::inherit())
                    .spawn()
                    .map_err(|e| format!("media decoder: {e}"))?;
                self.0.insert(id.clone(), (endpoint, geometry, child));
            }
            let geometry = &self.0.get(&id).ok_or("missing media geometry")?.1;
            surface["content"] = json!({"kind":"live_video","source_id":id,"width":geometry["width"],"height":geometry["height"]});
        }
        self.0.retain(|id, (_, _, child)| {
            if wanted.contains(id) {
                true
            } else {
                let _ = child.kill();
                let _ = child.wait();
                false
            }
        });
        Ok(())
    }
}

fn events(presentation: &Value, sequence: u64, expired: bool) -> Result<Vec<Value>, String> {
    let doc = &presentation["document"];
    let mut output = Vec::new();
    for surface in doc["surfaces"].as_array().ok_or("missing surfaces")? {
        let id = surface["id"].as_str().ok_or("missing identity")?;
        let snapshot = &presentation["surfaces"][id];
        if snapshot.is_null() {
            continue;
        }
        let spec = &surface["content"]["spec"];
        // Snapshot contract carries WidgetKind, not the host WidgetSpec.
        // Chrome, theme, refresh cadence and widget identity stay with Canvas.
        let mut widget = match spec["kind"].as_str() {
            Some("signal_view") => {
                json!({"kind":"signal_view","title":spec["title"],"frame":snapshot["frame"]})
            }
            Some("status_list") => {
                json!({"kind":"status_list","title":spec["title"],"nodes":snapshot["frame"]["nodes"]})
            }
            _ => continue,
        };
        let mut data = snapshot["data"].clone();
        // Static authored signal views (e.g. the reel caption) have no transient
        // receive lease. Their document's source state is authoritative.
        if spec["refresh_ms"].is_null() && data["state"] == "no_data" {
            data = spec["data"].clone();
        }
        if expired {
            data["state"] = json!("stale");
            if matches!(
                widget["frame"]["view"].as_str(),
                Some("task_deck" | "attention")
            ) {
                widget["frame"]["panel"]["fresh"] = json!(false);
            }
        }
        output.push(json!({"schema_version":1,"owner":"canvas-tv","stream":id,"contract":CONTRACT,"sequence":sequence,"emitted_at_ms":canvas_feeds::now_ms(),"project_id":doc["project_id"],"payload":{"widget":widget,"data":data}}));
    }
    Ok(output)
}

fn deliver(event: &Value) -> Result<(), String> {
    for envelope in canvas_feeds::chunk_envelopes(event, 2048).map_err(|e| e.to_string())? {
        let deadline = std::time::Instant::now() + Duration::from_millis(250);
        loop {
            match canvas_feeds::send_raw(&envelope) {
                Ok(()) => break,
                Err(error) if std::time::Instant::now() >= deadline => {
                    return Err(error.to_string());
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
    Ok(())
}

fn media_unavailable(document: &mut Value, error: &str) -> Result<(), String> {
    for surface in document["surfaces"]
        .as_array_mut()
        .ok_or("missing surfaces")?
    {
        if surface["content"]["kind"] == "stream" {
            let id = surface["id"].as_str().ok_or("missing media identity")?;
            let detail: String = error.chars().take(256).collect();
            surface["content"] = json!({"kind":"widget","spec":{"schema_version":2,"id":format!("mirror-{id}"),"kind":"text","text":"Private reel temporarily unavailable","data":{"state":"error","detail":detail}}});
        }
    }
    Ok(())
}

fn receive() -> Result<(), String> {
    // A latest-value mailbox bounds memory and avoids replaying a backlog.
    let slot = std::sync::Arc::new(std::sync::Mutex::new(None));
    let input_slot = slot.clone();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let input_done = done.clone();
    std::thread::spawn(move || {
        let mut input = std::io::stdin().lock();
        loop {
            let mut line = Vec::new();
            let n = input
                .by_ref()
                .take((LIMIT + 1) as u64)
                .read_until(b'\n', &mut line)
                .unwrap_or(0);
            if n == 0 {
                break;
            }
            if n > LIMIT || !line.ends_with(b"\n") {
                eprintln!("mirror: oversized/incomplete presentation");
                break;
            }
            match serde_json::from_slice::<Value>(&line) {
                Ok(value) => *input_slot.lock().unwrap() = Some(value),
                Err(_) => eprintln!("mirror: invalid presentation JSON"),
            }
        }
        input_done.store(true, std::sync::atomic::Ordering::Release);
    });
    let mut previous_layout = Value::Null;
    let mut media = Media(Default::default());
    let mut latest = None;
    let mut received = std::time::Instant::now();
    let mut stale_sent = false;
    let mut sequence = canvas_feeds::now_ms();
    loop {
        let incoming = slot.lock().unwrap().take();
        if let Some(presentation) = incoming {
            let mut next_layout = layout(&presentation)?;
            if let Err(error) = media.prepare(&mut next_layout) {
                eprintln!("mirror media: {error}");
                media_unavailable(&mut next_layout, &error)?;
            }
            if next_layout != previous_layout {
                let inspect = call(
                    "canvasctl",
                    &[],
                    &request(json!({"command":"inspect"}), None),
                )?;
                let rev = inspect["revision"]
                    .as_u64()
                    .ok_or("missing desktop revision")?;
                let reply = call(
                    "canvasctl",
                    &[],
                    &request(
                        json!({"command":"workspace_import","document":next_layout,"activate":true}),
                        Some(rev),
                    ),
                )?;
                if reply["outcome"]["status"] != "ok" {
                    return Err(format!("workspace import rejected: {}", reply["outcome"]));
                }
                previous_layout = next_layout;
            }
            sequence = sequence.saturating_add(1);
            for event in events(&presentation, sequence, false)? {
                deliver(&event)?;
            }
            received = std::time::Instant::now();
            stale_sent = false;
            latest = Some(presentation);
        } else if !stale_sent
            && received.elapsed() > Duration::from_secs(7)
            && let Some(presentation) = &latest
        {
            sequence = sequence.saturating_add(1);
            for event in events(presentation, sequence, true)? {
                deliver(&event)?;
            }
            stale_sent = true;
        }
        if done.load(std::sync::atomic::Ordering::Acquire)
            && received.elapsed() > Duration::from_secs(7)
        {
            return Err("presentation transport ended; retained panels marked stale".into());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn follow(serial: &str) -> Result<(), String> {
    loop {
        match call(
            "canvas-tvctl",
            &[serial],
            &request(json!({"command":"read_presentation"}), None),
        ) {
            Ok(reply) => {
                let presentation = &reply["data"];
                if presentation["document"]["project_id"] != "tardy" {
                    std::thread::sleep(Duration::from_secs(2));
                    continue;
                }
                layout(presentation)?;
                let bytes = serde_json::to_vec(presentation).map_err(|e| e.to_string())?;
                if bytes.len() > LIMIT {
                    return Err("presentation exceeds mirror limit".into());
                }
                std::io::stdout()
                    .write_all(&bytes)
                    .and_then(|_| std::io::stdout().write_all(b"\n"))
                    .and_then(|_| std::io::stdout().flush())
                    .map_err(|e| e.to_string())?;
            }
            Err(error) => eprintln!("mirror source: {error}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [mode] if mode == "receive" => receive(),
        [mode, serial] if mode == "follow" => follow(serial),
        _ => Err("usage: canvas-presentation-mirror follow ADB_SERIAL | canvas-presentation-mirror receive".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Value {
        json!({"schema_version":1,"document":{"project_id":"tardy","surfaces":[{"id":"tasks","content":{"kind":"widget","spec":{"kind":"signal_view","frame":{"view":"task_deck","panel":{"items":[]}}}}}]},"surfaces":{"tasks":{"frame":{"view":"task_deck","panel":{"selected":"a","expanded":true,"fresh":true,"items":[{"id":"a"}]}},"data":{"state":"ready","source":"owner"}}}})
    }
    #[test]
    fn layout_excludes_live_selection_and_binds_latest_only() {
        let doc = layout(&fixture()).unwrap();
        assert!(doc["surfaces"][0]["content"]["spec"]["frame"]["panel"]["selected"].is_null());
        assert_eq!(doc["bindings"][0]["delivery"]["mode"], "latest");
    }
    #[test]
    fn unavailable_media_does_not_remove_the_agent_dashboard() {
        let mut doc = layout(&fixture()).unwrap();
        doc["surfaces"].as_array_mut().unwrap().push(
            json!({"id":"reel","content":{"kind":"stream","url":"http://127.0.0.1/reel.m3u8"}}),
        );
        media_unavailable(&mut doc, "private media probe failed").unwrap();
        assert_eq!(doc["surfaces"][0]["content"]["spec"]["kind"], "signal_view");
        assert_eq!(
            doc["surfaces"][1]["content"]["spec"]["data"]["state"],
            "error"
        );
    }
    #[test]
    fn static_caption_uses_authored_state_but_delivery_still_expires() {
        let mut presentation = fixture();
        presentation["document"]["surfaces"][0]["content"]["spec"]["data"] =
            json!({"state":"ready","source":"public-reels"});
        presentation["surfaces"]["tasks"]["data"]["state"] = json!("no_data");
        assert_eq!(
            events(&presentation, 1, false).unwrap()[0]["payload"]["data"]["state"],
            "ready"
        );
        assert_eq!(
            events(&presentation, 2, true).unwrap()[0]["payload"]["data"]["state"],
            "stale"
        );
        presentation["document"]["surfaces"][0]["content"]["spec"]["refresh_ms"] = json!(2000);
        assert_eq!(
            events(&presentation, 3, false).unwrap()[0]["payload"]["data"]["state"],
            "no_data"
        );
    }
    #[test]
    fn delivery_expiry_preserves_selection_but_never_fakes_owner_freshness() {
        let live = events(&fixture(), 1, false).unwrap();
        assert_eq!(
            live[0]["payload"]["widget"]["frame"]["panel"]["selected"],
            "a"
        );
        assert_eq!(
            live[0]["payload"]["widget"]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect::<Vec<_>>(),
            vec!["frame", "kind", "title"]
        );
        let stale = events(&fixture(), 2, true).unwrap();
        assert_eq!(
            stale[0]["payload"]["widget"]["frame"]["panel"]["expanded"],
            true
        );
        assert_eq!(
            stale[0]["payload"]["widget"]["frame"]["panel"]["fresh"],
            false
        );
        assert_eq!(stale[0]["payload"]["data"]["state"], "stale");
        assert_eq!(stale[0]["payload"]["data"]["source"], "owner");
    }
}
