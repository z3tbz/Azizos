//! Execution orchestration — the resilient core that turns a `Plan` into applied
//! changes, with failure isolation, post-verify, an automatic revert log and a
//! bilingual report.
//!
//! All OS-specific work is behind the [`Backend`] trait, so this orchestration is
//! pure Rust: it builds and is unit-tested on any platform with a mock backend.
//! The real `WindowsBackend` (see `exec_windows.rs`, cfg(windows)) implements the
//! trait with the `windows` / `windows-registry` crates. See `docs/ENGINE.md`.

use crate::catalog::{Catalog, Plan};
use crate::schema::{Action, Check, Tweak};
use serde::Serialize;

/// The OS primitives the engine needs. One method per concern so the risky FFI
/// stays small and every call site is auditable.
/// What a successful `apply` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Applied {
    /// The change was made.
    Done,
    /// Nothing to do on this machine (service not present, value already
    /// absent, …). Reported as skipped — not a failure, not a fake success.
    NotApplicable(String),
}

pub trait Backend {
    /// Apply one action. Ok(Done | NotApplicable) on success, Err(reason) on failure.
    fn apply(&mut self, action: &Action) -> Result<Applied, String>;

    /// Evaluate a post-apply verification probe (true = the change is in effect).
    fn verify(&mut self, check: &Check) -> Result<bool, String>;

    /// Capture the current state an action will overwrite, as an inverse action
    /// that would restore it. `None` when nothing needs saving (or cannot be).
    fn snapshot(&mut self, action: &Action) -> Option<Action>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Status {
    Applied,
    Verified,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct ActionOutcome {
    pub index: usize,
    pub status: Status,
    /// Failure reason / skip reason, if any.
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

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    pub tweaks: Vec<TweakOutcome>,
    pub skipped_incompatible: Vec<String>,
}

impl Report {
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut applied = 0;
        let mut failed = 0;
        let mut skipped = self.skipped_incompatible.len();
        for t in &self.tweaks {
            for a in &t.actions {
                match a.status {
                    Status::Applied | Status::Verified => applied += 1,
                    Status::Failed => failed += 1,
                    Status::Skipped => skipped += 1,
                }
            }
        }
        (applied, failed, skipped)
    }

    /// Flattened list of failures for the on-screen report and desktop archive.
    pub fn failures(&self) -> Vec<(String, usize, String)> {
        let mut out = Vec::new();
        for t in &self.tweaks {
            for a in &t.actions {
                if a.status == Status::Failed {
                    out.push((
                        t.id.clone(),
                        a.index,
                        a.detail.clone().unwrap_or_default(),
                    ));
                }
            }
        }
        out
    }
}

/// Which phase to run (the planner already split immediate vs reboot-required).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Immediate,
    Deferred,
}

/// Apply one phase of a plan. Every action is isolated: a failure is recorded and
/// execution continues (never aborts, never panics), exactly like AME's resilience
/// but with structured results. `dry_run` records what *would* happen without
/// calling `apply`.
pub fn run_phase<B: Backend>(
    catalog: &Catalog,
    plan: &Plan,
    phase: Phase,
    backend: &mut B,
    dry_run: bool,
) -> Report {
    run_phase_observed(catalog, plan, phase, backend, dry_run, &mut |_| {})
}

/// Same as [`run_phase`], calling `on_tweak` after each tweak finishes so the UI
/// can show live progress (the outcome carries id, per-action status and errors).
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
    let mut report = Report {
        tweaks: Vec::new(),
        skipped_incompatible: plan.skipped_incompatible.clone(),
    };

    for id in ids {
        let Some(tweak) = catalog.get(id) else { continue };
        let outcome = apply_tweak(tweak, backend, dry_run);
        on_tweak(&outcome);
        report.tweaks.push(outcome);
    }
    report
}

fn apply_tweak<B: Backend>(tweak: &Tweak, backend: &mut B, dry_run: bool) -> TweakOutcome {
    let mut outcome = TweakOutcome {
        id: tweak.id.clone(),
        actions: Vec::new(),
        revert: Vec::new(),
    };

    for (i, action) in tweak.actions.iter().enumerate() {
        if dry_run {
            outcome.actions.push(ActionOutcome {
                index: i,
                status: Status::Skipped,
                detail: Some("dry-run".into()),
            });
            continue;
        }

        // Capture revert info before mutating (newest first so rollback unwinds).
        if let Some(inv) = backend.snapshot(action) {
            outcome.revert.insert(0, inv);
        }

        // A panic in one action (a bug, never expected) becomes a recorded failure.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend.apply(action)))
            .unwrap_or_else(|p| {
                let msg = p
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| p.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "unknown".into());
                Err(format!("internal error: {msg}"))
            });
        let status = match result {
            Ok(Applied::Done) => Status::Applied,
            Ok(Applied::NotApplicable(why)) => {
                outcome.actions.push(ActionOutcome { index: i, status: Status::Skipped, detail: Some(why) });
                continue;
            }
            Err(e) => {
                outcome.actions.push(ActionOutcome {
                    index: i,
                    status: Status::Failed,
                    detail: Some(e),
                });
                continue; // isolate: keep going with the next action
            }
        };
        outcome.actions.push(ActionOutcome {
            index: i,
            status,
            detail: None,
        });
    }

    // Post-apply verification (best effort; a failed probe downgrades to Failed).
    if !dry_run {
        for check in &tweak.verify {
            match backend.verify(check) {
                Ok(true) => {
                    if let Some(last) = outcome
                        .actions
                        .iter_mut()
                        .rev()
                        .find(|a| a.status == Status::Applied)
                    {
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
    revert
        .iter()
        .enumerate()
        .map(|(i, a)| (i, backend.apply(a)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::SystemProfile;
    use crate::schema::{Device, Disk, Gpu};

    /// A backend that records calls and fails any action whose JSON mentions "boom".
    #[derive(Default)]
    struct MockBackend {
        applied: Vec<String>,
    }
    impl Backend for MockBackend {
        fn apply(&mut self, action: &Action) -> Result<Applied, String> {
            let j = serde_json::to_string(action).unwrap();
            self.applied.push(j.clone());
            if j.contains("boom") {
                Err("simulated failure".into())
            } else if j.contains("absent") {
                Ok(Applied::NotApplicable("not present".into()))
            } else {
                Ok(Applied::Done)
            }
        }
        fn verify(&mut self, _c: &Check) -> Result<bool, String> {
            Ok(true)
        }
        fn snapshot(&mut self, _a: &Action) -> Option<Action> {
            None
        }
    }

    fn sys() -> SystemProfile {
        SystemProfile {
            build: 26100,
            edition: "pro".into(),
            device: Device::Desktop,
            gpu: Gpu::Nvidia,
            disk: Disk::Ssd,
        }
    }

    fn tweak(id: &str, names: &[&str]) -> Tweak {
        let acts: Vec<String> = names
            .iter()
            .map(|n| {
                format!(
                    r#"{{"type":"registry","hive":"HKCU","path":"p","name":"{n}","kind":"dword","data":1}}"#
                )
            })
            .collect();
        let json = format!(
            r#"{{"id":"{id}","name":{{"ar":"x","en":"x"}},"category":"t","actions":[{}]}}"#,
            acts.join(",")
        );
        serde_json::from_str(&json).unwrap()
    }

    #[test]
    fn failure_is_isolated_and_reported() {
        let cat = Catalog::new(vec![
            tweak("ok", &["good1", "good2"]),
            tweak("mix", &["good3", "boom", "good4"]),
        ])
        .unwrap();
        let plan = cat.plan(&["ok".into(), "mix".into()], &sys()).unwrap();

        let mut be = MockBackend::default();
        let report = run_phase(&cat, &plan, Phase::Immediate, &mut be, false);

        // all 5 actions attempted despite the failure in the middle
        assert_eq!(be.applied.len(), 5);
        let (applied, failed, _skipped) = report.counts();
        assert_eq!(failed, 1);
        assert_eq!(applied, 4);
        assert_eq!(report.failures()[0].0, "mix");
    }

    #[test]
    fn not_applicable_is_skipped_not_failed() {
        let cat = Catalog::new(vec![tweak("t", &["good", "absent"])]).unwrap();
        let plan = cat.plan(&["t".into()], &sys()).unwrap();
        let mut be = MockBackend::default();
        let mut seen = 0;
        let report = run_phase_observed(&cat, &plan, Phase::Immediate, &mut be, false, &mut |_| seen += 1);
        assert_eq!(seen, 1);
        assert_eq!(report.counts(), (1, 0, 1));
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
