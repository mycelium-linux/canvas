//! Bounded tvOS host boundary. Canvas owns state; Apple owns lifecycle/input.
use canvas_core::Catalog;
use canvas_workspace::endpoint::{Command, Endpoint, Request as EndpointRequest};
use fpl_gfx::{AnalogClock, Rect, Scene, Theme};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::{Mutex, OnceLock};

const MAX_FRAME: usize = 64 * 1024;
const CATALOG: &str = include_str!("../apple/catalog.json");
static STATE: OnceLock<Mutex<Endpoint>> = OnceLock::new();

fn initial_state() -> Endpoint {
    let mut state = Endpoint::new("canvas-tvos");
    let catalog = Catalog::parse(CATALOG).expect("bundled catalog is tested");
    let documents = ["home", "studio"]
        .into_iter()
        .map(|project| {
            canvas_workspace::WorkspaceDocument::parse(
                &json!({
                    "schema_version":1,"id":format!("tvos-{project}"),"project_id":project,
                    "revision":1,"theme":{"source":"project"},
                    "surfaces":[{"id":"clock","title":"Clock","content":{"kind":"widget",
                        "spec":{"schema_version":2,"id":"clock","kind":"clock",
                            "format":"%H:%M:%S","mode":"analog","theme":null,
                            "refresh_ms":33,"chrome":"frameless",
                            "data":{"state":"ready","source":null,"observed_at":null,
                                "window":null,"detail":null,"provenance":"live"}}}}],
                    "layout":{"kind":"surface","surface_id":"clock"}
                })
                .to_string(),
                &catalog,
            )
            .expect("bundled workspace is tested")
        })
        .collect();
    state
        .execute(EndpointRequest {
            schema_version: 1,
            request_id: "bootstrap".into(),
            expected_revision: Some(1),
            command: Command::Install {
                catalog,
                documents,
                selected: "home".into(),
            },
        })
        .expect("bundled session is tested");
    state
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Request {
    Snapshot {},
    Select {
        project: String,
        expected_revision: u64,
    },
    Frame {
        width: f32,
        height: f32,
        hour: u8,
        minute: u8,
        second: f32,
    },
}

fn handle(input: &[u8]) -> Result<Value, String> {
    if input.len() > MAX_FRAME {
        return Err("Input exceeds 64 KiB".into());
    }
    let request: Request = serde_json::from_slice(input).map_err(|e| e.to_string())?;
    let state = STATE.get_or_init(|| Mutex::new(initial_state()));
    let mut state = state.lock().map_err(|_| "Canvas state unavailable")?;
    match request {
        Request::Snapshot {} => Ok(state.snapshot()),
        Request::Select {
            project,
            expected_revision,
        } => state.execute(EndpointRequest {
            schema_version: 1,
            request_id: "remote-select".into(),
            expected_revision: Some(expected_revision),
            command: Command::WorkspaceSwitch { name: project },
        }),
        Request::Frame {
            width,
            height,
            hour,
            minute,
            second,
        } => {
            if !width.is_finite()
                || !height.is_finite()
                || width < 180.0
                || height < 180.0
                || width > 4096.0
                || height > 4096.0
                || hour > 23
                || minute > 59
                || !second.is_finite()
                || !(0.0..60.0).contains(&second)
            {
                return Err("Invalid frame geometry or clock".into());
            }
            let theme = Theme::undergrowth();
            let mut scene = Scene::with_capacity(80);
            fpl_gfx::chrome::bar(&mut scene, Rect::new(0.0, 0.0, width, 72.0));
            AnalogClock {
                hour,
                minute,
                second,
            }
            .append(
                &mut scene,
                Rect::new(width * 0.28, 100.0, width * 0.44, height - 140.0),
            );
            let primitives: Vec<_> = scene.iter().map(|p| json!({
                "kind":p.kind as u32,"rect":[p.rect.x,p.rect.y,p.rect.width,p.rect.height],
                "radius":p.radius,"stroke":p.outline_width,"value":p.value,"rotation":p.rotation,
                "fill":theme.color(p.fill).to_srgb8(),"outline":theme.color(p.outline).to_srgb8()
            })).collect();
            let snapshot = state.snapshot();
            Ok(
                json!({"schema_version":1,"selected":snapshot["catalog"]["selected_project"],
                "background":theme.color(fpl_gfx::ThemeRole::Background).to_srgb8(),
                "text":theme.color(fpl_gfx::ThemeRole::Text).to_srgb8(),"primitives":primitives}),
            )
        }
    }
}

/// Copies one JSON response into caller-owned storage. Never retains pointers.
/// Returns written length, -1 for invalid pointers/bounds, -2 for insufficient
/// output capacity, or -3 for an unexpected panic. Errors are explicit JSON.
///
/// # Safety
/// Input must reference `input_len` readable bytes and output must reference
/// `output_capacity` writable bytes. Regions must not overlap, and must remain
/// valid for this call. The caller owns both buffers and their deallocation.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn canvas_tvos_request(
    input: *const u8,
    input_len: usize,
    output: *mut u8,
    output_capacity: usize,
) -> isize {
    if input.is_null() || output.is_null() || input_len > MAX_FRAME || output_capacity > MAX_FRAME {
        return -1;
    }
    let result = std::panic::catch_unwind(|| {
        // SAFETY: the C caller guarantees readable input for this bounded call.
        let bytes = unsafe { std::slice::from_raw_parts(input, input_len) };
        let response = match handle(bytes) {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":error}),
        };
        serde_json::to_vec(&response).expect("JSON Value serialization")
    });
    let Ok(bytes) = result else {
        return -3;
    };
    if bytes.len() > output_capacity {
        return -2;
    }
    // SAFETY: caller guarantees writable, nonoverlapping output capacity.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len());
    }
    bytes.len() as isize
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_and_clock_use_shared_core() {
        let state = initial_state();
        assert_eq!(state.snapshot()["catalog"]["selected_project"], "home");
        let value = handle(
            br#"{"action":"frame","width":1280,"height":720,"hour":12,"minute":30,"second":0}"#,
        )
        .unwrap();
        assert_eq!(value["primitives"].as_array().unwrap().len(), 66);
        assert!(
            handle(br#"{"action":"select","project":"missing","expected_revision":2}"#).is_err()
        );
    }
    #[test]
    fn invalid_and_oversized_frames_fail() {
        assert!(handle(&vec![b' '; MAX_FRAME + 1]).is_err());
        assert!(
            handle(br#"{"action":"frame","width":0,"height":720,"hour":25,"minute":0,"second":0}"#)
                .is_err()
        );
        assert!(handle(br#"{"action":"snapshot","extra":true}"#).is_err());
    }
    #[test]
    fn ffi_is_bounded_and_returns_explicit_errors() {
        let input = br#"{"action":"select","project":"missing","expected_revision":2}"#;
        let mut output = [0_u8; MAX_FRAME];
        let length = unsafe {
            canvas_tvos_request(
                input.as_ptr(),
                input.len(),
                output.as_mut_ptr(),
                output.len(),
            )
        };
        assert!(length > 0);
        let response: Value = serde_json::from_slice(&output[..length as usize]).unwrap();
        assert_eq!(response["ok"], false);
        assert_eq!(
            unsafe { canvas_tvos_request(input.as_ptr(), input.len(), output.as_mut_ptr(), 1) },
            -2
        );
        assert_eq!(
            unsafe { canvas_tvos_request(std::ptr::null(), 0, output.as_mut_ptr(), output.len()) },
            -1
        );
    }
    #[test]
    fn switching_uses_shared_revision_admission() {
        let mut state = initial_state();
        let switch = |revision| EndpointRequest {
            schema_version: 1,
            request_id: "test".into(),
            expected_revision: Some(revision),
            command: Command::WorkspaceSwitch {
                name: "studio".into(),
            },
        };
        assert!(state.execute(switch(1)).is_err());
        state.execute(switch(2)).unwrap();
        assert_eq!(state.snapshot()["catalog"]["selected_project"], "studio");
        assert!(state.execute(switch(2)).is_err());
    }
}
