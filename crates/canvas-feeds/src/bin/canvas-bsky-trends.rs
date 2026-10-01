//! Poll Bluesky's public AppView into Canvas's social trend contract.
use canvas_feeds::{
    self,
    bsky_trends::{BlueskyApi, DEFAULT_APPVIEW},
    x_trends::{self, Config},
};
use std::{
    env, fs,
    process::ExitCode,
    time::{Duration, Instant},
};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(path) = value(&args, "--config") else {
        eprintln!("usage: canvas-bsky-trends --config PATH [--once]");
        return ExitCode::from(2);
    };
    let config: Config = match fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("canvas-bsky-trends: cannot load {path}: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = config.validate() {
        eprintln!("canvas-bsky-trends: invalid config: {e}");
        return ExitCode::from(2);
    }
    let appview = env::var("BSKY_APPVIEW_URL").unwrap_or_else(|_| DEFAULT_APPVIEW.into());
    let api = match BlueskyApi::new(&appview) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("canvas-bsky-trends: {e}");
            return ExitCode::from(2);
        }
    };
    let once = args.iter().any(|v| v == "--once");
    let mut sequence = canvas_feeds::now_ms();
    let mut previous = Vec::new();
    let mut last_success_at_ms = None;
    loop {
        let started = Instant::now();
        let now = canvas_feeds::now_ms();
        let mut payload =
            x_trends::refresh_named(&config, &api, now, &previous, "Bluesky / War Room");
        if payload.status.successful_queries > 0 {
            last_success_at_ms = Some(now);
        } else {
            payload.status.last_success_at_ms = last_success_at_ms;
        }
        previous = payload.items.clone();
        sequence = sequence.wrapping_add(1);
        let mut event = x_trends::envelope(&config, &payload, sequence, now);
        while serde_json::to_vec(&event).map_or(usize::MAX, |v| v.len())
            > canvas_feeds::MAX_DATAGRAM - canvas_feeds::ENVELOPE_OVERHEAD
        {
            let Some(items) = event
                .pointer_mut("/payload/items")
                .and_then(|v| v.as_array_mut())
            else {
                break;
            };
            if items.pop().is_none() {
                break;
            }
        }
        match canvas_feeds::send_raw(&event) {
            Ok(()) => eprintln!(
                "canvas-bsky-trends: {:?}, {} items, {}ms",
                payload.status.health,
                previous.len(),
                started.elapsed().as_millis()
            ),
            Err(e) => eprintln!("canvas-bsky-trends: {e}"),
        }
        if once {
            return ExitCode::SUCCESS;
        }
        let interval = Duration::from_secs(config.refresh_seconds);
        if started.elapsed() < interval {
            std::thread::sleep(interval - started.elapsed());
        }
    }
}
fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|v| v == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
