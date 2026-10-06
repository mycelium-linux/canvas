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
static STORE: OnceLock<std::path::PathBuf> = OnceLock::new();

fn endpoint() -> &'static Mutex<Endpoint> {
    ENDPOINT.get_or_init(|| Mutex::new(Endpoint::new("firetv-omni")))
}

fn dispatch(raw: &str) -> String {
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
                *endpoint = next;
            }
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
    let result = (|| -> Result<String, String> {
        let raw: String = env.get_string(&raw).map_err(|e| e.to_string())?.into();
        let asset =
            serde_json::from_str::<canvas_workspace::AssetRef>(&raw).map_err(|e| e.to_string())?;
        let root = STORE
            .get()
            .and_then(|path| path.parent())
            .ok_or("App asset store unavailable")?;
        crate::assets::verified_pdf(root, &asset).map(|path| path.to_string_lossy().into_owned())
    })();
    let reply = match result {
        Ok(path) => serde_json::json!({"path":path}),
        Err(error) => serde_json::json!({"error":error}),
    };
    string(&mut env, reply.to_string())
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
        Ok(snapshot)
    })();
    let output = match result {
        Ok(value) => value.to_string(),
        Err(error) => serde_json::json!({"error":error}).to_string(),
    };
    string(&mut env, output)
}
