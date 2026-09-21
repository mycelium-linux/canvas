use canvas_feeds::PrometheusSamples as Samples;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const OWNER: &str = "shroud";
pub const REGIONS_STREAM: &str = "regions";
pub const SUMMARY_STREAM: &str = "runtime-summary";
pub const VMS_STREAM: &str = "virtual-machines";
pub const NETWORK_STREAM: &str = "network";

pub fn stream_payloads(metrics: &str, selected_region: &str) -> [(&'static str, Value); 4] {
    let samples = Samples::parse(metrics);
    let vms = vm_rows(&samples);
    [
        (
            REGIONS_STREAM,
            json!({
                "selected_region": selected_region,
                "regions": [
                    {"id":"agora-one","label":"Agora One","state":if selected_region == "agora-one" {"selected"} else {"available"},"configured":true},
                    {"id":"beachhead","label":"Beachhead","state":"not_configured","configured":false},
                    {"id":"prod","label":"Production","state":"not_configured","configured":false}
                ]
            }),
        ),
        (
            SUMMARY_STREAM,
            json!({
                "region": selected_region,
                "daemon_up": samples.scalar("shroud_daemon_up"),
                "configured_vms": samples.scalar("shroud_vm_configured_total"),
                "running_vms": samples.scalar("shroud_vm_running_total"),
                "memory_total_bytes": samples.scalar("shroud_host_memory_total_bytes"),
                "memory_available_bytes": samples.scalar("shroud_host_memory_available_bytes"),
                "filesystem_size_bytes": samples.scalar("shroud_host_filesystem_size_bytes"),
                "filesystem_free_bytes": samples.scalar("shroud_host_filesystem_free_bytes"),
                "load_1m": samples.labeled("shroud_host_load_average", "interval", "1m"),
            }),
        ),
        (VMS_STREAM, json!({"region":selected_region,"vms":vms})),
        (
            NETWORK_STREAM,
            json!({
                "region": selected_region,
                "nat_enabled": samples.scalar("shroud_network_nat_enabled"),
                "dnsmasq_running": samples.scalar("shroud_dnsmasq_running"),
                "iptables_rules": samples.scalar("shroud_iptables_rules_total"),
                "service_records": samples.scalar("shroud_service_discovery_records_total"),
                "starts": samples.labeled("shroud_vm_lifecycle_events_total", "event", "start"),
                "stops": samples.labeled("shroud_vm_lifecycle_events_total", "event", "stop"),
            }),
        ),
    ]
}

pub fn unavailable_payloads(selected_region: &str, detail: &str) -> [(&'static str, Value); 4] {
    let regions = json!({
        "selected_region":selected_region,
        "regions":[
            {"id":"agora-one","label":"Agora One","state":"unavailable","configured":true},
            {"id":"beachhead","label":"Beachhead","state":"not_configured","configured":false},
            {"id":"prod","label":"Production","state":"not_configured","configured":false}
        ],
        "error":detail
    });
    [
        (REGIONS_STREAM, regions),
        (
            SUMMARY_STREAM,
            json!({"region":selected_region,"error":detail}),
        ),
        (
            VMS_STREAM,
            json!({"region":selected_region,"vms":[],"error":detail}),
        ),
        (
            NETWORK_STREAM,
            json!({"region":selected_region,"error":detail}),
        ),
    ]
}

pub fn fit_event(mut event: Value, budget: usize) -> Value {
    loop {
        if serde_json::to_vec(&event).is_ok_and(|bytes| bytes.len() <= budget) {
            return event;
        }
        let Some(rows) = event
            .pointer_mut("/payload/vms")
            .and_then(Value::as_array_mut)
        else {
            return event;
        };
        if rows.is_empty() {
            return event;
        }
        rows.pop();
    }
}

fn vm_rows(samples: &Samples) -> Vec<Value> {
    let mut vms = BTreeMap::<String, Value>::new();
    for sample in &samples.0 {
        let Some(vm) = sample.labels.get("vm") else {
            continue;
        };
        let row = vms.entry(vm.clone()).or_insert_with(|| json!({"id":vm}));
        match sample.name.as_str() {
            "shroud_vm_info" => row["ip"] = json!(sample.labels.get("ip")),
            "shroud_vm_power_state" if sample.value == 1.0 => {
                row["power"] = json!(sample.labels.get("state"))
            }
            "shroud_vm_health_status" if sample.value == 1.0 => {
                row["health"] = json!(sample.labels.get("status"))
            }
            "shroud_vm_resource_cpu" => row["vcpu"] = json!(sample.value),
            "shroud_vm_resource_memory_mib" => row["memory_mib"] = json!(sample.value),
            "shroud_vm_process_resident_memory_bytes" => {
                row["resident_bytes"] = json!(sample.value)
            }
            _ => {}
        }
    }
    vms.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const METRICS: &str = r#"
shroud_daemon_up 1
shroud_vm_configured_total 2
shroud_vm_running_total 1
shroud_host_load_average{interval="1m"} 0.5
shroud_vm_info{vm="alpha",ip="172.16.0.2"} 1
shroud_vm_power_state{vm="alpha",state="running"} 1
shroud_vm_health_status{vm="alpha",status="ready"} 1
shroud_vm_resource_cpu{vm="alpha"} 2
shroud_vm_resource_memory_mib{vm="alpha"} 512
"#;

    #[test]
    fn projects_owner_metrics_into_region_and_vm_packets() {
        let packets = stream_payloads(METRICS, "agora-one");
        assert_eq!(packets[0].1["regions"][0]["state"], "selected");
        assert_eq!(packets[1].1["running_vms"], 1.0);
        assert_eq!(packets[2].1["vms"][0]["health"], "ready");
        assert_eq!(packets[2].1["vms"][0]["memory_mib"], 512.0);
        assert_eq!(packets[0].1["regions"][1]["state"], "not_configured");
    }

    #[test]
    fn unavailable_is_not_reported_as_zero() {
        let packets = unavailable_payloads("agora-one", "connection refused");
        assert!(packets[1].1.get("running_vms").is_none());
        assert_eq!(packets[0].1["regions"][0]["state"], "unavailable");
    }
}
