//! Polls Prometheus through the desktop bridge and emits Canvas widget snapshots.
//!
//! This process holds no credential. It asks the bridge for a named read; the
//! bridge attaches the owner's bearer and returns bounded JSON. When the session
//! has expired the bridge says so distinctly, and this re-authenticates by
//! running `fpl login`, which opens a browser where one exists.
use canvas_metrics::{Mapping, Unavailable, classify, envelope, rfc3339, stream_identity};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    os::unix::net::UnixDatagram,
    path::PathBuf,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

/// Re-authentication opens a browser. Doing that per failing widget on every
/// poll would be a popup storm, so one attempt covers all sources for a while.
const REAUTH_COOLDOWN: Duration = Duration::from_secs(300);

fn now() -> (u64, u64) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (now.as_secs(), now.as_millis() as u64)
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Runs one brokered read. A non-zero exit or unparsable output is reported as
/// an unreachable source rather than being allowed to kill the loop.
fn read(bridge: &OsString, action: &str, query: &str, range: Option<(u64, u64)>) -> Option<Value> {
    let mut command = Command::new(bridge);
    command.arg("metric-read").arg(action).arg(query);
    if let Some((window, step)) = range {
        command
            .arg("--window")
            .arg(window.to_string())
            .arg("--step")
            .arg(step.to_string());
    }
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    serde_json::from_slice(&output.stdout).ok()
}

/// Opens the browser sign-in. `fpl login` is idempotent on an enrolled machine
/// and detects headless hosts itself, so this is the same call a person makes.
fn reauthenticate() {
    match Command::new("fpl").arg("login").arg("--json").spawn() {
        Ok(_) => eprintln!("canvas-metrics: FPL session expired; started re-authentication"),
        Err(error) => eprintln!("canvas-metrics: cannot run `fpl login`: {error}"),
    }
}

fn window_label(spec: &canvas_metrics::SourceSpec) -> String {
    match spec.window_seconds {
        None => "now".into(),
        Some(seconds) if seconds % 3600 == 0 => format!("{}h", seconds / 3600),
        Some(seconds) => format!("{}m", seconds / 60),
    }
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let once = args.iter().any(|a| a == "--once");
    let config = arg(&args, "--config").unwrap_or_else(|| "config/metrics.json".into());
    let bridge: OsString = arg(&args, "--bridge")
        .map(OsString::from)
        .or_else(|| std::env::var_os("FPL_DESKTOP_BRIDGE_BIN"))
        .unwrap_or_else(|| "fpl-desktop-bridge".into());
    let socket: Option<PathBuf> = arg(&args, "--socket").map(PathBuf::from);

    let raw = std::fs::read_to_string(&config)
        .map_err(|error| format!("cannot read {config}: {error}"))?;
    let mapping: Mapping =
        serde_json::from_str(&raw).map_err(|error| format!("cannot parse {config}: {error}"))?;
    if mapping.schema_version != 1 {
        return Err(format!(
            "unsupported metrics mapping schema {}",
            mapping.schema_version
        ));
    }
    // Fail before the first poll rather than emitting to nothing for every id.
    for id in mapping.sources.keys() {
        stream_identity(id)?;
    }

    let datagram = UnixDatagram::unbound().map_err(|error| error.to_string())?;
    let mut sequence: BTreeMap<String, u64> = BTreeMap::new();
    let mut last_reauth: Option<SystemTime> = None;

    loop {
        let mut unauthorized = false;
        for (id, spec) in &mapping.sources {
            let result = if spec.unavailable.is_some() {
                None
            } else {
                let (action, range) = match spec.range() {
                    Some(range) => ("query_range", Some(range)),
                    None => ("query", None),
                };
                read(&bridge, action, &spec.query, range)
            };
            if result
                .as_ref()
                .and_then(classify)
                .is_some_and(|reason| reason == Unavailable::Unauthorized)
            {
                unauthorized = true;
            }
            let (seconds, millis) = now();
            let next = sequence.entry(id.clone()).or_default();
            *next += 1;
            let event = envelope(
                id,
                spec,
                result.as_ref(),
                *next,
                millis,
                &rfc3339(seconds),
                &window_label(spec),
            )?;
            let bytes = serde_json::to_vec(&event).map_err(|error| error.to_string())?;
            if bytes.len() > canvas_metrics::MAX_DATAGRAM {
                eprintln!("canvas-metrics: {id} snapshot too large; skipped");
                continue;
            }
            match &socket {
                Some(path) => {
                    if let Err(error) = datagram.send_to(&bytes, path) {
                        // Canvas may not be running. That is not fatal: keep
                        // polling so widgets populate as soon as it comes back.
                        eprintln!("canvas-metrics: cannot reach Canvas ({error})");
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
        std::thread::sleep(Duration::from_secs(mapping.poll_seconds.clamp(5, 3600)));
    }
}
