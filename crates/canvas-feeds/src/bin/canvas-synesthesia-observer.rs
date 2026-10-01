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

#[derive(Clone, Deserialize, serde::Serialize)]
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
    let mut tracks = snapshot.meters.output_peaks
        .into_iter()
        .take(2)
        .enumerate()
        .map(|(index, peak)| json!({"label":format!("OUT{}", index + 1),"level":meter_level(peak),"clipping":snapshot.meters.output_clips > 0 && peak >= 0.999}))
        .collect::<Vec<_>>();
    tracks.extend(snapshot.meters.input_peaks.into_iter().take(8).enumerate().map(|(index, peak)| {
        json!({"label":format!("IN{}", index + 1),"level":meter_level(peak),"clipping":snapshot.meters.input_clips > 0 && peak >= 0.999})
    }));
    let notes = evenly_sample(snapshot.transport.notes, 192);
    json!({
        "title":"Synesthesia studio", "playing":snapshot.transport.playing,
        "paused":snapshot.transport.paused, "recording":snapshot.recording.active,
        "bpm":snapshot.transport.bpm, "position_ticks":snapshot.transport.position_ticks,
        "cycle_ticks":snapshot.transport.cycle_ticks.max(1), "tracks":tracks,
        "notes":notes
    })
}

fn meter_level(peak: f32) -> f32 {
    ((20.0 * peak.max(0.001).log10() + 60.0) / 60.0).clamp(0.0, 1.0)
}

fn evenly_sample(notes: Vec<Note>, limit: usize) -> Vec<Note> {
    if notes.len() <= limit {
        return notes;
    }
    let len = notes.len();
    (0..limit)
        .map(|index| index * len / limit)
        .map(|index| notes[index].clone())
        .collect()
}

fn envelope(client: &reqwest::blocking::Client, endpoint: &str, now: u64) -> serde_json::Value {
    let result = client
        .get(endpoint)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.json::<Snapshot>());
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
        assert_eq!(value["tracks"].as_array().unwrap().len(), 4);
        assert_eq!(value["tracks"][0]["label"], "OUT1");
        assert_eq!(value["tracks"][1]["label"], "OUT2");
        assert_eq!(value["notes"][0]["note"], 60);
        assert_eq!(value["notes"][0]["track"], 0);
    }

    #[test]
    fn piano_roll_sampling_preserves_the_whole_loop() {
        let notes = (0..256)
            .map(|tick| Note { track: 0, tick, length_ticks: 1, note: 60, velocity: 90 })
            .collect();
        let sampled = evenly_sample(notes, 192);
        assert_eq!(sampled.first().unwrap().tick, 0);
        assert!(sampled.last().unwrap().tick >= 254);
    }

    #[test]
    fn meters_use_decibel_height() {
        assert!((meter_level(1.0) - 1.0).abs() < f32::EPSILON);
        assert!((meter_level(0.1) - (2.0 / 3.0)).abs() < 0.001);
        assert_eq!(meter_level(0.001), 0.0);
    }
}

fn main() -> ExitCode {
    let endpoint = std::env::var("SYNESTHESIA_DAW_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7893/v1/daw".into());
    let client = match reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            eprintln!("canvas-synesthesia-observer: build HTTP client: {error}");
            return ExitCode::FAILURE;
        }
    };
    loop {
        let now = canvas_feeds::now_ms();
        if let Err(error) = canvas_feeds::send_raw(&envelope(&client, &endpoint, now)) {
            eprintln!("canvas-synesthesia-observer: {error}");
        }
        thread::sleep(Duration::from_millis(50));
    }
}
