use canvas_core::stream_socket_path;
use canvas_eval::{CANVAS_CONTRACT, SseDecoder, decode_snapshot, payload_for_stream};
use serde_json::json;
use std::{
    io::Read,
    os::unix::net::UnixDatagram,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|value| value == name)
        .and_then(|index| args.get(index + 1))
        .cloned()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn main() -> Result<(), String> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let endpoint = arg(&args, "--endpoint").ok_or("--endpoint is required")?;
    let project = arg(&args, "--project").unwrap_or_else(|| "eval".into());
    let owner = arg(&args, "--owner").unwrap_or_else(|| "eval-studio".into());
    let socket = arg(&args, "--socket")
        .map(PathBuf::from)
        .or_else(stream_socket_path)
        .ok_or("cannot resolve Canvas stream socket; pass --socket")?;
    let once = args.iter().any(|value| value == "--once");
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;
    let datagram = UnixDatagram::unbound().map_err(|e| e.to_string())?;
    let streams = [
        "eval-summary",
        "eval-tree",
        "eval-decisions",
        "eval-events",
        "eval-verification",
    ];
    let mut sequence = 0_u64;
    let mut last_event_id: Option<String> = None;
    loop {
        let mut request = client.get(&endpoint).header("Accept", "text/event-stream");
        if let Some(id) = &last_event_id {
            request = request.header("Last-Event-ID", id);
        }
        match request
            .send()
            .and_then(|response| response.error_for_status())
        {
            Ok(mut response) => {
                let mut decoder = SseDecoder::default();
                let mut chunk = [0_u8; 32 * 1024];
                loop {
                    let read = response.read(&mut chunk).map_err(|e| e.to_string())?;
                    if read == 0 {
                        break;
                    }
                    for (id, event, data) in decoder.push(&chunk[..read])? {
                        let snapshot = decode_snapshot(&event, &data)?;
                        let payload = snapshot.project();
                        sequence = sequence.saturating_add(1);
                        for stream in streams {
                            let stream_payload = payload_for_stream(&payload, stream);
                            let envelope = json!({"schema_version":1,"owner":owner,"stream":stream,"contract":CANVAS_CONTRACT,"sequence":sequence,"emitted_at_ms":now_ms(),"project_id":project,"payload":stream_payload});
                            let bytes = serde_json::to_vec(&envelope).map_err(|e| e.to_string())?;
                            if bytes.len() > 2_048 {
                                return Err(format!(
                                    "projected {stream} snapshot is {} bytes; Canvas datagram limit is 2048",
                                    bytes.len()
                                ));
                            }
                            datagram
                                .send_to(&bytes, &socket)
                                .map_err(|e| format!("send {stream}: {e}"))?;
                            std::thread::sleep(Duration::from_millis(2));
                        }
                        last_event_id = id;
                        if once {
                            return Ok(());
                        }
                    }
                }
            }
            Err(error) => eprintln!("canvas-eval: {error}"),
        }
        if once {
            return Err("eval SSE ended before a snapshot".into());
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
