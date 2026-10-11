//! Receiver-local geographic presentation; no new observations or network I/O.
use fpl_gfx::geo_map::motion::{Config, Kiosk, Motion};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct SurfaceMotion {
    motion: Motion,
    observed: Option<String>,
    received_ms: u64,
    connected: Option<bool>,
    pub kiosk: Kiosk,
}
impl SurfaceMotion {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            motion: Motion::new(Config::default()).map_err(|e| e.to_string())?,
            observed: None,
            received_ms: 0,
            connected: None,
            kiosk: Kiosk::default(),
        })
    }

    pub fn present(&mut self, snapshot: &mut Value, now_ms: u64) -> Result<(), String> {
        if snapshot["frame"]["view"] != "geo_map" {
            return Ok(());
        }
        let map =
            serde_json::from_value(snapshot["frame"]["map"].clone()).map_err(|e| e.to_string())?;
        let observed = snapshot["data"]["observed_at"].as_str().map(str::to_owned);
        if observed != self.observed || self.received_ms == 0 {
            self.observed = observed;
            self.received_ms = now_ms;
        }
        let connected = matches!(
            snapshot["data"]["state"].as_str(),
            Some("ready" | "partial")
        );
        if self.connected != Some(connected) {
            self.received_ms = now_ms;
            self.connected = Some(connected);
        }
        // This feed carries fetch timestamps, not per-aircraft fix timestamps.
        // Record explicit receiver sampling times; never claim true fix timing.
        self.motion
            .ingest(&map, &BTreeMap::new(), self.received_ms, connected)
            .map_err(|e| e.to_string())?;
        if let Some(map) = self.motion.render(now_ms) {
            snapshot["frame"]["map"] = serde_json::to_value(map).map_err(|e| e.to_string())?;
        }
        snapshot["motion"] = serde_json::json!({
            "timing":"receiver_sample", "samples":self.motion.sample_count(),
            "fallback_samples":self.motion.fallback_count(), "delay_ms":5000,
            "extrapolation":false
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn snapshot() -> Value {
        json!({"data":{"observed_at":"first","state":"ready"},"frame":{"view":"geo_map","map":{"extent":{"south_deg":46,"north_deg":49,"west_deg":-124,"east_deg":-121},"markers":[{"id":"one","label":"ONE","latitude_deg":47,"longitude_deg":-122,"role":"primary"}],"paths":[],"selected":null}}})
    }
    #[test]
    fn selection_does_not_create_an_observation() {
        let mut state = SurfaceMotion::new().unwrap();
        let mut first = snapshot();
        state.present(&mut first, 10000).unwrap();
        let mut next = snapshot();
        next["frame"]["map"]["selected"] = json!("one");
        state.present(&mut next, 11000).unwrap();
        assert_eq!(next["motion"]["samples"], 1);
        assert_eq!(next["frame"]["map"]["selected"], "one");
    }

    #[test]
    fn outage_freezes_at_detection_and_reconnect_resets() {
        let mut state = SurfaceMotion::new().unwrap();
        let mut first = snapshot();
        state.present(&mut first, 10000).unwrap();
        let mut second = snapshot();
        second["data"]["observed_at"] = json!("second");
        second["frame"]["map"]["markers"][0]["latitude_deg"] = json!(47.1);
        state.present(&mut second, 15000).unwrap();
        let mut stale = snapshot();
        stale["data"]["observed_at"] = json!("second");
        stale["data"]["state"] = json!("stale");
        state.present(&mut stale, 18000).unwrap();
        let frozen = stale["frame"]["map"]["markers"][0]["latitude_deg"]
            .as_f64()
            .unwrap();
        assert!((frozen - 47.06).abs() < 1e-8);
        let mut later = stale.clone();
        state.present(&mut later, 40000).unwrap();
        assert_eq!(
            later["frame"]["map"]["markers"][0]["latitude_deg"],
            stale["frame"]["map"]["markers"][0]["latitude_deg"]
        );
        let mut resumed = snapshot();
        resumed["data"]["observed_at"] = json!("resumed");
        state.present(&mut resumed, 41000).unwrap();
        assert_eq!(resumed["motion"]["samples"], 1);
    }
}
