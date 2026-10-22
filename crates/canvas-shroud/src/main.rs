use canvas_feeds::{
    CONTRACT, ENVELOPE_OVERHEAD, Http, MAX_DATAGRAM, ReqwestHttp, SCHEMA_VERSION, now_ms, send_raw,
};
use canvas_shroud::{OWNER, fit_event, stream_payloads, unavailable_payloads};
use serde_json::{Value, json};
use std::{env, process::ExitCode, thread, time::Duration};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| args.get(index + 1).cloned())
}

fn emit(stream: &str, payload: Value, emitted: u64) {
    let event = json!({
        "schema_version":SCHEMA_VERSION,"owner":OWNER,"stream":stream,"contract":CONTRACT,
        "sequence":emitted,"emitted_at_ms":emitted,"project_id":"shroud","payload":payload,
    });
    let event = fit_event(event, MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD));
    if let Err(error) = send_raw(&event) {
        eprintln!("canvas-shroud: {stream} send failed: {error}");
    }
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let url = arg(&args, "--url").unwrap_or_else(|| "http://127.0.0.1:9108/metrics".into());
    let region = arg(&args, "--region").unwrap_or_else(|| "agora-one".into());
    let every = arg(&args, "--every")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10)
        .max(2);
    let http = ReqwestHttp::default();
    eprintln!("canvas-shroud: {region} from {url} every {every}s");
    loop {
        let emitted = now_ms();
        let packets = match http.get_text(&url) {
            Ok(metrics) => stream_payloads(&metrics, &region),
            Err(error) => unavailable_payloads(&region, &error.to_string()),
        };
        for (stream, payload) in packets {
            emit(stream, payload, emitted);
        }
        thread::sleep(Duration::from_secs(every));
    }
}
