//! Owner-local reads -> existing Unibus -> bounded Canvas binder input.
use canvas_widget_host::first_party::{self, Cursor, Owner, Projection, KIND, MAX_BYTES};
use reqwest::{blocking::Client as Http, redirect::Policy, Url};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeSet,
    error::Error,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
use unibus::{
    client::{Client, ClientConfig},
    Envelope, Message,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    owner: Owner,
    source: String,
    scope: String,
    /// Existing authorized owner origin; credentials are a separate private file.
    base_url: String,
    bearer_file: Option<PathBuf>,
    #[serde(default)]
    session_ids: BTreeSet<String>,
    #[serde(default)]
    account_ids: BTreeSet<String>,
    bus: Option<ClientConfig>,
    /// The exact enrolled producer a recipient accepts, not a discovery wildcard.
    source_producer: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CodingInput {
    context: Option<Projection>,
    sessions: Option<Projection>,
}

fn bounded(path: &Path, limit: usize) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err("file exceeds input budget".into());
    }
    Ok(bytes)
}

fn endpoint(config: &Config) -> Result<Url, Box<dyn Error>> {
    let mut url = Url::parse(&config.base_url)?;
    let loopback = url.host_str().is_some_and(|host| {
        host == "localhost"
            || host
                .trim_matches(['[', ']'])
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if !(url.scheme() == "https" || (url.scheme() == "http" && loopback))
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err("owner must be an HTTPS or loopback HTTP origin without credentials".into());
    }
    url.set_path(&match config.owner {
        Owner::OhmBridge => "/api/snapshot".into(),
        Owner::Ohmos => "/inbox".into(),
        Owner::Edgerunner => "/v1/owned-sessions".into(),
        Owner::Alfalfa => format!("/api/v1/projects/{}/view", config.scope),
    });
    Ok(url)
}

fn observe(config: &Config, http: &Http) -> Result<Projection, Box<dyn Error>> {
    let mut request = http.get(endpoint(config)?);
    if let Some(path) = &config.bearer_file {
        let bytes = bounded(path, 16384)?;
        let token = std::str::from_utf8(&bytes)?.trim();
        if token.is_empty() {
            return Err("owner bearer file is empty".into());
        }
        request = request.bearer_auth(token);
    }
    // Never print an owner URL, credential, response body, or private error payload.
    let response = request.send().map_err(|_| "owner request failed")?;
    if !response.status().is_success() {
        return Err(format!("owner returned HTTP {}", response.status().as_u16()).into());
    }
    let mut bytes = Vec::new();
    response.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 4 * 1024 * 1024 {
        return Err("owner response exceeds 4MiB".into());
    }
    let input = serde_json::from_slice(&bytes).map_err(|_| "owner returned invalid JSON")?;
    Ok(first_party::project(
        config.owner,
        &config.source,
        &config.scope,
        &input,
        unibus::envelope::now_ms(),
        if config.owner == Owner::OhmBridge {
            &config.account_ids
        } else {
            &config.session_ids
        },
    )?)
}

fn messaging_reply(
    config: &Config,
    bus: &ClientConfig,
    request_id: &str,
    envelope: &Envelope,
    payload: &[u8],
    now: u64,
) -> Result<Projection, Box<dyn Error>> {
    envelope.validate(&bus.installation_id, now)?;
    if envelope.message.kind != "messaging.reply.v1"
        || envelope.producer_id
            != config
                .source_producer
                .as_deref()
                .ok_or("source producer required")?
        || envelope.destination.as_deref() != Some(bus.producer_id.as_str())
        || !payload.is_empty()
        || serde_json::to_vec(&envelope.message.body)?.len() > 4 * 1024 * 1024
    {
        return Err("unexpected messaging reply".into());
    }
    let body = &envelope.message.body;
    if body.get("request_id").and_then(|id| id.as_str()) != Some(request_id) {
        return Err("messaging request mismatch".into());
    }
    if body.get("success").and_then(|value| value.as_bool()) != Some(true) {
        return Err("messaging owner rejected read".into());
    }
    Ok(first_party::project(
        Owner::OhmBridge,
        &config.source,
        &config.scope,
        body.get("result").ok_or("missing messaging result")?,
        now,
        &config.account_ids,
    )?)
}

fn admit(
    config: &Config,
    bus: &ClientConfig,
    cursor: &mut Cursor,
    envelope: &Envelope,
    payload: &[u8],
    now: u64,
) -> Result<Projection, Box<dyn Error>> {
    envelope.validate(&bus.installation_id, now)?;
    if envelope.message.kind != KIND
        || envelope.producer_id
            != config
                .source_producer
                .as_deref()
                .ok_or("source producer required")?
        || !payload.is_empty()
        || serde_json::to_vec(&envelope.message.body)?.len() > MAX_BYTES + 1024
    {
        return Err("unexpected source or oversized payload".into());
    }
    let body = &envelope.message.body;
    let projection: Projection =
        serde_json::from_value(body.get("projection").ok_or("missing projection")?.clone())?;
    if projection.owner != config.owner
        || projection.source != config.source
        || projection.scope != config.scope
    {
        return Err("owner/source/scope mismatch".into());
    }
    let sequence = body
        .get("sequence")
        .and_then(|value| value.as_u64())
        .ok_or("missing sequence")?;
    cursor.accept(&projection, sequence, now)?;
    Ok(projection)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "coding" {
        let mut bytes = Vec::new();
        std::io::stdin()
            .take((2 * MAX_BYTES + 2048) as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 2 * MAX_BYTES + 2048 {
            return Err("coding input exceeds bound".into());
        }
        let input: CodingInput = serde_json::from_slice(&bytes)?;
        let view = first_party::coding(
            &args[1],
            input.context.as_ref(),
            input.sessions.as_ref(),
            unibus::envelope::now_ms(),
        )?;
        println!("{}", serde_json::to_string(&view)?);
        return Ok(());
    }
    if args.len() < 2
        || args.len() > 3
        || !matches!(
            args[0].as_str(),
            "project" | "observe" | "publish" | "receive" | "messaging"
        )
    {
        return Err(
            "usage: canvas-first-party project|observe CONFIG | publish|receive|messaging CONFIG CYCLES(1..120) | coding PROJECT < OBSERVATIONS.json"
                .into(),
        );
    }
    let path = Path::new(&args[1]);
    let mut config: Config = serde_json::from_slice(&bounded(path, 16384)?)?;
    if !first_party::identifier(&config.source) || !first_party::identifier(&config.scope) {
        return Err("invalid source/scope".into());
    }
    let root = path.parent().unwrap_or(Path::new("."));
    if let Some(token) = &mut config.bearer_file {
        if token.is_relative() {
            *token = root.join(&*token);
        }
    }
    let http = Http::builder()
        .timeout(Duration::from_secs(6))
        .redirect(Policy::none())
        .build()?;
    if args[0] == "project" {
        if args.len() != 3 {
            return Err("project needs CONFIG OWNER_READ_AT_MS; cached input must not acquire a new read time".into());
        }
        let owner_read_at = args[2].parse::<u64>()?;
        let mut bytes = Vec::new();
        std::io::stdin()
            .take(4 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 4 * 1024 * 1024 {
            return Err("owner input exceeds bound".into());
        }
        let input = serde_json::from_slice(&bytes)?;
        let projection = first_party::project(
            config.owner,
            &config.source,
            &config.scope,
            &input,
            owner_read_at,
            if config.owner == Owner::OhmBridge {
                &config.account_ids
            } else {
                &config.session_ids
            },
        )?;
        println!("{}", serde_json::to_string(&projection)?);
        return Ok(());
    }
    if args[0] == "observe" {
        if args.len() != 2 {
            return Err("observe accepts one config".into());
        }
        println!("{}", serde_json::to_string(&observe(&config, &http)?)?);
        return Ok(());
    }
    let cycles = args
        .get(2)
        .ok_or("finite cycle count required")?
        .parse::<usize>()?;
    if !(1..=120).contains(&cycles) {
        return Err("cycles must be 1..120".into());
    }
    if args[0] == "publish" {
        if config.owner == Owner::OhmBridge {
            return Err("use messaging request/reply for the existing ohm bridge owner".into());
        }
        endpoint(&config)?;
    }
    let mut bus = config.bus.clone().ok_or("Unibus config required")?;
    bus.resolve_token_file(path);
    let address: std::net::SocketAddr = bus.hub_endpoint.parse()?;
    if !address.ip().is_loopback() || bus.schema_version != 2 {
        return Err("use authenticated v2 Unibus through a loopback tunnel".into());
    }
    bus.read_timeout_ms = 500;
    let client = Client::connect(bus.clone())?;
    if args[0] == "messaging" {
        if config.owner != Owner::OhmBridge || config.account_ids.is_empty() {
            return Err("messaging needs ohm_bridge and explicit account IDs".into());
        }
        let producer = config
            .source_producer
            .as_deref()
            .ok_or("exact messaging producer required")?;
        for cycle in 0..cycles {
            let request = client.envelope(
                Some(producer),
                Message::new("messaging.request.v1", json!({"op":"snapshot"})),
            )?;
            client.send(&request, &[])?;
            let deadline = std::time::Instant::now() + Duration::from_secs(6);
            let mut projection = None;
            while std::time::Instant::now() < deadline {
                let Some((envelope, payload)) = client.try_read()? else {
                    continue;
                };
                if envelope.message.kind != "messaging.reply.v1" {
                    continue;
                }
                if envelope
                    .message
                    .body
                    .get("request_id")
                    .and_then(|id| id.as_str())
                    != Some(request.request_id.as_str())
                {
                    continue;
                }
                projection = Some(messaging_reply(
                    &config,
                    &bus,
                    &request.request_id,
                    &envelope,
                    &payload,
                    unibus::envelope::now_ms(),
                )?);
                break;
            }
            let projection = projection.ok_or("messaging owner read deadline exceeded")?;
            println!("{}", serde_json::to_string(&projection)?);
            std::io::stdout().flush()?;
            if cycle + 1 < cycles {
                std::thread::sleep(Duration::from_secs(5));
            }
        }
        return Ok(());
    }
    if args[0] == "publish" {
        for sequence in 1..=cycles {
            let projection = match observe(&config, &http) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("{} owner read failed: {error}", config.source);
                    Projection::disconnected(
                        config.owner,
                        &config.source,
                        &config.scope,
                        unibus::envelope::now_ms(),
                    )
                }
            };
            projection.validate()?;
            let envelope = client.envelope(
                Some("first-party"),
                Message::new(KIND, json!({"sequence":sequence,"projection":projection})),
            )?;
            client.send(&envelope, &[])?;
            if sequence < cycles {
                std::thread::sleep(Duration::from_secs(5));
            }
        }
    } else {
        config
            .source_producer
            .as_deref()
            .filter(|id| !id.is_empty())
            .ok_or("exact source_producer required")?;
        let mut cursor = Cursor::default();
        let deadline = std::time::Instant::now() + Duration::from_secs(cycles as u64 * 5);
        let mut accepted = 0;
        while std::time::Instant::now() < deadline && accepted < cycles {
            let Some((envelope, payload)) = client.try_read()? else {
                continue;
            };
            if envelope.message.kind != KIND {
                continue;
            }
            let projection = match admit(
                &config,
                &bus,
                &mut cursor,
                &envelope,
                &payload,
                unibus::envelope::now_ms(),
            ) {
                Ok(value) => value,
                Err(error) => {
                    eprintln!("rejected observation: {error}");
                    continue;
                }
            };
            println!("{}", serde_json::to_string(&projection)?);
            std::io::stdout().flush()?;
            accepted += 1;
        }
        if accepted == 0 {
            return Err("no matching first-party observation received before deadline".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> Config {
        serde_json::from_value(json!({"owner":"alfalfa","source":"projects","scope":"alfalfa","base_url":"http://127.0.0.1:8791"})).unwrap()
    }
    #[test]
    fn fetches_only_owner_reads_and_never_redirects_credentials_to_a_path_from_data() {
        assert_eq!(
            endpoint(&config()).unwrap().path(),
            "/api/v1/projects/alfalfa/view"
        );
        for origin in [
            "http://example.com",
            "https://user:secret@example.com",
            "https://example.com/admin",
            "https://example.com?token=x",
        ] {
            let mut c = config();
            c.base_url = origin.into();
            assert!(endpoint(&c).is_err());
        }
    }

    #[test]
    fn wrong_source_scope_and_binary_data_cannot_advance_recipient_cursor() {
        let mut config = config();
        config.source_producer = Some("source".into());
        let bus:ClientConfig=serde_json::from_value(json!({"installation_id":"test","producer_id":"viewer","hub_endpoint":"127.0.0.1:1","token_file":"unused","zones":["private"],"schema_version":2})).unwrap();
        let now = unibus::envelope::now_ms();
        let projection = Projection::disconnected(Owner::Alfalfa, "projects", "alfalfa", now);
        let envelope = Envelope::new(
            "test",
            "source",
            None,
            "TEST_ONLY",
            Message::new(KIND, json!({"sequence":1,"projection":projection})),
        );
        let mut cursor = Cursor::default();
        let mut wrong = envelope.clone();
        wrong.producer_id = "other".into();
        assert!(admit(&config, &bus, &mut cursor, &wrong, &[], now).is_err());
        wrong = envelope.clone();
        wrong.message.body["projection"]["scope"] = json!("other");
        assert!(admit(&config, &bus, &mut cursor, &wrong, &[], now).is_err());
        assert!(admit(&config, &bus, &mut cursor, &envelope, &[1], now).is_err());
        assert!(admit(&config, &bus, &mut cursor, &envelope, &[], now).is_ok());
        assert!(admit(&config, &bus, &mut cursor, &envelope, &[], now).is_err());
    }

    #[test]
    fn loopback_owner_read_uses_the_documented_path_and_normalizes_its_response() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            let mut request = [0; 4096];
            let size = stream.read(&mut request).unwrap();
            assert!(std::str::from_utf8(&request[..size])
                .unwrap()
                .starts_with("GET /api/v1/projects/alfalfa/view HTTP/1.1"));
            let body=json!({"schema":"alfalfa.manifest-view/1","project_id":"alfalfa","name":"Alfalfa","status":"active","purpose":"Shared context","secret":"DO_NOT_FORWARD"}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        });
        let mut config = config();
        config.base_url = format!("http://{address}");
        let http = Http::builder()
            .timeout(Duration::from_secs(10))
            .no_proxy()
            .build()
            .unwrap();
        let projection = observe(&config, &http).unwrap();
        assert_eq!(projection.rows[0].title, "Alfalfa");
        assert!(!serde_json::to_string(&projection)
            .unwrap()
            .contains("DO_NOT_FORWARD"));
        server.join().unwrap();
    }

    #[test]
    fn existing_messaging_reply_requires_exact_correlation_provider_and_destination() {
        let mut config = config();
        config.owner = Owner::OhmBridge;
        config.account_ids = BTreeSet::from(["iroh:sms".into()]);
        config.source_producer = Some("messages-studio".into());
        let bus:ClientConfig=serde_json::from_value(json!({"installation_id":"test","producer_id":"canvas-reader","hub_endpoint":"127.0.0.1:1","token_file":"unused","zones":["private"],"schema_version":2})).unwrap();
        let now = unibus::envelope::now_ms();
        let reply = Envelope::new(
            "test",
            "messages-studio",
            Some("canvas-reader"),
            "TEST_ONLY",
            Message::new(
                "messaging.reply.v1",
                json!({"request_id":"read-1","success":true,"result":{"accounts":[{"id":"iroh:sms","transport":"SMS","demo":false}],"conversations":[],"errors":[]}}),
            ),
        );
        assert_eq!(
            messaging_reply(&config, &bus, "read-1", &reply, &[], now)
                .unwrap()
                .state,
            "empty"
        );
        assert!(messaging_reply(&config, &bus, "other-read", &reply, &[], now).is_err());
        let mut wrong = reply.clone();
        wrong.destination = Some("other-reader".into());
        assert!(messaging_reply(&config, &bus, "read-1", &wrong, &[], now).is_err());
        wrong = reply.clone();
        wrong.producer_id = "other-provider".into();
        assert!(messaging_reply(&config, &bus, "read-1", &wrong, &[], now).is_err());
        wrong = reply.clone();
        wrong.message.body["success"] = json!(false);
        assert!(messaging_reply(&config, &bus, "read-1", &wrong, &[], now).is_err());
    }
}
