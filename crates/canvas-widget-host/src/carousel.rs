//! Deterministic presentation traversal and bounded derived-preview eviction.
//! The renderer owns completed orbit counts and transition acknowledgements.
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    /// Immutable owner source identity, never an unstable array index.
    pub current: String,
}
impl Cursor {
    pub fn successor<'a>(&self, ordered: &'a [String]) -> Option<&'a str> {
        ordered
            .iter()
            .find(|id| id.as_str() > self.current.as_str())
            .or_else(|| ordered.first())
            .map(String::as_str)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Holding,
    Preloading(String),
    Ready(String),
    Fading(String),
}
pub struct Gate {
    pub current: String,
    pub phase: Phase,
    pub orbit_count: u32,
    pub quarantined: BTreeMap<String, String>,
}
impl Gate {
    pub fn new(current: String, orbit_count: u32) -> Result<Self, &'static str> {
        if !(1..=10).contains(&orbit_count) {
            return Err("orbit_count must be 1..10");
        }
        Ok(Self {
            current,
            phase: Phase::Holding,
            orbit_count,
            quarantined: BTreeMap::new(),
        })
    }
    pub fn preload(&mut self, next: String) -> Result<(), &'static str> {
        if self.phase != Phase::Holding || next == self.current {
            return Err("preload already active or same model");
        }
        self.phase = Phase::Preloading(next);
        Ok(())
    }
    pub fn prepared(&mut self, id: &str) -> Result<(), &'static str> {
        if self.phase != Phase::Preloading(id.to_owned()) {
            return Err("stale preload acknowledgement");
        }
        self.phase = Phase::Ready(id.to_owned());
        Ok(())
    }
    pub fn failed(&mut self, id: &str, detail: String) -> Result<(), &'static str> {
        if self.phase != Phase::Preloading(id.to_owned()) {
            return Err("stale preload failure");
        }
        self.quarantined
            .insert(id.to_owned(), detail.chars().take(512).collect());
        self.phase = Phase::Holding;
        Ok(())
    }
    /// Counts are acknowledged by rendered frames, not a wall-clock guess.
    pub fn begin_transition(&mut self, rendered_full_orbits: u32) -> bool {
        if rendered_full_orbits < self.orbit_count {
            return false;
        }
        if let Phase::Ready(id) = &self.phase {
            self.phase = Phase::Fading(id.clone());
            true
        } else {
            false
        }
    }
    /// Only this renderer acknowledgement releases the old current slot.
    pub fn transition_complete(&mut self, id: &str) -> Result<String, &'static str> {
        if self.phase != Phase::Fading(id.to_owned()) {
            return Err("stale transition acknowledgement");
        }
        let previous = std::mem::replace(&mut self.current, id.to_owned());
        self.phase = Phase::Holding;
        Ok(previous)
    }
}

#[derive(Debug, Clone)]
pub struct DerivedPreview {
    pub bytes: u64,
    pub last_used: u64,
}
pub fn eviction_plan(
    entries: &BTreeMap<String, DerivedPreview>,
    protected: &BTreeSet<String>,
    budget: u64,
) -> Result<Vec<String>, &'static str> {
    if protected.len() > 2 {
        return Err("only current and next may be protected");
    }
    let mut total = entries
        .values()
        .try_fold(0u64, |sum, value| sum.checked_add(value.bytes))
        .ok_or("cache byte overflow")?;
    let mut candidates: Vec<_> = entries
        .iter()
        .filter(|(id, _)| !protected.contains(*id))
        .collect();
    candidates.sort_by_key(|(id, item)| (item.last_used, id.as_str()));
    let mut victims = Vec::new();
    for (id, item) in candidates {
        if total <= budget {
            break;
        }
        // This planner must never name immutable STEP files or owner receipts.
        if !id.ends_with(".glb") || id.contains('/') || id.contains("..") {
            return Err("unsafe derived cache entry");
        }
        total -= item.bytes;
        victims.push(id.clone());
    }
    if total > budget {
        return Err("current plus next exceed the cache budget");
    }
    Ok(victims)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_inventory_traverses_more_than_two_and_wraps() {
        let ids = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let mut cursor = Cursor {
            current: "a".into(),
        };
        let mut seen = Vec::new();
        for _ in 0..4 {
            cursor.current = cursor.successor(&ids).unwrap().into();
            seen.push(cursor.current.clone());
        }
        assert_eq!(seen, ["b", "c", "d", "a"]);
    }
    #[test]
    fn preload_and_two_rendered_orbits_are_both_required() {
        let mut gate = Gate::new("a".into(), 2).unwrap();
        gate.preload("b".into()).unwrap();
        assert!(!gate.begin_transition(3));
        gate.prepared("b").unwrap();
        assert!(!gate.begin_transition(1));
        assert!(gate.begin_transition(2));
        assert_eq!(gate.current, "a");
        assert_eq!(gate.transition_complete("b").unwrap(), "a");
        assert_eq!(gate.current, "b");
    }
    #[test]
    fn failed_preload_retains_current_and_is_explicit() {
        let mut gate = Gate::new("a".into(), 2).unwrap();
        gate.preload("b".into()).unwrap();
        gate.failed("b", "owner conversion failed".into()).unwrap();
        assert_eq!(gate.current, "a");
        assert!(!gate.begin_transition(10));
        assert_eq!(gate.quarantined["b"], "owner conversion failed");
        gate.preload("c".into()).unwrap();
    }
    #[test]
    fn eviction_never_releases_live_slots_or_source_artifacts() {
        let entries = BTreeMap::from([
            (
                "a.glb".into(),
                DerivedPreview {
                    bytes: 5,
                    last_used: 1,
                },
            ),
            (
                "b.glb".into(),
                DerivedPreview {
                    bytes: 5,
                    last_used: 2,
                },
            ),
            (
                "c.glb".into(),
                DerivedPreview {
                    bytes: 5,
                    last_used: 3,
                },
            ),
        ]);
        let protected = BTreeSet::from(["a.glb".into(), "b.glb".into()]);
        assert_eq!(eviction_plan(&entries, &protected, 10).unwrap(), ["c.glb"]);
        assert!(eviction_plan(&entries, &protected, 9).is_err());
        let source = BTreeMap::from([(
            "source.step".into(),
            DerivedPreview {
                bytes: 20,
                last_used: 0,
            },
        )]);
        assert!(eviction_plan(&source, &BTreeSet::new(), 1).is_err());
    }
}
