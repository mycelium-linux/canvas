use serde::Deserialize;
use serde_json::json;
use std::{process::ExitCode, thread, time::Duration};

#[derive(Deserialize)]
struct Snapshot {
    transport: Transport,
    meters: Meters,
    recording: Recording,
}

#[derive(Deserialize)]
struct Transport {
    playing: bool,
    paused: bool,
    bpm: Option<f64>,
    position_ticks: u64,
    cycle_ticks: u64,
    #[serde(default)]
    notes: Vec<Note>,
}

#[derive(Deserialize, serde::Serialize)]
struct Note {
    track: usize,
    tick: u64,
    length_ticks: u64,
    note: u8,
    velocity: u8,
}

#[derive(Deserialize)]
struct Meters {
    input_peaks: Vec<f32>,
    output_peaks: Vec<f32>,
    input_clips: usize,
    output_clips: usize,
}

#[derive(Deserialize)]
struct Recording {
    active: bool,
}

fn payload(snapshot: Snapshot) -> serde_json::Value {
    let mut tracks = snapshot
        .meters
        .input_peaks
        .into_iter()
        .enumerate()
        .filter(|(_, level)| *level > 0.001)
        .map(|(index, level)| json!({"label":format!("IN{}", index + 1),"level":level.clamp(0.0,1.0),"clipping":snapshot.meters.input_clips > 0 && level >= 0.999}))
        .collect::<Vec<_>>();
    tracks.extend(snapshot.meters.output_peaks.into_iter().take(2).enumerate().map(|(index, level)| {
        json!({"label":format!("OUT{}", index + 1),"level":level.clamp(0.0,1.0),"clipping":snapshot.meters.output_clips > 0 && level >= 0.999})
    }));
    tracks.sort_by(|left, right| {
        right["level"]
            .as_f64()
            .partial_cmp(&left["level"].as_f64())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    tracks.truncate(10);
    json!({
        "title":"Synesthesia studio", "playing":snapshot.transport.playing,
        "paused":snapshot.transport.paused, "recording":snapshot.recording.active,
        "bpm":snapshot.transport.bpm, "position_ticks":snapshot.transport.position_ticks,
        "cycle_ticks":snapshot.transport.cycle_ticks.max(1), "tracks":tracks,
        "notes":snapshot.transport.notes.into_iter().take(128).collect::<Vec<_>>()
    })
}

fn envelope(endpoint: &str, now: u64) -> serde_json::Value {
    let result = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .and_then(|client| {
            client
                .get(endpoint)
                .send()?
                .error_for_status()?
                .json::<Snapshot>()
        });
    let payload = match result {
        Ok(snapshot) => payload(snapshot),
        Err(error) => {
            json!({"title":"Synesthesia studio","playing":false,"paused":false,"recording":false,"bpm":null,"position_ticks":0,"cycle_ticks":1,"tracks":[{"label":"API","level":0.0,"clipping":true}],"notes":[],"degraded":true,"error":error.to_string().chars().take(120).collect::<String>()})
        }
    };
    fit(
        json!({"schema_version":1,"owner":"synesthesia","stream":"daw","contract":"canvas.widget.snapshot.v1","sequence":now,"emitted_at_ms":now,"payload":payload}),
    )
}

fn fit(mut event: serde_json::Value) -> serde_json::Value {
    let budget = canvas_feeds::MAX_DATAGRAM - canvas_feeds::ENVELOPE_OVERHEAD;
    while serde_json::to_vec(&event).map_or(usize::MAX, |bytes| bytes.len()) > budget {
        let removed = event
            .pointer_mut("/payload/notes")
            .and_then(|value| value.as_array_mut())
            .is_some_and(|notes| notes.pop().is_some())
            || event
                .pointer_mut("/payload/tracks")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|tracks| tracks.len() > 1 && tracks.pop().is_some());
        if !removed {
            break;
        }
        event["payload"]["truncated"] = json!(true);
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_snapshot_projects_active_meters_and_notes() {
        let value = payload(Snapshot {
            transport: Transport {
                playing: true,
                paused: false,
                bpm: Some(104.0),
                position_ticks: 12,
                cycle_ticks: 96,
                notes: vec![Note {
                    track: 0,
                    tick: 0,
                    length_ticks: 6,
                    note: 60,
                    velocity: 100,
                }],
            },
            meters: Meters {
                input_peaks: vec![0.0, 0.5],
                output_peaks: vec![0.8, 0.7],
                input_clips: 0,
                output_clips: 0,
            },
            recording: Recording { active: true },
        });
        assert_eq!(value["recording"], true);
        assert_eq!(value["tracks"].as_array().unwrap().len(), 3);
        assert_eq!(value["notes"][0]["note"], 60);
        assert_eq!(value["notes"][0]["track"], 0);
    }
}

fn main() -> ExitCode {
    let endpoint = std::env::var("SYNESTHESIA_DAW_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7893/v1/daw".into());
    loop {
        let now = canvas_feeds::now_ms();
        if let Err(error) = canvas_feeds::send_raw(&envelope(&endpoint, now)) {
            eprintln!("canvas-synesthesia-observer: {error}");
        }
        thread::sleep(Duration::from_millis(250));
    }
}
