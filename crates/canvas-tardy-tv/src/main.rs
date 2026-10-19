//! Anonymous public feed -> explicit private media relay -> Canvas presentation.
//! No credentials, publication, discovery, or agent-state collection.
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const API: &str = "https://api.tardy.news/v1/feed?limit=100";
const HOST: &str = "agora-one";
const ROOT: &str = "/home/ajmwagar/.local/share/canvas-tv-hls";
const MEDIA_LIMIT: u64 = 100 * 1024 * 1024;

#[derive(Deserialize)]
struct Feed {
    items: Vec<Post>,
}
#[derive(Deserialize)]
struct Post {
    id: String,
    caption: String,
    format: String,
    media: Vec<Media>,
}
#[derive(Deserialize)]
struct Media {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    #[serde(default)]
    duration_ms: u64,
}
struct Reel {
    id: String,
    caption: String,
    url: String,
    duration: Duration,
    generation: u128,
}

fn select(feed: Feed) -> Vec<Reel> {
    let mut seen = std::collections::HashSet::new();
    feed.items
        .into_iter()
        .filter_map(|post| {
            if post.format != "reel"
                || post.id.len() != 36
                || !post.id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
                || !seen.insert(post.id.clone())
            {
                return None;
            }
            let media = post.media.into_iter().find(|m| m.kind == "video")?;
            let url = reqwest::Url::parse(&media.url).ok()?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || !url.host_str().is_some_and(|host| {
                    host.ends_with(".r2.cloudflarestorage.com") || host == "media.tardy.news"
                })
            {
                return None;
            }
            if !(1000..=300_000).contains(&media.duration_ms) {
                return None;
            }
            Some(Reel {
                id: post.id,
                caption: post.caption.chars().take(6000).collect(),
                url: media.url,
                duration: Duration::from_millis(media.duration_ms),
                generation: 0,
            })
        })
        .take(32)
        .collect()
}

fn http(client: &reqwest::blocking::Client, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    // Error strings deliberately omit expiring media URLs.
    let response = client
        .get(url)
        .send()
        .map_err(|_| "Public feed/media transport failed")?;
    if !response.status().is_success() {
        return Err(format!(
            "Public feed/media HTTP {}",
            response.status().as_u16()
        ));
    }
    if response.content_length().is_some_and(|size| size > limit) {
        return Err("Public response exceeds limit".into());
    }
    let mut bytes = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Public response read failed")?;
    if bytes.len() as u64 > limit {
        return Err("Public response exceeds limit".into());
    }
    Ok(bytes)
}

fn control(serial: &str, request: &str) -> Result<Value, String> {
    let mut child = Command::new("canvas-tvctl")
        .arg(serial)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Canvas control unavailable")?;
    child
        .stdin
        .take()
        .ok_or("No control input")?
        .write_all(request.as_bytes())
        .map_err(|_| "Control write failed")?;
    let output = child.wait_with_output().map_err(|_| "Control failed")?;
    if !output.status.success() {
        return Err("Canvas rejected update; retrying current revision".into());
    }
    serde_json::from_slice(&output.stdout).map_err(|_| "Invalid control response".into())
}

fn relay(
    client: &reqwest::blocking::Client,
    reel: &Reel,
    directory: &std::path::Path,
) -> Result<(), String> {
    let bytes = http(client, &reel.url, MEDIA_LIMIT)?;
    if let Some(root) = std::env::var_os("CANVAS_TARDY_UNIBUS_SOURCE_DIR") {
        let root = std::path::PathBuf::from(root);
        std::fs::create_dir_all(&root).map_err(|_| "Unibus source directory unavailable")?;
        // Promote complete content first, then its generation. A running
        // FFmpeg keeps the old inode until the media adapter changes generation.
        std::fs::write(root.join("current.mp4.incoming"), bytes)
            .and_then(|_| {
                std::fs::rename(root.join("current.mp4.incoming"), root.join("current.mp4"))
            })
            .map_err(|_| "Unibus media promotion failed")?;
        let expires = reel
            .generation
            .saturating_add(reel.duration.as_millis())
            .saturating_add(30_000);
        let manifest =
            json!({"generation":reel.generation.to_string(),"expires_at_unix_ms":expires});
        std::fs::write(root.join("current.json.incoming"), manifest.to_string())
            .and_then(|_| {
                std::fs::rename(
                    root.join("current.json.incoming"),
                    root.join("current.json"),
                )
            })
            .map_err(|_| "Unibus generation promotion failed")?;
        return Ok(());
    }
    let path = directory.join("reel.mp4");
    std::fs::write(&path, bytes).map_err(|_| "Local media cache failed")?;
    let destination = format!("{HOST}:{ROOT}/tardy-public.mp4");
    let status = Command::new("scp")
        .args(["-q", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5"])
        .arg(path)
        .arg(destination)
        .status()
        .map_err(|_| "Relay transfer unavailable")?;
    if !status.success() {
        return Err("Relay transfer failed".into());
    }
    // One adapter-owned transient unit; never stop another media producer.
    let remote = format!(
        "systemctl --user stop canvas-tardy-reel.service >/dev/null 2>&1\nsystemctl --user reset-failed canvas-tardy-reel.service >/dev/null 2>&1\nrm -f {ROOT}/tardy-public.m3u8\nsystemd-run --user --quiet --collect --unit=canvas-tardy-reel /usr/bin/ffmpeg -hide_banner -loglevel error -re -i {ROOT}/tardy-public.mp4 -c:v copy -c:a aac -f hls -hls_time 1 -hls_list_size 6 -hls_start_number_source epoch -hls_flags delete_segments+independent_segments+temp_file -hls_segment_filename {ROOT}/tardy-public-%d.ts {ROOT}/tardy-public.m3u8 || exit 1\nfor attempt in 1 2 3 4 5 6 7 8 9 10; do test -s {ROOT}/tardy-public.m3u8 && exit 0; sleep 1; done; exit 1"
    );
    let status = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=5",
            HOST,
            &remote,
        ])
        .status()
        .map_err(|_| "Relay start unavailable")?;
    if !status.success() {
        return Err("Relay start failed".into());
    }
    Ok(())
}

fn tardy_presentation(serial: &str) -> Result<Value, String> {
    control(
        serial,
        &json!({"schema_version":1,"request_id":"tardy-source",
            "command":{"command":"workspace_presentation","name":"tardy"}}).to_string(),
    )
}

fn patch(serial: &str, reel: Option<&Reel>, error: Option<&str>) -> Result<(), String> {
    let state = tardy_presentation(serial)?;
    let doc = &state["data"]["document"];
    if doc["project_id"] != "tardy" {
        return Err("Canvas catalog/document disagree".into());
    }
    let mut operations = Vec::new();
    for surface in doc["surfaces"].as_array().ok_or("No Canvas surfaces")? {
        let mut surface = surface.clone();
        match surface["id"].as_str() {
            Some("tardy-video") if reel.is_some() => {
                let reel = reel.unwrap();
                surface["title"] = json!("Tardy · public reel");
                surface["content"] = json!({"kind":"stream","url":reel_endpoint(reel)?});
            }
            Some("tardy-caption") => {
                let spec = &mut surface["content"]["spec"];
                if spec["kind"] == "signal_view" {
                    spec["frame"] = caption_brief(reel.map(|r| r.caption.as_str()), error);
                    spec["data"]["state"] = json!(if error.is_some() { "error" } else { "ready" });
                    operations.push(json!({"op":"upsert_surface","surface":surface}));
                    continue;
                }
                spec["title"] = json!(if error.is_some() {
                    "Tardy · degraded"
                } else {
                    "Tardy · public reels"
                });
                spec["body"] = json!(if let Some(error) = error {
                    error.to_owned()
                } else if let Some(reel) = reel {
                    format!("{}\n\nPublic caption · Tardy", reel.caption)
                } else {
                    "Waiting for public reels".into()
                });
                spec["data"]["state"] = json!(if error.is_some() { "error" } else { "ready" });
            }
            _ => continue,
        }
        operations.push(json!({"op":"upsert_surface","surface":surface}));
    }
    let revision = doc["revision"].as_u64().ok_or("No document revision")?;
    let request = json!({"schema_version":1,"request_id":"tardy-public-feed","expected_revision":state["revision"],"command":{"command":"apply_patch","patch":{"schema_version":1,"workspace_id":doc["id"],"base_revision":revision,"revision":revision+1,"operations":operations}}});
    control(serial, &request.to_string()).map(|_| ())
}

fn reel_endpoint(reel: &Reel) -> Result<String, String> {
    let base = std::env::var("CANVAS_TARDY_MEDIA_BASE")
        .unwrap_or_else(|_| "http://192.168.2.67:18089/tardy-public.m3u8".into());
    private_reel_endpoint(&base, &reel.id, reel.generation)
}

fn private_reel_endpoint(base: &str, id: &str, generation: u128) -> Result<String, String> {
    let mut url = reqwest::Url::parse(base).map_err(|_| "Invalid private reel endpoint")?;
    let host: std::net::Ipv4Addr = url
        .host_str()
        .ok_or("Missing reel host")?
        .parse()
        .map_err(|_| "Reel endpoint requires private IPv4")?;
    if url.scheme() != "http"
        || !host.is_private()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || !url.path().ends_with(".m3u8")
    {
        return Err("Expected a private HLS media-edge endpoint".into());
    }
    url.query_pairs_mut()
        .append_pair("post", id)
        .append_pair("generation", &generation.to_string());
    Ok(url.to_string())
}

fn bounded_text(text: &str, maximum: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control() || *c == '\n')
        .scan(0, |bytes, c| {
            *bytes += c.len_utf8();
            (*bytes <= maximum).then_some(c)
        })
        .collect()
}

fn caption_brief(caption: Option<&str>, error: Option<&str>) -> Value {
    let text = error.unwrap_or_else(|| caption.unwrap_or("Waiting for public reels"));
    let (first, rest) = text.trim().split_once('\n').unwrap_or((text, ""));
    let (headline, body) = if first.len() > 160 {
        ("Public reel", text)
    } else {
        (first, rest)
    };
    json!({"view":"brief","panel":{"kicker":if error.is_some() {"PUBLIC FEED · UNAVAILABLE"} else {"TARDY / PUBLIC REEL"},
        "headline":bounded_text(headline,160),"body":bounded_text(body.trim(),3500),
        "footer":if body.len()>3500 {"Caption excerpt · Tardy"} else {"Public caption · Tardy"}}})
}

fn validate_visual(value: &Value) -> Result<(), String> {
    let frame: fpl_gfx::signals::SignalFrame = serde_json::from_value(value.clone())
        .map_err(|error| format!("Invalid GFX presentation frame: {error}"))?;
    frame
        .validate()
        .map_err(|_| "GFX presentation exceeds bounds".to_owned())
}

fn agent_frame(raw: &[u8], now_ms: i64) -> Result<Value, String> {
    let value: Value = serde_json::from_slice(raw).map_err(|_| "Invalid agent snapshot")?;
    if value["widget"]["kind"] != "status_list" {
        return Err("Expected agent status list".into());
    }
    let nodes = value["widget"]["nodes"]
        .as_array()
        .ok_or("Missing agent rows")?;
    if nodes.len() > 512 {
        return Err("Too many agent rows".into());
    }
    let mut data = value["data"].clone();
    let observed = chrono::DateTime::parse_from_rfc3339(
        data["observed_at"]
            .as_str()
            .ok_or("No agent observation time")?,
    )
    .map_err(|_| "Invalid agent observation time")?
    .timestamp_millis();
    let age = now_ms.saturating_sub(observed);
    if !(-5000..=30000).contains(&age) {
        data["state"] = json!("stale");
    }
    let mut output = json!({"frame":{"title":value["widget"]["title"],"nodes":nodes},"data":data});
    if let Some(visual) = value.get("visual") {
        if visual["view"] != "attention" {
            return Err("Invalid attention instrument".into());
        }
        validate_visual(visual)?;
        output["visual"] = visual.clone();
        if !(-5000..=30000).contains(&age) {
            output["visual"]["panel"]["fresh"] = json!(false);
        }
    }
    if let Some(details) = value.get("details") {
        let rows = details["widget"]["nodes"]
            .as_array()
            .ok_or("Missing attention rows")?;
        if details["widget"]["kind"] != "status_list" || rows.len() > 4 {
            return Err("Invalid attention detail widget".into());
        }
        output["details"] = json!({"title":details["widget"]["title"],"nodes":rows});
    }
    if let Some(deck) = value.get("deck") {
        if deck["view"] != "task_deck" {
            return Err("Expected task deck".into());
        }
        let mut deck = deck.clone();
        // Lua's empty table has no array/object distinction. Only this exact
        // empty representation is canonicalized; malformed nonempty maps fail.
        if deck["panel"]["items"]
            .as_object()
            .is_some_and(|items| items.is_empty())
        {
            deck["panel"]["items"] = json!([]);
        }
        validate_visual(&deck)?;
        output["deck"] = deck;
        if !(-5000..=30000).contains(&age) {
            output["deck"]["panel"]["fresh"] = json!(false);
        }
    }
    if !(-5000..=30000).contains(&age) && value["data"]["source"] == "unibus:agent.sessions.v2" {
        for row in output["frame"]["nodes"]
            .as_array_mut()
            .ok_or("Missing summary rows")?
        {
            row["detail"] = json!("—");
            row["state"] = json!("neutral");
        }
        if let Some(rows) = output["details"]["nodes"].as_array_mut() {
            for row in rows {
                row["detail"] = json!("Telemetry stale · status unknown");
                row["state"] = json!("neutral");
            }
        }
    }
    Ok(output)
}

fn agents(serial: &str) -> Result<(), String> {
    loop {
        let next_refresh = Instant::now() + Duration::from_secs(2);
        let result: Result<(), String> = (|| {
            let state = tardy_presentation(serial)?;
            let file = std::fs::File::open(
                "/Users/ajmwagar/.local/share/agent-telemetry/agent-widget.json",
            )
            .map_err(|_| "Agent owner snapshot unavailable")?;
            let mut raw = Vec::new();
            file.take(65537)
                .read_to_end(&mut raw)
                .map_err(|_| "Agent snapshot read failed")?;
            if raw.len() > 65536 {
                return Err("Agent snapshot exceeds limit".into());
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "Invalid clock")?
                .as_millis() as i64;
            let frame = agent_frame(&raw, now)?;
            let symbolic = state["data"]["document"]["surfaces"]
                .as_array()
                .is_some_and(|surfaces| {
                    surfaces.iter().any(|surface| {
                        surface["id"] == "agent-dashboard"
                            && surface["content"]["spec"]["kind"] == "signal_view"
                    })
                });
            let summary = if symbolic && frame.get("visual").is_some() {
                &frame["visual"]
            } else {
                &frame["frame"]
            };
            let mut targets = vec![("agent-dashboard", summary)];
            if frame.get("details").is_some() {
                let tiled = state["data"]["document"]["surfaces"]
                    .as_array()
                    .is_some_and(|surfaces| {
                        surfaces.iter().any(|surface| {
                            surface["id"] == "agent-details"
                                && surface["content"]["spec"]["kind"] == "signal_view"
                        })
                    });
                targets.push((
                    "agent-details",
                    if tiled && frame.get("deck").is_some() {
                        &frame["deck"]
                    } else {
                        &frame["details"]
                    },
                ));
            }
            for (surface_id, content) in targets {
                if !state["data"]["document"]["surfaces"]
                    .as_array()
                    .ok_or("Missing surfaces")?
                    .iter()
                    .any(|surface| surface["id"] == surface_id)
                {
                    continue;
                }
                let request = json!({"schema_version":1,"request_id":"tardy-agent-snapshot","expected_revision":state["revision"],"command":{"command":"surface_snapshot","workspace_id":state["data"]["document"]["id"],"surface_id":surface_id,"frame":content,"data":frame["data"]}});
                control(serial, &request.to_string())?;
            }
            Ok(())
        })();
        if let Err(error) = result {
            eprintln!("canvas-tardy-tv agents: {error}");
        }
        std::thread::sleep(next_refresh.saturating_duration_since(Instant::now()));
    }
}

fn main() -> Result<(), String> {
    if std::env::args().nth(1).as_deref() == Some("--check-agents") {
        let path = std::env::args()
            .nth(2)
            .ok_or("usage: canvas-tardy-tv --check-agents SNAPSHOT")?;
        let bytes = std::fs::read(path).map_err(|_| "Cannot read agent snapshot")?;
        if bytes.len() > 65536 {
            return Err("Agent snapshot exceeds limit".into());
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "Invalid clock")?
            .as_millis() as i64;
        agent_frame(&bytes, now)?;
        println!("Agent snapshot valid");
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--agents") {
        return agents(
            &std::env::args()
                .nth(2)
                .ok_or("usage: canvas-tardy-tv --agents ADB_SERIAL")?,
        );
    }
    let serial = std::env::args()
        .nth(1)
        .ok_or("usage: canvas-tardy-tv ADB_SERIAL")?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "HTTP client failed")?;
    let directory = std::env::temp_dir().join(format!("canvas-tardy-tv-{}", std::process::id()));
    std::fs::create_dir(&directory).map_err(|_| "Media scratch directory unavailable")?;
    let mut cursor = 0;
    let mut next = Instant::now();
    let mut pending: Option<Reel> = None;
    loop {
        let result = (|| {
            tardy_presentation(&serial)?;
            if let Some(reel) = pending.as_ref() {
                patch(&serial, Some(reel), None)?;
                pending = None;
            }
            if Instant::now() < next {
                return Ok(());
            }
            let bytes = http(&client, API, 3 * 1024 * 1024)?;
            let feed: Feed = serde_json::from_slice(&bytes).map_err(|_| "Malformed public feed")?;
            let mut reels = select(feed);
            if reels.is_empty() {
                return Err("No public video reels available".into());
            }
            let index = cursor % reels.len();
            let mut reel = reels.swap_remove(index);
            reel.generation = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|_| "Invalid system clock")?
                .as_millis();
            relay(&client, &reel, &directory)?;
            next = Instant::now() + reel.duration + Duration::from_secs(2);
            cursor += 1;
            pending = Some(reel);
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            eprintln!("canvas-tardy-tv: {error}");
            let _ = patch(&serial, None, Some(&error));
            next = Instant::now() + Duration::from_secs(10);
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reel_media_edge_is_private_and_generation_bound() {
        let endpoint =
            private_reel_endpoint("http://192.168.2.74:18090/tardy-public.m3u8", "reel", 123)
                .unwrap();
        assert!(endpoint.ends_with("?post=reel&generation=123"));
        for base in [
            "http://8.8.8.8/a.m3u8",
            "http://user@192.168.2.74/a.m3u8",
            "http://192.168.2.74/a.m3u8?x=1",
            "http://192.168.2.74/a.mp4",
        ] {
            assert!(private_reel_endpoint(base, "reel", 123).is_err());
        }
    }
    fn post(id: &str, url: &str) -> Value {
        json!({"id":id,"author_id":"author","caption":"caption","format":"reel","media":[{"type":"video","url":url,"duration_ms":20000}]})
    }
    #[test]
    fn only_bounded_public_feed_video_sources() {
        let id = "0664f863-74f5-4812-aa0c-3d7c47e75312";
        let good = post(id, "https://bucket.r2.cloudflarestorage.com/public.mp4");
        let bad = post(
            "1664f863-74f5-4812-aa0c-3d7c47e75312",
            "http://127.0.0.1/private",
        );
        let feed = serde_json::from_value(json!({"items":[good.clone(),good,bad]})).unwrap();
        assert_eq!(select(feed).len(), 1);
        let mut long = post(id, "https://bucket.r2.cloudflarestorage.com/public.mp4");
        long["media"][0]["duration_ms"] = json!(999999);
        assert!(select(serde_json::from_value(json!({"items":[long]})).unwrap()).is_empty());
    }
    #[test]
    fn preserves_partial_agents_and_marks_stale_observations() {
        let raw = br#"{"widget":{"kind":"status_list","title":"Agents","nodes":[]},"data":{"state":"partial","observed_at":"1970-01-01T00:00:10Z"}}"#;
        assert_eq!(agent_frame(raw, 11000).unwrap()["data"]["state"], "partial");
        assert_eq!(agent_frame(raw, 50000).unwrap()["data"]["state"], "stale");
        assert!(agent_frame(b"{}", 0).is_err());
    }
    #[test]
    fn accepts_bounded_attention_details() {
        let mut value = json!({"widget":{"kind":"status_list","title":"Agents","nodes":[]},
            "data":{"state":"ready","source":"unibus:agent.sessions.v2","observed_at":"1970-01-01T00:00:10Z"},
            "details":{"widget":{"kind":"status_list","title":"Attention","nodes":[{"label":"Canvas","detail":"Needs response"}]}}});
        assert_eq!(
            agent_frame(&serde_json::to_vec(&value).unwrap(), 11000).unwrap()["details"]["nodes"]
                [0]["label"],
            "Canvas"
        );
        let stale = agent_frame(&serde_json::to_vec(&value).unwrap(), 50000).unwrap();
        assert_eq!(
            stale["details"]["nodes"][0]["detail"],
            "Telemetry stale · status unknown"
        );
        value["details"]["widget"]["nodes"] = json!([{}, {}, {}, {}, {}]);
        assert!(agent_frame(&serde_json::to_vec(&value).unwrap(), 11000).is_err());
    }
    #[test]
    fn symbolic_attention_is_bounded_and_cannot_claim_stale_waits() {
        let mut value = json!({"widget":{"kind":"status_list","title":"Agents","nodes":[]},
            "data":{"state":"ready","source":"unibus:agent.sessions.v2","observed_at":"1970-01-01T00:00:10Z"},
            "visual":{"view":"attention","panel":{"input":2,"approval":1,"active":3,"idle":4,"unknown":0,"fresh":true}}});
        let frame = agent_frame(&serde_json::to_vec(&value).unwrap(), 11000).unwrap();
        assert_eq!(frame["visual"]["panel"]["input"], 2);
        assert_eq!(
            agent_frame(&serde_json::to_vec(&value).unwrap(), 50000).unwrap()["visual"]["panel"]["fresh"],
            false
        );
        value["visual"]["panel"]["input"] = json!(32769);
        assert!(agent_frame(&serde_json::to_vec(&value).unwrap(), 11000).is_err());
    }
    #[test]
    fn editorial_caption_is_owner_authored_and_bounds_unicode() {
        let brief = caption_brief(Some("A real headline\n\nA real paragraph."), None);
        assert_eq!(brief["panel"]["headline"], "A real headline");
        assert_eq!(brief["panel"]["body"], "A real paragraph.");
        validate_visual(&brief).unwrap();
        let long = "世界".repeat(1000);
        let brief = caption_brief(Some(&long), None);
        assert_eq!(brief["panel"]["headline"], "Public reel");
        assert!(brief["panel"]["body"].as_str().unwrap().len() <= 3500);
        validate_visual(&brief).unwrap();
        assert_eq!(
            caption_brief(None, Some("Feed offline"))["panel"]["headline"],
            "Feed offline"
        );
    }
    #[test]
    fn task_deck_forwards_bounded_tiles_and_overrides_stale_state() {
        let mut value = json!({"widget":{"kind":"status_list","title":"Agents","nodes":[]},
            "data":{"state":"ready","source":"unibus:agent.sessions.v2","observed_at":"1970-01-01T00:00:10Z"},
            "deck":{"view":"task_deck","panel":{"fresh":true,"items":[{"project":"Canvas","title":"UI pass","meta":"codex @ neo","state":"approval"}]}}});
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            agent_frame(&bytes, 11000).unwrap()["deck"]["panel"]["items"][0]["state"],
            "approval"
        );
        assert_eq!(
            agent_frame(&bytes, 50000).unwrap()["deck"]["panel"]["fresh"],
            false
        );
        value["deck"]["panel"]["items"] = json!([{}, {}, {}, {}, {}]);
        assert!(agent_frame(&serde_json::to_vec(&value).unwrap(), 11000).is_err());
        value["deck"]["panel"]["items"] = json!({});
        assert_eq!(
            agent_frame(&serde_json::to_vec(&value).unwrap(), 11000).unwrap()["deck"]["panel"]["items"],
            json!([])
        );
        value["deck"]["panel"]["items"] = json!({"unexpected":"not an array"});
        assert!(agent_frame(&serde_json::to_vec(&value).unwrap(), 11000).is_err());
    }
}
