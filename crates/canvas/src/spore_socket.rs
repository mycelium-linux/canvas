//! The socket a Spore presents on.
//!
//! A Spore owns its semantics; the host owns a window. This is where the two
//! meet: a line-delimited JSON protocol on `<runtime>/ui.sock`, so a producer in
//! any language can open a surface, hand over a payload from `soil-scene`, and be
//! told when the window moved. Deliberately not a Rust ABI and not WIT: the
//! payload is versioned data, and a producer that ships from a build farm should
//! not need to link the compositor to draw into it.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::OnceLock;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Something a producer told the host.
#[derive(Debug, Clone)]
pub enum SporeEvent {
    Hello {
        surface_id: String,
        title: String,
        width: u32,
        height: u32,
    },
    /// A frame is waiting. Only the newest one per surface is kept: the point of
    /// a UI frame is what it looks like now, and a queue of stale frames is
    /// latency pretending to be smoothness.
    PresentAvailable {
        surface_id: String,
    },
    Gone {
        surface_id: String,
    },
}

static SENDER: OnceLock<UnboundedSender<SporeEvent>> = OnceLock::new();

/// Newest payload per surface, plus whether the app has been told about it.
struct Slot {
    present: Option<soil_scene::Present>,
    signature: u64,
    signalled: bool,
    /// A frame has been asked for and not yet delivered. At most one is
    /// outstanding: that is the whole flow control.
    requested: bool,
    writer: Option<UnixStream>,
}

static LATEST: Mutex<Option<std::collections::BTreeMap<String, Slot>>> = Mutex::new(None);
/// Frames replaced before the host drew them. This is the number that says
/// whether the producer is outrunning the compositor, which is fine, or the
/// compositor is outrunning the producer, which is not.
static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn dropped_frames() -> u64 {
    DROPPED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Keep only the newest frame for a surface, and say whether the app needs
/// waking for it.
fn publish(surface_id: &str, present: soil_scene::Present, signature: u64) -> bool {
    let Ok(mut guard) = LATEST.lock() else {
        return false;
    };
    let slots = guard.get_or_insert_with(std::collections::BTreeMap::new);
    match slots.get_mut(surface_id) {
        Some(slot) => {
            if slot.signalled {
                // The app has not taken the previous frame yet; replace it.
                DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            slot.present = Some(present);
            slot.signature = signature;
            slot.requested = false;
            let wake = !slot.signalled;
            slot.signalled = true;
            wake
        }
        None => {
            slots.insert(
                surface_id.to_owned(),
                Slot {
                    present: Some(present),
                    signature,
                    signalled: true,
                    requested: false,
                    writer: None,
                },
            );
            true
        }
    }
}

/// Ask a producer for its next frame, at most one outstanding per surface.
///
/// This is the frame callback: the compositor says when it can draw, so a
/// producer cannot outrun it, nothing queues, and the clock the producer
/// animates from is the host's rather than its own.
pub fn request_frame(surface_id: &str, now_ms: u64, dt_ms: u64) {
    let Ok(mut guard) = LATEST.lock() else {
        return;
    };
    let Some(slots) = guard.as_mut() else { return };
    let Some(slot) = slots.get_mut(surface_id) else {
        return;
    };
    if slot.requested {
        return;
    }
    let Some(writer) = slot.writer.as_mut() else {
        return;
    };
    slot.requested = true;
    let body = serde_json::json!({
        "kind": "frame",
        "now_ms": now_ms,
        "dt_ms": dt_ms,
    });
    let mut line = serde_json::to_vec(&body).unwrap_or_default();
    line.push(b'\n');
    let _ = writer.write_all(&line);
    let _ = writer.flush();
}

/// Every surface a producer has registered, for the frame clock.
pub fn surfaces() -> Vec<String> {
    LATEST
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|slots| slots.keys().cloned().collect()))
        .unwrap_or_default()
}

fn remember_writer(surface_id: &str, writer: &UnixStream) {
    let Ok(mut guard) = LATEST.lock() else {
        return;
    };
    let slots = guard.get_or_insert_with(std::collections::BTreeMap::new);
    let slot = slots
        .entry(surface_id.to_owned())
        .or_insert_with(|| Slot {
            present: None,
            signature: 0,
            signalled: false,
            requested: false,
            writer: None,
        });
    slot.writer = writer.try_clone().ok();
}

/// Take the newest frame for a surface, if any.
pub fn take(surface_id: &str) -> Option<(soil_scene::Present, u64)> {
    let mut guard = LATEST.lock().ok()?;
    let slots = guard.as_mut()?;
    let slot = slots.get_mut(surface_id)?;
    slot.signalled = false;
    Some((slot.present.clone()?, slot.signature))
}

fn forget(surface_id: &str) {
    if let Ok(mut guard) = LATEST.lock()
        && let Some(slots) = guard.as_mut()
    {
        slots.remove(surface_id);
    }
}
static RECEIVER: Mutex<Option<UnboundedReceiver<SporeEvent>>> = Mutex::new(None);

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

fn serve(stream: UnixStream, sender: UnboundedSender<SporeEvent>) {
    let Ok(write_half) = stream.try_clone() else {
        return;
    };
    let mut writer = write_half;
    let reader = BufReader::new(stream);
    let mut surface: Option<String> = None;
    let mut generation: u64 = 1;

    for line in reader.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<serde_json::Value>(&line) else {
            let _ = reply(&mut writer, "error", "message is not JSON", generation);
            continue;
        };
        match message.get("kind").and_then(|kind| kind.as_str()) {
            Some("hello") => {
                let Some(id) = message
                    .get("surface_id")
                    .and_then(|value| value.as_str())
                    .filter(|id| safe_id(id))
                else {
                    let _ = reply(&mut writer, "error", "surface_id must be a safe id", generation);
                    continue;
                };
                let title = message
                    .get("title")
                    .and_then(|value| value.as_str())
                    .unwrap_or(id)
                    .to_owned();
                let width = extent(message.get("width"));
                let height = extent(message.get("height"));
                surface = Some(id.to_owned());
                remember_writer(id, &writer);
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
                let _ = reply(&mut writer, "state", "ok", generation);
            }
            Some("present") => {
                let Some(id) = surface.clone() else {
                    let _ = reply(&mut writer, "error", "present before hello", generation);
                    continue;
                };
                let Some(payload) = message.get("payload") else {
                    let _ = reply(&mut writer, "error", "present needs a payload", generation);
                    continue;
                };
                let signature = digest(line.as_bytes());
                let decoded = serde_json::from_value::<soil_scene::Present>(payload.clone())
                    .map_err(|error| error.to_string())
                    .and_then(|present| {
                        present.validate().map(|()| present).map_err(|e| e.0)
                    });
                match decoded {
                    Ok(present) => {
                        if publish(&id, present, signature)
                            && sender
                                .send(SporeEvent::PresentAvailable {
                                    surface_id: id,
                                })
                                .is_err()
                        {
                            return;
                        }
                        generation = generation.saturating_add(1);
                        let _ = reply(&mut writer, "state", "ok", generation);
                    }
                    Err(error) => {
                        // Refusals name the reason: a producer that cannot see why
                        // its scene was dropped has nothing to fix.
                        let _ = reply(&mut writer, "error", &error, generation);
                    }
                }
            }
            Some("close") => {
                if let Some(id) = surface.take() {
                    forget(&id);
                    let _ = sender.send(SporeEvent::Gone { surface_id: id });
                }
                let _ = reply(&mut writer, "state", "ok", generation);
                return;
            }
            _ => {
                let _ = reply(&mut writer, "error", "unknown kind", generation);
            }
        }
    }
    if let Some(id) = surface {
        forget(&id);
        let _ = sender.send(SporeEvent::Gone { surface_id: id });
    }
}

fn digest(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

fn reply(writer: &mut UnixStream, kind: &str, message: &str, generation: u64) -> std::io::Result<()> {
    let body = serde_json::json!({
        "kind": kind,
        "message": message,
        "generation": generation,
    });
    let mut line = serde_json::to_vec(&body).unwrap_or_default();
    line.push(b'\n');
    writer.write_all(&line)?;
    writer.flush()
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
