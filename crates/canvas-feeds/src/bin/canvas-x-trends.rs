//! Poll X's official recent-search API and publish a ranked Canvas war-room feed.
use canvas_feeds::x_trends::{self, ApiResponse, Config, OfficialXApi, XApi};
use std::{
    env, fs,
    process::ExitCode,
    time::{Duration, Instant},
};

const USAGE: &str = "usage: canvas-x-trends --config PATH [--once]";

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(path) = value(&args, "--config") else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let config: Config = match fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("canvas-x-trends: cannot load {path}: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = config.validate() {
        eprintln!("canvas-x-trends: invalid config: {e}");
        return ExitCode::from(2);
    }
    let api: Box<dyn XApi> = match env::var("X_BEARER_TOKEN") {
        Ok(v) if !v.trim().is_empty() => match OfficialXApi::new(v) {
            Ok(api) => Box::new(api),
            Err(error) => Box::new(UnavailableApi { error }),
        },
        _ => Box::new(UnavailableApi {
            error: "X_BEARER_TOKEN is not configured".into(),
        }),
    };
    let once = args.iter().any(|v| v == "--once");
    let mut sequence = canvas_feeds::now_ms();
    let mut previous = Vec::new();
    let mut last_success_at_ms = None;
    loop {
        let started = Instant::now();
        let now = canvas_feeds::now_ms();
        let mut payload = x_trends::refresh(&config, api.as_ref(), now, &previous);
        if payload.status.successful_queries > 0 {
            last_success_at_ms = Some(now);
        } else {
            payload.status.last_success_at_ms = last_success_at_ms;
        }
        previous = payload.items.clone();
        sequence = sequence.wrapping_add(1);
        let mut event = x_trends::envelope(&config, &payload, sequence, now);
        // AF_UNIX datagrams are deliberately bounded. Drop the lowest-ranked
        // items until this atomic snapshot fits; never split an event.
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
                "canvas-x-trends: {:?}, {} items, {}ms",
                payload.status.health,
                previous.len(),
                started.elapsed().as_millis()
            ),
            Err(e) => eprintln!("canvas-x-trends: {e}"),
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

struct UnavailableApi {
    error: String,
}
impl XApi for UnavailableApi {
    fn recent(&self, _saved: &canvas_feeds::x_trends::SavedFeed) -> Result<ApiResponse, String> {
        Err(self.error.clone())
    }
}

fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|v| v == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
