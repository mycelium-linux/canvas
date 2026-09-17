//! `canvas-comics` — put the last N webcomics on the wall as one image.
//!
//!   canvas-comics --url https://xkcd.com/comics.rss --title xkcd \
//!                 --source-id xkcd --count 4 --every 900
//!
//! Fetches the feed, resolves the newest comics' image URLs, downloads them,
//! and composites a 2x2 mosaic to `<runtime root>/frames/<source-id>.jpg`.
//! The Canvas host reloads that `live_image` surface when the bytes change.
//! The grid is composed here (see the design note in the comics module); the
//! screen renders one honest rectangle.

use canvas_feeds::{Http, ReqwestHttp, comics};
use std::{env, process::ExitCode, thread, time::Duration};

const ARG_ERR: &str = "usage: canvas-comics --url <feed> [--title T] [--source-id ID] [--count N] [--every SECS] \
     [--tile PX] [--gutter PX] [--runtime-dir PATH]";

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1).cloned())
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let Some(url) = arg(&args, "--url") else {
        eprintln!("canvas-comics: --url is required\n{ARG_ERR}");
        return ExitCode::from(2);
    };
    let source_id = arg(&args, "--source-id").unwrap_or_else(|| "comics".into());
    if !canvas_core::is_safe_frame_id(&source_id) {
        eprintln!("canvas-comics: unsafe --source-id {source_id:?}\n{ARG_ERR}");
        return ExitCode::from(2);
    }
    let root = match arg(&args, "--runtime-dir") {
        Some(dir) => std::path::PathBuf::from(dir),
        None => {
            let Some(root) = canvas_core::runtime_root() else {
                eprintln!(
                    "canvas-comics: no runtime root (set CANVAS_CONFIG_DIR or --runtime-dir)"
                );
                return ExitCode::from(2);
            };
            root
        }
    };
    let dest = match canvas_core::frame_file(&root, &source_id) {
        Some(dest) => dest,
        None => {
            eprintln!("canvas-comics: unsafe source id {source_id:?}");
            return ExitCode::from(2);
        }
    };
    let count = arg(&args, "--count")
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|n| (1..=4).contains(n))
        .unwrap_or(4);
    let every = arg(&args, "--every")
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n >= 30)
        .unwrap_or(900);
    let tile = arg(&args, "--tile")
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| (64..=2048).contains(n))
        .unwrap_or(360);
    let gutter = arg(&args, "--gutter")
        .and_then(|s| s.parse::<u32>().ok())
        .filter(|n| (1..=128).contains(n))
        .unwrap_or(16);

    let http = ReqwestHttp::default();
    eprintln!(
        "canvas-comics: {url} -> {} ({count} newest, every {every}s)",
        dest.display()
    );

    loop {
        match refresh(&http, &url, &dest, count, tile, gutter) {
            Ok(rows) => eprintln!("canvas-comics: {rows} comics refreshed"),
            // Fail loud but stay up: the previous frame remains on the wall,
            // which is the truthful state for a feed that is briefly down.
            Err(error) => eprintln!("canvas-comics: {error}"),
        }
        thread::sleep(Duration::from_secs(every));
    }
}

fn refresh(
    http: &dyn Http,
    url: &str,
    dest: &std::path::Path,
    count: usize,
    tile: u32,
    gutter: u32,
) -> Result<usize, String> {
    let xml = http.get_text(url).map_err(|e| e.to_string())?;
    let strips = comics::latest_comics(&xml, count);
    if strips.is_empty() {
        return Err("no comic images found in feed".into());
    }
    let mut slots: Vec<Option<Vec<u8>>> = Vec::with_capacity(strips.len());
    for comic in &strips {
        match http.get_bytes(&comic.image_url) {
            Ok(bytes) => slots.push(Some(bytes)),
            Err(error) => {
                eprintln!("canvas-comics: {} image unavailable: {error}", comic.title);
                slots.push(None);
            }
        }
    }
    let image_count = slots.iter().filter(|slot| slot.is_some()).count();
    let mosaic = comics::mosaic(&slots, tile, gutter, [20, 22, 28]).map_err(|e| e.to_string())?;
    canvas_core::write_atomic(dest, &mosaic).map_err(|e| e.to_string())?;
    Ok(image_count)
}
