//! Startup phase timing, with declared ceilings.
//!
//! A demo is a latency problem wearing a features costume, and the number that
//! decides it — how long from asking to seeing — is not currently measured
//! anywhere. This module makes it measured, and keeps it measured, rather than
//! measuring it once by hand and hoping.
//!
//! Three decisions are baked in, each because of a way a latency budget usually
//! stops meaning anything:
//!
//! - **A budget that is not enforced is a wish.** Exceeding a ceiling is
//!   reported as a breach, loudly and by name, not folded into an average.
//! - **Cold and warm are different products.** One budget covering both leaves
//!   either the cold path unenforced or the warm path untested, so a [`Budget`]
//!   declares which start it describes and a report says which it was measured
//!   against.
//! - **A boot that fails still has a story.** Phases are recorded as they
//!   happen, so a start that died in the third phase reports two completed
//!   phases and where it stopped, rather than reporting nothing at all.
//!
//! Clock readings are passed in rather than taken here, so a test can assert on
//! a boot that took nine seconds without waiting nine seconds.

use serde::{Deserialize, Serialize};

/// Whether caches were warm when this start began.
///
/// Not cosmetic: a first run downloading a binary and a second run with
/// everything resident are different experiences with different acceptable
/// numbers, and holding them to one ceiling means lying about one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Start {
    Cold,
    Warm,
}

/// A ceiling for one named phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ceiling {
    pub phase: String,
    pub limit_ms: u64,
}

/// What this start is expected to cost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Budget {
    pub start: Start,
    pub ceilings: Vec<Ceiling>,
    /// Ceiling for the whole sequence. Separate from the sum of the parts on
    /// purpose: a start can hold every phase budget and still take too long,
    /// and it can blow one phase badly while finishing inside the total. Both
    /// are worth knowing and neither implies the other.
    pub total_ms: u64,
}

impl Budget {
    fn limit(&self, phase: &str) -> Option<u64> {
        self.ceilings
            .iter()
            .find(|ceiling| ceiling.phase == phase)
            .map(|ceiling| ceiling.limit_ms)
    }
}

/// A phase that has finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Phase {
    pub name: String,
    pub started_ms: u64,
    pub ended_ms: u64,
}

impl Phase {
    pub fn took_ms(&self) -> u64 {
        self.ended_ms.saturating_sub(self.started_ms)
    }
}

/// A start in progress, or one that has finished.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trace {
    pub start: Start,
    pub began_ms: u64,
    pub phases: Vec<Phase>,
    /// The phase currently running, if any. Present in a trace that never
    /// finished, which is exactly the trace worth reading.
    pub open: Option<(String, u64)>,
}

impl Trace {
    pub fn begin(start: Start, now_ms: u64) -> Self {
        Self {
            start,
            began_ms: now_ms,
            phases: Vec::new(),
            open: None,
        }
    }

    /// Start a phase, closing whichever one was open.
    ///
    /// Sequential by construction: a boot sequence is a sequence, and letting
    /// phases overlap would make the numbers unattributable.
    pub fn enter(&mut self, name: &str, now_ms: u64) {
        self.close(now_ms);
        self.open = Some((name.to_string(), now_ms));
    }

    /// Close the open phase without starting another.
    pub fn close(&mut self, now_ms: u64) {
        if let Some((name, started_ms)) = self.open.take() {
            self.phases.push(Phase {
                name,
                started_ms,
                ended_ms: now_ms.max(started_ms),
            });
        }
    }

    pub fn elapsed_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.began_ms)
    }

    /// Score this start against a budget.
    ///
    /// Refuses to score against a budget for the other kind of start, rather
    /// than quietly producing a number that means nothing.
    pub fn report(&self, budget: &Budget, now_ms: u64) -> Result<Report, String> {
        if budget.start != self.start {
            return Err(format!(
                "cannot judge a {:?} start against a {:?} budget",
                self.start, budget.start
            ));
        }
        let mut phases = Vec::new();
        for phase in &self.phases {
            let limit_ms = budget.limit(&phase.name);
            phases.push(PhaseReport {
                name: phase.name.clone(),
                took_ms: phase.took_ms(),
                limit_ms,
                over_ms: limit_ms
                    .map(|limit| phase.took_ms().saturating_sub(limit))
                    .unwrap_or(0),
            });
        }
        let total_ms = self.elapsed_ms(now_ms);
        Ok(Report {
            start: self.start,
            unfinished: self.open.as_ref().map(|(name, _)| name.clone()),
            unbudgeted: phases
                .iter()
                .filter(|phase| phase.limit_ms.is_none())
                .map(|phase| phase.name.clone())
                .collect(),
            breaches: phases
                .iter()
                .filter(|phase| phase.over_ms > 0)
                .map(|phase| phase.name.clone())
                .collect(),
            over_total_ms: total_ms.saturating_sub(budget.total_ms),
            total_ms,
            phases,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhaseReport {
    pub name: String,
    pub took_ms: u64,
    /// `None` when nothing declared a ceiling for this phase — which is itself
    /// a finding, not a pass.
    pub limit_ms: Option<u64>,
    pub over_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub start: Start,
    pub phases: Vec<PhaseReport>,
    pub total_ms: u64,
    /// Phases that ran over their own ceiling.
    pub breaches: Vec<String>,
    /// How far past the whole-sequence ceiling, zero if inside it.
    pub over_total_ms: u64,
    /// Phases nothing budgeted. An unbudgeted phase is not a passing phase; it
    /// is a gap in the budget, and it is where the next regression will hide.
    pub unbudgeted: Vec<String>,
    /// Set when the trace never finished, naming where it stopped.
    pub unfinished: Option<String>,
}

impl Report {
    /// Whether this start met its budget.
    ///
    /// An unfinished start is never a pass, even if what completed was fast: a
    /// boot that stopped in phase three is the worst possible latency.
    pub fn met(&self) -> bool {
        self.unfinished.is_none() && self.breaches.is_empty() && self.over_total_ms == 0
    }

    /// One line for a log, naming what to look at rather than only the total.
    pub fn summary(&self) -> String {
        if let Some(phase) = &self.unfinished {
            return format!(
                "boot did not finish: stopped in {phase} after {}ms",
                self.total_ms
            );
        }
        if self.met() {
            return format!("boot {:?} {}ms, within budget", self.start, self.total_ms);
        }
        let mut parts = vec![format!("boot {:?} {}ms", self.start, self.total_ms)];
        if self.over_total_ms > 0 {
            parts.push(format!("{}ms over total", self.over_total_ms));
        }
        for phase in &self.phases {
            if phase.over_ms > 0 {
                parts.push(format!(
                    "{} {}ms over its {}ms ceiling",
                    phase.name,
                    phase.over_ms,
                    phase.limit_ms.unwrap_or(0)
                ));
            }
        }
        parts.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budget(start: Start) -> Budget {
        Budget {
            start,
            ceilings: vec![
                Ceiling {
                    phase: "displays".into(),
                    limit_ms: 200,
                },
                Ceiling {
                    phase: "workspace".into(),
                    limit_ms: 400,
                },
                Ceiling {
                    phase: "first-frame".into(),
                    limit_ms: 600,
                },
            ],
            total_ms: 1_500,
        }
    }

    fn good_boot() -> Trace {
        let mut trace = Trace::begin(Start::Warm, 0);
        trace.enter("displays", 0);
        trace.enter("workspace", 100);
        trace.enter("first-frame", 400);
        trace.close(900);
        trace
    }

    #[test]
    fn a_start_inside_every_ceiling_passes() {
        let report = good_boot().report(&budget(Start::Warm), 900).expect("report");
        assert!(report.met(), "{}", report.summary());
        assert_eq!(report.total_ms, 900);
        assert!(report.breaches.is_empty());
        assert_eq!(report.summary(), "boot Warm 900ms, within budget");
    }

    #[test]
    fn a_phase_over_its_ceiling_is_named_not_averaged_away() {
        let mut trace = Trace::begin(Start::Warm, 0);
        trace.enter("displays", 0);
        // 900ms in a phase budgeted for 200.
        trace.enter("workspace", 900);
        trace.close(1_000);
        let report = trace.report(&budget(Start::Warm), 1_000).expect("report");
        assert!(!report.met());
        assert_eq!(report.breaches, vec!["displays"]);
        assert!(
            report.summary().contains("displays 700ms over its 200ms ceiling"),
            "{}",
            report.summary()
        );
    }

    #[test]
    fn holding_every_phase_budget_does_not_excuse_a_slow_total() {
        let mut budget = budget(Start::Warm);
        budget.total_ms = 500;
        let report = good_boot().report(&budget, 900).expect("report");
        assert!(report.breaches.is_empty(), "no single phase misbehaved");
        assert_eq!(report.over_total_ms, 400);
        assert!(!report.met(), "the sum still blew the budget");
    }

    #[test]
    fn a_boot_that_never_finished_is_never_a_pass() {
        let mut trace = Trace::begin(Start::Warm, 0);
        trace.enter("displays", 0);
        trace.enter("workspace", 50);
        // Never closed: the boot hung here.
        let report = trace.report(&budget(Start::Warm), 60_000).expect("report");
        assert!(!report.met());
        assert_eq!(report.unfinished.as_deref(), Some("workspace"));
        assert!(
            report.summary().starts_with("boot did not finish: stopped in workspace"),
            "{}",
            report.summary()
        );
        assert_eq!(
            report.phases.len(),
            1,
            "the phase that did complete is still reported"
        );
    }

    #[test]
    fn a_phase_nobody_budgeted_is_reported_as_a_gap_not_a_pass() {
        let mut trace = Trace::begin(Start::Warm, 0);
        trace.enter("displays", 0);
        trace.enter("shader-warmup", 100);
        trace.close(200);
        let report = trace.report(&budget(Start::Warm), 200).expect("report");
        assert_eq!(report.unbudgeted, vec!["shader-warmup"]);
        assert!(
            report.met(),
            "an unbudgeted phase does not fail the run - but it is visible"
        );
    }

    #[test]
    fn a_cold_start_is_not_judged_against_a_warm_budget() {
        let cold = Trace::begin(Start::Cold, 0);
        let error = cold.report(&budget(Start::Warm), 100).expect_err("must refuse");
        assert!(error.contains("Cold"), "{error}");
        assert!(
            cold.report(&budget(Start::Cold), 100).is_ok(),
            "the matching budget is accepted"
        );
    }

    #[test]
    fn entering_a_phase_closes_the_one_before_it() {
        let mut trace = Trace::begin(Start::Warm, 0);
        trace.enter("a", 0);
        trace.enter("b", 10);
        trace.close(30);
        assert_eq!(trace.phases.len(), 2);
        assert_eq!(trace.phases[0].took_ms(), 10);
        assert_eq!(trace.phases[1].took_ms(), 20);
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_produce_a_negative_phase() {
        let mut trace = Trace::begin(Start::Warm, 1_000);
        trace.enter("displays", 1_000);
        trace.close(500);
        assert_eq!(trace.phases[0].took_ms(), 0);
        assert_eq!(trace.elapsed_ms(500), 0);
    }
}
