//! Frozen MRU window cycling. Preview never changes focus or recent order.
#[derive(Debug, Default)]
pub struct WindowCycle {
    recent: Vec<String>,
    preview: Option<(Vec<String>, usize)>,
}

impl WindowCycle {
    pub fn observe_focus(&mut self, id: &str) {
        if self.preview.is_none() && self.recent.first().map(String::as_str) != Some(id) {
            self.recent.retain(|old| old != id);
            self.recent.insert(0, id.to_owned());
            self.recent.truncate(512);
        }
    }

    pub fn active(&self) -> bool {
        self.preview.is_some()
    }

    pub fn cycle(&mut self, available: &[String], reverse: bool) -> Option<&str> {
        if self.preview.is_none() {
            self.recent.retain(|id| available.contains(id));
            let mut order = self.recent.clone();
            for id in available {
                if !order.contains(id) {
                    order.push(id.clone());
                }
            }
            if order.is_empty() {
                return None;
            }
            self.preview = Some((order, 0));
        }
        let (order, selected) = self.preview.as_mut()?;
        *selected = if reverse {
            (*selected + order.len() - 1) % order.len()
        } else {
            (*selected + 1) % order.len()
        };
        Some(&order[*selected])
    }

    pub fn order(&self) -> &[String] {
        self.preview
            .as_ref()
            .map(|(order, _)| order.as_slice())
            .unwrap_or(&[])
    }

    pub fn commit(&mut self, available: &[String]) -> Option<String> {
        let (order, selected) = self.preview.take()?;
        let id = order[selected].clone();
        // A closed window must not accidentally redirect input to another one.
        if !available.contains(&id) {
            return None;
        }
        self.observe_focus(&id);
        Some(id)
    }

    pub fn cancel(&mut self) {
        self.preview = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn windows() -> Vec<String> {
        ["a", "b", "c"].map(String::from).to_vec()
    }

    #[test]
    fn release_promotes_selected_and_next_chord_toggles_back() {
        let mut cycle = WindowCycle::default();
        cycle.observe_focus("a");
        assert_eq!(cycle.cycle(&windows(), false), Some("b"));
        assert_eq!(cycle.commit(&windows()).as_deref(), Some("b"));
        assert_eq!(cycle.cycle(&windows(), false), Some("a"));
    }

    #[test]
    fn reverse_wraps_and_preview_is_frozen() {
        let mut cycle = WindowCycle::default();
        cycle.observe_focus("a");
        assert_eq!(cycle.cycle(&windows(), true), Some("c"));
        cycle.observe_focus("b");
        assert_eq!(cycle.cycle(&windows(), false), Some("a"));
        cycle.cancel();
        assert_eq!(cycle.cycle(&windows(), false), Some("b"));
    }

    #[test]
    fn closed_selection_is_not_replaced_on_commit() {
        let mut cycle = WindowCycle::default();
        cycle.cycle(&windows(), false);
        assert_eq!(cycle.commit(&["a".into(), "c".into()]), None);
        assert!(!cycle.active());
    }

    #[test]
    fn empty_and_single_window_are_safe() {
        let mut cycle = WindowCycle::default();
        assert_eq!(cycle.cycle(&[], false), None);
        assert_eq!(cycle.cycle(&["a".into()], true), Some("a"));
        assert_eq!(cycle.commit(&["a".into()]), Some("a".into()));
    }
}
