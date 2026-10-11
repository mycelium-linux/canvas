//! Receive owner observations through Unibus; never fetch provider data here.
use canvas_widget_host::panopticon_wall;
use serde::Deserialize;
use serde_json::Value;
use std::{error::Error, path::Path, thread, time::Duration};
use unibus::client::{Client, ClientConfig};

const KIND: &str = "panopticon.wall.snapshot.v1";
type Pending = ((u64, u64), Value);
fn retain_latest(pending: &mut Option<Pending>, generation: (u64, u64), projection: Value) {
    if pending
        .as_ref()
        .is_none_or(|(previous, _)| generation > *previous)
    {
        *pending = Some((generation, projection));
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    bus: ClientConfig,
    source_producer: String,
    widget_host: String,
    binder: String,
    tvctl: String,
    endpoint: String,
}

fn accept(body: &Value, now: u64, previous: (u64, u64)) -> Result<(u64, u64), &'static str> {
    let observed = body["observed_at_unix_ms"]
        .as_u64()
        .ok_or("missing observation time")?;
    let sequence = body["sequence"].as_u64().ok_or("missing sequence")?;
    if observed > now.saturating_add(5000) || now.saturating_sub(observed) > 20000 {
        return Err("expired or future observation");
    }
    if (observed, sequence) <= previous {
        return Err("duplicate or reordered observation");
    }
    if body["projection"]["schema"] != "canvas.panopticon-wall.v1"
        || body["projection"]["observed_at_unix_ms"].as_u64() != Some(observed / 1000 * 1000)
    {
        return Err("projection contract mismatch");
    }
    Ok((observed, sequence))
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: canvas-panopticon-presenter CONFIG")?;
    let bytes = std::fs::read(&path)?;
    if bytes.len() > 16384 {
        return Err("configuration too large".into());
    }
    let mut config: Config = serde_json::from_slice(&bytes)?;
    config.bus.resolve_token_file(Path::new(&path));
    let address: std::net::SocketAddr = config.bus.hub_endpoint.parse()?;
    if !address.ip().is_loopback() || config.bus.schema_version != 2 {
        return Err("presenter requires a loopback authenticated v2 Unibus connection".into());
    }
    config.bus.read_timeout_ms = 500;
    let mut previous = (0, 0);
    let mut lost = None;
    let mut tiles = std::collections::BTreeSet::new();
    let mut delay = 2;
    loop {
        match Client::connect(config.bus.clone()) {
            Err(error) => eprintln!("Unibus connection failed: {error}"),
            Ok(client) => {
                delay = 2;
                loop {
                    let mut pending = None;
                    let mut disconnected = false;
                    // Admit every bounded asset, but render only the newest valid
                    // snapshot after a slow owner call; never replay a UI backlog.
                    for _ in 0..16 {
                        let (envelope, payload) = match client.try_read() {
                            Ok(Some(frame)) => frame,
                            Ok(None) => break,
                            Err(error) => {
                                eprintln!("Unibus disconnected: {error}");
                                disconnected = true;
                                break;
                            }
                        };
                        if envelope.message.kind != KIND {
                            continue;
                        }
                        if envelope.producer_id != config.source_producer
                            || envelope.installation_id != config.bus.installation_id
                            || payload.len() > 65536
                            || serde_json::to_vec(&envelope.message.body)?.len() > 65536
                        {
                            eprintln!("rejected unexpected Panopticon sender or payload");
                            continue;
                        }
                        let body = Value::Object(envelope.message.body.clone());
                        match accept(&body, unibus::envelope::now_ms(), previous) {
                            Err(error) => eprintln!("rejected Panopticon observation: {error}"),
                            Ok(generation) => {
                                let asset = &body["tile_asset"];
                                if !payload.is_empty() {
                                    if let Err(error) = canvas_widget_host::panopticon_tiles::verify(
                                        asset, &payload,
                                    ) {
                                        eprintln!("rejected tile: {error}");
                                        continue;
                                    }
                                    let sha = asset["sha256"]
                                        .as_str()
                                        .ok_or("missing verified digest")?;
                                    if let Some(root) = std::env::var_os("CANVAS_MAP_ASSETS") {
                                        std::fs::create_dir_all(&root)?;
                                        let path = std::path::PathBuf::from(root).join(sha);
                                        // Bytes have already passed the common identity verifier.
                                        if !path.exists() {
                                            std::fs::write(path, &payload)?;
                                        }
                                    }
                                    if !tiles.contains(sha) {
                                        if tiles.len() >= 64 {
                                            eprintln!("tile cache admission budget exhausted");
                                            continue;
                                        }
                                        if let Err(error) =
                                            canvas_widget_host::panopticon_tiles::provision(
                                                &config.endpoint,
                                                asset,
                                                &payload,
                                            )
                                        {
                                            eprintln!("tile delivery failed: {error}");
                                            continue;
                                        }
                                        tiles.insert(sha.to_owned());
                                        eprintln!("delivered viewport tile {}", sha);
                                    }
                                } else if !asset.is_null() {
                                    eprintln!("missing tile payload");
                                    continue;
                                }
                                previous = generation;
                                retain_latest(&mut pending, generation, body["projection"].clone());
                            }
                        }
                    }
                    if let Some((generation, projection)) = pending {
                        if let Ok(project) = std::env::var("PANOPTICON_DESKTOP_PROJECT") {
                            let mut desktop = projection.clone();
                            panopticon_wall::select(&mut desktop, None)?;
                            let composed = panopticon_wall::compose(
                                &config.widget_host,
                                &config.binder,
                                &desktop,
                            )?;
                            if let Some(path) = std::env::var_os("PANOPTICON_LOCAL_MAP_FILE") {
                                let map = &composed["map"]["frame"]["map"];
                                if map.is_null() {
                                    return Err("local Galileo projection has no map".into());
                                }
                                let path = std::path::PathBuf::from(path);
                                let parent = path
                                    .parent()
                                    .ok_or("map projection needs a parent directory")?;
                                std::fs::create_dir_all(parent)?;
                                let temporary = path.with_extension("pending");
                                std::fs::write(
                                    &temporary,
                                    serde_json::to_vec(
                                        &serde_json::json!({"schema_version":1,"observed_at_ms":generation.0,"map":map}),
                                    )?,
                                )?;
                                std::fs::rename(temporary, path)?;
                            }
                            for key in ["map", "evidence", "detail"] {
                                if key == "map"
                                    && std::env::var_os("PANOPTICON_LOCAL_MAP_FILE").is_some()
                                {
                                    continue;
                                }
                                let spec = &composed[key];
                                let mut widget = spec.clone();
                                if let Some(object) = widget.as_object_mut() {
                                    for field in [
                                        "schema_version",
                                        "id",
                                        "data",
                                        "chrome",
                                        "refresh_ms",
                                        "theme",
                                    ] {
                                        object.remove(field);
                                    }
                                }
                                let event = serde_json::json!({"schema_version":1,"owner":"panopticon","stream":spec["id"],"contract":"canvas.widget.snapshot.v1","sequence":generation.1,"emitted_at_ms":unibus::envelope::now_ms(),"project_id":project,"payload":{"widget":widget,"data":spec["data"]}});
                                use std::io::Write;
                                let mut child = std::process::Command::new(
                                    std::env::var_os("CANVAS_WIDGET_INGEST_BIN")
                                        .unwrap_or_else(|| "canvas-widget-ingest".into()),
                                )
                                .stdin(std::process::Stdio::piped())
                                .spawn()?;
                                child
                                    .stdin
                                    .take()
                                    .ok_or("missing desktop ingress")?
                                    .write_all(&serde_json::to_vec(&event)?)?;
                                if !child.wait()?.success() {
                                    eprintln!("Neo map ingress rejected event");
                                }
                            }
                        }
                        if let Err(error) = panopticon_wall::present(
                            projection,
                            &config.widget_host,
                            &config.binder,
                            &config.tvctl,
                            &config.endpoint,
                            &mut lost,
                        ) {
                            eprintln!("Canvas presentation failed: {error}");
                        } else {
                            eprintln!(
                                "accepted Panopticon observation {}:{}",
                                generation.0, generation.1
                            );
                        }
                    }
                    if disconnected {
                        break;
                    }
                }
            }
        }
        thread::sleep(Duration::from_secs(delay));
        delay = (delay * 2).min(32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn snapshot() -> Value {
        json!({"sequence":1,"observed_at_unix_ms":10000,"projection":{"schema":"canvas.panopticon-wall.v1","observed_at_unix_ms":10000}})
    }
    #[test]
    fn coalesces_backlog_without_rewinding_the_display() {
        let mut pending = None;
        retain_latest(&mut pending, (10000, 1), json!({"frame":"old"}));
        retain_latest(&mut pending, (15000, 2), json!({"frame":"latest"}));
        retain_latest(&mut pending, (12000, 3), json!({"frame":"reordered"}));
        let (generation, projection) = pending.unwrap();
        assert_eq!(generation, (15000, 2));
        assert_eq!(projection["frame"], "latest");
    }
    #[test]
    fn rejects_replay_age_and_contract_drift() {
        let body = snapshot();
        assert_eq!(accept(&body, 10001, (0, 0)), Ok((10000, 1)));
        assert!(accept(&body, 10001, (10000, 1)).is_err());
        assert!(accept(&body, 40000, (0, 0)).is_err());
        assert!(accept(&body, 1, (0, 0)).is_err());
        let mut wrong = body;
        wrong["projection"]["observed_at_unix_ms"] = json!(9999);
        assert!(accept(&wrong, 10001, (0, 0)).is_err());
    }
}
