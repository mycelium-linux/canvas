//! Project completed owner SPICE artifacts into bounded, explicit time samples.
//! No simulator, model resolution, or circuit interpretation belongs here.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};
fn read(path: &str) -> Result<(Vec<u8>, Value), Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    fs::File::open(path)?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("owner artifact exceeds 2MiB".into());
    }
    let value = serde_json::from_slice(&bytes)?;
    Ok((bytes, value))
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn validate_reference(request: &Value) -> Result<(), String> {
    if request["source_sha256"]
        != "a677e9eb780cd584e88d34e47c010a7b6626dee267a36f4b0a3be2beec373ded"
        || request["action"]
            != json!({"kind":"transient","from_v":0.0,"to_v":5.0,"step_at_s":0.0001,"step_s":0.000001,"stop_s":0.003})
    {
        return Err("not the explicitly admitted RC reference and stimulus".into());
    }
    Ok(())
}
fn resample(points: &[[f64; 2]], count: usize) -> Result<Vec<f32>, String> {
    if points.len() < 2
        || points.len() > 100_000
        || !(2..=4096).contains(&count)
        || points
            .iter()
            .any(|p| !p.iter().all(|v| v.is_finite()) || p[1].abs() > 1e6)
        || points.windows(2).any(|p| p[1][0] <= p[0][0])
    {
        return Err("invalid owner time series".into());
    }
    let first = points[0][0];
    let last = points[points.len() - 1][0];
    if !(last - first).is_finite() {
        return Err("time span overflow".into());
    }
    let mut index = 0;
    (0..count)
        .map(|i| {
            let time = first + (last - first) * i as f64 / (count - 1) as f64;
            while index + 2 < points.len() && points[index + 1][0] < time {
                index += 1;
            }
            let [t0, v0] = points[index];
            let [t1, v1] = points[index + 1];
            let sample = (v0 + (v1 - v0) * (time - t0) / (t1 - t0)) as f32;
            if !sample.is_finite() {
                return Err("resampled signal overflow".into());
            }
            Ok(sample)
        })
        .collect()
}
fn project(
    execution_bytes: &[u8],
    execution: &Value,
    receipt: &Value,
    request_bytes: &[u8],
    request: &Value,
    image_sha256: &str,
    simulator_version: &str,
) -> Result<Value, String> {
    validate_reference(request)?;
    if image_sha256.len() != 64
        || !image_sha256
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || simulator_version.is_empty()
        || simulator_version.len() > 32
    {
        return Err("invalid operator-recorded tool pin".into());
    }
    if receipt["schema"] != "lob.experiment.result.v1"
        || request["schema"] != "lob.experiment.v1"
        || request["action"]["kind"] != "transient"
        || execution["state"] != "complete"
        || receipt["state"] != "complete"
        || receipt["execution"] != *execution
        || receipt["request_sha256"] != digest(request_bytes)
        || receipt["source_sha256"] != request["source_sha256"]
    {
        return Err("owner receipt/request/execution mismatch or incomplete run".into());
    }
    let netlist = request["netlist"]
        .as_str()
        .ok_or("missing canonical source")?;
    if request["source_sha256"] != digest(netlist.as_bytes()) {
        return Err("canonical circuit digest mismatch".into());
    }
    if !receipt["artifacts"].as_array().is_some_and(|a| {
        a.iter()
            .any(|a| a["path"] == "execution.json" && a["sha256"] == digest(execution_bytes))
    }) {
        return Err("execution digest absent or mismatched".into());
    }
    let points: Vec<[f64; 2]> = serde_json::from_value(execution["payload"]["points"].clone())
        .map_err(|e| e.to_string())?;
    let samples = resample(&points, 1024)?;
    let start = points[0][0];
    let stop = points[points.len() - 1][0];
    Ok(
        json!({"schema":"canvas.spice-trace.v1","reference_id":"owned-rc-lowpass-v1","run_state":"complete","scope":"owner reference circuit, not LED-Blinker-Circuit","probe":"OUT","source_sha256":request["source_sha256"],"request_sha256":receipt["request_sha256"],"worker_sha256":receipt["worker_sha256"],"image_sha256":image_sha256,"simulator":"ngspice","simulator_version":simulator_version,"tool_pin_provenance":"operator-recorded pinned run environment","source_points":points.len(),"resampling":"piecewise-linear, evenly spaced over complete owner time interval","start_s":start,"stop_s":stop,"sample_period_s":(stop-start)/1023.0,"stimulus":request["action"],"range":{"minimum":0,"maximum":5.5},"samples":samples,"rise_time_s":execution["payload"]["rise_time_s"],"elapsed_ms":receipt["elapsed_ms"]}),
    )
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 6 {
        return Err("usage: canvas-spice-projection EXECUTION RECEIPT REQUEST OUTPUT IMAGE_SHA256 NGSPICE_VERSION".into());
    }
    let (execution_bytes, execution) = read(&args[0])?;
    let (_, receipt) = read(&args[1])?;
    let (request_bytes, request) = read(&args[2])?;
    let result = project(
        &execution_bytes,
        &execution,
        &receipt,
        &request_bytes,
        &request,
        &args[4],
        &args[5],
    )?;
    let output = Path::new(&args[3]);
    if output.exists() {
        return Err("projection output must be a new artifact".into());
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec(&result)?)?;
    println!(
        "projected {} owner points into 1024 uniform scope samples",
        result["source_points"]
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn irregular_source_has_explicit_uniform_interpolation() {
        assert_eq!(
            resample(&[[0., 0.], [0.1, 1.], [1., 1.]], 3).unwrap(),
            vec![0., 1., 1.]
        );
    }
    #[test]
    fn refuses_unknown_or_nonmonotonic_points() {
        assert!(resample(&[[0., 0.], [0., 1.]], 3).is_err());
        assert!(resample(&[[0., 0.], [1., f64::NAN]], 3).is_err());
        assert!(resample(&[[-f64::MAX, 0.], [f64::MAX, 1.]], 3).is_err());
    }
    #[test]
    fn another_circuit_or_stimulus_cannot_be_labeled_reference() {
        let mut request = json!({"source_sha256":"a677e9eb780cd584e88d34e47c010a7b6626dee267a36f4b0a3be2beec373ded","action":{"kind":"transient","from_v":0.0,"to_v":5.0,"step_at_s":0.0001,"step_s":0.000001,"stop_s":0.003}});
        assert!(validate_reference(&request).is_ok());
        request["action"]["to_v"] = json!(12.0);
        assert!(validate_reference(&request).is_err());
        request["source_sha256"] = json!("b".repeat(64));
        assert!(validate_reference(&request).is_err());
    }
    #[test]
    fn incomplete_receipt_cannot_become_a_trace() {
        assert!(project(
            b"",
            &json!({"state":"failed"}),
            &json!({}),
            b"",
            &json!({}),
            &"a".repeat(64),
            "42"
        )
        .is_err());
    }
    #[test]
    fn verified_reference_roundtrip_and_tamper_checks() {
        let source="(export (components (comp (ref R1) (value 1k)) (comp (ref C1) (value 159n))) (nets (net (name IN) (node (ref R1) (pin 1))) (net (name OUT) (node (ref R1) (pin 2)) (node (ref C1) (pin 1))) (net (name GND) (node (ref C1) (pin 2)))))";
        let request = json!({"schema":"lob.experiment.v1","netlist":source,"source_sha256":digest(source.as_bytes()),"action":{"kind":"transient","from_v":0.0,"to_v":5.0,"step_at_s":0.0001,"step_s":0.000001,"stop_s":0.003}});
        let request_bytes = serde_json::to_vec(&request).unwrap();
        // Synthetic unit fixture only; never published as measured data.
        let execution = json!({"state":"complete","payload":{"points":[[0.0,0.0],[0.001,1.0],[0.003,5.0]],"rise_time_s":0.001}});
        let execution_bytes = serde_json::to_vec(&execution).unwrap();
        let receipt = json!({"schema":"lob.experiment.result.v1","state":"complete","execution":execution,"request_sha256":digest(&request_bytes),"source_sha256":request["source_sha256"],"artifacts":[{"path":"execution.json","sha256":digest(&execution_bytes)}]});
        let pin = "a".repeat(64);
        let result = project(
            &execution_bytes,
            &execution,
            &receipt,
            &request_bytes,
            &request,
            &pin,
            "42",
        )
        .unwrap();
        assert_eq!(result["samples"].as_array().unwrap().len(), 1024);
        let mut changed_receipt = receipt.clone();
        changed_receipt["artifacts"][0]["sha256"] = json!("b".repeat(64));
        assert!(project(
            &execution_bytes,
            &execution,
            &changed_receipt,
            &request_bytes,
            &request,
            &pin,
            "42"
        )
        .is_err());
        let mut changed_request = request.clone();
        changed_request["netlist"] = json!("different circuit");
        assert!(project(
            &execution_bytes,
            &execution,
            &receipt,
            &request_bytes,
            &changed_request,
            &pin,
            "42"
        )
        .is_err());
        let mut failed = execution.clone();
        failed["state"] = json!("failed");
        assert!(project(
            &execution_bytes,
            &failed,
            &receipt,
            &request_bytes,
            &request,
            &pin,
            "42"
        )
        .is_err());
    }
}
