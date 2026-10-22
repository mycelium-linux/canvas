//! Bounded CPU event-loop measurements. These do not measure GPU presentation.
use std::{collections::VecDeque, time::Duration};

#[derive(Debug, Default)]
pub struct UpdateTimings {
    samples: VecDeque<u64>,
    total: u64,
}

impl UpdateTimings {
    pub fn record(&mut self, elapsed: Duration) {
        if self.samples.len() == 512 {
            self.samples.pop_front();
        }
        self.samples
            .push_back(elapsed.as_micros().min(u64::MAX as u128) as u64);
        self.total = self.total.saturating_add(1);
    }

    pub fn snapshot(&self) -> serde_json::Value {
        let mut samples: Vec<_> = self.samples.iter().copied().collect();
        samples.sort_unstable();
        let percentile = |percent: usize| {
            (!samples.is_empty()).then(|| samples[(samples.len() - 1) * percent / 100])
        };
        serde_json::json!({
            "measurement": "cpu_update_duration",
            "unit": "microseconds",
            "total_updates": self.total,
            "window_samples": samples.len(),
            "p50": percentile(50),
            "p95": percentile(95),
            "p99": percentile(99),
            "max": samples.last(),
            "gpu_present_latency": null,
            "input_to_present_latency": null
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_only_recent_samples_and_reports_empty_as_unknown() {
        let mut timings = UpdateTimings::default();
        assert!(timings.snapshot()["p50"].is_null());
        timings.record(Duration::from_secs(10));
        for _ in 0..512 {
            timings.record(Duration::from_micros(3));
        }
        let snapshot = timings.snapshot();
        assert_eq!(snapshot["window_samples"], 512);
        assert_eq!(snapshot["total_updates"], 513);
        assert_eq!(snapshot["max"], 3);
    }
}
