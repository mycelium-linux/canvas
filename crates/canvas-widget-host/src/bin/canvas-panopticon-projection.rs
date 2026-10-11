//! Read-only public Panopticon entity projection. No renderer or source credentials.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, io::Read};

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
            if id.is_empty() || id.len() > 120 {
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
                .or_else(|| {
                    if domain == "ais" {
                        e["payload"]["Static"]["Name"]
                            .as_str()
                            .or_else(|| e["payload"]["Name"].as_str())
                            .or_else(|| e["payload"]["shipname"].as_str())
                    } else {
                        None
                    }
                })
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(id);
            if label.len() > 48 {
                return Err("label exceeds bound".into());
            }
            markers.push(json!({"id":key,"label":label,"latitude_deg":lat,"longitude_deg":lon,"observed_at_unix_ms":observed.saturating_mul(1000),"role":if domain=="adsb" {"primary"} else {"secondary"}}));
            observations.insert(key,json!({"last_update":observed,"owner_updated_at":chrono::DateTime::from_timestamp(observed as i64,0).ok_or("invalid owner update time")?.to_rfc3339(),"owner_update_age_secs":now.saturating_sub(observed),"ttl_secs":ttl,"source":e["payload"]["source"].as_str().filter(|s|s.len()<=128),"callsign":label,"latitude_deg":lat,"longitude_deg":lon,"aircraft":if domain=="adsb" {aircraft(&e["payload"]["aircraft"])}else{Value::Null},"vessel":if domain=="ais" {vessel(id,&e["payload"])}else{Value::Null}}));
            accepted += 1;
        }
        sources.push(json!({"domain":domain,"state":if accepted==0 {"empty"} else {"ready"},"visible":accepted,"expired":expired,"owner_entities":rows.len()}));
    }
    markers.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    let omitted = markers.len().saturating_sub(512);
    markers.truncate(512);
    observations.retain(|key, _| markers.iter().any(|marker| marker["id"] == *key));
    let mut distribution = altitude_distribution(&observations);
    let altitude_source_available = input["adsb"]["entities"].is_array();
    distribution["state"] = json!(if altitude_source_available {
        "ready"
    } else {
        "disconnected"
    });
    if !altitude_source_available {
        for point in distribution["plot"]["series"][0]["points"]
            .as_array_mut()
            .ok_or("missing distribution points")?
        {
            point["y"] = Value::Null;
        }
    }
    Ok(
        json!({"schema":"canvas.panopticon-wall.v1","observed_at":chrono::DateTime::from_timestamp(now as i64,0).ok_or("invalid fetch time")?.to_rfc3339(),"observed_at_unix_ms":now.saturating_mul(1000),"observations":observations,"omitted":omitted,"sources":sources,"selection":{"state":"none"},"distribution":distribution,"map":{"extent":{"south_deg":46.8,"north_deg":48.7,"west_deg":-123.5,"east_deg":-121.5},"markers":markers,"paths":[],"selected":null}}),
    )
}

fn bounded_number(value: &Value, minimum: f64, maximum: f64) -> Option<f64> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && *v >= minimum && *v <= maximum)
}
fn bounded_text(value: &Value) -> Option<&str> {
    value.as_str().filter(|s| !s.is_empty() && s.len() <= 128)
}
fn vessel(id: &str, raw: &Value) -> Value {
    let speed = raw["Sog"]
        .as_f64()
        .or_else(|| raw["speed_knots"].as_f64())
        .filter(|v| v.is_finite() && *v >= 0.0 && *v < 102.3);
    let course = raw["Cog"]
        .as_f64()
        .or_else(|| raw["course_deg"].as_f64())
        .filter(|v| v.is_finite() && *v >= 0.0 && *v < 360.0);
    let heading = raw["TrueHeading"]
        .as_f64()
        .filter(|v| v.is_finite() && *v >= 0.0 && *v < 360.0);
    let call = raw["Static"]["CallSign"]
        .as_str()
        .or_else(|| raw["callsign"].as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= 48 && !s.chars().any(char::is_control));
    json!({"mmsi":id,"speed_knots":speed,"course_deg":course,"true_heading_deg":heading,"radio_callsign":call})
}

fn aircraft(raw: &Value) -> Value {
    json!({"barometric_altitude_ft":bounded_number(&raw["alt_baro"],-1500.0,100000.0),"ground":raw["alt_baro"]=="ground","ground_speed_knots":bounded_number(&raw["gs"],0.0,2000.0),"track_deg":bounded_number(&raw["track"],0.0,359.999999),"registration":bounded_text(&raw["r"]),"type_designator":bounded_text(&raw["t"]),"photo":{"state":"unavailable","reason":"No aircraft image is supplied by this owner feed"}})
}
fn altitude_distribution(observations: &serde_json::Map<String, Value>) -> Value {
    let mut bins = [0u32; 21];
    let mut ground = 0;
    let mut unknown = 0;
    let mut known = 0;
    for (id, observation) in observations {
        if !id.starts_with("adsb:") {
            continue;
        }
        let a = &observation["aircraft"];
        if a["ground"] == true {
            ground += 1;
            continue;
        }
        if let Some(altitude) = a["barometric_altitude_ft"].as_f64() {
            let slot = ((altitude + 5000.0) / 5000.0).floor().clamp(0.0, 20.0) as usize;
            bins[slot] += 1;
            known += 1;
        } else {
            unknown += 1;
        }
    }
    let upper = (bins.iter().rposition(|count| *count > 0).unwrap_or(1) + 1).max(2);
    let points: Vec<_> = bins[..upper]
        .iter()
        .enumerate()
        .map(|(index, count)| json!({"x":-2500.0+index as f64*5000.0,"y":count}))
        .collect();
    json!({"known":known,"ground":ground,"unknown":unknown,"band_width_ft":5000,"plot":{"x":{"label":"Barometric altitude","unit":"ft","minimum":-5000.0,"maximum":-5000.0+upper as f64*5000.0},"y":{"label":"Aircraft per band","unit":"count","minimum":0.0,"maximum":bins.iter().copied().max().unwrap_or(0).max(1)},"series":[{"id":"altitude-bands","label":"5,000 ft altitude bands","role":"primary","points":points}],"viewport":null}})
}
#[cfg(test)]
use canvas_widget_host::panopticon_wall::select;
#[cfg(test)]
use std::io::Write;
fn fetch(host: &str) -> Result<Value, String> {
    fetch_from(Some(host))
}
fn fetch_from(host: Option<&str>) -> Result<Value, String> {
    // Private SSH backhaul; public read endpoint only, no provider keys exposed.
    if host.is_some_and(|host| {
        host.starts_with('-')
            || host.len() > 256
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"@.-".contains(&b))
    }) {
        return Err("invalid SSH owner".into());
    }
    let mut result = json!({});
    for (domain, port) in [("adsb", 18080), ("ais", 18581)] {
        let command=format!("curl --max-time 8 --fail --silent --show-error 'http://127.0.0.1:{port}/entities/{domain}/near?lat=47.65&lon=-122.35&radius=500000'");
        let mut source = if let Some(host) = host {
            let mut source = std::process::Command::new("ssh");
            source.args([
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
            ]);
            source
        } else {
            let mut source = std::process::Command::new("curl");
            source.args(["--max-time", "8", "--fail", "--silent", "--show-error"])
                .arg(format!("http://127.0.0.1:{port}/entities/{domain}/near?lat=47.65&lon=-122.35&radius=500000"));
            source
        };
        let mut child = source
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
    let mut lost_selection: Option<String> = None;
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
        canvas_widget_host::panopticon_wall::present(
            projection.clone(),
            &args[5],
            &args[4],
            &args[6],
            &args[7],
            &mut lost_selection,
        )?;
        println!(
            "{}",
            json!({"delivered_at":projection["observed_at"],"sources":projection["sources"],"selection":projection["selection"]["id"],"distribution":{"known":projection["distribution"]["known"],"ground":projection["distribution"]["ground"],"unknown":projection["distribution"]["unknown"]},"cycle":tick+1,"cycles":cycles})
        );
        if tick + 1 < cycles {
            std::thread::sleep(std::time::Duration::from_secs(5).saturating_sub(start.elapsed()));
        }
    }
    Ok(())
}
/// Source payload has no recipient-local UI selection.
fn source_projection(
    input: &Value,
    now: u64,
    paths: &[Value],
    digest: &str,
) -> Result<Value, String> {
    let mut value = project(input, now)?;
    value
        .as_object_mut()
        .ok_or("projection is not an object")?
        .remove("selection");
    value["map"]
        .as_object_mut()
        .ok_or("map is not an object")?
        .remove("selected");
    value["map"]["paths"] = json!(paths);
    value["boundary_source"] = json!("Panopticon owner Washington state boundary · not terrain");
    value["boundary_sha256"] = json!(digest);
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 64 * 1024 {
        return Err("source projection exceeds 64KiB".into());
    }
    Ok(value)
}

fn source(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use unibus::{
        client::{Client, ClientConfig},
        Message,
    };
    if args.len() != 4 && args.len() != 5 {
        return Err("usage: source CLIENT_CONFIG OWNER_BOUNDARY SHA256 [FINITE_CYCLES]".into());
    }
    let limit = if args.len() == 5 {
        Some(args[4].parse::<u32>()?)
    } else {
        None
    };
    if limit.is_some_and(|n| !(1..=120).contains(&n)) {
        return Err("finite cycles must be 1..120".into());
    }
    let config_path = std::path::Path::new(&args[1]);
    let mut config: ClientConfig = serde_json::from_slice(&std::fs::read(config_path)?)?;
    config.resolve_token_file(config_path);
    config.read_timeout_ms = 100;
    let paths = boundaries(&args[2], &args[3])?;
    let mut client = None;
    let mut failures = 0u32;
    let mut sequence = unibus::envelope::now_ms();
    let mut cycles = 0u32;
    loop {
        let start = std::time::Instant::now();
        if client.is_none() {
            match Client::connect(config.clone()) {
                Ok(connection) => {
                    client = Some(connection);
                    failures = 0;
                }
                Err(error) => {
                    failures = failures.saturating_add(1);
                    eprintln!("panopticon source: Unibus connection failed: {error}");
                    if limit.is_some() {
                        return Err(error.into());
                    }
                    std::thread::sleep(std::time::Duration::from_secs(1u64 << failures.min(5)));
                    continue;
                }
            }
        }
        let input = match fetch_from(None) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("panopticon source: owner fetch failed: {error}");
                json!({"adsb":{"error":"owner fetch rejected"},"ais":{"error":"owner fetch rejected"}})
            }
        };
        let now = unibus::envelope::now_ms().max(1);
        let mut projection = match source_projection(&input, now / 1000, &paths, &args[3]) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("panopticon source: invalid owner observation: {error}");
                source_projection(
                    &json!({"adsb":{"error":"owner observation invalid"},"ais":{"error":"owner observation invalid"}}),
                    now / 1000,
                    &paths,
                    &args[3],
                )?
            }
        };
        sequence = sequence
            .checked_add(1)
            .ok_or("sequence exhausted")?
            .max(now);
        let connection = client.as_ref().ok_or("missing connection")?;
        let mut tile_asset = Value::Null;
        let mut tile_payload = Vec::new();
        let mut tile_batch = Vec::new();
        if let Some(root) = std::env::var_os("PANOPTICON_TILE_CACHE") {
            match canvas_widget_host::panopticon_tiles::cached(
                &projection["map"],
                std::path::Path::new(&root),
            ) {
                Ok(canvas_widget_host::panopticon_tiles::CachedViewport { layer, payloads }) => {
                    let index = cycles as usize * 8 % payloads.len();
                    tile_asset = layer["tiles"][index].clone();
                    tile_payload = payloads[index].clone();
                    for offset in 1..payloads.len().min(8) {
                        let i = (index + offset) % payloads.len();
                        tile_batch.push((layer["tiles"][i].clone(), payloads[i].clone()));
                    }
                    projection["map"]["basemap"] = layer;
                }
                Err(error) => eprintln!("Panopticon viewport cache unavailable: {error}"),
            }
        }
        let body = json!({"sequence":sequence,"observed_at_unix_ms":now,"projection":projection,"tile_asset":tile_asset});
        let delivered = (|| -> Result<(), Box<dyn std::error::Error>> {
            let mut envelope = connection.envelope(
                Some("panopticon"),
                Message::new("panopticon.wall.snapshot.v1", body),
            )?;
            envelope.payload_bytes = tile_payload.len();
            connection.send(&envelope, &tile_payload)?;
            // Bounded warm-up burst: assets are still independently verified and
            // snapshots coalesced by the receiver, rather than a 100-second
            // one-tile-per-poll cold start. No unbounded network/GPU queue.
            for (asset, bytes) in &tile_batch {
                sequence = sequence.checked_add(1).ok_or("sequence exhausted")?;
                let mut extra = connection.envelope(
                    Some("panopticon"),
                    Message::new("panopticon.wall.snapshot.v1", json!({"sequence":sequence,"observed_at_unix_ms":now,"projection":projection,"tile_asset":asset})),
                )?;
                extra.payload_bytes = bytes.len();
                connection.send(&extra, bytes)?;
            }
            while let Some((reply, _)) = connection.try_read()? {
                if reply.message.body.get("success").and_then(Value::as_bool) == Some(false) {
                    return Err("source publication rejected".into());
                }
            }
            Ok(())
        })();
        match delivered {
            Ok(()) => println!(
                "{}",
                json!({"published_sequence":sequence,"observed_at_unix_ms":now,"sources":projection["sources"]})
            ),
            Err(error) => {
                eprintln!("panopticon source: publication failed: {error}");
                client = None;
                if limit.is_some() {
                    return Err(error);
                }
            }
        }
        cycles = cycles.checked_add(1).ok_or("cycle count exhausted")?;
        if limit.is_some_and(|n| cycles >= n) {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(5).saturating_sub(start.elapsed()));
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "source") {
        return source(&args);
    }
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
    #[test]
    fn vessel_units_preserve_zero_and_reject_ais_unavailable_sentinels() {
        let reported = vessel(
            "338250000",
            &json!({"Sog":0.0,"Cog":270.5,"TrueHeading":271,"Static":{"CallSign":" TEST "}}),
        );
        assert_eq!(reported["speed_knots"], 0.0);
        assert_eq!(reported["course_deg"], 270.5);
        assert_eq!(reported["radio_callsign"], "TEST");
        let unavailable = vessel(
            "338250000",
            &json!({"Sog":102.3,"Cog":360,"TrueHeading":511}),
        );
        assert!(unavailable["speed_knots"].is_null());
        assert!(unavailable["course_deg"].is_null());
        assert!(unavailable["true_heading_deg"].is_null());
    }
    #[test]
    fn source_has_no_recipient_selection_and_preserves_owner_evidence() {
        let source = source_projection(&input(), 101, &[], "digest").unwrap();
        assert!(source.get("selection").is_none());
        assert!(source["map"].get("selected").is_none());
        assert_eq!(source["observations"]["adsb:abc"]["last_update"], 100);
        assert_eq!(source["observations"]["adsb:abc"]["ttl_secs"], 180);
        let disconnected = source_projection(
            &json!({"adsb":{"error":"failed"},"ais":{"error":"failed"}}),
            101,
            &[],
            "digest",
        )
        .unwrap();
        assert_eq!(disconnected["sources"][0]["state"], "disconnected");
        assert!(disconnected["map"]["markers"]
            .as_array()
            .unwrap()
            .is_empty());
    }
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
        assert_eq!(p["distribution"]["state"], "disconnected");
        assert!(p["distribution"]["plot"]["series"][0]["points"]
            .as_array()
            .unwrap()
            .iter()
            .all(|point| point["y"].is_null()));
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
        input["adsb"]["entities"][0]["entity"]["last_update"] = json!(100);
        input["adsb"]["entities"][0]["entity"]["callsign"] = json!("x".repeat(49));
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
    #[test]
    fn aircraft_units_ground_and_missing_are_not_invented() {
        let a = aircraft(&json!({"alt_baro":"ground","gs":0,"track":90,"r":"N123","t":"B738"}));
        assert_eq!(a["ground"], true);
        assert!(a["barometric_altitude_ft"].is_null());
        assert_eq!(a["ground_speed_knots"], 0.0);
        assert_eq!(a["registration"], "N123");
        let missing = aircraft(&json!({"gs":-3,"alt_baro":200000,"track":400}));
        assert!(missing["ground_speed_knots"].is_null());
        assert!(missing["barometric_altitude_ft"].is_null());
        assert!(missing["track_deg"].is_null());
        assert_eq!(missing["photo"]["state"], "unavailable");
    }
    #[test]
    fn distribution_counts_actual_altitudes_not_ground_or_missing() {
        let pool=serde_json::from_value::<serde_json::Map<String,Value>>(json!({"adsb:a":{"aircraft":{"barometric_altitude_ft":8000,"ground":false}},"adsb:b":{"aircraft":{"barometric_altitude_ft":null,"ground":true}},"adsb:c":{"aircraft":{"barometric_altitude_ft":null,"ground":false}},"ais:d":{"aircraft":null}})).unwrap();
        let distribution = altitude_distribution(&pool);
        assert_eq!(distribution["known"], 1);
        assert_eq!(distribution["ground"], 1);
        assert_eq!(distribution["unknown"], 1);
        assert_eq!(distribution["plot"]["x"]["unit"], "ft");
        assert_eq!(
            distribution["plot"]["series"][0]["points"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["y"].as_u64().unwrap())
                .sum::<u64>(),
            1
        );
    }
    #[test]
    fn selection_is_exact_and_lost_contact_never_selects_another() {
        let mut projection = project(&input(), 101).unwrap();
        select(&mut projection, Some("adsb:abc")).unwrap();
        assert_eq!(projection["map"]["selected"], "adsb:abc");
        assert_eq!(projection["selection"]["observation"]["callsign"], "REAL");
        select(&mut projection, Some("adsb:missing")).unwrap();
        assert_eq!(projection["selection"]["state"], "lost");
        assert_eq!(projection["selection"]["id"], "adsb:missing");
        assert!(projection["map"]["selected"].is_null());
        select(&mut projection, None).unwrap();
        assert_eq!(projection["selection"]["state"], "none");
    }
}
