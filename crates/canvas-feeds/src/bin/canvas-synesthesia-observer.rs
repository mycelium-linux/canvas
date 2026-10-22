use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;
use std::{
    process::ExitCode,
    thread,
    time::{Duration, Instant},
};

#[derive(Deserialize)]
struct Snapshot {
    transport: Transport,
    meters: Meters,
    recording: Recording,
    #[serde(default)]
    mix_fields: Vec<MixField>,
}

#[derive(Deserialize)]
struct MixField {
    surface: String,
    strips: Vec<MixStrip>,
}

#[derive(Deserialize)]
struct MixStrip {
    index: usize,
    cc: u8,
    sources: Vec<String>,
    gain: f32,
    active: bool,
    peak: f32,
    stereo_position: f32,
    left_peak: f32,
    right_peak: f32,
    #[serde(default)]
    bands: Vec<f32>,
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

#[derive(Deserialize)]
struct StudioSnapshot {
    graph: StudioGraph,
    state: StudioState,
}

#[derive(Deserialize)]
struct StudioGraph {
    #[serde(default)]
    midi_sources: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    instruments: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    audio_sources: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    inserts: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct StudioState {
    #[serde(default)]
    midi_routes: Vec<MidiRoute>,
    #[serde(default)]
    monitor_routes: Vec<MonitorRoute>,
    #[serde(default)]
    insert_routes: BTreeMap<String, Vec<String>>,
}

#[derive(Deserialize)]
struct MidiRoute {
    source: String,
    destination: String,
}

#[derive(Deserialize)]
struct MonitorRoute {
    source: String,
    bus: String,
}

fn routing_payload(snapshot: StudioSnapshot) -> serde_json::Value {
    let mut routes = Vec::new();
    routes.extend(snapshot.state.midi_routes.into_iter().take(8).map(|route| {
        json!({"label":format!("{} → {}", route.source, route.destination),"state":"good","detail":"MIDI"})
    }));
    routes.extend(snapshot.state.monitor_routes.into_iter().take(8).map(|route| {
        json!({"label":format!("{} → {}", route.source, route.bus),"state":"good","detail":"monitor"})
    }));
    routes.extend(snapshot.state.insert_routes.into_iter().take(8).map(|(lane, chain)| {
        json!({"label":lane,"state":"good","detail":if chain.is_empty() { "dry".into() } else { chain.join(" → ") }})
    }));
    json!({
        "title":"Studio routing", "routes":routes,
        "inventory":{
            "controllers":snapshot.graph.midi_sources.len(), "instruments":snapshot.graph.instruments.len(),
            "sources":snapshot.graph.audio_sources.len(), "inserts":snapshot.graph.inserts.len()
        }
    })
}

fn quality_payload(snapshot: &Snapshot) -> serde_json::Value {
    let input_peak = snapshot
        .meters
        .input_peaks
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    let output_peak = snapshot
        .meters
        .output_peaks
        .iter()
        .copied()
        .fold(0.0_f32, f32::max);
    json!({"title":"Signal headroom","style":"bullet","unit":"dB","metrics":[
        {"label":"Input headroom","value":headroom_db(input_peak),"max":60.0,"target":6.0,"state":if snapshot.meters.input_clips == 0 {"good"} else {"critical"}},
        {"label":"Output headroom","value":headroom_db(output_peak),"max":60.0,"target":6.0,"state":if snapshot.meters.output_clips == 0 {"good"} else {"critical"}}
    ],"detail":format!("{} input clips · {} output clips", snapshot.meters.input_clips, snapshot.meters.output_clips)})
}

fn mix_field_payload(fields: &[MixField]) -> serde_json::Value {
    let Some(field) = fields.first() else {
        return json!({"title":"Studio mix field","surface":"unavailable","strips":[],"degraded":true,"error":"Synesthesia reported no control surfaces"});
    };
    let strips = field
        .strips
        .iter()
        .take(8)
        .map(|strip| json!({
            "index":strip.index,
            "cc":strip.cc,
            "label":strip.sources.join(" + "),
            "gain":strip.gain.clamp(0.0, 2.0),
            "active":strip.active,
            "peak":strip.peak.clamp(0.0, 2.0),
            "stereo_position":strip.stereo_position.clamp(-1.0, 1.0),
            "left_peak":strip.left_peak.clamp(0.0, 2.0),
            "right_peak":strip.right_peak.clamp(0.0, 2.0),
            "bands":strip.bands.iter().copied().take(64).map(|value| value.clamp(0.0, 1.0)).collect::<Vec<_>>()
        }))
        .collect::<Vec<_>>();
    json!({"title":"Studio mix field","surface":field.surface,"strips":strips})
}

fn headroom_db(peak: f32) -> f32 {
    (-20.0 * peak.max(0.001).log10()).clamp(0.0, 60.0)
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

fn degraded_daw_payload(error: String) -> serde_json::Value {
    json!({"title":"Synesthesia studio","playing":false,"paused":false,"recording":false,"bpm":null,"position_ticks":0,"cycle_ticks":1,"tracks":[{"label":"API","level":0.0,"clipping":true}],"notes":[],"degraded":true,"error":error.chars().take(120).collect::<String>()})
}

fn stream_envelope(stream: &str, payload: serde_json::Value, now: u64) -> serde_json::Value {
    fit(
        json!({"schema_version":1,"owner":"synesthesia","stream":stream,"contract":"canvas.widget.snapshot.v1","sequence":now,"emitted_at_ms":now,"payload":payload}),
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
                .is_some_and(|tracks| tracks.len() > 1 && tracks.pop().is_some())
            || event
                .pointer_mut("/payload/routes")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|routes| routes.pop().is_some())
            || event
                .pointer_mut("/payload/metrics")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|metrics| metrics.len() > 1 && metrics.pop().is_some())
            || event
                .pointer_mut("/payload/strips")
                .and_then(|value| value.as_array_mut())
                .is_some_and(|strips| strips.len() > 1 && strips.pop().is_some());
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
            mix_fields: vec![],
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
            .map(|tick| Note {
                track: 0,
                tick,
                length_ticks: 1,
                note: 60,
                velocity: 90,
            })
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

    #[test]
    fn signal_quality_reports_positive_headroom() {
        let snapshot = Snapshot {
            transport: Transport {
                playing: false,
                paused: false,
                bpm: None,
                position_ticks: 0,
                cycle_ticks: 1,
                notes: vec![],
            },
            meters: Meters {
                input_peaks: vec![0.5],
                output_peaks: vec![1.0],
                input_clips: 0,
                output_clips: 1,
            },
            recording: Recording { active: false },
            mix_fields: vec![],
        };
        let value = quality_payload(&snapshot);
        assert!((value["metrics"][0]["value"].as_f64().unwrap() - 6.0206).abs() < 0.01);
        assert_eq!(value["metrics"][1]["state"], "critical");
    }

    #[test]
    fn routing_envelope_is_thinned_to_the_transport_budget() {
        let routes = (0..100).map(|index| json!({"label":format!("very-long-source-{index:03} → very-long-destination-{index:03}"),"state":"good","detail":"hardware insert route"})).collect::<Vec<_>>();
        let event = stream_envelope("studio-routing", json!({"routes":routes}), 1);
        assert!(
            serde_json::to_vec(&event).unwrap().len()
                <= canvas_feeds::MAX_DATAGRAM - canvas_feeds::ENVELOPE_OVERHEAD
        );
        assert_eq!(event["payload"]["truncated"], true);
    }

    #[test]
    fn mix_field_preserves_graph_derived_knob_and_stereo_state() {
        let payload = mix_field_payload(&[MixField {
            surface: "launchkey".into(),
            strips: vec![MixStrip {
                index: 1,
                cc: 21,
                sources: vec!["modular".into()],
                gain: 0.5,
                active: true,
                peak: 0.75,
                stereo_position: -0.25,
                left_peak: 0.75,
                right_peak: 0.5,
                bands: vec![0.8, 0.2],
            }],
        }]);
        assert_eq!(payload["surface"], "launchkey");
        assert_eq!(payload["strips"][0]["label"], "modular");
        assert_eq!(payload["strips"][0]["cc"], 21);
        assert!((payload["strips"][0]["bands"][0].as_f64().unwrap() - 0.8).abs() < 0.0001);
    }
}

fn main() -> ExitCode {
    let endpoint = std::env::var("SYNESTHESIA_DAW_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7893/v1/daw".into());
    let studio_endpoint = std::env::var("SYNESTHESIA_STUDIO_URL")
        .unwrap_or_else(|_| "http://127.0.0.1:7893/v1/studio".into());
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
    let mut last_routing = Instant::now() - Duration::from_secs(1);
    loop {
        let now = canvas_feeds::now_ms();
        let daw = client
            .get(&endpoint)
            .send()
            .and_then(|response| response.error_for_status())
            .and_then(|response| response.json::<Snapshot>());
        let (daw_event, quality_event, mix_field_event) = match daw {
            Ok(snapshot) => {
                let quality = stream_envelope("studio-quality", quality_payload(&snapshot), now);
                let mix_field =
                    stream_envelope("mix-field", mix_field_payload(&snapshot.mix_fields), now);
                (
                    stream_envelope("daw", payload(snapshot), now),
                    quality,
                    mix_field,
                )
            }
            Err(error) => {
                let detail = error.to_string().chars().take(120).collect::<String>();
                (
                    stream_envelope("daw", degraded_daw_payload(detail.clone()), now),
                    stream_envelope(
                        "studio-quality",
                        json!({"degraded":true,"error":detail}),
                        now,
                    ),
                    stream_envelope("mix-field", json!({"degraded":true,"error":detail}), now),
                )
            }
        };
        if let Err(error) = canvas_feeds::send_raw(&daw_event) {
            eprintln!("canvas-synesthesia-observer: {error}");
        }
        if let Err(error) = canvas_feeds::send_raw(&quality_event) {
            eprintln!("canvas-synesthesia-observer: {error}");
        }
        if let Err(error) = canvas_feeds::send_raw(&mix_field_event) {
            eprintln!("canvas-synesthesia-observer: {error}");
        }
        if last_routing.elapsed() >= Duration::from_secs(1) {
            last_routing = Instant::now();
            let routing = client
                .get(&studio_endpoint)
                .send()
                .and_then(|response| response.error_for_status())
                .and_then(|response| response.json::<StudioSnapshot>());
            let payload = routing.map(routing_payload).unwrap_or_else(|error| json!({"degraded":true,"error":error.to_string().chars().take(120).collect::<String>()}));
            if let Err(error) =
                canvas_feeds::send_raw(&stream_envelope("studio-routing", payload, now))
            {
                eprintln!("canvas-synesthesia-observer: {error}");
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
}
