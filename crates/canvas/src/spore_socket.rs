//! The socket a Spore presents on.
//!
//! A Spore owns its semantics; the host owns a window. This is where the two
//! meet: a line-delimited JSON protocol on `<runtime>/ui.sock`, so a producer in
//! any language can open a surface, hand over a payload from `soil-scene`, and be
//! told when the window moved. Deliberately not a Rust ABI and not WIT: the
//! payload is versioned data, and a producer that ships from a build farm should
//! not need to link the compositor to draw into it.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// One decoded GPU frame. The socket carries bytes as base64 because the
/// protocol is line-delimited JSON; the host still owns validation and never
/// writes a producer-chosen path.
const MAX_GPU_FRAME_BYTES: usize = 64 * 1024 * 1024;
const MAX_GPU_FRAME_DIMENSION: u32 = 4096;

/// Something a producer told the host.
#[derive(Debug, Clone)]
pub enum SporeEvent {
    Hello {
        surface_id: String,
        title: String,
        width: u32,
        height: u32,
    },
    Present {
        surface_id: String,
        present: soil_scene::Present,
        /// Digest of the message bytes, taken here so the host never re-encodes
        /// a payload it has already parsed.
        signature: u64,
    },
    GpuFrame {
        /// The lease named by a later or earlier `Present::GpuLease`.
        source_id: String,
        width: u32,
        height: u32,
        revision: u64,
        rgba: Vec<u8>,
    },
    Gone {
        surface_id: String,
    },
}

static SENDER: OnceLock<UnboundedSender<SporeEvent>> = OnceLock::new();
static RECEIVER: Mutex<Option<UnboundedReceiver<SporeEvent>>> = Mutex::new(None);
/// Producers keep the same socket open for replies and host input. Canvas owns
/// the connection, so it can send pointer/scroll/key events back without a
/// second listener or a producer-chosen path.
static WRITERS: OnceLock<Mutex<HashMap<String, Arc<Mutex<UnixStream>>>>> = OnceLock::new();

/// Start listening, once. Returns the receiver the app subscribes to.
pub fn start() -> Option<UnboundedReceiver<SporeEvent>> {
    let (sender, receiver) = unbounded_channel();
    if SENDER.set(sender.clone()).is_err() {
        return RECEIVER.lock().ok().and_then(|mut slot| slot.take());
    }
    if let Some(mut slot) = RECEIVER.lock().ok().and_then(|mut slot| slot.take()) {
        // A second caller wants the stream this process already owns.
        return Some(slot);
    }
    let Some(path) = socket_path() else {
        return None;
    };
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return None;
    }
    // A stale socket from a crashed host would make every producer's connect
    // fail with ECONNREFUSED, which reads as "the compositor is broken".
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else {
        return None;
    };
    std::thread::Builder::new()
        .name("canvas-spore-socket".into())
        .spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let sender = sender.clone();
                std::thread::Builder::new()
                    .name("canvas-spore-conn".into())
                    .spawn(move || serve(stream, sender))
                    .ok();
            }
        })
        .ok()?;
    Some(receiver)
}

pub fn socket_path() -> Option<PathBuf> {
    canvas_core::runtime_root().map(|root| root.join("ui.sock"))
}

/// Send one host input event to the producer that owns `surface_id`.
///
/// The producer always sees the same line-delimited JSON stream it writes on.
/// A producer that does not read input can ignore these lines; Canvas never
/// waits for an acknowledgement.
pub fn send_input(surface_id: &str, message: serde_json::Value) -> Result<(), String> {
    let writers = WRITERS
        .get()
        .ok_or_else(|| "spore socket is not listening".to_string())?;
    let writer = writers
        .lock()
        .map_err(|_| "spore writer registry poisoned".to_string())?
        .get(surface_id)
        .cloned()
        .ok_or_else(|| format!("surface {surface_id} is not connected"))?;
    let mut line = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
    line.push(b'\n');
    let mut stream = writer
        .lock()
        .map_err(|_| "spore connection poisoned".to_string())?;
    stream.write_all(&line).map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())
}

fn writers() -> &'static Mutex<HashMap<String, Arc<Mutex<UnixStream>>>> {
    WRITERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_writer(surface_id: &str, writer: Arc<Mutex<UnixStream>>) {
    if let Ok(mut writers) = writers().lock() {
        writers.insert(surface_id.to_owned(), writer);
    }
}

fn unregister_writer(surface_id: &str) {
    if let Ok(mut writers) = writers().lock() {
        writers.remove(surface_id);
    }
}

fn serve(stream: UnixStream, sender: UnboundedSender<SporeEvent>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let writer = Arc::new(Mutex::new(write_half));
    let reader = BufReader::new(stream);
    let mut surface: Option<String> = None;
    let mut generation: u64 = 1;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            let _ = reply(&writer, "error", "message is not JSON", generation);
            continue;
        };
        match message.get("kind").and_then(|kind| kind.as_str()) {
            Some("hello") => {
                let Some(id) = message
                    .get("surface_id")
                    .and_then(|value| value.as_str())
                    .filter(|id| safe_id(id))
                else {
                    let _ = reply(&writer, "error", "surface_id must be a safe id", generation);
                    continue;
                };
                let title = message
                    .get("title")
                    .and_then(|value| value.as_str())
                    .unwrap_or(id)
                    .to_owned();
                let width = extent(message.get("width"));
                let height = extent(message.get("height"));
                if let Some(previous) = surface.replace(id.to_owned()) {
                    unregister_writer(&previous);
                }
                register_writer(id, writer.clone());
                if sender
                    .send(SporeEvent::Hello {
                        surface_id: id.to_owned(),
                        title,
                        width,
                        height,
                    })
                    .is_err()
                {
                    return;
                }
                let _ = reply(&writer, "state", "ok", generation);
            }
            Some("present") => {
                let Some(id) = surface.clone() else {
                    let _ = reply(&writer, "error", "present before hello", generation);
                    continue;
                };
                let Some(payload) = message.get("payload") else {
                    let _ = reply(&writer, "error", "present needs a payload", generation);
                    continue;
                };
                let signature = digest(line.as_bytes());
                let decoded = serde_json::from_value::<soil_scene::Present>(payload.clone())
                    .map_err(|error| error.to_string())
                    .and_then(|present| present.validate().map(|()| present).map_err(|e| e.0));
                match decoded {
                    Ok(present) => {
                        if sender
                            .send(SporeEvent::Present {
                                surface_id: id,
                                present,
                                signature,
                            })
                            .is_err()
                        {
                            return;
                        }
                        generation = generation.saturating_add(1);
                        let _ = reply(&writer, "state", "ok", generation);
                    }
                    Err(error) => {
                        // Refusals name the reason: a producer that cannot see why
                        // its scene was dropped has nothing to fix.
                        let _ = reply(&writer, "error", &error, generation);
                    }
                }
            }
            Some("gpu_frame") => {
                let source_id = message
                    .get("source_id")
                    .and_then(|value| value.as_str())
                    .filter(|id| safe_id(id));
                let Some(source_id) = source_id else {
                    let _ = reply(
                        &writer,
                        "error",
                        "gpu_frame source_id must be a safe id",
                        generation,
                    );
                    continue;
                };
                let width = message
                    .get("width")
                    .and_then(|value| value.as_u64())
                    .and_then(|value| u32::try_from(value).ok());
                let height = message
                    .get("height")
                    .and_then(|value| value.as_u64())
                    .and_then(|value| u32::try_from(value).ok());
                let (Some(width), Some(height)) = (width, height) else {
                    let _ = reply(
                        &writer,
                        "error",
                        "gpu_frame needs integer width and height",
                        generation,
                    );
                    continue;
                };
                if width == 0
                    || height == 0
                    || width > MAX_GPU_FRAME_DIMENSION
                    || height > MAX_GPU_FRAME_DIMENSION
                {
                    let _ = reply(
                        &writer,
                        "error",
                        "gpu_frame extent is outside 1..=4096",
                        generation,
                    );
                    continue;
                }
                let Some(encoded) = message.get("rgba").and_then(|value| value.as_str()) else {
                    let _ = reply(&writer, "error", "gpu_frame needs rgba", generation);
                    continue;
                };
                let Some(expected) = (width as usize)
                    .checked_mul(height as usize)
                    .and_then(|pixels| pixels.checked_mul(4))
                else {
                    let _ = reply(&writer, "error", "gpu_frame extent overflows", generation);
                    continue;
                };
                if expected > MAX_GPU_FRAME_BYTES {
                    let _ = reply(
                        &writer,
                        "error",
                        "gpu_frame is over the byte ceiling",
                        generation,
                    );
                    continue;
                }
                let Ok(rgba) = base64::engine::general_purpose::STANDARD.decode(encoded) else {
                    let _ = reply(&writer, "error", "gpu_frame rgba is not base64", generation);
                    continue;
                };
                if rgba.len() != expected {
                    let _ = reply(
                        &writer,
                        "error",
                        "gpu_frame rgba length does not match its extent",
                        generation,
                    );
                    continue;
                }
                let revision = message
                    .get("revision")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                if sender
                    .send(SporeEvent::GpuFrame {
                        source_id: source_id.to_owned(),
                        width,
                        height,
                        revision,
                        rgba,
                    })
                    .is_err()
                {
                    return;
                }
                generation = generation.saturating_add(1);
                let _ = reply(&writer, "state", "ok", generation);
            }
            Some("gpu_frame_shm") => {
                let source_id = message
                    .get("source_id")
                    .and_then(|value| value.as_str())
                    .filter(|id| safe_id(id));
                let Some(source_id) = source_id else {
                    let _ = reply(
                        &writer,
                        "error",
                        "gpu_frame_shm source_id must be a safe id",
                        generation,
                    );
                    continue;
                };
                let Some(path) = message.get("path").and_then(|value| value.as_str()) else {
                    let _ = reply(&writer, "error", "gpu_frame_shm needs path", generation);
                    continue;
                };
                match soil_scene::shm::read(path) {
                    Ok(frame) => {
                        if sender
                            .send(SporeEvent::GpuFrame {
                                source_id: source_id.to_owned(),
                                width: frame.width,
                                height: frame.height,
                                revision: frame.revision,
                                rgba: frame.rgba,
                            })
                            .is_err()
                        {
                            return;
                        }
                        generation = generation.saturating_add(1);
                        let _ = reply(&writer, "state", "ok", generation);
                    }
                    Err(error) => {
                        let _ = reply(
                            &writer,
                            "error",
                            &format!("gpu_frame_shm refused: {error}"),
                            generation,
                        );
                    }
                }
            }
            Some("close") => {
                if let Some(id) = surface.take() {
                    unregister_writer(&id);
                    let _ = sender.send(SporeEvent::Gone { surface_id: id });
                }
                let _ = reply(&writer, "state", "ok", generation);
                return;
            }
            _ => {
                let _ = reply(&writer, "error", "unknown kind", generation);
            }
        }
    }
    if let Some(id) = surface {
        unregister_writer(&id);
        let _ = sender.send(SporeEvent::Gone { surface_id: id });
    }
}

fn digest(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn reply(
    writer: &Arc<Mutex<UnixStream>>,
    kind: &str,
    message: &str,
    generation: u64,
) -> std::io::Result<()> {
    let body = serde_json::json!({
        "kind": kind,
        "message": message,
        "generation": generation,
    });
    let mut line = serde_json::to_vec(&body).unwrap_or_default();
    line.push(b'\n');
    let mut stream = writer
        .lock()
        .map_err(|_| std::io::Error::other("spore writer poisoned"))?;
    stream.write_all(&line)?;
    stream.flush()
}

fn extent(value: Option<&serde_json::Value>) -> u32 {
    value
        .and_then(|value| value.as_u64())
        .unwrap_or(640)
        .clamp(160, 8192) as u32
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}
