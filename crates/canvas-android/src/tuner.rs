//! Bounded private projection control. This is not tuner discovery or transport.
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    time::Duration,
};

/// Admission is enforced before any tuner I/O, not only by button visibility.
pub fn admit_surface(snapshot: &serde_json::Value, id: &str) -> Result<(), &'static str> {
    if snapshot["data"]["catalog"]["selected_project"] != "television"
        || snapshot["data"]["document"]["project_id"] != "television"
    {
        return Err("Tuner controls belong to the Television workspace");
    }
    let surfaces = snapshot["data"]["document"]["surfaces"]
        .as_array()
        .ok_or("Missing surfaces")?;
    if !surfaces
        .iter()
        .any(|surface| surface["id"] == id && surface["content"]["kind"] == "stream")
    {
        return Err("Tuner video surface unavailable");
    }
    Ok(())
}

pub fn request(base: &str, channel: Option<&str>) -> Result<serde_json::Value, String> {
    crate::validate_stream(base).map_err(str::to_string)?;
    let mut url = url::Url::parse(base).map_err(|e| e.to_string())?;
    if url.path() != "/" || url.query().is_some() {
        return Err("Expected a private control origin".into());
    }
    let body = if let Some(channel) = channel {
        if channel.len() > 16
            || channel.is_empty()
            || !channel.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        {
            return Err("Invalid channel".into());
        }
        serde_json::json!({"channel":channel}).to_string()
    } else {
        String::new()
    };
    url.set_path(if channel.is_some() {
        "/v1/tune"
    } else {
        "/v1/channels"
    });
    let address: SocketAddr = format!(
        "{}:{}",
        url.host_str().ok_or("Missing host")?,
        url.port_or_known_default().ok_or("Missing port")?
    )
    .parse()
    .map_err(|_| "Invalid address")?;
    let mut socket =
        TcpStream::connect_timeout(&address, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(22)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    write!(socket,"{} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",if channel.is_some(){"POST"}else{"GET"},url.path(),address,body.len(),body).map_err(|e|e.to_string())?;
    let mut response = Vec::new();
    socket
        .take(131073)
        .read_to_end(&mut response)
        .map_err(|e| e.to_string())?;
    if response.len() > 131072 {
        return Err("Control response too large".into());
    }
    let offset = response
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or("Invalid HTTP response")?;
    let headers = std::str::from_utf8(&response[..offset]).map_err(|_| "Invalid headers")?;
    let value: serde_json::Value =
        serde_json::from_slice(&response[offset + 4..]).map_err(|e| e.to_string())?;
    if !headers.starts_with("HTTP/1.1 200 ") && !headers.starts_with("HTTP/1.0 200 ") {
        return Err(value["error"]
            .as_str()
            .unwrap_or("Tuner control failed")
            .into());
    }
    if value["schema"] != "unibus.hdhomerun.hls.v1" {
        return Err("Unexpected tuner contract".into());
    }
    if channel.is_some() {
        crate::validate_stream(value["url"].as_str().ok_or("Missing playback URL")?)
            .map_err(str::to_string)?;
        if value["channel"].as_str() != channel {
            return Err("Unexpected channel reply".into());
        }
    }
    Ok(value)
}

/// Use the renderer's existing authenticated control socket; never mirror its
/// workspace state inside the independently built tuner library.
#[cfg(all(target_os = "android", feature = "android-tuner-control"))]
fn presentation(directory: &str, command: serde_json::Value) -> Result<serde_json::Value, String> {
    use std::io::BufRead;
    let token = std::fs::read_to_string(std::path::Path::new(directory).join("control-token"))
        .map_err(|e| e.to_string())?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Invalid local control credential".into());
    }
    let mut socket = TcpStream::connect_timeout(
        &SocketAddr::from(([127, 0, 0, 1], crate::CONTROL_PORT)),
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    writeln!(socket, "{token}\n{command}").map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    std::io::BufReader::new(socket)
        .take(4 * 1024 * 1024 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 4 * 1024 * 1024 || bytes.last() != Some(&b'\n') {
        return Err("Invalid presentation receipt".into());
    }
    let receipt: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if receipt["ok"] != true {
        return Err(receipt["error"]
            .as_str()
            .unwrap_or("Presentation command failed")
            .into());
    }
    Ok(receipt)
}

#[cfg(all(target_os = "android", feature = "android-tuner-control"))]
#[unsafe(no_mangle)]
pub extern "system" fn Java_dev_fpl_canvas_TvActivity_nativeTunerControl(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    base: jni::objects::JString,
    channel: jni::objects::JString,
    surface: jni::objects::JString,
    directory: jni::objects::JString,
) -> jni::sys::jstring {
    let result = (|| -> Result<serde_json::Value, String> {
        let base = String::from(env.get_string(&base).map_err(|e| e.to_string())?);
        let channel = String::from(env.get_string(&channel).map_err(|e| e.to_string())?);
        let id = String::from(env.get_string(&surface).map_err(|e| e.to_string())?);
        let directory = String::from(env.get_string(&directory).map_err(|e| e.to_string())?);
        let inspect = serde_json::json!({"schema_version":1,"request_id":"tv-channel-inspect","command":{"command":"inspect"}});
        let before = presentation(&directory, inspect.clone())?;
        admit_surface(&before, &id)?;
        let value = request(&base, (!channel.is_empty()).then_some(channel.as_str()))?;
        if !channel.is_empty() {
            let current = presentation(&directory, inspect)?;
            admit_surface(&current, &id)?;
            let document = &current["data"]["document"];
            if document["id"] != before["data"]["document"]["id"] {
                return Err("Channel tuned, but workspace changed".into());
            }
            let mut target = document["surfaces"]
                .as_array()
                .ok_or("Missing surfaces")?
                .iter()
                .find(|s| s["id"] == id && s["content"]["kind"] == "stream")
                .ok_or("Video surface unavailable")?
                .clone();
            let previous = before["data"]["document"]["surfaces"]
                .as_array()
                .ok_or("Missing surfaces")?
                .iter()
                .find(|s| s["id"] == id)
                .ok_or("Video surface changed")?;
            if &target != previous {
                return Err("Channel tuned, but video surface changed".into());
            }
            target["content"]["url"] = value["url"].clone();
            target["title"] = serde_json::json!(format!(
                "{} · {}",
                channel,
                value["name"].as_str().unwrap_or("TV")
            ));
            let revision = document["revision"]
                .as_u64()
                .ok_or("Missing document revision")?;
            presentation(
                &directory,
                serde_json::json!({"schema_version":1,"request_id":"tv-channel","expected_revision":current["revision"],"command":{"command":"apply_patch","patch":{"schema_version":1,"workspace_id":document["id"],"base_revision":revision,"revision":revision+1,"operations":[{"op":"upsert_surface","surface":target}]}}}),
            )?;
        }
        Ok(value)
    })();
    let value = match result {
        Ok(v) => v,
        Err(e) => serde_json::json!({"error":e}),
    };
    env.new_string(value.to_string())
        .map(|s| s.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_non_tv_workspaces_even_with_a_valid_video_surface() {
        for project in ["holodeck", "airplay", "studio", "security"] {
            let snapshot = serde_json::json!({"data":{"catalog":{"selected_project":project},
                "document":{"project_id":project,"surfaces":[{"id":"video","content":{"kind":"stream"}}]}}});
            assert!(admit_surface(&snapshot, "video").is_err());
        }
        let mut snapshot = serde_json::json!({"data":{"catalog":{"selected_project":"television"},
            "document":{"project_id":"television","surfaces":[{"id":"komo4","content":{"kind":"stream"}}]}}});
        assert!(admit_surface(&snapshot, "komo4").is_ok());
        assert!(admit_surface(&snapshot, "missing").is_err());
        snapshot["data"]["document"]["project_id"] = serde_json::json!("holodeck");
        assert!(admit_surface(&snapshot, "komo4").is_err());
    }
    #[test]
    fn rejects_non_origin_and_untrusted_control() {
        for base in [
            "http://8.8.8.8/",
            "http://192.168.2.67/path",
            "http://user@192.168.2.67/",
        ] {
            assert!(request(base, None).is_err());
        }
        assert!(request("http://192.168.2.67:18090/", Some("4.1;reboot")).is_err());
    }
}
