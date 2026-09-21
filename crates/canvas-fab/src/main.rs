use canvas_fab::{OWNER, fit_event, stream_payloads};
use canvas_feeds::{
    CONTRACT, ENVELOPE_OVERHEAD, Http, MAX_DATAGRAM, ReqwestHttp, SCHEMA_VERSION, now_ms, send_raw,
};
use serde_json::json;
use std::{env, process::ExitCode, thread, time::Duration};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|value| value == flag)
        .and_then(|index| args.get(index + 1).cloned())
}

fn main() -> ExitCode {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let url = arg(&args, "--url").unwrap_or_else(|| "http://127.0.0.1:7788/api/overview".into());
    let every = arg(&args, "--every")
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10)
        .max(2);
    let http = ReqwestHttp::default();
    eprintln!("canvas-fab: {url} every {every}s");
    loop {
        match http.get_json(&url) {
            Ok(overview) => {
                let emitted = now_ms();
                for (stream, payload) in stream_payloads(&overview) {
                    let event = json!({
                        "schema_version": SCHEMA_VERSION,
                        "owner": OWNER,
                        "stream": stream,
                        "contract": CONTRACT,
                        "sequence": emitted,
                        "emitted_at_ms": emitted,
                        "project_id": "fab",
                        "payload": payload,
                    });
                    let (event, _) =
                        fit_event(event, MAX_DATAGRAM.saturating_sub(ENVELOPE_OVERHEAD));
                    if let Err(error) = send_raw(&event) {
                        eprintln!("canvas-fab: {stream} send failed: {error}");
                    }
                }
            }
            Err(error) => eprintln!("canvas-fab: fetch failed: {error}"),
        }
        thread::sleep(Duration::from_secs(every));
    }
}
