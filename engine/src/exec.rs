//! Execution orchestration — the resilient core that turns a `Plan` into applied
//! changes: per-action failure isolation (errors *and* panics), honest statuses,
//! post-verify, and a revert log captured before anything is changed.
//!
//! All OS work sits behind the [`Backend`] trait, so this file is pure Rust and
//! unit-tested anywhere with a mock. `exec_windows.rs` is the real backend.
//!
//! Status contract — only a real error is an error:
//!   Applied        the change was made now
//!   AlreadyApplied the system was already in the wanted state (nothing written)
//!   NotApplicable  the target does not exist on this Windows (service, app,
//!                  task, process…) — nothing to do, not a failure
//!   Failed         the change was attempted and Windows refused / it errored
//!   Skipped        dry-run only

use crate::catalog::{Catalog, Plan};
use crate::schema::{Action, Check, Tweak};
use serde::Serialize;

/// What a successful `apply` found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The change was made.
    Done,
    /// Already in the wanted state; nothing was written. Reason for the log.
    Already(String),
    /// The target does not exist on this machine. Reason for the log.
    NotApplicable(String),
}

/// The OS primitives the engine needs. One method per concern so the FFI stays
/// small and every call site is auditable.
pub trait Backend {
    /// Apply one action. `Ok(Applied)` on success, `Err(reason)` on failure.
    fn apply(&mut self, action: &Action) -> Result<Applied, String>;

    /// Evaluate a post-apply verification probe (true = the change is in effect).
    fn verify(&mut self, check: &Check) -> Result<bool, String>;

    /// The inverse of `action`, read from the system *before* it runs, so a
    /// rollback can restore the exact original. `None` when nothing to restore.
    fn snapshot(&mut self, action: &Action) -> Option<Action>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    Applied,
    Verified,
    AlreadyApplied,
    NotApplicable,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionOutcome {
    pub index: usize,
    pub status: Status,
    /// Error text (with code) for Failed; short reason for the other states.
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TweakOutcome {
    pub id: String,
    pub actions: Vec<ActionOutcome>,
    /// Inverse actions captured before applying, newest first (for rollback).
    pub revert: Vec<Action>,
}

impl TweakOutcome {
    pub fn failed(&self) -> bool {
        self.actions.iter().any(|a| a.status == Status::Failed)
    }
}

/// Per-status totals (actions), plus tweaks skipped as incompatible.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Counts {
    pub applied: usize,
    pub already: usize,
    pub not_applicable: usize,
    pub failed: usize,
    pub skipped: usize,
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub tweaks: Vec<TweakOutcome>,
    pub skipped_incompatible: Vec<String>,
}

impl Report {
    pub fn counts(&self) -> Counts {
        let mut c = Counts { skipped: self.skipped_incompatible.len(), ..Counts::default() };
        for a in self.tweaks.iter().flat_map(|t| &t.actions) {
            match a.status {
                Status::Applied | Status::Verified => c.applied += 1,
                Status::AlreadyApplied => c.already += 1,
                Status::NotApplicable => c.not_applicable += 1,
                Status::Failed => c.failed += 1,
                Status::Skipped => c.skipped += 1,
            }
        }
        c
    }

    /// Only real failures: (tweak id, action index, error incl. code).
    pub fn failures(&self) -> Vec<(String, usize, String)> {
        self.tweaks
            .iter()
            .flat_map(|t| {
                t.actions
                    .iter()
                    .filter(|a| a.status == Status::Failed)
                    .map(move |a| (t.id.clone(), a.index, a.detail.clone().unwrap_or_default()))
            })
            .collect()
    }
}

/// Which phase to run (the planner already split immediate vs reboot-required).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Immediate,
    Deferred,
}

/// Apply one phase of a plan (see [`run_phase_observed`]).
pub fn run_phase<B: Backend>(catalog: &Catalog, plan: &Plan, phase: Phase, backend: &mut B, dry_run: bool) -> Report {
    run_phase_observed(catalog, plan, phase, backend, dry_run, &mut |_| {})
}

/// Apply one phase, calling `on_tweak` after each tweak so the UI shows live
/// progress. Every action is isolated: an error or even a panic is recorded and
/// execution continues — never aborts. `dry_run` touches nothing.
pub fn run_phase_observed<B: Backend>(
    catalog: &Catalog,
    plan: &Plan,
    phase: Phase,
    backend: &mut B,
    dry_run: bool,
    on_tweak: &mut dyn FnMut(&TweakOutcome),
) -> Report {
    let ids = match phase {
        Phase::Immediate => &plan.immediate,
        Phase::Deferred => &plan.deferred,
    };
    let mut report = Report { tweaks: Vec::new(), skipped_incompatible: plan.skipped_incompatible.clone() };
    for id in ids {
        let Some(tweak) = catalog.get(id) else { continue };
        let outcome = apply_tweak(tweak, backend, dry_run);
        on_tweak(&outcome);
        report.tweaks.push(outcome);
    }
    report
}

fn panic_text(p: Box<dyn std::any::Any + Send>) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".into())
}

fn apply_tweak<B: Backend>(tweak: &Tweak, backend: &mut B, dry_run: bool) -> TweakOutcome {
    let mut outcome = TweakOutcome { id: tweak.id.clone(), actions: Vec::new(), revert: Vec::new() };

    for (i, action) in tweak.actions.iter().enumerate() {
        if dry_run {
            outcome.actions.push(ActionOutcome { index: i, status: Status::Skipped, detail: Some("dry-run".into()) });
            continue;
        }

        // Original state first (newest first, so a rollback unwinds in order).
        let snap = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.snapshot(action)))
            .unwrap_or(None);

        // A panic inside one action (a bug — never expected) becomes its error.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.apply(action)))
            .unwrap_or_else(|p| Err(format!("internal error: {}", panic_text(p))));

        let (status, detail) = match result {
            Ok(Applied::Done) => {
                // keep the inverse only for changes we actually made
                if let Some(inv) = snap {
                    outcome.revert.insert(0, inv);
                }
                (Status::Applied, None)
            }
            Ok(Applied::Already(why)) => (Status::AlreadyApplied, Some(why)),
            Ok(Applied::NotApplicable(why)) => (Status::NotApplicable, Some(why)),
            Err(e) => (Status::Failed, Some(e)),
        };
        outcome.actions.push(ActionOutcome { index: i, status, detail });
    }

    // Post-apply verification: a passing probe upgrades, a failing one is an error.
    if !dry_run {
        for check in &tweak.verify {
            match backend.verify(check) {
                Ok(true) => {
                    if let Some(last) = outcome.actions.iter_mut().rev().find(|a| a.status == Status::Applied) {
                        last.status = Status::Verified;
                    }
                }
                Ok(false) => outcome.actions.push(ActionOutcome {
                    index: usize::MAX,
                    status: Status::Failed,
                    detail: Some("verify: change not in effect".into()),
                }),
                Err(e) => outcome.actions.push(ActionOutcome {
                    index: usize::MAX,
                    status: Status::Failed,
                    detail: Some(format!("verify error: {e}")),
                }),
            }
        }
    }
    outcome
}

/// Roll back a prior run from its captured revert actions (newest first).
pub fn rollback<B: Backend>(revert: &[Action], backend: &mut B) -> Vec<(usize, Result<Applied, String>)> {
    revert.iter().enumerate().map(|(i, a)| (i, backend.apply(a))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SystemProfile;
    use crate::schema::{Device, Disk, Gpu};

    /// Records calls. Value name decides the result: "boom" fails, "panic"
    /// panics, "absent" is not applicable, "same" is already applied.
    #[derive(Default)]
    struct MockBackend {
        applied: Vec<String>,
    }
    impl Backend for MockBackend {
        fn apply(&mut self, action: &Action) -> Result<Applied, String> {
            let j = serde_json::to_string(action).unwrap();
            self.applied.push(j.clone());
            if j.contains("boom") {
                Err("simulated failure (0x80070005)".into())
            } else if j.contains("panic") {
                panic!("simulated bug")
            } else if j.contains("absent") {
                Ok(Applied::NotApplicable("not present".into()))
            } else if j.contains("same") {
                Ok(Applied::Already("value already set".into()))
            } else {
                Ok(Applied::Done)
            }
        }
        fn verify(&mut self, _c: &Check) -> Result<bool, String> {
            Ok(true)
        }
        fn snapshot(&mut self, a: &Action) -> Option<Action> {
            Some(a.clone())
        }
    }

    fn sys() -> SystemProfile {
        SystemProfile { build: 26100, edition: "pro".into(), device: Device::Desktop, gpu: Gpu::Nvidia, disk: Disk::Ssd }
    }

    fn tweak(id: &str, names: &[&str]) -> Tweak {
        let acts: Vec<String> = names
            .iter()
            .map(|n| format!(r#"{{"type":"registry","hive":"HKCU","path":"p","name":"{n}","kind":"dword","data":1}}"#))
            .collect();
        let json =
            format!(r#"{{"id":"{id}","name":{{"ar":"x","en":"x"}},"category":"t","actions":[{}]}}"#, acts.join(","));
        serde_json::from_str(&json).unwrap()
    }

    fn run(cat: &Catalog, ids: &[&str]) -> (Report, MockBackend) {
        let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
        let plan = cat.plan(&ids, &sys()).unwrap();
        let mut be = MockBackend::default();
        let r = run_phase(cat, &plan, Phase::Immediate, &mut be, false);
        (r, be)
    }

    #[test]
    fn failure_is_isolated_and_reported() {
        let cat =
            Catalog::new(vec![tweak("ok", &["good1", "good2"]), tweak("mix", &["good3", "boom", "good4"])]).unwrap();
        let (report, be) = run(&cat, &["ok", "mix"]);
        assert_eq!(be.applied.len(), 5); // every action attempted
        let c = report.counts();
        assert_eq!((c.applied, c.failed), (4, 1));
        assert_eq!(report.failures()[0].0, "mix");
    }

    #[test]
    fn panic_in_one_action_does_not_stop_the_run() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {})); // keep test output clean
        let cat = Catalog::new(vec![tweak("t", &["a", "panic", "b"])]).unwrap();
        let (report, be) = run(&cat, &["t"]);
        std::panic::set_hook(prev);
        assert_eq!(be.applied.len(), 3);
        let c = report.counts();
        assert_eq!((c.applied, c.failed), (2, 1));
        assert!(report.failures()[0].2.contains("simulated bug"));
    }

    #[test]
    fn only_real_errors_count_as_failures() {
        let cat = Catalog::new(vec![tweak("t", &["good", "absent", "same"])]).unwrap();
        let (report, _) = run(&cat, &["t"]);
        let c = report.counts();
        assert_eq!((c.applied, c.not_applicable, c.already, c.failed), (1, 1, 1, 0));
        assert!(report.failures().is_empty());
        // revert log only holds changes that were actually made
        assert_eq!(report.tweaks[0].revert.len(), 1);
    }

    #[test]
    fn observer_sees_every_tweak() {
        let cat = Catalog::new(vec![tweak("a", &["x"]), tweak("b", &["y"])]).unwrap();
        let plan = cat.plan(&["a".into(), "b".into()], &sys()).unwrap();
        let mut seen = Vec::new();
        let mut be = MockBackend::default();
        run_phase_observed(&cat, &plan, Phase::Immediate, &mut be, false, &mut |t| seen.push(t.id.clone()));
        seen.sort();
        assert_eq!(seen, vec!["a", "b"]);
    }

    #[test]
    fn dry_run_touches_nothing() {
        let cat = Catalog::new(vec![tweak("ok", &["a", "b"])]).unwrap();
        let plan = cat.plan(&["ok".into()], &sys()).unwrap();
        let mut be = MockBackend::default();
        let _ = run_phase(&cat, &plan, Phase::Immediate, &mut be, true);
        assert!(be.applied.is_empty());
    }
}
