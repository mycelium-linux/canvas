//! Deterministic, looping camera travel between named areas of interest.
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tour {
    pub stops: Vec<Stop>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stop {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    pub distance_m: f64,
    pub hold_secs: f64,
    pub travel_secs: f64,
}

#[derive(Debug, PartialEq)]
pub struct Pose {
    pub latitude: f64,
    pub longitude: f64,
    pub distance_m: f64,
}

impl Tour {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(2..=16).contains(&self.stops.len()) {
            return Err("tour needs 2..=16 stops");
        }
        for stop in &self.stops {
            if stop.name.trim().is_empty()
                || !stop.latitude.is_finite()
                || stop.latitude.abs() > 90.0
                || !stop.longitude.is_finite()
                || stop.longitude.abs() > 180.0
                || !stop.distance_m.is_finite()
                || !(2.0..=31_890_685.0).contains(&stop.distance_m)
                || !stop.hold_secs.is_finite()
                || !(0.0..=120.0).contains(&stop.hold_secs)
                || !stop.travel_secs.is_finite()
                || !(1.0..=120.0).contains(&stop.travel_secs)
            {
                return Err("invalid tour stop, camera range or duration");
            }
        }
        Ok(())
    }

    /// Holds each stop, then eases into the next; final travel closes the loop.
    pub fn sample(&self, elapsed_secs: f64) -> Pose {
        let duration: f64 = self.stops.iter().map(|s| s.hold_secs + s.travel_secs).sum();
        let mut time = elapsed_secs.rem_euclid(duration);
        for (index, stop) in self.stops.iter().enumerate() {
            let segment = stop.hold_secs + stop.travel_secs;
            if time < segment {
                let next = &self.stops[(index + 1) % self.stops.len()];
                let t = ((time - stop.hold_secs) / stop.travel_secs).clamp(0.0, 1.0);
                let t = t * t * (3.0 - 2.0 * t);
                let delta_longitude =
                    (next.longitude - stop.longitude + 180.0).rem_euclid(360.0) - 180.0;
                return Pose {
                    latitude: stop.latitude + (next.latitude - stop.latitude) * t,
                    longitude: (stop.longitude + delta_longitude * t + 180.0).rem_euclid(360.0)
                        - 180.0,
                    // Geometric interpolation makes orbital and regional zooms smooth.
                    distance_m: (stop.distance_m.ln()
                        + (next.distance_m.ln() - stop.distance_m.ln()) * t)
                        .exp(),
                };
            }
            time -= segment;
        }
        unreachable!("validated finite tour duration covers every sample")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tour() -> Tour {
        Tour {
            stops: vec![
                Stop {
                    name: "west".into(),
                    latitude: 40.0,
                    longitude: 170.0,
                    distance_m: 100.0,
                    hold_secs: 2.0,
                    travel_secs: 4.0,
                },
                Stop {
                    name: "east".into(),
                    latitude: 20.0,
                    longitude: -170.0,
                    distance_m: 10_000.0,
                    hold_secs: 2.0,
                    travel_secs: 4.0,
                },
            ],
        }
    }
    #[test]
    fn holds_then_uses_short_arc_and_geometric_zoom() {
        let tour = tour();
        tour.validate().unwrap();
        assert_eq!(tour.sample(0.0), tour.sample(1.0));
        let middle = tour.sample(4.0);
        assert_eq!(middle.latitude, 30.0);
        assert_eq!(middle.longitude, -180.0);
        assert!((middle.distance_m - 1_000.0).abs() < 0.001);
        assert_eq!(tour.sample(0.0), tour.sample(12.0));
    }
    #[test]
    fn rejects_empty_and_invalid_routes() {
        assert!(Tour { stops: vec![] }.validate().is_err());
        let mut tour = tour();
        tour.stops[0].travel_secs = f64::NAN;
        assert!(tour.validate().is_err());
    }
    #[test]
    fn route_stays_above_surface_and_continuous_at_loop() {
        let tour = tour();
        for step in 0..1_200 {
            assert!(tour.sample(f64::from(step) / 100.0).distance_m >= 99.99);
        }
        let end = tour.sample(12.0 - 0.00001);
        let start = tour.sample(0.0);
        assert!((end.distance_m - start.distance_m).abs() < 0.001);
        assert!((end.latitude - start.latitude).abs() < 0.001);
    }
}
