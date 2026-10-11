//! Read-only public Panopticon entity projection. No renderer or source credentials.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
};

fn project(input: &Value, now: u64) -> Result<Value, String> {
    let mut markers = Vec::new();
    let mut sources = Vec::new();
    let mut ids = BTreeSet::new();
    let mut observations = serde_json::Map::new();
    for domain in ["adsb", "ais"] {
        let source = &input[domain];
        let Some(rows) = source["entities"].as_array() else {
            sources.push(json!({"domain":domain,"state":"disconnected","error":source["error"]}));
            continue;
        };
        if rows.len() > 4096 {
            return Err("owner result exceeds 4096 entities".into());
        }
        let mut expired = 0;
        let mut accepted = 0;
        for row in rows {
            let id = row["id"].as_str().ok_or("missing owner entity id")?;
            if id.is_empty() || id.len() > 128 {
                return Err("invalid owner id".into());
            }
            let e = &row["entity"];
            let observed = e["last_update"]
                .as_u64()
                .ok_or("missing observation timestamp")?;
            let ttl = e["ttl_secs"].as_u64().ok_or("missing owner TTL")?;
            if ttl == 0 || ttl > 86400 {
                return Err("invalid owner TTL".into());
            }
            if observed > now.saturating_add(5) {
                return Err("future observation timestamp".into());
            }
            if now.saturating_sub(observed) >= ttl {
                expired += 1;
                continue;
            }
            let lat = e["coords"][0].as_f64().ok_or("missing latitude")?;
            let lon = e["coords"][1].as_f64().ok_or("missing longitude")?;
            if !lat.is_finite()
                || !lon.is_finite()
                || !(-90.0..=90.0).contains(&lat)
                || !(-180.0..=180.0).contains(&lon)
            {
                return Err("invalid WGS84 point".into());
            }
            let key = format!("{domain}:{id}");
            if !ids.insert(key.clone()) {
                return Err("duplicate owner entity".into());
            }
            let label = e["callsign"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or(id);
            if label.len() > 128 {
                return Err("label exceeds bound".into());
            }
            markers.push(json!({"id":key,"label":label,"latitude_deg":lat,"longitude_deg":lon,"role":if domain=="adsb" {"primary"} else {"secondary"}}));
            observations.insert(key,json!({"last_update":observed,"ttl_secs":ttl,"source":e["payload"]["source"].as_str().filter(|s|s.len()<=128)}));
            accepted += 1;
        }
        sources.push(json!({"domain":domain,"state":if accepted==0 {"empty"} else {"ready"},"visible":accepted,"expired":expired,"owner_entities":rows.len()}));
    }
    markers.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let omitted = markers.len().saturating_sub(512);
    markers.truncate(512);
    observations.retain(|key, _| markers.iter().any(|marker| marker["id"] == *key));
    Ok(
        json!({"schema":"canvas.panopticon-wall.v1","observed_at":chrono::DateTime::from_timestamp(now as i64,0).ok_or("invalid fetch time")?.to_rfc3339(),"observed_at_unix_ms":now.saturating_mul(1000),"observations":observations,"omitted":omitted,"sources":sources,"map":{"extent":{"south_deg":46.8,"north_deg":48.7,"west_deg":-123.5,"east_deg":-121.5},"markers":markers,"paths":[],"selected":null}}),
    )
}

fn fetch(host: &str) -> Result<Value, String> {
    // Private SSH backhaul; public read endpoint only, no provider keys exposed.
    if host.starts_with('-')
        || host.len() > 256
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"@.-".contains(&b))
    {
        return Err("invalid SSH owner".into());
    }
    let mut result = json!({});
    for (domain, port) in [("adsb", 18080), ("ais", 18581)] {
        let command=format!("curl --max-time 8 --fail --silent --show-error 'http://127.0.0.1:{port}/entities/{domain}/near?lat=47.65&lon=-122.35&radius=500000'");
        let mut child = std::process::Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "ServerAliveInterval=5",
                "-o",
                "ServerAliveCountMax=2",
                host,
                &command,
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        child
            .stdout
            .take()
            .ok_or("missing source pipe")?
            .take(2 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 2 * 1024 * 1024 {
            let _ = child.kill();
            let _ = child.wait();
            return Err("owner response exceeds 2MiB".into());
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        result[domain] = if status.success() {
            json!({"entities":serde_json::from_slice::<Value>(&bytes).map_err(|e|format!("invalid {domain} owner JSON: {e}"))?})
        } else {
            json!({"error":"owner request failed"})
        };
    }
    Ok(result)
}
fn boundaries(path: &str, digest: &str) -> Result<Vec<Value>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 256 * 1024 || format!("{:x}", Sha256::digest(&bytes)) != digest {
        return Err("owner boundary size/digest mismatch".into());
    }
    let data: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let mut paths = Vec::new();
    let mut count = 0;
    for feature in data["features"]
        .as_array()
        .ok_or("missing owner features")?
    {
        if feature["properties"]["name"] != "Washington" {
            continue;
        }
        if feature["geometry"]["type"] != "MultiPolygon" {
            return Err("unexpected owner Washington geometry".into());
        }
        for polygon in feature["geometry"]["coordinates"]
            .as_array()
            .ok_or("missing polygons")?
        {
            for ring in polygon.as_array().ok_or("missing rings")? {
                let mut points = Vec::new();
                for coordinate in ring.as_array().ok_or("missing ring points")? {
                    let longitude = coordinate[0].as_f64().ok_or("invalid boundary longitude")?;
                    let latitude = coordinate[1].as_f64().ok_or("invalid boundary latitude")?;
                    if !longitude.is_finite()
                        || !latitude.is_finite()
                        || !(-180.0..=180.0).contains(&longitude)
                        || !(-90.0..=90.0).contains(&latitude)
                    {
                        return Err("invalid boundary WGS84 point".into());
                    }
                    points.push(json!({"latitude_deg":latitude,"longitude_deg":longitude}));
                    count += 1;
                    if count > 8192 {
                        return Err("boundary point budget exceeded".into());
                    }
                }
                paths.push(json!({"id":format!("owner-washington-{}",paths.len()),"role":"outline","points":points}));
                if paths.len() > 128 {
                    return Err("boundary path budget exceeded".into());
                }
            }
        }
    }
    if paths.is_empty() {
        return Err("Washington boundary absent".into());
    }
    Ok(paths)
}
fn compose(
    host: &str,
    binder: &str,
    projection: &Value,
) -> Result<Value, Box<dyn std::error::Error>> {
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
fn watch(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    if args.len() != 9 {
        return Err(
            "usage: watch SSH_OWNER BOUNDARY SHA BINDER WIDGET_HOST TVCTL ENDPOINT CYCLES".into(),
        );
    }
    let cycles = args[8].parse::<u32>()?;
    if !(1..=120).contains(&cycles) {
        return Err("finite cycles must be 1..120".into());
    }
    let paths = boundaries(&args[2], &args[3])?;
    for tick in 0..cycles {
        let start = std::time::Instant::now();
        let input = fetch(&args[1])?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let mut projection = project(&input, now)?;
        projection["map"]["paths"] = json!(paths);
        projection["boundary_source"] =
            json!("Panopticon owner Washington state boundary · not terrain");
        let composed = compose(&args[5], &args[4], &projection)?;
        let request = json!({"schema_version":1,"request_id":format!("panopticon-wall-{now}-{tick}"),"command":{"command":"surface_snapshot","workspace_id":"tv-panopticon-wall","surface_id":"panopticon-map","frame":composed["map"]["frame"],"data":composed["map"]["data"]}});
        let receipt = canvas_widget_host::presentation::call(&args[6], &args[7], &request)?;
        if receipt["ok"] != true {
            return Err(format!("map snapshot rejected: {receipt}").into());
        }
        // Health edits affect only the selected owner workspace, never switch it.
        let current = canvas_widget_host::presentation::call(
            &args[6],
            &args[7],
            &json!({"schema_version":1,"request_id":"panopticon-health-read","command":{"command":"read_document"}}),
        )?;
        if current["ok"] == true && current["data"]["id"] == "tv-panopticon-wall" {
            let revision = current["data"]["revision"]
                .as_u64()
                .ok_or("missing workspace revision")?;
            let surface = json!({"id":"panopticon-source","title":"Source health","content":{"kind":"widget","spec":composed["evidence"]}});
            let patch = json!({"schema_version":1,"request_id":format!("panopticon-health-{now}"),"expected_revision":current["revision"],"command":{"command":"apply_patch","patch":{"schema_version":1,"workspace_id":"tv-panopticon-wall","base_revision":revision,"revision":revision+1,"operations":[{"op":"upsert_surface","surface":surface}]}}});
            let receipt = canvas_widget_host::presentation::call(&args[6], &args[7], &patch)?;
            if receipt["ok"] != true {
                return Err("health patch rejected; selection/generation may have changed".into());
            }
        }
        println!(
            "{}",
            json!({"delivered_at":projection["observed_at"],"sources":projection["sources"],"cycle":tick+1,"cycles":cycles})
        );
        if tick + 1 < cycles {
            std::thread::sleep(std::time::Duration::from_secs(5).saturating_sub(start.elapsed()));
        }
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "watch") {
        return watch(&args);
    }
    if args.first().is_some_and(|a| a == "fetch") {
        if args.len() != 2 && args.len() != 4 {
            return Err(
                "usage: canvas-panopticon-projection fetch SSH_OWNER [OWNER_BOUNDARY_FILE SHA256]"
                    .into(),
            );
        }
        let input = fetch(&args[1])?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_secs();
        let mut projection = project(&input, now)?;
        if args.len() == 4 {
            projection["map"]["paths"] = json!(boundaries(&args[2], &args[3])?);
            projection["boundary_source"] = json!(
                "Panopticon owner us_states.geojson · Washington state boundary, not terrain"
            );
            projection["boundary_sha256"] = json!(args[3]);
        }
        println!("{projection}");
        return Ok(());
    }
    let mut input = String::new();
    std::io::stdin()
        .take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut input)?;
    if input.len() > 2 * 1024 * 1024 {
        return Err("input exceeds 2MiB".into());
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    println!("{}", project(&serde_json::from_str(&input)?, now)?);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn input() -> Value {
        json!({"adsb":{"entities":[{"id":"abc","entity":{"coords":[47.5,-122.3,0],"last_update":100,"ttl_secs":180,"callsign":"REAL"}}]},"ais":{"entities":[]}})
    }
    #[test]
    fn retains_real_points_and_honest_empty_source() {
        let p = project(&input(), 101).unwrap();
        assert_eq!(p["map"]["markers"][0]["id"], "adsb:abc");
        assert_eq!(p["sources"][1]["state"], "empty");
        assert_eq!(p["observations"]["adsb:abc"]["last_update"], 100);
        assert_eq!(p["observations"]["adsb:abc"]["ttl_secs"], 180);
    }
    #[test]
    fn successful_fetch_does_not_refresh_expired_observation() {
        let p = project(&input(), 280).unwrap();
        assert_eq!(p["map"]["markers"].as_array().unwrap().len(), 0);
        assert_eq!(p["sources"][0]["expired"], 1);
    }
    #[test]
    fn missing_and_invalid_are_not_empty() {
        let p = project(
            &json!({"adsb":{"error":"timeout"},"ais":{"entities":[]}}),
            101,
        )
        .unwrap();
        assert_eq!(p["sources"][0]["state"], "disconnected");
        let mut bad = input();
        bad["adsb"]["entities"][0]["entity"]["coords"][0] = json!(91);
        assert!(project(&bad, 101).is_err());
    }
    #[test]
    fn rejects_duplicates_and_future_owner_observations() {
        let mut input = input();
        let duplicate = input["adsb"]["entities"][0].clone();
        input["adsb"]["entities"]
            .as_array_mut()
            .unwrap()
            .push(duplicate);
        assert!(project(&input, 101).is_err());
        input["adsb"]["entities"].as_array_mut().unwrap().pop();
        input["adsb"]["entities"][0]["entity"]["last_update"] = json!(107);
        assert!(project(&input, 101).is_err());
    }
    #[test]
    fn owner_boundary_requires_actual_digest_and_bounded_wgs84() {
        let fixture = json!({"features":[{"properties":{"name":"Washington"},"geometry":{"type":"MultiPolygon","coordinates":[[[[-123.0,47.0],[-122.0,48.0],[-123.0,47.0]]]]}}]});
        let bytes = serde_json::to_vec(&fixture).unwrap();
        let path = std::env::temp_dir().join(format!(
            "canvas-panopticon-boundary-test-{}-{}.json",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.write_all(&bytes).unwrap();
        drop(file);
        let digest = format!("{:x}", Sha256::digest(&bytes));
        let points = boundaries(path.to_str().unwrap(), &digest).unwrap();
        assert_eq!(points[0]["points"][0]["latitude_deg"], 47.0);
        assert_eq!(points[0]["role"], "outline");
        assert!(boundaries(path.to_str().unwrap(), "incorrect").is_err());
        std::fs::remove_file(path).unwrap();
    }
}
