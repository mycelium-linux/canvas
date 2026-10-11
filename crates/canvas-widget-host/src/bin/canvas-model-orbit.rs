//! Bounded presentation-only camera driver for an existing model host.
use std::{
    env, fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = PathBuf::from(args.next().ok_or("camera path required")?);
    let stream = args.next().ok_or("stream id required")?;
    if stream.is_empty()
        || !stream
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("unsafe stream id".into());
    }
    let seconds: u64 = args.next().ok_or("bounded duration required")?.parse()?;
    let scene = args.next().map(PathBuf::from);
    let models: Vec<String> = args.collect();
    if !(1..=3600).contains(&seconds) {
        return Err("duration must be 1..3600 seconds".into());
    }
    if scene.is_some()
        && (models.len() < 2
            || models.len() > 1000
            || models.iter().any(|model| {
                !model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
            }))
    {
        return Err("carousel requires 2..1000 safe asset filenames".into());
    }
    fs::create_dir_all(path.parent().ok_or("camera parent required")?)?;
    let started = Instant::now();
    let temporary = path.with_extension(format!("camera-{}.tmp", std::process::id()));
    let mut previous_model = usize::MAX;
    while started.elapsed() < Duration::from_secs(seconds) {
        if let Some(scene) = &scene {
            // Two complete 360-degree orbits at the configured 18 deg/s.
            // This diagnostic host has no GPU-preload acknowledgement yet.
            let model_seconds = 2.0 * 360.0 / 18.0;
            let index = (started.elapsed().as_secs_f64() / model_seconds) as usize % models.len();
            if index != previous_model {
                let contents = format!("return {{ scene_id = \"verk-recovered-showcase\", asset = \"{}\", model_rotation_degrees = {{ -90, 0, 0 }}, camera = \"iso\", material = {{ color = {{ 178, 196, 212 }}, glow = 0, wireframe = false, opacity = 1 }}, transition_ms = 500, fly_in = false }}", models[index]);
                let staged = scene.with_extension(format!("{}.tmp", std::process::id()));
                fs::write(&staged, contents)?;
                fs::rename(staged, scene)?;
                previous_model = index;
                eprintln!("showcase model {}", models[index]);
            }
        }
        let yaw = orbit_yaw(started.elapsed().as_secs_f32());
        let observed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        let pose = format!("{{\"schema_version\":1,\"stream_id\":\"{stream}\",\"yaw_degrees\":{yaw},\"pitch_degrees\":24,\"distance\":1.75,\"grabbed\":false,\"instant\":false,\"auto_orbit\":false,\"at_unix_ms\":{observed}}}");
        fs::write(&temporary, pose)?;
        fs::rename(&temporary, &path)?;
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}

fn orbit_yaw(elapsed_seconds: f32) -> f32 {
    (45.0 + elapsed_seconds * 18.0).rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::orbit_yaw;
    #[test]
    fn pending_successor_never_stops_orbit_after_two_rotations() {
        assert_eq!(orbit_yaw(40.0), 45.0);
        assert_eq!(orbit_yaw(41.0), 63.0);
        assert_eq!(orbit_yaw(1001.0), 63.0);
    }
}
