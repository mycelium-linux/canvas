//! Subscribes to SSE topics and emits Canvas widget snapshots.
//!
//! The same loop as `canvas-metrics`, driven by a subscription instead of a
//! poll. This process holds no credential: the transport names a topic, attaches
//! the owner's bearer and hands frames up; nothing here sees one.
//!
//! No transport is wired yet. Until one is, [`Unsubscribed`] stands in and every
//! widget reports `disconnected` with a reason — which is the point of running
//! this today, because the alternative is three widgets on Loading forever with
//! nothing saying why. `notes/live-subscription.md` records what a real
//! subscription needs.
use canvas_sse::{
    Delivery, Mapping, Subscription, Unavailable, Unsubscribed, Window, envelope, observe,
    stream_identity,
};
use std::{
    collections::BTreeMap,
    os::unix::net::UnixDatagram,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Re-authentication opens a browser. Doing that per failing source on every
/// tick would be a popup storm, so one attempt covers all sources for a while.
const REAUTH_COOLDOWN: Duration = Duration::from_secs(300);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Opens the browser sign-in. `fpl login` is idempotent on an enrolled machine
/// and detects headless hosts itself, so this is the same call a person makes.
fn reauthenticate() {
    match std::process::Command::new("fpl")
        .arg("login")
        .arg("--json")
        .spawn()
    {
        Ok(_) => eprintln!("canvas-sse: FPL session expired; started re-authentication"),
        Err(error) => eprintln!("canvas-sse: cannot run `fpl login`: {error}"),
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let once = args.iter().any(|a| a == "--once");
    let config = arg(&args, "--config").unwrap_or_else(|| "config/sse.json".into());
    let socket: Option<PathBuf> = arg(&args, "--socket").map(PathBuf::from);

    let raw = std::fs::read_to_string(&config)
        .map_err(|error| format!("cannot read {config}: {error}"))?;
    let mapping: Mapping =
        serde_json::from_str(&raw).map_err(|error| format!("cannot parse {config}: {error}"))?;
    if mapping.schema_version != 1 {
        return Err(format!(
            "unsupported sse mapping schema {}",
            mapping.schema_version
        ));
    }
    // Fail before the first tick rather than emitting to nothing, or emitting a
    // widget with a column nobody can answer, once per source per tick forever.
    for (id, spec) in &mapping.sources {
        stream_identity(id)?;
        spec.validate(id)?;
    }

    let datagram = UnixDatagram::unbound().map_err(|error| error.to_string())?;
    let mut transport = Unsubscribed;
    eprintln!(
        "canvas-sse: no SSE transport is compiled in; {} source(s) will report disconnected",
        mapping.sources.len()
    );

    let mut windows: BTreeMap<String, Window> = BTreeMap::new();
    let mut sequence: BTreeMap<String, u64> = BTreeMap::new();
    let mut last_reauth: Option<SystemTime> = None;

    loop {
        let now = now_ms();
        let mut unauthorized = false;
        for (id, spec) in &mapping.sources {
            let window = windows
                .entry(id.clone())
                .or_insert_with(|| Window::new(spec.retention()));
            let interruption = if spec.unavailable.is_some() {
                // The spec's own reason is more specific than anything the
                // transport could say about a topic that does not exist.
                None
            } else {
                match transport.drain(&spec.topic) {
                    Delivery::Frames(frames) => {
                        for frame in &frames {
                            if let Some(observation) = observe(spec, frame, now) {
                                window.push(observation);
                            }
                        }
                        None
                    }
                    Delivery::Interrupted(reason) => {
                        unauthorized |= reason == Unavailable::Unauthorized;
                        Some(reason)
                    }
                }
            };
            let next = sequence.entry(id.clone()).or_default();
            *next += 1;
            let event = envelope(id, spec, window, interruption, *next, now)?;
            let bytes = serde_json::to_vec(&event).map_err(|error| error.to_string())?;
            if bytes.len() > canvas_sse::MAX_DATAGRAM {
                eprintln!("canvas-sse: {id} snapshot too large; skipped");
                continue;
            }
            match &socket {
                Some(path) => {
                    if let Err(error) = datagram.send_to(&bytes, path) {
                        // Canvas may not be running. That is not fatal: keep
                        // ticking so widgets populate as soon as it comes back.
                        eprintln!("canvas-sse: cannot reach Canvas ({error})");
                    }
                }
                None => println!("{event}"),
            }
        }
        if unauthorized
            && last_reauth.is_none_or(|at| at.elapsed().unwrap_or_default() >= REAUTH_COOLDOWN)
        {
            last_reauth = Some(SystemTime::now());
            reauthenticate();
        }
        if once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(mapping.tick_seconds.clamp(1, 3600)));
    }
}
