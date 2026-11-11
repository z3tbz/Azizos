//! Load, validate and plan tweaks — all platform-independent (no system calls).
//! The Windows-only executor consumes the ordered plan this module produces.

use crate::schema::{Action, Tweak};
use std::collections::{HashMap, HashSet};

/// What `detect` (Windows side) fills in; here it is just data used for gating.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemProfile {
    pub build: u32,
    pub edition: String, // "pro" | "ltsc" | "iot" | ...
    pub device: crate::schema::Device,
    pub gpu: crate::schema::Gpu,
    pub disk: crate::schema::Disk,
}

#[derive(Debug)]
pub enum PlanError {
    DuplicateId(String),
    UnknownDependency { tweak: String, needs: String },
    DependencyCycle(Vec<String>),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::DuplicateId(id) => write!(f, "duplicate tweak id: {id}"),
            PlanError::UnknownDependency { tweak, needs } => {
                write!(f, "tweak '{tweak}' depends on unknown '{needs}'")
            }
            PlanError::DependencyCycle(ids) => write!(f, "dependency cycle: {}", ids.join(" -> ")),
        }
    }
}

/// The full set of known tweaks.
pub struct Catalog {
    tweaks: Vec<Tweak>,
    index: HashMap<String, usize>,
}

impl Catalog {
    /// Build from already-parsed tweaks, checking ids are unique and deps resolve.
    pub fn new(tweaks: Vec<Tweak>) -> Result<Self, PlanError> {
        let mut index = HashMap::new();
        for (i, t) in tweaks.iter().enumerate() {
            if index.insert(t.id.clone(), i).is_some() {
                return Err(PlanError::DuplicateId(t.id.clone()));
            }
        }
        for t in &tweaks {
            for d in &t.depends_on {
                if !index.contains_key(d) {
                    return Err(PlanError::UnknownDependency {
                        tweak: t.id.clone(),
                        needs: d.clone(),
                    });
                }
            }
        }
        Ok(Catalog { tweaks, index })
    }

    /// Parse a batch of JSON documents (one tweak each) into a catalog.
    pub fn from_json_docs<I: IntoIterator<Item = String>>(docs: I) -> Result<Self, String> {
        let mut tweaks = Vec::new();
        for (n, doc) in docs.into_iter().enumerate() {
            let mut t: Tweak = serde_json::from_str(&doc)
                .map_err(|e| format!("tweak #{n}: {e}"))?;
            normalize(&mut t);
            // `"ignore_errors": true` sits on the action itself in the file.
            if let Ok(raw) = serde_json::from_str::<serde_json::Value>(&doc) {
                for (i, a) in raw["actions"].as_array().into_iter().flatten().enumerate() {
                    if a["ignore_errors"].as_bool() == Some(true) && !t.ignore_errors.contains(&i) {
                        t.ignore_errors.push(i);
                    }
                }
            }
            tweaks.push(t);
        }
        Catalog::new(tweaks).map_err(|e| e.to_string())
    }

    pub fn all(&self) -> &[Tweak] {
        &self.tweaks
    }
}

/// A registry key path exactly as the Reg* API wants it: no leading or
/// trailing backslash and no doubled ones. Playbooks are written by hand
/// (`HKCU\\Software\\\\X`, `\SOFTWARE\X\`), and Windows rejects those with
/// ERROR_BAD_PATHNAME — so they are cleaned once, when the catalog loads, and
/// every later step (snapshot, apply, verify, rollback) sees the clean path.
pub fn registry_path(p: &str) -> String {
    p.split('\\').filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\\")
}

fn normalize(t: &mut Tweak) {
    use crate::schema::{Action, Check};
    let fix = |a: &mut Action| {
        if let Action::Registry { path, .. } = a {
            *path = registry_path(path);
        }
    };
    t.actions.iter_mut().for_each(fix);
    if let Some(r) = t.revert.as_mut() {
        r.iter_mut().for_each(fix);
    }
    for c in &mut t.verify {
        if let Check::Registry { path, .. } = c {
            *path = registry_path(path);
        }
    }
}

impl Catalog {

    pub fn get(&self, id: &str) -> Option<&Tweak> {
        self.index.get(id).map(|&i| &self.tweaks[i])
    }

    /// True when the tweak matches the system (build/edition/device/gpu/disk).
    pub fn is_compatible(&self, t: &Tweak, sys: &SystemProfile) -> bool {
        let c = &t.compat;
        if !c.builds.contains(sys.build) {
            return false;
        }
        let ed_ok = c.editions.iter().any(|e| e == "any")
            || c.editions.iter().any(|e| e.eq_ignore_ascii_case(&sys.edition));
        if !ed_ok {
            return false;
        }
        use crate::schema::{Device, Disk, Gpu};
        if c.device != Device::Any && c.device != sys.device {
            return false;
        }
        if c.gpu != Gpu::Any && c.gpu != sys.gpu {
            return false;
        }
        if c.disk != Disk::Any && c.disk != sys.disk {
            return false;
        }
        true
    }

    /// Resolve a user selection into an ordered, deduplicated execution plan:
    /// 1. drop anything the system is not compatible with (skipped, not failed),
    /// 2. pull in dependencies transitively,
    /// 3. topologically sort by `depends_on`,
    /// 4. split into (immediate, reboot-required) phases, preserving order.
    pub fn plan(
        &self,
        selected: &[String],
        sys: &SystemProfile,
    ) -> Result<Plan, PlanError> {
        // 1 + 2: expand selection with dependencies, keeping only compatible tweaks.
        let mut wanted: HashSet<String> = HashSet::new();
        let mut stack: Vec<String> = selected.to_vec();
        let mut skipped_incompatible: Vec<String> = Vec::new();
        while let Some(id) = stack.pop() {
            let Some(t) = self.get(&id) else { continue };
            if !self.is_compatible(t, sys) {
                skipped_incompatible.push(id.clone());
                continue;
            }
            if wanted.insert(id.clone()) {
                for d in &t.depends_on {
                    stack.push(d.clone());
                }
            }
        }

        // 3: topological sort (Kahn) over the wanted set.
        let ids: Vec<String> = wanted.iter().cloned().collect();
        let mut indeg: HashMap<&str, usize> = ids.iter().map(|s| (s.as_str(), 0)).collect();
        let mut edges: HashMap<&str, Vec<&str>> = HashMap::new();
        for id in &ids {
            let t = self.get(id).unwrap();
            for d in &t.depends_on {
                if wanted.contains(d) {
                    edges.entry(d.as_str()).or_default().push(id.as_str());
                    *indeg.get_mut(id.as_str()).unwrap() += 1;
                }
            }
        }
        let mut ready: Vec<&str> = indeg
            .iter()
            .filter(|(_, &d)| d == 0)
            .map(|(&k, _)| k)
            .collect();
        let priority = |a: &&str, b: &&str| self.get(a).unwrap().order.cmp(&self.get(b).unwrap().order).then(a.cmp(b));
        ready.sort_by(priority);
        ready.reverse(); // pop the earliest source action first
        let mut order: Vec<String> = Vec::new();
        while let Some(n) = ready.pop() {
            order.push(n.to_string());
            if let Some(outs) = edges.get(n) {
                let mut newly: Vec<&str> = Vec::new();
                for &m in outs {
                    let e = indeg.get_mut(m).unwrap();
                    *e -= 1;
                    if *e == 0 {
                        newly.push(m);
                    }
                }
                ready.extend(newly);
                ready.sort_by(priority);
                ready.reverse();
            }
        }
        if order.len() != ids.len() {
            let cycle: Vec<String> = ids.into_iter().filter(|i| !order.contains(i)).collect();
            return Err(PlanError::DependencyCycle(cycle));
        }

        // 4: phase split — immediate first, reboot-required last, order preserved.
        let mut immediate = Vec::new();
        let mut deferred = Vec::new();
        let mut defer_remaining = false;
        for id in order {
            let t = self.get(&id).unwrap();
            // A phase split must never move a dependent/finalizer before its
            // prerequisite. Once deferring starts, keep the remaining suffix.
            defer_remaining |= t.requires_reboot;
            if defer_remaining {
                deferred.push(id);
            } else {
                immediate.push(id);
            }
        }

        Ok(Plan {
            immediate,
            deferred,
            skipped_incompatible,
        })
    }

    /// Conflicts among a selection (pairs of ids that declare `conflicts_with`).
    pub fn conflicts(&self, selected: &[String]) -> Vec<(String, String)> {
        let set: HashSet<&String> = selected.iter().collect();
        let mut out = Vec::new();
        for id in selected {
            if let Some(t) = self.get(id) {
                for c in &t.conflicts_with {
                    if set.contains(c) && id < c {
                        out.push((id.clone(), c.clone()));
                    }
                }
            }
        }
        out
    }
}

/// An ordered plan ready for the executor.
#[derive(Debug, Default)]
pub struct Plan {
    /// Applied first, in dependency order.
    pub immediate: Vec<String>,
    /// Applied last: their effect needs the restart that follows the run.
    pub deferred: Vec<String>,
    /// Dropped because the system does not match their `compat`.
    pub skipped_incompatible: Vec<String>,
}

impl Plan {
    pub fn total(&self) -> usize {
        self.immediate.len() + self.deferred.len()
    }
}

/// Whether a tweak's actions may run concurrently with others.
pub fn tweak_is_parallelizable(t: &Tweak) -> bool {
    t.actions.iter().all(|a: &Action| !a.is_serial())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Device, Disk, Gpu};

    fn sys() -> SystemProfile {
        SystemProfile {
            build: 26100,
            edition: "pro".into(),
            device: Device::Desktop,
            gpu: Gpu::Nvidia,
            disk: Disk::Ssd,
        }
    }

    fn mk(id: &str, build_min: u32, reboot: bool, deps: &[&str]) -> Tweak {
        let json = format!(
            r#"{{"id":"{id}","name":{{"ar":"x","en":"x"}},"category":"t",
                "requires_reboot":{reboot},
                "compat":{{"builds":{{"min":{build_min},"max":26300}}}},
                "depends_on":[{}],
                "actions":[{{"type":"registry","hive":"HKCU","path":"p","name":"n","kind":"dword","data":1}}]}}"#,
            deps.iter().map(|d| format!("\"{d}\"")).collect::<Vec<_>>().join(",")
        );
        serde_json::from_str(&json).unwrap()
    }

    /// The shipped catalog: loads, and on every supported edition nothing is
    /// wrongly dropped as incompatible.
    #[test]
    fn real_catalog_plans_fully_on_every_edition() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/tweaks");
        let docs: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().map(|x| x == "json").unwrap_or(false))
            .map(|e| std::fs::read_to_string(e.path()).unwrap())
            .collect();
        let cat = Catalog::from_json_docs(docs).expect("real catalog must load");
        assert_eq!(cat.all().len(), 25);
        let all: Vec<String> = cat.all().iter().map(|t| t.id.clone()).collect();
        for (build, edition) in [(19044, "home"), (19045, "pro"), (22631, "enterprise"), (26100, "ltsc"), (26200, "iot"), (26100, "education")] {
            let mut s = sys();
            s.build = build;
            s.edition = edition.into();
            let plan = cat.plan(&all, &s).unwrap();
            assert!(plan.skipped_incompatible.is_empty(), "{build}/{edition}: {:?}", plan.skipped_incompatible);
            assert_eq!(plan.total(), 25);
        }
    }

    #[test]
    fn reboot_phase_never_moves_a_dependency_after_its_consumer() {
        let mut first = mk("first", 19044, true, &[]);
        first.order = 1;
        let mut last = mk("last", 19044, false, &["first"]);
        last.order = 2;
        let cat = Catalog::new(vec![last, first]).unwrap();
        let plan = cat.plan(&["last".into()], &sys()).unwrap();
        assert!(plan.immediate.is_empty());
        assert_eq!(plan.deferred, ["first", "last"]);
    }

    #[test]
    fn shipped_finalizer_is_always_last() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../data/tweaks");
        let cat = Catalog::from_json_docs(std::fs::read_dir(dir).unwrap().map(|e| std::fs::read_to_string(e.unwrap().path()).unwrap())).unwrap();
        let selected = cat.all().iter().map(|t| t.id.clone()).collect::<Vec<_>>();
        let plan = cat.plan(&selected, &sys()).unwrap();
        assert_eq!(plan.deferred.last().map(String::as_str), Some("core-finaltasks"));
    }

    #[test]
    fn plans_with_deps_compat_and_phases() {
        let cat = Catalog::new(vec![
            mk("a", 19044, false, &[]),
            mk("b", 19044, true, &["a"]),   // depends on a, needs reboot
            mk("old", 30000, false, &[]),   // incompatible (future-only build)
        ])
        .unwrap();

        let plan = cat.plan(&["b".into(), "old".into()], &sys()).unwrap();
        // a pulled in as dependency of b
        assert!(plan.immediate.contains(&"a".to_string()));
        assert!(plan.deferred.contains(&"b".to_string()));
        assert!(plan.skipped_incompatible.contains(&"old".to_string()));
        // dependency ordering: a before b overall
        assert_eq!(plan.immediate, vec!["a".to_string()]);
    }
}
