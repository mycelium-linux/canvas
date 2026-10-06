//! Shared MRU navigation, independent of storage and presentation.
//! Owners admit/authorize a switch before recording it. Saved JSON is unchanged.
pub const MAX_RECENT_PROJECTS: usize = 16;

pub fn record_switch(recent: &mut Vec<String>, departed: Option<&str>, selected: &str) {
    if departed == Some(selected) {
        return;
    }
    if let Some(departed) = departed {
        recent.insert(0, departed.to_owned());
    }
    let mut seen = std::collections::BTreeSet::new();
    recent.retain(|id| id != selected && seen.insert(id.clone()));
    recent.truncate(MAX_RECENT_PROJECTS);
}

pub fn previous(recent: &[String], contains: impl Fn(&str) -> bool) -> Option<&str> {
    recent.iter().map(String::as_str).find(|id| contains(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn repeated_switches_are_mru_not_a_browser_stack() {
        let mut recent = vec![];
        record_switch(&mut recent, Some("a"), "b");
        record_switch(&mut recent, Some("b"), "a");
        assert_eq!(recent, ["b"]);
        record_switch(&mut recent, Some("a"), "a");
        assert_eq!(recent, ["b"]);
        assert_eq!(previous(&recent, |id| id == "b"), Some("b"));
        assert_eq!(previous(&recent, |_| false), None);
    }
    #[test]
    fn history_is_bounded_and_deduplicated() {
        let mut recent = vec![];
        for index in 0..40 {
            record_switch(&mut recent, Some(&format!("p-{index}")), "active");
        }
        assert_eq!(recent.len(), MAX_RECENT_PROJECTS);
        record_switch(&mut recent, Some("p-39"), "active");
        assert_eq!(recent.iter().filter(|id| *id == "p-39").count(), 1);
    }
}
