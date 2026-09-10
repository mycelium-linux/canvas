//! Emit the demo swarm onto Canvas's stream socket.
//!
//! Uses `canvas.widget.snapshot.v1`, the same contract the Prometheus connector
//! uses, so Canvas needs no change to render this: the binder replaces the
//! widget kind and its data, and a roster is an ordinary node summary.
//!
//! Run it against a Canvas that has a widget bound to the matching stream:
//!
//! ```text
//! canvas-agents --socket ~/Library/Application\ Support/Dock/canvas/stream.sock
//! ```

use canvas_agents::{Swarm, TICK_MS};
use serde::Serialize;
use std::{
    os::unix::net::UnixDatagram,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CONTRACT: &str = "canvas.widget.snapshot.v1";
const STREAM_SCHEMA_VERSION: u32 = 1;

/// macOS caps an AF_UNIX datagram at `net.local.dgram.maxdgram`, 2048 by
/// default, and an oversized send fails outright rather than truncating. Same
/// number canvas-metrics fits against; duplicated rather than shared because the
/// connector crates deliberately depend on nothing but serde. Worth converging
/// on one definition when a third connector appears.
const MAX_DATAGRAM: usize = 2048;

/// What the envelope adds around the widget: identity, contract, sequence and
/// timestamp. Measured against the widest ids this emitter uses.
const ENVELOPE_OVERHEAD: usize = 420;

#[derive(Serialize)]
struct Envelope<'a> {
    schema_version: u32,
    owner: &'a str,
    stream: &'a str,
    contract: &'a str,
    sequence: u64,
    emitted_at_ms: u64,
    payload: Payload,
}

#[derive(Serialize)]
struct Payload {
    widget: canvas_widgets::WidgetSpec,
}

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|at| args.get(at + 1))
        .cloned()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or_default()
}

/// Render the roster at a size that fits the datagram.
///
/// Shrinking CAPACITY rather than truncating the rendered list is what keeps the
/// title honest: `not_shown` is derived from capacity, so a roster that had to
/// drop rows to fit still reports the full fleet count. Truncating afterwards
/// would silently make "6 of 30" describe four rows.
fn fitted(
    swarm: &mut Swarm,
    id: &str,
    tick: u64,
    capacity: usize,
) -> Option<(canvas_widgets::WidgetSpec, usize)> {
    for size in (1..=capacity).rev() {
        swarm.resize(size, tick);
        let widget = swarm.widget(id, tick);
        let Ok(bytes) = serde_json::to_vec(&widget) else {
            continue;
        };
        if bytes.len() + ENVELOPE_OVERHEAD <= MAX_DATAGRAM {
            return Some((widget, size));
        }
    }
    None
}

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    let socket: PathBuf = arg(&args, "--socket")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| {
                PathBuf::from(home)
                    .join("Library/Application Support/Dock/canvas")
                    .join("stream.sock")
            })
        })
        .ok_or_else(|| "pass --socket, or set HOME".to_string())?;
    let owner = arg(&args, "--owner").unwrap_or_else(|| "agents".into());
    let stream = arg(&args, "--stream").unwrap_or_else(|| "roster".into());
    let id = arg(&args, "--widget").unwrap_or_else(|| "agents/roster".into());
    let capacity: usize = arg(&args, "--capacity")
        .and_then(|value| value.parse().ok())
        .unwrap_or(8);
    let ticks: Option<u64> = arg(&args, "--ticks").and_then(|value| value.parse().ok());
    if let Some(project) = arg(&args, "--focus") {
        eprintln!("canvas-agents: focusing {project}");
    }

    if !socket.exists() {
        // Fail loud: emitting into a socket nobody is listening on looks
        // identical to a working demo from this side.
        return Err(format!(
            "no Canvas stream socket at {} — start Canvas first",
            socket.display()
        ));
    }

    let datagram = UnixDatagram::unbound().map_err(|error| error.to_string())?;
    let mut swarm = Swarm::new(capacity);
    swarm.focus_on(arg(&args, "--focus"));

    eprintln!(
        "canvas-agents: DEMO data as {owner}/{stream} -> {}",
        socket.display()
    );
    let mut sequence = 0_u64;
    let mut tick = 0_u64;
    loop {
        if ticks.is_some_and(|limit| tick >= limit) {
            return Ok(());
        }
        let Some((widget, fitted_capacity)) = fitted(&mut swarm, &id, tick, capacity) else {
            return Err("a single agent row does not fit in one datagram".into());
        };
        if fitted_capacity < capacity {
            eprintln!(
                "canvas-agents: tick {tick} fitted {fitted_capacity} of {capacity} rows into the datagram"
            );
        }
        sequence += 1;
        let event = Envelope {
            schema_version: STREAM_SCHEMA_VERSION,
            owner: &owner,
            stream: &stream,
            contract: CONTRACT,
            sequence,
            emitted_at_ms: now_ms(),
            payload: Payload { widget },
        };
        let bytes = serde_json::to_vec(&event).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_DATAGRAM {
            return Err(format!(
                "envelope is {} bytes, over the {MAX_DATAGRAM} datagram cap",
                bytes.len()
            ));
        }
        if let Err(error) = datagram.send_to(&bytes, &socket) {
            eprintln!("canvas-agents: send failed: {error}");
        }
        tick += 1;
        std::thread::sleep(Duration::from_millis(TICK_MS));
    }
}
