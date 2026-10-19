//! App-private control endpoint behind authorized ADB forwarding, never LAN.
use canvas_workspace::endpoint::{Endpoint, MAX_FRAME};
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::{Mutex, OnceLock},
    time::Duration,
};
use subtle::ConstantTimeEq;

static ENDPOINT: OnceLock<Mutex<Endpoint>> = OnceLock::new();
static MAP_MOTION: OnceLock<Mutex<std::collections::BTreeMap<String, crate::map_motion::SurfaceMotion>>> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeResetMapMotion(
    _env: jni::JNIEnv, _class: jni::objects::JClass,
) {
    if let Some(states) = MAP_MOTION.get() {
        if let Ok(mut states) = states.lock() { states.clear(); }
    }
}
static STORE: OnceLock<std::path::PathBuf> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeMapManual(
    mut env: jni::JNIEnv, _class: jni::objects::JClass, id: jni::objects::JString,
) {
    let Ok(id) = env.get_string(&id).map(String::from) else { return; };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64);
    if let Some(states) = MAP_MOTION.get() {
        if let Ok(mut states) = states.lock() {
            if let Some(state) = states.get_mut(&id) { state.kiosk.manual(now); }
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeMapKiosk(
    mut env: jni::JNIEnv, _class: jni::objects::JClass, id: jni::objects::JString,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let id: String = env.get_string(&id).map_err(|e| e.to_string())?.into();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?.as_millis() as u64;
        let mut endpoint = endpoint().lock().map_err(|_| "map endpoint unavailable")?;
        let snapshot = endpoint.surface_snapshot(&id).ok_or("map snapshot unavailable")?;
        if snapshot["frame"]["view"] != "geo_map"
            || !matches!(snapshot["data"]["state"].as_str(), Some("ready" | "partial")) {
            return Ok(serde_json::json!({"advanced":false}));
        }
        let selected = snapshot["frame"]["map"]["selected"].as_str();
        let mut states = MAP_MOTION.get_or_init(|| Mutex::new(Default::default())).lock()
            .map_err(|_| "map motion unavailable")?;
        let Some(state) = states.get_mut(&id) else { return Ok(serde_json::json!({"advanced":false})); };
        if !state.kiosk.due(now, selected) { return Ok(serde_json::json!({"advanced":false})); }
        let reply = endpoint.execute(canvas_workspace::endpoint::Request {
            schema_version:1, request_id:"map-kiosk-next".into(), expected_revision:None,
            command:canvas_workspace::endpoint::Command::SurfaceNavigate {
                surface_id:id, action:fpl_gfx::panels::TaskNavigation::Next,
            },
        })?;
        if reply["handled"] == true { state.kiosk.mark_advanced(now); }
        Ok(serde_json::json!({"advanced":reply["handled"],"selected":reply["selected"]}))
    })();
    string(&mut env, result.unwrap_or_else(|error| serde_json::json!({"error":error})).to_string())
}
static AUDIO: OnceLock<(
    Mutex<canvas_workspace::audio::AudioControl>,
    std::sync::Condvar,
)> = OnceLock::new();

fn audio() -> &'static (
    Mutex<canvas_workspace::audio::AudioControl>,
    std::sync::Condvar,
) {
    AUDIO.get_or_init(|| (Mutex::new(Default::default()), std::sync::Condvar::new()))
}

fn dispatch_audio(raw: &str) -> String {
    use canvas_workspace::audio::{Command, Request};
    let result = (|| -> Result<serde_json::Value, String> {
        let request: Request = serde_json::from_str(raw).map_err(|e| e.to_string())?;
        if request.schema_version != 1 {
            return Err("unsupported_audio_version".into());
        }
        let (lock, changed) = audio();
        let mut control = lock.lock().map_err(|_| "audio_state_unavailable")?;
        if let Command::AudioSetMuted { source, muted } = request.command {
            let revision = request
                .expected_revision
                .ok_or("expected_revision_required")?;
            control.request(&request.request_id, revision, &source, muted)?;
            let deadline = std::time::Instant::now() + Duration::from_secs(2);
            while control
                .snapshot()
                .pending
                .as_ref()
                .is_some_and(|p| p.request_id == request.request_id)
            {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Err("renderer_acknowledgement_timeout; outcome_unknown".into());
                }
                control = changed
                    .wait_timeout(control, remaining)
                    .map_err(|_| "audio_state_unavailable")?
                    .0;
            }
            let observed = control.snapshot();
            if observed.source.as_ref() != Some(&source)
                || !observed.ready
                || observed.muted != muted
            {
                return Err("renderer_did_not_confirm_requested_gain".into());
            }
        }
        Ok(
            serde_json::json!({"ok":true,"schema_version":1,"request_id":request.request_id,"audio":control.snapshot()}),
        )
    })();
    result
        .unwrap_or_else(|error| serde_json::json!({"ok":false,"error":error}))
        .to_string()
}

fn endpoint() -> &'static Mutex<Endpoint> {
    ENDPOINT.get_or_init(|| Mutex::new(Endpoint::new("firetv-omni")))
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeTaskNavigate(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    id: jni::objects::JString,
    action: jni::objects::JString,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let id = String::from(env.get_string(&id).map_err(|e| e.to_string())?);
        let action = String::from(env.get_string(&action).map_err(|e| e.to_string())?);
        let action =
            serde_json::from_value(serde_json::json!(action)).map_err(|_| "Invalid navigation")?;
        endpoint()
            .lock()
            .map_err(|_| "Presentation unavailable")?
            .execute(canvas_workspace::endpoint::Request {
                schema_version: 1,
                request_id: "remote-task-navigation".into(),
                expected_revision: None,
                command: canvas_workspace::endpoint::Command::SurfaceNavigate {
                    surface_id: id,
                    action,
                },
            })
    })();
    string(
        &mut env,
        result
            .unwrap_or_else(|error| serde_json::json!({"error":error}))
            .to_string(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeObserverFrame(
    env: jni::JNIEnv,
    _class: jni::objects::JClass,
    seconds: jni::sys::jfloat,
) -> jni::sys::jfloatArray {
    let records = endpoint()
        .lock()
        .ok()
        .and_then(|e| e.observer_frame().cloned())
        .map(|f| crate::reactor::records(
            if f.activity == fpl_gfx::reactor::Activity::Hidden {
                // The platform adapter owns the shared 1.4s idle readback.
                fpl_gfx::reactor::Activity::Resting
            } else { f.activity }, seconds, f.playback_level))
        .unwrap_or_default();
    let result = (|| -> jni::errors::Result<jni::sys::jfloatArray> {
        let array = env.new_float_array(records.len() as i32)?;
        env.set_float_array_region(&array, 0, &records)?;
        Ok(array.into_raw())
    })();
    result.unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeObserverState(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
) -> jni::sys::jstring {
    let value = endpoint()
        .lock()
        .ok()
        .and_then(|e| e.observer_frame().cloned());
    let value = value.map(|frame| {
        let popup = crate::reactor::popup_descriptor(frame.activity, frame.caption.as_deref());
        let mut value = serde_json::to_value(frame).expect("display observation is serializable");
        value.as_object_mut().expect("display observation is an object").insert("popup".into(), popup);
        value
    });
    string(&mut env, serde_json::to_string(&value).unwrap_or_else(|_| "null".into()))
}

fn dispatch(raw: &str) -> String {
    if serde_json::from_str::<serde_json::Value>(raw)
        .ok()
        .and_then(|v| v["command"]["command"].as_str().map(str::to_owned))
        .is_some_and(|command| command == "audio_inspect" || command == "audio_set_muted")
    {
        return dispatch_audio(raw);
    }
    match endpoint().lock() {
        Ok(mut endpoint) => {
            let mut next = endpoint.clone();
            let reply = next.dispatch(raw);
            if next.checkpoint() != endpoint.checkpoint() {
                let saved = (|| -> Result<(), String> {
                    use std::os::unix::fs::OpenOptionsExt;
                    let path = STORE.get().ok_or("Checkpoint path unavailable")?;
                    let temporary = path.with_extension("pending");
                    let mut file = std::fs::OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .mode(0o600)
                        .open(&temporary)
                        .map_err(|e| e.to_string())?;
                    serde_json::to_writer(&mut file, &next.checkpoint())
                        .map_err(|e| e.to_string())?;
                    file.sync_all().map_err(|e| e.to_string())?;
                    std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
                    std::fs::File::open(path.parent().ok_or("Missing app directory")?)
                        .and_then(|directory| directory.sync_all())
                        .map_err(|e| e.to_string())
                })();
                if let Err(error) = saved {
                    return serde_json::json!({"ok":false,"error":format!("Checkpoint failed: {error}")}).to_string();
                }
            }
            // Transient display frames intentionally leave the checkpoint
            // unchanged, but must still become the live renderer's state.
            *endpoint = next;
            reply
        }
        Err(_) => r#"{"ok":false,"error":"Presentation state unavailable"}"#.into(),
    }
}

fn line(reader: &mut impl BufRead, max: usize) -> Result<String, String> {
    let mut bytes = Vec::new();
    reader
        .take(max as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > max || bytes.last() != Some(&b'\n') {
        return Err("Invalid frame".into());
    }
    bytes.pop();
    String::from_utf8(bytes).map_err(|_| "Invalid UTF-8".into())
}

fn start(directory: &str) -> Result<(), String> {
    if STORE.get().is_some() {
        return Ok(());
    }
    use std::os::unix::fs::OpenOptionsExt;
    let listener =
        TcpListener::bind(("127.0.0.1", crate::CONTROL_PORT)).map_err(|e| e.to_string())?;
    let mut entropy = [0u8; 32];
    getrandom::fill(&mut entropy).map_err(|_| "Credential generation failed")?;
    let token: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    let path = std::path::Path::new(directory).join("control-token");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(token.as_bytes())
        .map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    let checkpoint = std::path::Path::new(directory).join("workspaces.json");
    match std::fs::File::open(&checkpoint) {
        Ok(file) => {
            let mut raw = String::new();
            file.take(MAX_FRAME as u64 + 1)
                .read_to_string(&mut raw)
                .map_err(|e| e.to_string())?;
            *endpoint()
                .lock()
                .map_err(|_| "Presentation state unavailable")? =
                Endpoint::restore("firetv-omni", &raw)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    STORE
        .set(checkpoint)
        .map_err(|_| "Control already initialized")?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            if stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .is_err()
                || stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .is_err()
            {
                continue;
            }
            let mut reader = BufReader::new(&mut stream);
            let reply = match line(&mut reader, 65) {
                Ok(provided) if bool::from(provided.as_bytes().ct_eq(token.as_bytes())) => {
                    match line(&mut reader, MAX_FRAME) {
                        Ok(raw) => dispatch(&raw),
                        Err(_) => r#"{"ok":false,"error":"Invalid control frame"}"#.into(),
                    }
                }
                _ => r#"{"ok":false,"error":"Unauthorized"}"#.into(),
            };
            let _ = writeln!(stream, "{reply}");
        }
    });
    Ok(())
}

fn string(env: &mut jni::JNIEnv, value: String) -> jni::sys::jstring {
    env.new_string(value)
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeAudioUpdate(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    id: jni::objects::JString,
    title: jni::objects::JString,
    stream: jni::objects::JString,
    ready: jni::sys::jboolean,
    muted: jni::sys::jboolean,
) -> jni::sys::jstring {
    use sha2::{Digest, Sha256};
    let result = (|| -> Result<String, String> {
        let id: String = env.get_string(&id).map_err(|e| e.to_string())?.into();
        let title: String = env.get_string(&title).map_err(|e| e.to_string())?.into();
        let stream: String = env.get_string(&stream).map_err(|e| e.to_string())?.into();
        let source = if id.is_empty() || stream.is_empty() {
            None
        } else {
            Some(canvas_workspace::audio::Source {
                surface_id: id,
                title,
                stream_id: format!("sha256:{:x}", Sha256::digest(stream.as_bytes())),
            })
        };
        let (lock, changed) = audio();
        let mut control = lock.lock().map_err(|_| "audio_state_unavailable")?;
        control.observe(source, ready != 0, muted != 0);
        changed.notify_all();
        let mut snapshot = serde_json::to_value(control.snapshot()).map_err(|e| e.to_string())?;
        snapshot["control_notice"] =
            serde_json::to_value(control.notice(std::time::Instant::now()))
                .map_err(|e| e.to_string())?;
        serde_json::to_string(&snapshot).map_err(|e| e.to_string())
    })();
    string(
        &mut env,
        result.unwrap_or_else(|error| serde_json::json!({"error":error}).to_string()),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeAudioAcknowledge(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    id: jni::objects::JString,
    revision: jni::sys::jlong,
    muted: jni::sys::jboolean,
) -> jni::sys::jboolean {
    let success = (|| -> Result<(), String> {
        let id: String = env.get_string(&id).map_err(|e| e.to_string())?.into();
        let (lock, changed) = audio();
        let mut control = lock.lock().map_err(|_| "audio_state_unavailable")?;
        let source = control.snapshot().source.ok_or("source_unavailable")?;
        control.acknowledge(
            &id,
            revision.try_into().map_err(|_| "invalid_revision")?,
            &source,
            muted != 0,
        )?;
        crate::SURFACE_MUTED.store(muted != 0, std::sync::atomic::Ordering::Relaxed);
        changed.notify_all();
        Ok(())
    })()
    .is_ok();
    success as jni::sys::jboolean
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeSignalSnapshot(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    id: jni::objects::JString,
) -> jni::sys::jstring {
    let mut snapshot = env
        .get_string(&id)
        .ok()
        .and_then(|s| endpoint().lock().ok()?.surface_snapshot(&String::from(s)));
    if let Some(ref mut snapshot) = snapshot {
        if snapshot["frame"]["view"] == "geo_map" {
            let result = (|| -> Result<(), String> {
                let id: String = env.get_string(&id).map_err(|e| e.to_string())?.into();
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map_err(|e| e.to_string())?.as_millis() as u64;
                let mut states = MAP_MOTION.get_or_init(|| Mutex::new(Default::default()))
                    .lock().map_err(|_| "map motion lock unavailable")?;
                if !states.contains_key(&id) {
                    if states.len() >= 8 { return Err("map motion surface budget exceeded".into()); }
                    states.insert(id.clone(), crate::map_motion::SurfaceMotion::new()?);
                }
                states.get_mut(&id).ok_or("missing map motion")?.present(snapshot, now)
            })();
            if let Err(error) = result { *snapshot = serde_json::json!({"error":error}); }
        }
    }
    string(
        &mut env,
        snapshot
            .unwrap_or_else(|| serde_json::json!({"error":"Signal snapshot unavailable"}))
            .to_string(),
    )
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeSignalLabels(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jint,
    height: jni::sys::jint,
    raw: jni::objects::JString,
) -> jni::sys::jstring {
    let result = env
        .get_string(&raw)
        .map_err(|e| e.to_string())
        .and_then(|raw| {
            crate::signals::label_records(
                width.try_into().map_err(|_| "Invalid width")?,
                height.try_into().map_err(|_| "Invalid height")?,
                &String::from(raw),
            )
        });
    match result {
        Ok(labels) => string(&mut env, labels),
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", error);
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeSignalFrame(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jint,
    height: jni::sys::jint,
    raw: jni::objects::JString,
    appearance: jni::objects::JString,
) -> jni::sys::jfloatArray {
    let result = (|| -> Result<Vec<f32>, String> {
        let raw: String = env.get_string(&raw).map_err(|e| e.to_string())?.into();
        let appearance: String = env
            .get_string(&appearance)
            .map_err(|e| e.to_string())?
            .into();
        crate::signals::paint_records_with_appearance(
            u32::try_from(width).map_err(|_| "Invalid width")?,
            u32::try_from(height).map_err(|_| "Invalid height")?,
            &raw,
            &appearance,
        )
    })();
    match result {
        Ok(records) => match env.new_float_array(records.len() as i32) {
            Ok(array) => {
                if env.set_float_array_region(&array, 0, &records).is_err() {
                    return std::ptr::null_mut();
                }
                array.into_raw()
            }
            Err(_) => std::ptr::null_mut(),
        },
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", error);
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeBackgroundProgram(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    id: jni::objects::JString,
) -> jni::sys::jstring {
    let result = env
        .get_string(&id)
        .map(String::from)
        .map_err(|e| e.to_string())
        .and_then(|id| crate::background::program(&id));
    string(
        &mut env,
        match result {
            Ok(program) => program.to_string(),
            Err(error) => serde_json::json!({"error": error}).to_string(),
        },
    )
}

static BOIDS_RUNTIME: std::sync::OnceLock<std::sync::Mutex<crate::boids::Runtime>> =
    std::sync::OnceLock::new();

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeBoidsFrame(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jint,
    height: jni::sys::jint,
    elapsed: jni::sys::jfloat,
    aquarium: jni::sys::jboolean,
) -> jni::sys::jfloatArray {
    let result = (|| -> Result<Vec<f32>, &'static str> {
        BOIDS_RUNTIME
            .get_or_init(|| std::sync::Mutex::new(crate::boids::Runtime::default()))
            .lock()
            .map_err(|_| "Flock state unavailable")?
            .frame(
                u32::try_from(width).map_err(|_| "Invalid width")?,
                u32::try_from(height).map_err(|_| "Invalid height")?,
                elapsed,
                aquarium != 0,
            )
    })();
    match result {
        Ok(records) => match env.new_float_array(records.len() as i32) {
            Ok(array) => {
                if env.set_float_array_region(&array, 0, &records).is_err() {
                    return std::ptr::null_mut();
                }
                array.into_raw()
            }
            Err(_) => std::ptr::null_mut(),
        },
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", error);
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeClockFrame(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jint,
    height: jni::sys::jint,
    hour: jni::sys::jint,
    minute: jni::sys::jint,
    second: jni::sys::jfloat,
) -> jni::sys::jfloatArray {
    let result = (|| -> Result<Vec<f32>, &'static str> {
        crate::clock::paint_records(
            u32::try_from(width).map_err(|_| "Invalid width")?,
            u32::try_from(height).map_err(|_| "Invalid height")?,
            u8::try_from(hour).map_err(|_| "Invalid hour")?,
            u8::try_from(minute).map_err(|_| "Invalid minute")?,
            second,
        )
    })();
    match result {
        Ok(records) => match env.new_float_array(records.len() as i32) {
            Ok(array) => {
                if env.set_float_array_region(&array, 0, &records).is_err() {
                    return std::ptr::null_mut();
                }
                array.into_raw()
            }
            Err(_) => std::ptr::null_mut(),
        },
        Err(error) => {
            let _ = env.throw_new("java/lang/IllegalArgumentException", error);
            std::ptr::null_mut()
        }
    }
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativePdfAsset(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    raw: jni::objects::JString,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let raw: String = env.get_string(&raw).map_err(|e| e.to_string())?.into();
        let asset =
            serde_json::from_str::<canvas_workspace::AssetRef>(&raw).map_err(|e| e.to_string())?;
        let root = STORE
            .get()
            .and_then(|path| path.parent())
            .ok_or("App asset store unavailable")?;
        let path = crate::assets::verified_pdf(root, &asset)?;
        let viewports = crate::assets::verified_pdf_viewports(root, &asset)?;
        Ok(serde_json::json!({"path":path.to_string_lossy(),"viewports":viewports}))
    })();
    let reply = match result {
        Ok(value) => value,
        Err(error) => serde_json::json!({"error":error}),
    };
    string(&mut env, reply.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeMapRaster(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    raw: jni::objects::JString,
    width: jni::sys::jint,
    height: jni::sys::jint,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value,String> {
        let raw:String=env.get_string(&raw).map_err(|e|e.to_string())?.into();
        if raw.len()>65536 {return Err("Raster map exceeds contract budget".into());}
        let map=serde_json::from_str(&raw).map_err(|e|e.to_string())?;
        let root=STORE.get().and_then(|path|path.parent()).ok_or("App asset store unavailable")?;
        crate::map_raster::frame(root,&map,width as f32,height as f32)
    })();
    string(&mut env, match result {Ok(v)=>v,Err(e)=>serde_json::json!({"error":e})}.to_string())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeStartControl(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    path: jni::objects::JString,
) -> jni::sys::jstring {
    let result = env
        .get_string(&path)
        .map(String::from)
        .map_err(|_| "Cannot read app directory".into())
        .and_then(|path| start(&path));
    string(&mut env, result.err().unwrap_or_default())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeSelectWorkspace(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    project: jni::objects::JString,
) -> jni::sys::jstring {
    let result = env
        .get_string(&project)
        .map(String::from)
        .map_err(|_| "Cannot read project".to_string())
        .and_then(|name| {
            let endpoint = endpoint()
                .lock()
                .map_err(|_| "Presentation state unavailable")?;
            let revision = endpoint.snapshot()["revision"]
                .as_u64()
                .ok_or("Missing revision")?;
            let raw = serde_json::to_string(&canvas_workspace::endpoint::Request {
                schema_version: 1,
                request_id: "tv-remote".into(),
                expected_revision: Some(revision),
                command: canvas_workspace::endpoint::Command::WorkspaceSwitch { name },
            })
            .map_err(|error| error.to_string())?;
            drop(endpoint);
            let reply: serde_json::Value =
                serde_json::from_str(&dispatch(&raw)).map_err(|_| "Invalid switch receipt")?;
            if reply["ok"] == true {
                Ok(())
            } else {
                Err(reply["error"].as_str().unwrap_or("Switch failed").into())
            }
        });
    string(&mut env, result.err().unwrap_or_default())
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeProjection(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    width: jni::sys::jint,
    height: jni::sys::jint,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let endpoint = endpoint()
            .lock()
            .map_err(|_| "Presentation state unavailable")?;
        let mut snapshot = endpoint.snapshot();
        if snapshot["document"].is_null() {
            return Ok(snapshot);
        }
        let document: canvas_workspace::WorkspaceDocument =
            serde_json::from_value(snapshot["document"].clone()).map_err(|e| e.to_string())?;
        let frames = canvas_core::layout_tree::solve(
            document.active_layout(),
            canvas_core::layout::Rect {
                x: 0,
                y: 0,
                width: u32::try_from(width).map_err(|_| "Invalid width")?,
                height: u32::try_from(height).map_err(|_| "Invalid height")?,
            },
        )
        .map_err(|e| e.to_string())?;
        snapshot["tiles"] = serde_json::to_value(frames.iter().map(|placement| {
            let surface = document.surfaces.iter().find(|s| s.id == placement.surface_id);
            serde_json::json!({"surface":surface,
                "bounds":[placement.frame.x,placement.frame.y,placement.frame.width,placement.frame.height]})
        }).collect::<Vec<_>>()).map_err(|e| e.to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "Invalid system clock")?
            .as_millis() as u64;
        canvas_core::stream_lease::present_projection(&mut snapshot, now);
        Ok(snapshot)
    })();
    let output = match result {
        Ok(value) => value.to_string(),
        Err(error) => serde_json::json!({"error":error}).to_string(),
    };
    string(&mut env, output)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeSelectPage(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    index: jni::sys::jint,
) -> jni::sys::jstring {
    let result = (|| -> Result<(), String> {
        let index = usize::try_from(index).map_err(|_| "Invalid page index")?;
        let revision = endpoint()
            .lock()
            .map_err(|_| "Presentation state unavailable")?
            .snapshot()["revision"]
            .as_u64()
            .ok_or("Missing revision")?;
        let reply: serde_json::Value = serde_json::from_str(&dispatch(
            &serde_json::json!({
                "schema_version":1,"request_id":"tv-select-page","expected_revision":revision,
                "command":{"command":"workspace_page","index":index}
            })
            .to_string(),
        ))
        .map_err(|_| "Invalid page receipt")?;
        if reply["ok"] == true {
            Ok(())
        } else {
            Err(reply["error"]
                .as_str()
                .unwrap_or("Page switch failed")
                .into())
        }
    })();
    string(&mut env, result.err().unwrap_or_default())
}
