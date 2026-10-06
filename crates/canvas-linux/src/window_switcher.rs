//! Frozen MRU order for a held Alt-Tab gesture. No platform I/O.
#[derive(Default)]
pub struct WindowSwitcher {
    order: Vec<String>,
    selected: Option<String>,
}
impl WindowSwitcher {
    pub fn active(&self) -> bool {
        self.selected.is_some()
    }
    /// Presentation reads the frozen gesture order, never the changing live MRU.
    pub fn snapshot(&self) -> serde_json::Value {
        serde_json::json!({
            "active": self.active(),
            "windows": self.order,
            "selected_window": self.selected,
        })
    }
    pub fn finish(&mut self) {
        self.order.clear();
        self.selected = None;
    }
    pub fn reconcile(&mut self, live_mru: &[String], focused: Option<&str>) {
        if !self.active() {
            return;
        }
        self.order.retain(|id| live_mru.contains(id));
        if self.order.is_empty() {
            self.finish();
            return;
        }
        if !self
            .selected
            .as_ref()
            .is_some_and(|id| self.order.contains(id))
        {
            self.selected = focused
                .filter(|id| self.order.iter().any(|window| window == id))
                .map(str::to_owned)
                .or_else(|| self.order.first().cloned());
        }
    }
    pub fn next(
        &mut self,
        live_mru: &[String],
        focused: Option<&str>,
        reverse: bool,
    ) -> Option<String> {
        if self.order.is_empty() {
            self.order = live_mru.to_vec();
            self.selected = focused.map(str::to_owned);
        }
        self.order.retain(|id| live_mru.contains(id));
        if self.order.is_empty() {
            self.finish();
            return None;
        }
        let index = self
            .selected
            .as_ref()
            .and_then(|id| self.order.iter().position(|v| v == id));
        let next = match index {
            Some(i) if reverse => (i + self.order.len() - 1) % self.order.len(),
            Some(i) => (i + 1) % self.order.len(),
            None if reverse => self.order.len() - 1,
            None => 0,
        };
        let id = self.order[next].clone();
        self.selected = Some(id.clone());
        Some(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids(s: &[&str]) -> Vec<String> {
        s.iter().map(|v| v.to_string()).collect()
    }
    #[test]
    fn held_gesture_keeps_order_despite_focus_updates() {
        let mut s = WindowSwitcher::default();
        assert_eq!(
            s.next(&ids(&["a", "b", "c"]), Some("a"), false).as_deref(),
            Some("b")
        );
        assert_eq!(
            s.next(&ids(&["b", "a", "c"]), Some("b"), false).as_deref(),
            Some("c")
        );
        assert_eq!(s.snapshot()["windows"], serde_json::json!(["a", "b", "c"]));
        assert_eq!(s.snapshot()["selected_window"], "c");
        assert_eq!(
            s.next(&ids(&["c", "b", "a"]), Some("c"), true).as_deref(),
            Some("b")
        );
        s.finish();
        assert_eq!(s.snapshot()["active"], false);
        assert!(s.snapshot()["selected_window"].is_null());
        assert_eq!(
            s.next(&ids(&["b", "c", "a"]), Some("b"), false).as_deref(),
            Some("c")
        );
    }
    #[test]
    fn closed_windows_are_skipped_and_new_windows_wait_for_next_gesture() {
        let mut s = WindowSwitcher::default();
        s.next(&ids(&["a", "b", "c"]), Some("a"), false);
        assert_eq!(
            s.next(&ids(&["b", "a", "d"]), Some("b"), false).as_deref(),
            Some("a")
        );
        assert!(s.next(&[], None, false).is_none());
        assert_eq!(s.next(&ids(&["a"]), Some("a"), true).as_deref(), Some("a"));
    }
    #[test]
    fn closing_the_preview_updates_presentation_without_another_tab_press() {
        let mut s = WindowSwitcher::default();
        s.next(&ids(&["a", "b", "c"]), Some("a"), false);
        s.reconcile(&ids(&["a", "c", "new"]), Some("a"));
        assert_eq!(s.snapshot()["windows"], serde_json::json!(["a", "c"]));
        assert_eq!(s.snapshot()["selected_window"], "a");
        s.reconcile(&[], None);
        assert!(!s.active());
    }
}
