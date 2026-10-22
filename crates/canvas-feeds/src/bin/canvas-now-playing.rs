use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    env, fs,
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    path::Path,
    process::{Command, ExitCode},
    thread,
    time::Duration,
};

const OWNER: &str = "home-pi-media";
const STREAM: &str = "now-playing";
const SPOTIFY_STATE: &str = "/run/raspotify/now-playing.json";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct Playing {
    source: String,
    title: String,
    artist: String,
    album: Option<String>,
    uri: Option<String>,
    cover_url: Option<String>,
    playing: bool,
    elapsed_seconds: u64,
    duration_seconds: u64,
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let event_invocation = env::args()
        .next()
        .is_some_and(|path| path.contains("librespot-event"))
        || args.first().is_some_and(|arg| arg == "librespot-event");
    let result = if event_invocation {
        record_librespot_event(Path::new(SPOTIFY_STATE))
    } else {
        run_daemon()
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("canvas-now-playing: {error}");
            ExitCode::FAILURE
        }
    }
}

fn record_librespot_event(path: &Path) -> Result<(), String> {
    let event = env::var("PLAYER_EVENT").unwrap_or_default();
    let previous = fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Playing>(&bytes).ok())
        .unwrap_or_default();
    let value = Playing {
        source: "Spotify".into(),
        title: env::var("NAME").unwrap_or(previous.title),
        artist: env::var("ARTISTS").unwrap_or(previous.artist),
        album: env::var("ALBUM").ok().or(previous.album),
        uri: env::var("URI").ok().or(previous.uri),
        cover_url: env::var("COVERS")
            .ok()
            .and_then(|value| first_url(&value))
            .or(previous.cover_url),
        playing: match event.as_str() {
            "playing" | "track_changed" | "position_correction" => true,
            "paused" | "stopped" | "end_of_track" | "unavailable" => false,
            _ => previous.playing,
        },
        elapsed_seconds: env_u64("POSITION_MS").map_or(previous.elapsed_seconds, |ms| ms / 1_000),
        duration_seconds: env_u64("DURATION_MS").map_or(previous.duration_seconds, |ms| ms / 1_000),
    };
    let bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    let temp = path.with_extension("tmp");
    fs::write(&temp, bytes).map_err(|error| format!("write {}: {error}", temp.display()))?;
    fs::rename(&temp, path).map_err(|error| format!("publish {}: {error}", path.display()))
}

fn run_daemon() -> Result<(), String> {
    let root = canvas_core::runtime_root().ok_or("Canvas runtime root is unavailable")?;
    let art =
        canvas_core::frame_file(&root, "now-playing-art").ok_or("invalid artwork source id")?;
    if let Some(parent) = art.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut last_art = String::new();
    let mut last_published: Option<(Playing, u64)> = None;
    loop {
        let spotify = fs::read(SPOTIFY_STATE)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Playing>(&bytes).ok());
        let current = spotify
            .filter(|state| state.playing)
            .or_else(|| mpd_snapshot().ok());
        if let Some(current) = current {
            let art_key = current
                .cover_url
                .clone()
                .or_else(|| current.uri.clone())
                .unwrap_or_default();
            if !art_key.is_empty() && art_key != last_art {
                match refresh_art(&current, &art) {
                    Ok(()) => last_art = art_key,
                    Err(error) => eprintln!("canvas-now-playing: artwork: {error}"),
                }
            }
            let now = canvas_feeds::now_ms();
            if media_state_changed(last_published.as_ref(), &current, now) {
                match publish(&current, now) {
                    Ok(()) => last_published = Some((current, now)),
                    Err(error) => eprintln!("canvas-now-playing: publish: {error}"),
                }
            }
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// A playback observation only crosses the Lua boundary when it changes the
/// retained card. Rust extrapolates ordinary playback progress from the last
/// observation; polling MPD once a second must not mean executing Lua once a
/// second. A seek is a state change and is detected as elapsed-time drift.
fn media_state_changed(last: Option<&(Playing, u64)>, current: &Playing, now_ms: u64) -> bool {
    let Some((previous, published_at_ms)) = last else {
        return true;
    };
    if previous.source != current.source
        || previous.title != current.title
        || previous.artist != current.artist
        || previous.album != current.album
        || previous.uri != current.uri
        || previous.playing != current.playing
        || previous.duration_seconds != current.duration_seconds
    {
        return true;
    }
    let expected = previous
        .elapsed_seconds
        .saturating_add(if previous.playing {
            now_ms.saturating_sub(*published_at_ms) / 1_000
        } else {
            0
        });
    expected.abs_diff(current.elapsed_seconds) > 2
}

fn publish(current: &Playing, now: u64) -> Result<(), String> {
    let event = json!({
        "schema_version": 1,
        "owner": OWNER,
        "stream": STREAM,
        "contract": "canvas.music.nowplaying.v1",
        "sequence": now,
        "emitted_at_ms": now,
        "payload": {
            "project": "home",
            "track": current.title,
            "artist": current.artist,
            "album": current.album,
            "source": current.source,
            "elapsed_sec": current.elapsed_seconds,
            "duration_sec": current.duration_seconds,
            "is_playing": current.playing
        }
    });
    canvas_feeds::send_raw(&event).map_err(|error| error.to_string())
}

fn mpd_snapshot() -> Result<Playing, String> {
    let status = mpd("status")?;
    let song = mpd("currentsong")?;
    let state = status.get("state").map(String::as_str).unwrap_or("stop");
    if state == "stop" || song.is_empty() {
        return Err("MPD is idle".into());
    }
    let duration = number(&status, "duration")
        .or_else(|| number(&song, "Time"))
        .unwrap_or(0.0);
    let elapsed = number(&status, "elapsed")
        .unwrap_or(0.0)
        .min(duration.max(0.0));
    Ok(Playing {
        source: "MPD".into(),
        title: song
            .get("Title")
            .cloned()
            .or_else(|| {
                song.get("file")
                    .and_then(|path| Path::new(path).file_stem()?.to_str().map(str::to_owned))
            })
            .unwrap_or_else(|| "Unknown track".into()),
        artist: song
            .get("Artist")
            .cloned()
            .unwrap_or_else(|| "Unknown artist".into()),
        album: song.get("Album").cloned(),
        uri: song.get("file").cloned(),
        cover_url: None,
        playing: state == "play",
        elapsed_seconds: elapsed as u64,
        duration_seconds: duration as u64,
    })
}

fn mpd(command: &str) -> Result<BTreeMap<String, String>, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", 6600)).map_err(|error| error.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(stream.try_clone().map_err(|error| error.to_string())?);
    let mut greeting = String::new();
    reader
        .read_line(&mut greeting)
        .map_err(|error| error.to_string())?;
    if !greeting.starts_with("OK MPD") {
        return Err("invalid MPD greeting".into());
    }
    writeln!(stream, "{command}").map_err(|error| error.to_string())?;
    let mut values = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        let line = line.trim_end();
        if line == "OK" {
            return Ok(values);
        }
        if line.starts_with("ACK") || line.is_empty() {
            return Err(format!("MPD {command}: {line}"));
        }
        if let Some((key, value)) = line.split_once(": ") {
            values.insert(key.to_owned(), value.to_owned());
        }
    }
}

fn refresh_art(current: &Playing, destination: &Path) -> Result<(), String> {
    let bytes = if let Some(url) = &current.cover_url {
        reqwest::blocking::get(url)
            .map_err(|error| error.to_string())?
            .error_for_status()
            .map_err(|error| error.to_string())?
            .bytes()
            .map_err(|error| error.to_string())?
            .to_vec()
    } else if let Some(uri) = &current.uri {
        let output = Command::new("mpc")
            .args(["albumart", uri])
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() || output.stdout.is_empty() {
            return Err("MPD returned no album art".into());
        }
        output.stdout
    } else {
        return Err("track has no artwork locator".into());
    };
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("artwork exceeds 8 MiB".into());
    }
    image::load_from_memory(&bytes).map_err(|error| format!("decode artwork: {error}"))?;
    let temp = destination.with_extension("tmp");
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp, destination).map_err(|error| error.to_string())
}

fn env_u64(name: &str) -> Option<u64> {
    env::var(name).ok()?.parse().ok()
}

fn number(values: &BTreeMap<String, String>, key: &str) -> Option<f64> {
    values.get(key)?.parse().ok()
}

fn first_url(value: &str) -> Option<String> {
    let start = value.find("http")?;
    let rest = &value[start..];
    let end = rest.find(['\"', '\'', ',', ']', ' ']).unwrap_or(rest.len());
    Some(rest[..end].replace("\\/", "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_url_is_extracted_from_librespot_json() {
        assert_eq!(
            first_url(r#"[{"url":"https:\/\/i.scdn.co\/image\/abc"}]"#).as_deref(),
            Some("https://i.scdn.co/image/abc")
        );
    }

    #[test]
    fn ordinary_progress_does_not_rebuild_the_retained_card() {
        let initial = Playing {
            source: "MPD".into(),
            title: "Pyramid Song".into(),
            artist: "Radiohead".into(),
            playing: true,
            elapsed_seconds: 42,
            duration_seconds: 289,
            ..Playing::default()
        };
        assert!(!media_state_changed(
            Some(&(initial.clone(), 1_000)),
            &Playing {
                elapsed_seconds: 43,
                ..initial.clone()
            },
            2_000,
        ));
        assert!(media_state_changed(
            Some(&(initial.clone(), 1_000)),
            &Playing {
                elapsed_seconds: 120,
                ..initial
            },
            2_000,
        ));
    }
}
