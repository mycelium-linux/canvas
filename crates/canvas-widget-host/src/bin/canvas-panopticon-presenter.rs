//! Receive owner observations through Unibus; never fetch provider data here.
use canvas_widget_host::panopticon_wall;
use serde::Deserialize;
use serde_json::Value;
use std::{error::Error, path::Path, thread, time::Duration};
use unibus::client::{Client, ClientConfig};

const KIND: &str = "panopticon.wall.snapshot.v1";
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
                    let (envelope, payload) = match client.try_read() {
                        Ok(Some(frame)) => frame,
                        Ok(None) => continue,
                        Err(error) => {
                            eprintln!("Unibus disconnected: {error}");
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
                                if let Err(error) =
                                    canvas_widget_host::panopticon_tiles::verify(asset, &payload)
                                {
                                    eprintln!("rejected tile: {error}");
                                    continue;
                                }
                                let sha =
                                    asset["sha256"].as_str().ok_or("missing verified digest")?;
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
                            if let Err(error) = panopticon_wall::present(
                                envelope.message.body["projection"].clone(),
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
