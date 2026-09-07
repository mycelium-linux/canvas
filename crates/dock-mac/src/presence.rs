//! Narrow consumer of JARVIS Presence's versioned local publication.
//!
//! JARVIS owns observation and identity fusion. Dock owns an explicit mapping
//! from a fresh, identified producer/zone pair to a stable display ID.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_FILE_BYTES: u64 = 1024 * 1024;
const MAX_SOURCES: usize = 32;
const MAX_ROUTES: usize = 64;
const MAX_IDENTITIES: usize = 32;

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub person_id: String,
    #[serde(default = "default_max_age_ms")]
    pub max_age_ms: u64,
    #[serde(default = "default_confidence")]
    pub min_confidence: f32,
    pub sources: Vec<PathBuf>,
    pub routes: Vec<Route>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Route {
    pub producer_id: String,
    pub zone: String,
    pub display_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ResolvedRoute {
    pub producer_id: String,
    pub zone: String,
    pub display_id: String,
    pub age_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Publication {
    schema_version: u32,
    producer_id: String,
    published_at_unix_ms: u64,
    snapshot: Snapshot,
}

#[derive(Debug, Deserialize)]
struct Snapshot {
    zone: String,
    occupied: bool,
    observed_identities: Vec<Identity>,
    confidence: f32,
    recency_ms: u64,
}

#[derive(Debug, Deserialize)]
struct Identity {
    person_id: Option<String>,
    confidence: f32,
}

fn default_max_age_ms() -> u64 {
    5000
}

fn default_confidence() -> f32 {
    0.7
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}

impl Config {
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != 1 {
            return Err("Unsupported presence routing schema".into());
        }
        if !bounded_text(&self.person_id, 128)
            || self.max_age_ms == 0
            || self.max_age_ms > 60_000
            || !(0.0..=1.0).contains(&self.min_confidence)
        {
            return Err("Invalid presence routing policy".into());
        }
        if self.sources.is_empty() || self.sources.len() > MAX_SOURCES {
            return Err("Presence routing requires 1–32 publication sources".into());
        }
        if self.routes.is_empty() || self.routes.len() > MAX_ROUTES {
            return Err("Presence routing requires 1–64 display routes".into());
        }
        if self.sources.iter().any(|path| !path.is_absolute()) {
            return Err("Presence publication paths must be absolute".into());
        }
        let mut keys = BTreeSet::new();
        for route in &self.routes {
            if !bounded_text(&route.producer_id, 128)
                || !bounded_text(&route.zone, 128)
                || !bounded_text(&route.display_id, 128)
                || !keys.insert((&route.producer_id, &route.zone))
            {
                return Err("Invalid or duplicate presence display route".into());
            }
        }
        Ok(())
    }
}

pub fn path() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("DOCK_PRESENCE_ROUTING_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    Ok(
        PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?)
            .join("Library/Application Support/Dock/presence-routing.json"),
    )
}

pub fn load() -> Result<Config, String> {
    load_from(&path()?)
}

fn load_from(path: &Path) -> Result<Config, String> {
    let bytes = read_bounded(path, "presence routing policy")?;
    let config: Config =
        serde_json::from_slice(&bytes).map_err(|_| "Invalid presence routing policy")?;
    config.validate()?;
    Ok(config)
}

fn read_bounded(path: &Path, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::symlink_metadata(path).map_err(|_| format!("Cannot read {label}"))?;
    if !metadata.file_type().is_file() || metadata.len() > MAX_FILE_BYTES {
        return Err(format!("Invalid {label} file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(format!("Unsafe {label} ownership or permissions"));
        }
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|_| format!("Cannot read {label}"))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(format!("Oversized {label}"));
    }
    Ok(bytes)
}

fn now_ms() -> Result<u64, String> {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "System clock is before the Unix epoch")?
            .as_millis(),
    )
    .map_err(|_| "System clock is outside the supported range".into())
}

pub fn resolve() -> Result<ResolvedRoute, String> {
    let config = load()?;
    resolve_at(&config, now_ms()?)
}

fn resolve_at(config: &Config, now: u64) -> Result<ResolvedRoute, String> {
    config.validate()?;
    let routes = config
        .routes
        .iter()
        .map(|route| ((route.producer_id.as_str(), route.zone.as_str()), route))
        .collect::<BTreeMap<_, _>>();
    let mut candidates = Vec::new();
    for source in &config.sources {
        let bytes = read_bounded(source, "presence publication")?;
        let publication: Publication =
            serde_json::from_slice(&bytes).map_err(|_| "Invalid presence publication")?;
        if publication.schema_version != 1
            || !bounded_text(&publication.producer_id, 128)
            || !bounded_text(&publication.snapshot.zone, 128)
            || publication.snapshot.observed_identities.len() > MAX_IDENTITIES
            || !(0.0..=1.0).contains(&publication.snapshot.confidence)
            || publication
                .snapshot
                .observed_identities
                .iter()
                .any(|identity| !(0.0..=1.0).contains(&identity.confidence))
        {
            return Err("Invalid presence publication".into());
        }
        let elapsed = now
            .checked_sub(publication.published_at_unix_ms)
            .ok_or("Presence publication is in the future")?;
        let age_ms = publication.snapshot.recency_ms.saturating_add(elapsed);
        let Some(route) = routes.get(&(
            publication.producer_id.as_str(),
            publication.snapshot.zone.as_str(),
        )) else {
            continue;
        };
        let identified = publication
            .snapshot
            .observed_identities
            .iter()
            .any(|identity| {
                identity.person_id.as_deref() == Some(config.person_id.as_str())
                    && identity.confidence >= config.min_confidence
            });
        if publication.snapshot.occupied
            && publication.snapshot.confidence >= config.min_confidence
            && age_ms < config.max_age_ms
            && identified
        {
            candidates.push(ResolvedRoute {
                producer_id: route.producer_id.clone(),
                zone: route.zone.clone(),
                display_id: route.display_id.clone(),
                age_ms,
            });
        }
    }
    if candidates.is_empty() {
        return Err("No fresh identified presence matches a configured display".into());
    }
    let displays = candidates
        .iter()
        .map(|candidate| candidate.display_id.as_str())
        .collect::<BTreeSet<_>>();
    if displays.len() != 1 {
        return Err("Presence is ambiguous across configured displays".into());
    }
    candidates.sort_by_key(|candidate| {
        (
            candidate.age_ms,
            candidate.producer_id.clone(),
            candidate.zone.clone(),
        )
    });
    Ok(candidates.remove(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TestDir(PathBuf);

    impl TestDir {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "dock-presence-{label}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn write(path: &Path, value: serde_json::Value) {
        fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    }

    fn publication(producer: &str, zone: &str, display_person: &str, at: u64) -> serde_json::Value {
        json!({
            "schema_version":1,
            "producer_id":producer,
            "published_at_unix_ms":at,
            "snapshot":{
                "zone":zone,"occupied":true,"confidence":0.9,"recency_ms":100,
                "observed_identities":[{"person_id":display_person,"confidence":0.91}]
            },
            "private_extension":"ignored"
        })
    }

    fn config(sources: Vec<PathBuf>) -> Config {
        Config {
            schema_version: 1,
            person_id: "avery".into(),
            max_age_ms: 5000,
            min_confidence: 0.8,
            sources,
            routes: vec![
                Route {
                    producer_id: "left-camera".into(),
                    zone: "desk-left".into(),
                    display_id: "left-display".into(),
                },
                Route {
                    producer_id: "right-camera".into(),
                    zone: "desk-right".into(),
                    display_id: "right-display".into(),
                },
            ],
        }
    }

    #[test]
    fn resolves_only_fresh_identified_presence() {
        let root = TestDir::new("fresh");
        let left = root.path().join("left.json");
        write(
            &left,
            publication("left-camera", "desk-left", "avery", 10_000),
        );
        let policy = config(vec![left.clone()]);
        assert_eq!(
            resolve_at(&policy, 10_200).unwrap().display_id,
            "left-display"
        );
        assert!(resolve_at(&policy, 15_000)
            .unwrap_err()
            .contains("No fresh"));
        write(
            &left,
            publication("left-camera", "desk-left", "someone-else", 10_000),
        );
        assert!(resolve_at(&policy, 10_200)
            .unwrap_err()
            .contains("No fresh"));
    }

    #[test]
    fn different_live_display_candidates_are_ambiguous() {
        let root = TestDir::new("ambiguous");
        let left = root.path().join("left.json");
        let right = root.path().join("right.json");
        write(
            &left,
            publication("left-camera", "desk-left", "avery", 10_000),
        );
        write(
            &right,
            publication("right-camera", "desk-right", "avery", 10_000),
        );
        assert!(resolve_at(&config(vec![left, right]), 10_200)
            .unwrap_err()
            .contains("ambiguous"));
    }

    #[test]
    fn policy_rejects_duplicate_routes_and_relative_sources() {
        let mut policy = config(vec![PathBuf::from("relative.json")]);
        assert!(policy.validate().is_err());
        policy.sources = vec![PathBuf::from("/tmp/source.json")];
        policy.routes.push(policy.routes[0].clone());
        assert!(policy.validate().is_err());
    }
}
