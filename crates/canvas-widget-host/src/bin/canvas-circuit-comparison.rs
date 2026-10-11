//! Compose one source-bound native/connectivity comparison with an invariant trace.
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs};
fn compose(
    trace: &Value,
    circuit: &Value,
    widgets: &Value,
    template: &Value,
    proof: &Value,
) -> Result<Vec<Value>, String> {
    if trace["schema"] != "canvas.modeled-trace.v1"
        || circuit["schema"] != "lob.circuit-observation.v1"
        || trace["source_sha256"] != circuit["source_sha256"]
    {
        return Err("trace/circuit provenance mismatch".into());
    }
    let parts = circuit["parts"].as_array().ok_or("missing parts")?;
    let nets = circuit["nets"].as_array().ok_or("missing nets")?;
    if proof["header"]["source_sha256"] != trace["source_sha256"]
        || proof["footer"]["checks"]["physical_pin_graph_exact"] != true
        || proof["footer"]["checks"]["complete_tree_replay"] != true
    {
        return Err("exact replay evidence mismatch".into());
    }
    let mut observed = Vec::new();
    for net in nets {
        let mut pins = Vec::new();
        for p in net["pins"].as_array().ok_or("missing pins")? {
            pins.push((
                p["ref"].as_str().ok_or("missing ref")?.to_owned(),
                p["pin"].as_str().ok_or("missing pin")?.to_owned(),
            ));
        }
        pins.sort();
        observed.push(pins);
    }
    observed.sort();
    let mut expected: Vec<Vec<(String, String)>> =
        serde_json::from_value(proof["header"]["target_graph"]["physical_pin_groups"].clone())
            .map_err(|e| e.to_string())?;
    for group in &mut expected {
        group.sort();
    }
    expected.sort();
    if observed != expected {
        return Err("reconstructed connectivity differs from verified target".into());
    }
    let mut owner_parts = proof["header"]["target_graph"]["parts"]
        .as_array()
        .ok_or("missing verified parts")?
        .iter()
        .map(|p| {
            (
                p["refdes"].as_str().unwrap_or(""),
                p["value"].as_str().unwrap_or(""),
            )
        })
        .collect::<Vec<_>>();
    owner_parts.sort();
    let mut run_parts = parts
        .iter()
        .map(|p| {
            (
                p["ref"].as_str().unwrap_or(""),
                p["value"].as_str().unwrap_or(""),
            )
        })
        .collect::<Vec<_>>();
    run_parts.sort();
    if owner_parts != run_parts {
        return Err("component inventory differs from verified reconstruction".into());
    }
    if parts.is_empty() || parts.len() + nets.len() > 64 {
        return Err("graph resource bound".into());
    }
    let mut names = BTreeSet::new();
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    for p in parts {
        let id = p["ref"].as_str().ok_or("missing part ref")?;
        if !names.insert(id) {
            return Err("duplicate part".into());
        }
        nodes.push(json!({"id":id,"label":format!("{} {}",id,p["value"].as_str().ok_or("missing value")?),"lane":0,"state":"configured","level":null}));
    }
    for (i, n) in nets.iter().enumerate() {
        let id = format!("net-{i}");
        let pins = n["pins"]
            .as_array()
            .ok_or("missing pins")?
            .iter()
            .map(|p| {
                format!(
                    "{}.{}",
                    p["ref"].as_str().unwrap_or("?"),
                    p["pin"].as_str().unwrap_or("?")
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        nodes.push(json!({"id":id,"label":format!("{} · {}",n["name"].as_str().ok_or("missing net name")?,pins),"lane":1,"state":"configured","level":null}));
        for pin in n["pins"].as_array().ok_or("missing pins")? {
            let part = pin["ref"].as_str().ok_or("missing pin ref")?;
            if !names.contains(part) {
                return Err("unknown connected part".into());
            }
            edges.push(json!({"source":part,"target":id,"state":"configured"}));
        }
    }
    if edges.len() > 128 {
        return Err("edge resource bound".into());
    }
    let data = json!({"state":"ready","provenance":"sample","source":format!("Owner circuit observation {}",circuit["source_sha256"].as_str().unwrap())});
    let graph = json!({"kind":"widget","spec":{"schema_version":2,"id":"drawing","kind":"signal_view","title":"Reconstructed connectivity · not original layout","frame":{"view":"topology","graph":{"nodes":nodes,"edges":edges}},"chrome":"frameless","data":data}});
    let mut native = template.clone();
    native["id"] = json!("tv-verk-led");
    native["project_id"] = json!("verk-led");
    let asset = template["surfaces"]
        .as_array()
        .ok_or("missing template")?
        .iter()
        .find(|s| s["content"]["kind"] == "asset")
        .ok_or("native PDF missing")?["content"]
        .clone();
    native["surfaces"] = json!([
 {"id":"drawing","title":"Native LED schematic","inputs":[],"content":asset},
 {"id":"scope","title":"LED modeled experiment","inputs":[],"content":{"kind":"widget","spec":widgets["widget"]}},
 {"id":"evidence","title":"Run evidence","inputs":[],"content":{"kind":"widget","spec":widgets["evidence"]}}]);
    let body = native["surfaces"][2]["content"]["spec"]["body"]
        .as_str()
        .ok_or("missing evidence caption")?;
    native["surfaces"][2]["content"]["spec"]["body"]=json!(format!("{}\n\nExact physical-pin graph verified\n{} parts · {} nets · {} pins\nEdges show connectivity, not direction",body,parts.len(),nets.len(),observed.iter().map(Vec::len).sum::<usize>()));
    native["layout"] = json!({"kind":"canvas","placements":[{"surface_id":"drawing","bounds":{"x":0.01,"y":0.01,"width":0.65,"height":0.57},"layer":0},{"surface_id":"scope","bounds":{"x":0.01,"y":0.60,"width":0.65,"height":0.39},"layer":0},{"surface_id":"evidence","bounds":{"x":0.68,"y":0.01,"width":0.31,"height":0.98},"layer":1}]});
    let mut reconstructed = native.clone();
    reconstructed["surfaces"][0]["content"] = graph;
    reconstructed["surfaces"][0]["title"] = json!("Reconstructed connectivity");
    Ok(vec![native, reconstructed])
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args().skip(1).collect();
    if !(6..=8).contains(&a.len()) {
        return Err(
            "usage: canvas-circuit-comparison TRACE CIRCUIT WIDGETS NATIVE_WORKSPACE PROOF OUTPUT [CONNECTIVITY_SVG [PLOT_SVG]]"
                .into(),
        );
    }
    let read = |p: &str| -> Result<Value, Box<dyn std::error::Error>> {
        let b = fs::read(p)?;
        if b.len() > 2 * 1024 * 1024 {
            return Err("artifact too large".into());
        }
        Ok(serde_json::from_slice(&b)?)
    };
    let docs = compose(
        &read(&a[0])?,
        &read(&a[1])?,
        &read(&a[2])?,
        &read(&a[3])?,
        &read(&a[4])?,
    )?;
    use std::io::Write;
    fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&a[5])?
        .write_all(&serde_json::to_vec(&docs)?)?;
    if a.len() >= 7 {
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&a[6])?
            .write_all(connectivity_svg(&read(&a[1])?)?.as_bytes())?;
    }
    if a.len() == 8 {
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&a[7])?
            .write_all(trace_svg(&read(&a[0])?)?.as_bytes())?;
    }
    Ok(())
}
fn trace_svg(trace: &Value) -> Result<String, String> {
    let samples = trace["frame"]["samples"]
        .as_array()
        .ok_or("missing samples")?;
    let lo = trace["frame"]["range"]["minimum"]
        .as_f64()
        .ok_or("missing minimum")?;
    let hi = trace["frame"]["range"]["maximum"]
        .as_f64()
        .ok_or("missing maximum")?;
    let start = trace["start_s"].as_f64().ok_or("missing start")?;
    let stop = trace["stop_s"].as_f64().ok_or("missing stop")?;
    if !(2..=4096).contains(&samples.len())
        || ![lo, hi, start, stop].iter().all(|v| v.is_finite())
        || hi <= lo
        || stop <= start
    {
        return Err("invalid trace scale".into());
    }
    let mut svg = String::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="520" viewBox="0 0 1600 520"><rect width="1600" height="520" fill="white"/><g font-family="sans-serif" fill="black"><text x="140" y="38" font-size="30">U1 pin 3 output · completed modeled transient</text><path d="M1160 30 H1220" stroke="#087f8c" stroke-width="5"/><text x="1240" y="38" font-size="30">U1.3 output</text>"##,
    );
    for i in 0..=5 {
        let y = 70 + i * 66;
        let v = hi - (hi - lo) * i as f64 / 5.;
        svg.push_str(&format!(r##"<path d="M140 {y} H1540" stroke="#dddddd"/><text x="125" y="{}" font-size="28" text-anchor="end">{v:.2}</text>"##,y+9));
    }
    for i in 0..=10 {
        let x = 140 + i * 140;
        svg.push_str(&format!(r##"<path d="M{x} 70 V400" stroke="#dddddd"/><text x="{x}" y="443" font-size="28" text-anchor="middle">{:.1}</text>"##,start+(stop-start)*i as f64/10.));
    }
    svg.push_str(r##"<text x="840" y="492" font-size="32" text-anchor="middle">Time (s)</text><text x="35" y="240" font-size="32" text-anchor="middle" transform="rotate(-90 35 240)">Voltage (V)</text><polyline fill="none" stroke="#087f8c" stroke-width="4" points=""##);
    for (i, v) in samples.iter().enumerate() {
        let v = v.as_f64().ok_or("invalid sample")?;
        if !v.is_finite() || v < lo || v > hi {
            return Err("nonfinite sample".into());
        }
        let x = 140. + 1400. * i as f64 / (samples.len() - 1) as f64;
        let y = 400. - 330. * (v - lo) / (hi - lo);
        svg.push_str(&format!("{x:.2},{y:.2} "));
    }
    svg.push_str("\"/></g></svg>");
    Ok(svg)
}
fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn connectivity_svg(circuit: &Value) -> Result<String, String> {
    let nets = circuit["nets"].as_array().ok_or("missing nets")?;
    if nets.is_empty() || nets.len() > 16 {
        return Err("sheet net bound".into());
    }
    let height = 100 + nets.len() * 90;
    let mut svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="1600" height="{height}" viewBox="0 0 1600 {height}"><rect width="1600" height="{height}" fill="#ffffff"/><g font-family="sans-serif" fill="#172b38"><text x="30" y="35" font-size="26">Reconstructed physical connectivity · exact owner pin graph</text><text x="30" y="62" font-size="18">Layout-derived comparison — not the original KiCad sheet; lines indicate electrical connection, not direction</text>"##
    );
    for (i, n) in nets.iter().enumerate() {
        let y = 115 + i * 90;
        let pins = n["pins"].as_array().ok_or("missing pins")?;
        if pins.is_empty() || pins.len() > 16 {
            return Err("sheet pin bound".into());
        }
        svg.push_str(&format!(
            r##"<text x="30" y="{y}" font-size="20">{}</text>"##,
            xml(n["name"].as_str().ok_or("missing net label")?)
        ));
        let spacing = 1100 / pins.len().saturating_sub(1).max(1);
        let end = 450 + (pins.len() - 1) * spacing;
        svg.push_str(&format!(
            r##"<path d="M450 {} H{end}" stroke="#087f8c" stroke-width="4" fill="none"/>"##,
            y + 20
        ));
        for (j, p) in pins.iter().enumerate() {
            let x = 450 + j * spacing;
            svg.push_str(&format!(r##"<circle cx="{x}" cy="{}" r="7" fill="#087f8c"/><text x="{x}" y="{}" text-anchor="middle" font-size="24">{}.{}</text>"##,y+20,y-3,xml(p["ref"].as_str().ok_or("missing ref")?),xml(p["pin"].as_str().ok_or("missing pin")?)));
            let value = circuit["parts"]
                .as_array()
                .and_then(|ps| ps.iter().find(|part| part["ref"] == p["ref"]))
                .and_then(|part| part["value"].as_str())
                .ok_or("missing component value")?;
            svg.push_str(&format!(
                r##"<text x="{x}" y="{}" text-anchor="middle" font-size="18">{}</text>"##,
                y + 49,
                xml(value)
            ));
        }
    }
    svg.push_str("</g></svg>");
    Ok(svg)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn vector_trace_has_physical_axes_and_rejects_bad_scale() {
        let t = json!({"frame":{"samples":[0,1],"range":{"minimum":0,"maximum":5}},"start_s":0,"stop_s":1});
        let s = trace_svg(&t).unwrap();
        assert!(s.contains("Voltage (V)") && s.contains("Time (s)"));
        let mut bad = t;
        bad["stop_s"] = json!(0);
        assert!(trace_svg(&bad).is_err());
    }
    #[test]
    fn pin_sheet_preserves_labels_and_escapes_source_text() {
        let c = json!({"parts":[{"ref":"R1","value":"1<&"}],"nets":[{"name":"N<&","pins":[{"ref":"R1","pin":"1"}]}]});
        let s = connectivity_svg(&c).unwrap();
        assert!(s.contains("R1.1") && s.contains("N&lt;&amp;") && s.contains("1&lt;&amp;"));
    }
    #[test]
    fn unrelated_circuit_cannot_share_trace() {
        assert!(compose(
            &json!({"schema":"canvas.modeled-trace.v1","source_sha256":"a"}),
            &json!({"schema":"lob.circuit-observation.v1","source_sha256":"b"}),
            &json!({}),
            &json!({}),
            &json!({})
        )
        .is_err());
    }
    #[test]
    fn admitted_pin_graph_roundtrip_rejects_pin_and_value_changes() {
        let trace = json!({"schema":"canvas.modeled-trace.v1","source_sha256":"a"});
        let circuit = json!({"schema":"lob.circuit-observation.v1","source_sha256":"a","parts":[{"ref":"R1","value":"1k"}],"nets":[{"name":"N","pins":[{"ref":"R1","pin":"1"}]}]});
        let proof = json!({"header":{"source_sha256":"a","target_graph":{"parts":[{"refdes":"R1","value":"1k"}],"physical_pin_groups":[[["R1","1"]]]}},"footer":{"checks":{"physical_pin_graph_exact":true,"complete_tree_replay":true}}});
        let widgets = json!({"widget":{},"evidence":{"body":"Actual receipt"}});
        let template = json!({"surfaces":[{"content":{"kind":"asset","asset":{"sha256":"x"}}}]});
        let docs = compose(&trace, &circuit, &widgets, &template, &proof).unwrap();
        assert_eq!(docs[0]["surfaces"][1], docs[1]["surfaces"][1]);
        let mut bad = circuit.clone();
        bad["nets"][0]["pins"][0]["pin"] = json!("2");
        assert!(compose(&trace, &bad, &widgets, &template, &proof).is_err());
        bad = circuit;
        bad["parts"][0]["value"] = json!("2k");
        assert!(compose(&trace, &bad, &widgets, &template, &proof).is_err());
    }
}
