use canvas_feeds::PrometheusSamples;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const OWNER: &str = "bifrost";
pub const SUMMARY_STREAM: &str = "production-summary";
pub const BACKENDS_STREAM: &str = "production-backends";
pub const TRAFFIC_STREAM: &str = "production-traffic";
pub const MESH_STREAM: &str = "production-mesh";
pub const MODELS_STREAM: &str = "production-model-usage";

pub fn stream_payloads(metrics: &str) -> [(&'static str, Value); 5] {
    let samples = PrometheusSamples::parse(metrics);
    let requests = samples.sum("bifrost_requests_total");
    let tokens = samples.sum("bifrost_tokens_total");
    let failures = samples.sum("bifrost_upstream_failure_kinds_total")
        + samples.sum("bifrost_no_reachable_backend_total");
    let throttled =
        samples.sum("bifrost_throttled_total") + samples.sum("bifrost_spend_cap_rejected_total");
    let cost = samples.sum("bifrost_cost_usd_total");
    let duration_count = samples.sum("bifrost_request_duration_seconds_count");
    let duration_sum = samples.sum("bifrost_request_duration_seconds_sum");
    let backends = backend_rows(&samples);
    let mesh = mesh_rows(&samples);
    let models = model_usage(&samples);
    [
        (
            SUMMARY_STREAM,
            json!({"environment":"production","requests":requests,"tokens":tokens,"cost_usd":cost,"failures":failures,"throttled":throttled,"backend_count":backends.len(),"healthy_backends":backends.iter().filter(|row| row["up"] == 1.0).count()}),
        ),
        (
            BACKENDS_STREAM,
            json!({"environment":"production","backends":backends}),
        ),
        (
            TRAFFIC_STREAM,
            json!({"environment":"production","requests":requests,"tokens":tokens,"failures":failures,"throttled":throttled,"mean_duration_seconds":if duration_count > 0.0 {Some(duration_sum / duration_count)} else {None}}),
        ),
        (
            MESH_STREAM,
            json!({"environment":"production","nodes":mesh}),
        ),
        (
            MODELS_STREAM,
            json!({"environment":"production","metric":"tokens","period":"Since gateway restart","models":models}),
        ),
    ]
}

pub fn unavailable_payloads(detail: &str) -> [(&'static str, Value); 5] {
    [
        (
            SUMMARY_STREAM,
            json!({"environment":"production","error":detail}),
        ),
        (
            BACKENDS_STREAM,
            json!({"environment":"production","backends":[],"error":detail}),
        ),
        (
            TRAFFIC_STREAM,
            json!({"environment":"production","error":detail}),
        ),
        (
            MESH_STREAM,
            json!({"environment":"production","nodes":[],"error":detail}),
        ),
        (
            MODELS_STREAM,
            json!({"environment":"production","models":[],"period":"Since gateway restart","error":detail}),
        ),
    ]
}

pub fn fit_event(mut event: Value, budget: usize) -> Value {
    loop {
        if serde_json::to_vec(&event).is_ok_and(|bytes| bytes.len() <= budget) {
            return event;
        }
        let rows = if event.pointer("/payload/backends").is_some() {
            event.pointer_mut("/payload/backends")
        } else if event.pointer("/payload/models").is_some() {
            event.pointer_mut("/payload/models")
        } else {
            event.pointer_mut("/payload/nodes")
        }
        .and_then(Value::as_array_mut);
        match rows {
            Some(rows) if !rows.is_empty() => {
                rows.pop();
            }
            _ => return event,
        }
    }
}

fn model_usage(samples: &PrometheusSamples) -> Vec<Value> {
    let mut totals = BTreeMap::<String, f64>::new();
    for sample in samples.named("bifrost_tokens_total") {
        if let Some(model) = sample.labels.get("model") {
            *totals.entry(model.clone()).or_default() += sample.value;
        }
    }
    let mut totals = totals.into_iter().collect::<Vec<_>>();
    totals.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    const DIRECT_SLICES: usize = 8;
    let other = totals
        .iter()
        .skip(DIRECT_SLICES)
        .map(|(_, value)| *value)
        .sum::<f64>();
    let mut rows = totals
        .into_iter()
        .take(DIRECT_SLICES)
        .map(|(model, value)| json!({"model":model,"value":value}))
        .collect::<Vec<_>>();
    if other > 0.0 {
        rows.push(json!({"model":"Other","value":other}));
    }
    rows
}

fn backend_rows(samples: &PrometheusSamples) -> Vec<Value> {
    let mut rows = BTreeMap::<String, Value>::new();
    for sample in &samples.0 {
        let Some(backend) = sample.labels.get("backend") else {
            continue;
        };
        let row = rows
            .entry(backend.clone())
            .or_insert_with(|| json!({"backend":backend,"up":0,"models":0,"circuit_open":0}));
        match sample.name.as_str() {
            "bifrost_backend_up" => row["up"] = json!(sample.value),
            "bifrost_backend_models" => row["models"] = json!(sample.value),
            "bifrost_backend_circuit_open" => row["circuit_open"] = json!(sample.value),
            _ => {}
        }
    }
    rows.into_values().collect()
}

fn mesh_rows(samples: &PrometheusSamples) -> Vec<Value> {
    let mut rows = BTreeMap::<String, Value>::new();
    for sample in &samples.0 {
        let Some(node) = sample.labels.get("node") else {
            continue;
        };
        let row = rows.entry(node.clone()).or_insert_with(|| json!({"node":node,"up":0,"utilization_pct":null,"memory_used_bytes":null,"memory_bytes":null,"power_watts":null}));
        match sample.name.as_str() {
            "bifrost_yggdrasil_node_up" => row["up"] = json!(sample.value),
            "bifrost_yggdrasil_accelerator_utilization_pct" => {
                row["utilization_pct"] = json!(sample.value)
            }
            "bifrost_yggdrasil_accelerator_memory_used_bytes" => {
                row["memory_used_bytes"] = json!(sample.value)
            }
            "bifrost_yggdrasil_accelerator_memory_bytes" => {
                row["memory_bytes"] = json!(sample.value)
            }
            "bifrost_yggdrasil_accelerator_power_watts" => row["power_watts"] = json!(sample.value),
            _ => {}
        }
    }
    rows.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn aggregates_identity_labels() {
        let packets = stream_payloads(
            "bifrost_requests_total{user=\"a@fpl.dev\",model=\"m\",backend=\"openai\",status=\"200\"} 4\nbifrost_requests_total{user=\"b@fpl.dev\",model=\"m\",backend=\"openai\",status=\"200\"} 3\nbifrost_backend_up{backend=\"openai\"} 1\nbifrost_backend_models{backend=\"openai\"} 3\n",
        );
        assert_eq!(packets[0].1["requests"], 7.0);
        assert_eq!(packets[1].1["backends"][0]["models"], 3.0);
        assert!(
            !serde_json::to_string(&packets)
                .unwrap()
                .contains("@fpl.dev")
        );
    }
}
