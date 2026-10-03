use canvas_feeds::{
    self, ReqwestHttp,
    reddit_trends::{self, Config},
    x_trends,
};
use std::{
    env, fs,
    process::ExitCode,
    time::{Duration, Instant},
};
fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(path) = value(&args, "--config") else {
        eprintln!("usage: canvas-reddit-trends --config PATH [--once]");
        return ExitCode::from(2);
    };
    let config: Config = match fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("canvas-reddit-trends: {e}");
            return ExitCode::from(2);
        }
    };
    if let Err(e) = config.validate() {
        eprintln!("canvas-reddit-trends: {e}");
        return ExitCode::from(2);
    }
    let http = ReqwestHttp::default();
    let once = args.iter().any(|v| v == "--once");
    let mut sequence = canvas_feeds::now_ms();
    let mut previous = Vec::new();
    loop {
        let started = Instant::now();
        let now = canvas_feeds::now_ms();
        let payload = reddit_trends::refresh_feed(&config, &http, now, &previous);
        previous = payload.items.clone();
        sequence = sequence.wrapping_add(1);
        let bridge = x_trends::Config {
            schema_version: 1,
            owner: config.owner.clone(),
            stream: config.stream.clone(),
            refresh_seconds: config.refresh_seconds,
            max_items: config.max_items,
            recency_half_life_minutes: 90,
            weights: Default::default(),
            saved: vec![],
        };
        let mut event = x_trends::envelope(&bridge, &payload, sequence, now);
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
                "canvas-reddit-trends: {:?}, {} items, {}ms",
                payload.status.health,
                previous.len(),
                started.elapsed().as_millis()
            ),
            Err(e) => eprintln!("canvas-reddit-trends: {e}"),
        }
        if once {
            return ExitCode::SUCCESS;
        }
        let interval = Duration::from_secs(config.refresh_seconds);
        if started.elapsed() < interval {
            std::thread::sleep(interval - started.elapsed())
        }
    }
}
fn value(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|v| v == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}
