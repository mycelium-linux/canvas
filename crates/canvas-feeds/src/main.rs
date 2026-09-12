//! `canvas-feeds` — surface public endpoints (HN, Reddit, RSS) as Canvas widgets.
//!
//! A no-Python producer for the demo reel: polls a public feed, normalizes it to
//! headline rows, and writes a `canvas.widget.snapshot.v1` envelope to the
//! stream socket. Rendering is delegated to the `feed-headlines-v1` Lua binder
//! (Lua has no network, so fetching stays here; presentation stays in Lua).
//!
//! Usage:
//!   canvas-feeds --feed hn --every 60
//!   canvas-feeds --feed reddit --sub programming --every 120
//!   canvas-feeds --feed rss --url https://xkcd.com/rss.xml --title XKCD --every 300
//!
//! When the socket is absent the loop still runs and reports failures loud, so a
//! widget marked `ready` always reflects a real snapshot rather than silence.
use canvas_feeds::{self, FeedKind, ReqwestHttp};
use std::{
    env,
    process::ExitCode,
    time::{Duration, Instant},
};

const ARG_ERR: &str = "usage: canvas-feeds --feed <hn|reddit|rss> [--sub r/x] [--url URL] [--title T] [--every SECS] [--owner OWNER] [--stream NAME] [--socket PATH]";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn parse_feed(args: &[String]) -> Result<FeedKind, String> {
    let kind = arg(args, "--feed").ok_or("--feed is required")?;
    match kind.as_str() {
        "hn" => Ok(FeedKind::HackerNews),
        "reddit" => {
            let sub = arg(args, "--sub").ok_or("--sub is required for reddit")?;
            Ok(FeedKind::Reddit { sub })
        }
        "rss" => {
            let url = arg(args, "--url").ok_or("--url is required for rss")?;
            let title = arg(args, "--title").unwrap_or_else(|| "RSS".into());
            Ok(FeedKind::Rss { url, title })
        }
        other => Err(format!("unknown --feed {other}; expected hn|reddit|rss")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let feed = match parse_feed(&args) {
        Ok(feed) => feed,
        Err(err) => {
            eprintln!("canvas-feeds: {err}\n{ARG_ERR}");
            return ExitCode::from(2);
        }
    };

    let every = arg(&args, "--every")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(60);
    let owner = arg(&args, "--owner").unwrap_or_else(|| "feeds".into());
    let stream = arg(&args, "--stream").unwrap_or_else(|| feed_stream(&feed));
    let http = ReqwestHttp::default();
    let mut sequence: u64 = 0;

    eprintln!(
        "canvas-feeds: {} -> {}/{} every {}s (Ctrl-C to stop)",
        feed.label(),
        owner,
        stream,
        every
    );

    loop {
        let since = Instant::now();
        sequence = sequence.wrapping_add(1);
        let emitted = canvas_feeds::now_ms();
        match canvas_feeds::fetch(&feed, &http) {
            Ok(feed_items) => {
                match canvas_feeds::emit(&owner, &stream, &feed_items, sequence, emitted) {
                    Ok(_) => eprintln!(
                        "canvas-feeds: {} rows refreshed ({}ms)",
                        feed_items.rows.len(),
                        since.elapsed().as_millis()
                    ),
                    Err(err) => eprintln!("canvas-feeds: {} ({} rows fetched, not displayed)", err, feed_items.rows.len()),
                }
            }
            Err(err) => {
                let envelope = canvas_feeds::error_envelope(&owner, &stream, &err.to_string(), sequence, emitted);
                if let Err(send_err) = canvas_feeds::send_raw(&envelope) {
                    eprintln!("canvas-feeds: {send_err}");
                } else {
                    eprintln!("canvas-feeds: source fault recorded as no_data");
                }
            }
        }

        if every == 0 {
            return ExitCode::SUCCESS;
        }
        let elapsed = since.elapsed();
        if elapsed < Duration::from_secs(every) {
            std::thread::sleep(Duration::from_secs(every) - elapsed);
        }
    }
}

fn feed_stream(feed: &FeedKind) -> String {
    match feed {
        FeedKind::HackerNews => "hn-headlines".to_string(),
        FeedKind::Reddit { sub } => format!("reddit-{sub}"),
        FeedKind::Rss { .. } => "rss-headlines".to_string(),
    }
}
