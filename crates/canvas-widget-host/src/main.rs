//! Canvas-owned bounded Lua composition and presentation delivery, not provider IO.
#[cfg(feature = "luau")]
use mlua::LuaSerdeExt;
#[cfg(feature = "luau")]
use std::io::{BufRead, Read, Write};
#[cfg(not(feature = "luau"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    Err("widget_projection requires bounded Luau runtime".into())
}
#[cfg(feature = "luau")]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<_> = std::env::args().skip(1).collect();
    let retained_file = if args.first().is_some_and(|s| s == "watch-tv-retained") {
        if args.len() != 7 {
            return Err("usage: canvas-widget-host watch-tv-retained INPUT BINDER TVCTL ENDPOINT PROJECT SURFACE".into());
        }
        let path = args.remove(1);
        args[0] = "stream-tv-retained".into();
        Some(path)
    } else {
        None
    };
    if args.first().is_some_and(|s| s == "stream-tv-retained") {
        if args.len() != 6 {
            return Err("usage: canvas-widget-host stream-tv-retained BINDER TVCTL ENDPOINT PROJECT SURFACE".into());
        }
        let mut stdin = std::io::stdin().lock();
        loop {
            let mut line = Vec::new();
            if let Some(path) = &retained_file {
                std::thread::sleep(std::time::Duration::from_secs(5));
                std::fs::File::open(path)?
                    .take(64 * 1024 + 1)
                    .read_to_end(&mut line)?;
                if line.is_empty() || line.len() > 64 * 1024 {
                    return Err("invalid retained observation size".into());
                }
            } else {
                let count = stdin
                    .by_ref()
                    .take(64 * 1024 + 1)
                    .read_until(b'\n', &mut line)?;
                if count == 0 {
                    return Ok(());
                }
                if count > 64 * 1024 || !line.ends_with(b"\n") {
                    return Err("oversized/incomplete observation".into());
                }
            }
            let result = compose(&args[1], &line)?;
            let widget = result["widget"]
                .as_object()
                .ok_or("missing composed widget")?;
            // Older endpoints support live StatusList snapshots but not RichCard.
            // Replace only this retained widget, with a revision-bound owner import.
            let mut accepted = false;
            for attempt in 0..3 {
                let read = tv_call(
                    &args[2],
                    &args[3],
                    &serde_json::json!({"schema_version":1,"request_id":"widget-retained-read","command":{"command":"workspace_read","name":args[4]}}),
                )?;
                if read["ok"] != true {
                    return Err("TV rejected workspace read".into());
                }
                let mut document = read["data"].clone();
                let surfaces = document["surfaces"]
                    .as_array_mut()
                    .ok_or("missing workspace surfaces")?;
                let surface = surfaces
                    .iter_mut()
                    .find(|surface| surface["id"] == args[5])
                    .ok_or("retained surface not declared")?;
                let spec = surface["content"]["spec"]
                    .as_object_mut()
                    .ok_or("retained surface is not a widget")?;
                if spec.get("kind") != widget.get("kind") {
                    return Err("retained widget kind change rejected".into());
                }
                for (key, value) in widget {
                    if !matches!(key.as_str(), "id" | "schema_version") {
                        spec.insert(key.clone(), value.clone());
                    }
                }
                spec.insert("data".into(), result["data"].clone());
                document["revision"] = (document["revision"]
                    .as_u64()
                    .ok_or("invalid document revision")?
                    + 1)
                .into();
                let reply = tv_call(
                    &args[2],
                    &args[3],
                    &serde_json::json!({"schema_version":1,"request_id":format!("widget-retained-{}-{attempt}",chrono::Utc::now().timestamp_millis()),"expected_revision":read["revision"],"command":{"command":"workspace_import","document":document,"activate":false}}),
                )?;
                if reply["ok"] == true {
                    accepted = true;
                    break;
                }
                if !reply["error"]
                    .as_str()
                    .is_some_and(|e| e.contains("revision"))
                {
                    return Err("TV rejected retained widget update".into());
                }
            }
            if !accepted {
                return Err("TV retained update conflicted three times".into());
            }
            println!("delivered retained {} {}", args[4], args[5]);
            std::io::stdout().flush()?;
        }
    }
    if args.first().is_some_and(|s| s == "stream-tv") {
        if args.len() != 6 {
            return Err(
                "usage: canvas-widget-host stream-tv BINDER TVCTL ENDPOINT WORKSPACE SURFACE"
                    .into(),
            );
        }
        let mut stdin = std::io::stdin().lock();
        loop {
            let mut line = Vec::new();
            let count = stdin
                .by_ref()
                .take(64 * 1024 + 1)
                .read_until(b'\n', &mut line)?;
            if count == 0 {
                return Ok(());
            }
            if count > 64 * 1024 || !line.ends_with(b"\n") {
                return Err("oversized/incomplete observation".into());
            }
            let result = compose(&args[1], &line)?;
            let mut frame = result["widget"].clone();
            frame
                .as_object_mut()
                .ok_or("invalid widget frame")?
                .remove("kind");
            let request = serde_json::json!({"schema_version":1,"request_id":format!("widget-{}",chrono::Utc::now().timestamp_millis()),"command":{"command":"surface_snapshot","workspace_id":args[4],"surface_id":args[5],"frame":frame,"data":result["data"]}});
            let mut child = std::process::Command::new(&args[2])
                .arg(&args[3])
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            child
                .stdin
                .take()
                .ok_or("missing TV input")?
                .write_all(&serde_json::to_vec(&request)?)?;
            let mut stdout = child.stdout.take().ok_or("missing TV reply")?;
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let read = stdout.by_ref().take(64 * 1024 + 1).read_to_end(&mut bytes);
                let _ = tx.send(read.map(|_| bytes));
            });
            let response = match rx.recv_timeout(std::time::Duration::from_secs(5)) {
                Ok(bytes) => bytes?,
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("TV delivery deadline exceeded".into());
                }
            };
            let status = child.wait()?;
            if response.len() > 64 * 1024 || !status.success() {
                return Err("TV delivery failed".into());
            }
            let reply: serde_json::Value = serde_json::from_slice(&response)?;
            // The legacy TV owner receipt is {ok, revision, data}, without a
            // request ID. Each trusted local CLI invocation owns one connection.
            if reply["ok"] != true || !reply["revision"].is_u64() {
                return Err("TV rejected widget delivery".into());
            }
            println!("delivered {} {}", args[4], args[5]);
            std::io::stdout().flush()?;
        }
    }
    let path = args
        .first()
        .ok_or("usage: canvas-widget-host BINDER.lua < OBSERVATION.json")?;
    let mut input = Vec::new();
    std::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_end(&mut input)?;
    if input.len() > 64 * 1024 {
        return Err("observation exceeds 64 KiB".into());
    }
    println!("{}", compose(path, &input)?);
    Ok(())
}
#[cfg(feature = "luau")]
fn tv_call(
    program: &str,
    endpoint: &str,
    request: &serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    canvas_widget_host::presentation::call(program, endpoint, request)
}
#[cfg(feature = "luau")]
fn compose(path: &str, input: &[u8]) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let script = std::fs::read(path)?;
    if script.len() > 64 * 1024 {
        return Err("binder exceeds 64 KiB".into());
    }
    compose_script(&script, input)
}
#[cfg(feature = "luau")]
fn compose_script(
    script: &[u8],
    input: &[u8],
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let value: serde_json::Value = serde_json::from_slice(input)?;
    let lua = mlua::Lua::new();
    lua.set_memory_limit(1024 * 1024)?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
    lua.set_interrupt(move |_| {
        if std::time::Instant::now() >= deadline {
            Err(mlua::Error::RuntimeError(
                "widget composition deadline exceeded".into(),
            ))
        } else {
            Ok(mlua::VmState::Continue)
        }
    });
    let module: mlua::Table = lua.load(script).eval()?;
    let render: mlua::Function = module.get("render")?;
    let now = chrono::Utc::now().timestamp_millis();
    let output: mlua::Value = render.call((lua.to_value(&value)?, now))?;
    let mut result: serde_json::Value = lua.from_value(output)?;
    if let Some(stamp) = result["data"]["observed_at_unix_ms"].as_i64() {
        result["data"]
            .as_object_mut()
            .ok_or("missing metadata")?
            .remove("observed_at_unix_ms");
        result["data"]["observed_at"] = chrono::DateTime::from_timestamp_millis(stamp)
            .ok_or("invalid observation time")?
            .to_rfc3339()
            .into();
    }
    let bytes = serde_json::to_vec(&result)?;
    if bytes.len() > 64 * 1024 {
        return Err("widget output exceeds 64 KiB".into());
    }
    Ok(result)
}

#[cfg(all(test, feature = "luau"))]
mod tests {
    use super::*;
    #[test]
    fn modeled_led_composes_the_shared_plot_without_transport_or_heartbeat() {
        let p = serde_json::json!({"schema":"canvas.modeled-trace.v1","source_sha256":"580061c37e6348d8e8c26eb0e8cb04e4440640d77cd2d84a119ab509dd393ca8","probe":{"part":"U1","pin":"3"},"supply":{"positive":"J1.2","ground":"J1.1","volts":5},"time":{"stop_s":1},"frame":{"range":{"minimum":0,"maximum":5}},"plot":{"x":{"label":"Time","unit":"s","minimum":0,"maximum":1},"y":{"label":"Voltage","unit":"V","minimum":0,"maximum":5},"series":[{"id":"output","label":"U1 pin3 output","role":"primary","points":[{"x":0,"y":0},{"x":1,"y":3}]}],"viewport":null},"frequency_hz":7.3,"rising_edges":7,"source_points":2,"simulator_version":"47"});
        let output = compose_script(
            include_bytes!("../../../config/binders/spice-modeled-v1.lua"),
            &serde_json::to_vec(&p).unwrap(),
        )
        .unwrap();
        assert_eq!(output["widget"]["frame"]["view"], "plot");
        assert_eq!(output["widget"]["frame"]["plot"], p["plot"]);
        assert_eq!(output["widget"]["data"]["provenance"], "sample");
        assert!(output["widget"]["data"].get("observed_at").is_none());
        let mut bad = p;
        bad["source_sha256"] = serde_json::json!("other");
        assert!(compose_script(
            include_bytes!("../../../config/binders/spice-modeled-v1.lua"),
            &serde_json::to_vec(&bad).unwrap()
        )
        .is_err());
    }
    #[test]
    fn reference_scope_is_recorded_not_live_and_rejects_other_circuits() {
        let value = serde_json::json!({"schema":"canvas.spice-trace.v1","reference_id":"owned-rc-lowpass-v1","run_state":"complete","scope":"owner reference circuit, not LED-Blinker-Circuit","source_sha256":"a677e9eb780cd584e88d34e47c010a7b6626dee267a36f4b0a3be2beec373ded","probe":"OUT","samples":[0,1,5],"start_s":0,"stop_s":0.003,"sample_period_s":0.0015,"range":{"minimum":0,"maximum":5.5},"source_points":3018,"rise_time_s":0.000349358,"simulator_version":"42"});
        let script = include_bytes!("../../../config/binders/spice-reference-v1.lua");
        let result = compose_script(script, &serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(result["widget"]["frame"]["samples"], value["samples"]);
        assert_eq!(result["data"]["provenance"], "sample");
        assert!(result["data"].get("observed_at").is_none());
        let mut wrong = value;
        wrong["reference_id"] = serde_json::json!("another-circuit");
        assert!(compose_script(script, &serde_json::to_vec(&wrong).unwrap()).is_err());
    }
    #[test]
    fn showcase_does_not_label_an_unacknowledged_request() {
        let script = include_bytes!("../../../config/binders/model-showcase-v1.lua");
        let value = serde_json::json!({"schema":"canvas.model-showcase.v1","catalog_count":954,"observed_at_unix_ms":chrono::Utc::now().timestamp_millis(),"failed_previews":0,"identity_state":"legacy_scene_requested_no_renderer_ack","current":{"sha256":"a".repeat(64),"step":"/owner/model/result/rebuilt.step","geometry":0.0,"training_admitted":false}});
        assert!(compose_script(script, &serde_json::to_vec(&value).unwrap()).is_err());
    }
    #[test]
    fn showcase_waits_without_inventing_an_identity() {
        let value = serde_json::json!({"schema":"canvas.model-showcase.v1","catalog_count":954,"observed_at_unix_ms":chrono::Utc::now().timestamp_millis(),"failed_previews":0,"identity_state":"renderer_acknowledged","current":null});
        let result = compose_script(
            include_bytes!("../../../config/binders/model-showcase-v1.lua"),
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert_eq!(result["data"]["state"], "no_data");
        assert_eq!(result["widget"]["title"], "Verk · preparing geometry");
    }
    #[test]
    fn showcase_projects_recorded_features_without_overwriting_array_schema() {
        let value = serde_json::json!({"schema":"canvas.model-showcase.v1","catalog_count":954,"observed_at_unix_ms":chrono::Utc::now().timestamp_millis(),"failed_previews":0,"identity_state":"renderer_acknowledged","current":{"sha256":"a".repeat(64),"step":"/owner/holdout-123-ABC/result/rebuilt.step","feature_counts":{"bore":2,"prismatic_pocket":1},"geometry":0,"training_admitted":false}});
        let result = compose_script(
            include_bytes!("../../../config/binders/model-showcase-v1.lua"),
            &serde_json::to_vec(&value).unwrap(),
        )
        .unwrap();
        assert!(result["widget"]["body"]
            .as_str()
            .unwrap()
            .contains("bore × 2"));
        assert!(result["widget"].get("citations").is_none());
        assert_eq!(result["data"]["state"], "ready");
    }
    #[test]
    fn preserves_observation_clock_not_render_time() {
        let result=compose_script(br#"return {render=function(value,now) return {widget={kind='status_list',title='Evidence'},data={observed_at_unix_ms=value.observed_at_unix_ms}} end}"#,br#"{"observed_at_unix_ms":1000}"#).unwrap();
        assert_eq!(result["data"]["observed_at"], "1970-01-01T00:00:01+00:00");
        assert!(result["data"].get("observed_at_unix_ms").is_none());
    }
    #[test]
    fn nonterminating_binder_is_bounded() {
        let start = std::time::Instant::now();
        let result = compose_script(b"return {render=function() while true do end end}", b"{}");
        assert!(result.is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }
    #[test]
    fn oversized_output_is_rejected() {
        assert!(compose_script(
            b"return {render=function() return {widget={body=string.rep('x',70000)}} end}",
            b"{}"
        )
        .is_err());
    }
}
