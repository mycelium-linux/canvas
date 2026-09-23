//! The bridge process: bus in, control planes out.
//!
//! Deliberately thin. Every decision worth arguing about — which session a
//! prompt goes to, what a state means, how a roster is ordered — lives in the
//! library where it is tested without a network. What is left here is transport.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use canvas::bus::Bus;
use canvas::stream_contract::{STREAM_SCHEMA_VERSION, StreamEnvelope};
use canvas_agent_bridge::{AdapterKind, BridgeConfig, Prompt, Session, Target, edgerunner, roster, surf};

const PROMPT_KIND: &str = "canvas.prompt.v1";
const WIDGET_CONTRACT: &str = "canvas.widget.snapshot.v1";
const RECONNECT_MIN: Duration = Duration::from_secs(1);
const RECONNECT_MAX: Duration = Duration::from_secs(30);

/// Control-plane calls are bounded: a hung agent host must not stall prompts to
/// every other target.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("usage: canvas-agent-bridge <bridge-config.json>")?;
    let config = Arc::new(BridgeConfig::read(std::path::Path::new(&path))?);
    let mut backoff = RECONNECT_MIN;
    loop {
        match run(Arc::clone(&config)) {
            Ok(()) => eprintln!("canvas-agent-bridge: hub connection closed; reconnecting"),
            Err(error) => eprintln!("canvas-agent-bridge: {error:#}"),
        }
        thread::sleep(backoff);
        backoff = (backoff * 2).min(RECONNECT_MAX);
    }
}

fn run(config: Arc<BridgeConfig>) -> Result<()> {
    let bus = Arc::new(
        Bus::connect(config.bus.clone())
            .with_context(|| format!("connect hub {}", config.bus.hub_endpoint))?,
    );
    eprintln!(
        "canvas-agent-bridge: registered as '{}' serving targets {:?}",
        config.bus.producer_id,
        config
            .targets
            .iter()
            .map(|target| target.name.as_str())
            .collect::<Vec<_>>()
    );
    let clients = Arc::new(Clients::new(&config)?);
    start_roster_polling(Arc::clone(&config), Arc::clone(&clients));

    loop {
        match bus.try_read() {
            Ok(Some((envelope, _payload))) => {
                if envelope.message.kind != PROMPT_KIND {
                    continue;
                }
                // One bad prompt must not end the loop: a malformed body or an
                // unreachable agent is a reportable event, not a reason to stop
                // serving every other target.
                let body = serde_json::Value::Object(envelope.message.body.clone());
                if let Err(error) = deliver(&config, &clients, &body) {
                    eprintln!("canvas-agent-bridge: prompt rejected: {error:#}");
                }
            }
            Ok(None) => {}
            Err(error) => return Err(error.context("hub read")),
        }
    }
}

/// One HTTP client per target, since each may carry its own identity.
struct Clients {
    by_target: HashMap<String, reqwest::blocking::Client>,
}

impl Clients {
    fn new(config: &BridgeConfig) -> Result<Self> {
        let mut by_target = HashMap::new();
        for target in &config.targets {
            let mut builder = reqwest::blocking::Client::builder().timeout(HTTP_TIMEOUT);
            if let Some(path) = &target.client_identity {
                // mTLS: edge listeners require a client certificate, so a
                // missing or unreadable identity is fatal at startup rather
                // than a puzzling 403 on the first prompt.
                let pem = std::fs::read(path).with_context(|| {
                    format!("read client identity for {}: {path}", target.name)
                })?;
                let identity = reqwest::Identity::from_pem(&pem)
                    .with_context(|| format!("parse client identity for {}", target.name))?;
                builder = builder.identity(identity);
            }
            by_target.insert(
                target.name.clone(),
                builder
                    .build()
                    .with_context(|| format!("build http client for {}", target.name))?,
            );
        }
        Ok(Self { by_target })
    }

    fn get(&self, target: &str) -> Result<&reqwest::blocking::Client> {
        self.by_target
            .get(target)
            .with_context(|| format!("no http client for target {target}"))
    }
}

fn token(target: &Target) -> Option<String> {
    target
        .token_env
        .as_ref()
        .and_then(|name| std::env::var(name).ok())
        .filter(|value| !value.trim().is_empty())
}

fn authorized(
    request: reqwest::blocking::RequestBuilder,
    target: &Target,
) -> reqwest::blocking::RequestBuilder {
    match token(target) {
        Some(token) => request.bearer_auth(token),
        None => request,
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

fn list_sessions(clients: &Clients, target: &Target) -> Result<Vec<Session>> {
    if target.kind == AdapterKind::DesktopBridge {
        return list_via_broker(target);
    }
    let path = match target.kind {
        AdapterKind::Edgerunner => edgerunner::list_path(target.project.as_deref()),
        AdapterKind::Surf => surf::list_path().to_string(),
        AdapterKind::DesktopBridge => unreachable!("handled above"),
    };
    let response = authorized(
        clients
            .get(&target.name)?
            .get(format!("{}{path}", target.http_base()?)),
        target,
    )
    .header("Accept", "application/json")
    .send()
    .with_context(|| format!("list sessions for {}", target.name))?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .with_context(|| format!("unreadable session listing from {}", target.name))?;
    anyhow::ensure!(
        status.is_success(),
        "{} listed sessions with {status}: {body}",
        target.name
    );
    // Tolerant of either a bare array or an envelope around one: the shape is
    // not this bridge's to dictate, and a wrapper appearing must not blank the
    // roster.
    let items = body
        .as_array()
        .or_else(|| body.get("sessions").and_then(serde_json::Value::as_array))
        .or_else(|| body.get("items").and_then(serde_json::Value::as_array))
        .cloned()
        .unwrap_or_default();
    let stamp = now_ms();
    Ok(items
        .iter()
        .filter_map(|item| match target.kind {
            AdapterKind::Edgerunner => edgerunner::parse_session(item, stamp),
            AdapterKind::Surf => surf::parse_session(item, stamp),
            AdapterKind::DesktopBridge => None,
        })
        .collect())
}


/// List sessions by asking the desktop bridge, which attaches the owner's OIDC
/// identity itself.
///
/// A subprocess rather than an HTTP call because that is the interface offered:
/// the bridge prints JSON and keeps the credential. Bounded output so a
/// misbehaving binary cannot grow this process without limit.
fn list_via_broker(target: &Target) -> Result<Vec<Session>> {
    use canvas_agent_bridge::desktop_bridge;

    let output = std::process::Command::new(target.broker_command())
        .args(desktop_bridge::list_args())
        .output()
        .with_context(|| format!("run {} for {}", target.broker_command(), target.name))?;
    anyhow::ensure!(
        output.status.success(),
        "{} owner-session list failed: {}",
        target.broker_command(),
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .last()
            .unwrap_or("no detail")
    );
    let reply: serde_json::Value = serde_json::from_slice(&output.stdout)
        .context("desktop bridge printed something that is not JSON")?;
    desktop_bridge::parse_list(&reply, now_ms())
}

fn deliver(config: &BridgeConfig, clients: &Clients, body: &serde_json::Value) -> Result<()> {
    let prompt = Prompt::parse(body)?;
    let target = config.target(&prompt.target).with_context(|| {
        format!(
            "no target named {:?} is configured; the prompt was not delivered",
            prompt.target
        )
    })?;

    // Refused here, with a reason, rather than further in: the desktop bridge
    // brokers reads and vends no token, so it has no way to send anything.
    anyhow::ensure!(
        target.kind.can_deliver(),
        "target {:?} is read-only through the desktop bridge; it can show sessions but not \
         receive a prompt",
        target.name
    );
    let session = match prompt.session.as_deref() {
        Some(pinned) => pinned.to_string(),
        None => {
            let sessions = list_sessions(clients, target)?;
            edgerunner::choose_session(&sessions, None)?
        }
    };

    let (path, payload) = match target.kind {
        AdapterKind::Edgerunner => (
            edgerunner::message_path(&session),
            edgerunner::message_body(&prompt.text),
        ),
        AdapterKind::Surf => (surf::execute_path(&session), surf::execute_body(&prompt.text)),
        AdapterKind::DesktopBridge => anyhow::bail!("refused above"),
    };

    let response = authorized(
        clients
            .get(&target.name)?
            .post(format!("{}{path}", target.http_base()?)),
        target,
    )
    .json(&payload)
    .send()
    .with_context(|| format!("deliver prompt to {}/{session}", target.name))?;
    let status = response.status();
    anyhow::ensure!(
        status.is_success(),
        "{}/{session} refused the prompt with {status}",
        target.name
    );
    eprintln!(
        "canvas-agent-bridge: delivered to {}/{session}",
        target.name
    );
    Ok(())
}

/// Publish each target's session roster on its own cadence.
///
/// Its own thread because the bus read blocks with a long timeout; polling
/// inside that loop would make the roster as slow as the quietest hour.
fn start_roster_polling(config: Arc<BridgeConfig>, clients: Arc<Clients>) {
    let interval = Duration::from_millis(config.roster_interval_ms.max(500));
    let sequences: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));
    thread::Builder::new()
        .name("agent-roster".into())
        .spawn(move || {
            loop {
                for target in &config.targets {
                    let sessions = match list_sessions(&clients, target) {
                        Ok(sessions) => sessions,
                        Err(error) => {
                            eprintln!("canvas-agent-bridge: roster for {}: {error:#}", target.name);
                            continue;
                        }
                    };
                    let stamp = now_ms();
                    let widget_id = format!("{}-sessions", target.name);
                    let spec = roster::widget(&widget_id, &target.name, &sessions, stamp);
                    let sequence = {
                        let mut held = sequences.lock().expect("roster sequences");
                        let counter = held.entry(target.name.clone()).or_insert(0);
                        *counter += 1;
                        *counter
                    };
                    let envelope = StreamEnvelope {
                        schema_version: STREAM_SCHEMA_VERSION,
                        owner: config.bus.producer_id.clone(),
                        stream: widget_id.clone(),
                        contract: WIDGET_CONTRACT.into(),
                        sequence,
                        emitted_at_ms: stamp,
                        project_id: target.project.clone(),
                        payload: serde_json::json!({
                            "widget": serde_json::to_value(&spec).unwrap_or_default(),
                        }),
                    };
                    if let Err(error) = publish_local(&envelope) {
                        eprintln!("canvas-agent-bridge: roster publish failed: {error:#}");
                    }
                }
                thread::sleep(interval);
            }
        })
        .expect("spawn roster poller");
}

/// Hand a widget snapshot to Canvas over its local stream socket.
///
/// The roster is for the screen in front of you, so it goes straight to the
/// local Canvas rather than out over the bus and back: fewer hops, and a
/// display that keeps its roster when the hub is unreachable.
fn publish_local(envelope: &StreamEnvelope) -> Result<()> {
    use std::io::Write;
    use std::os::unix::net::UnixDatagram;

    let path = canvas_core::stream_socket_path().context("Canvas stream socket is unavailable")?;
    let bytes = serde_json::to_vec(envelope).context("serialize roster snapshot")?;
    let socket = UnixDatagram::unbound().context("open datagram socket")?;
    socket
        .send_to(&bytes, &path)
        .with_context(|| format!("send roster to {}", path.display()))?;
    std::io::stderr().flush().ok();
    Ok(())
}
