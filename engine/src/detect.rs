//! System detection + pre-flight requirements — read-only.
//!
//! `SystemProfile` (in `catalog`) gates tweak compatibility; `Requirement`
//! models the AME-style pre-flight checklist. The actual probing is behind the
//! [`Detector`] trait (Windows impl in `detect_windows.rs`, cfg(windows)), so the
//! evaluation logic here is pure and testable. See `docs/DETECTION.md`.

use crate::catalog::SystemProfile;
use serde::Serialize;

/// The gates that must pass before the playbook can apply cleanly.
/// (No `Internet` — the app is fully self-contained; nothing is downloaded.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Requirement {
    /// Defender real-time protection is off / manageable.
    DefenderToggled,
    /// No third-party antivirus is registered with Security Center.
    NoThirdPartyAv,
    /// On AC power (laptops) so the run is not interrupted.
    PluggedIn,
}

impl Requirement {
    pub const ALL: [Requirement; 3] = [
        Requirement::DefenderToggled,
        Requirement::NoThirdPartyAv,
        Requirement::PluggedIn,
    ];

    pub fn title(&self, lang: &str) -> &'static str {
        let ar = lang.starts_with("ar");
        match self {
            Requirement::DefenderToggled => if ar { "حماية Defender اللحظية" } else { "Defender real-time protection" },
            Requirement::NoThirdPartyAv => if ar { "لا يوجد مضاد فيروسات آخر" } else { "No third-party antivirus" },
            Requirement::PluggedIn => if ar { "موصول بالشاحن" } else { "On AC power" },
        }
    }

    /// How the UI should offer to fix a failing requirement.
    pub fn remedy(&self) -> Remedy {
        match self {
            // Runs the author's own Defender script behind an explicit consent gate,
            // or opens Windows Security for the user to toggle it by hand.
            Requirement::DefenderToggled => Remedy::ConsentScriptOrManual {
                open: "windows-security",
            },
            // We never silently remove someone's AV — the user uninstalls it.
            Requirement::NoThirdPartyAv => Remedy::UserAction,
            Requirement::PluggedIn => Remedy::UserAction,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remedy {
    /// Offer "fix automatically" (runs a bundled script after explicit consent)
    /// and "I'll do it myself" (open the named settings page), then re-check.
    ConsentScriptOrManual { open: &'static str },
    /// Only the user can resolve it (plug in, uninstall AV); app just guides.
    UserAction,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Met {
    Pass,
    Fail,
    /// Not applicable (e.g. PluggedIn on a desktop).
    NotApplicable,
}

#[derive(Debug, Clone, Serialize)]
pub struct RequirementStatus {
    pub requirement: Requirement,
    pub met: Met,
    pub detail: Option<String>,
}

/// Read-only system probes. Nothing here changes the machine.
pub trait Detector {
    fn profile(&self) -> Result<SystemProfile, String>;
    fn defender_realtime_on(&self) -> Result<bool, String>;
    fn third_party_av(&self) -> Result<Vec<String>, String>;
    /// `None` on a desktop (no battery); `Some(true)` when on AC.
    fn on_ac_power(&self) -> Result<Option<bool>, String>;
}

/// Evaluate every requirement into a status list for the pre-flight screen.
pub fn check_requirements<D: Detector>(d: &D) -> Vec<RequirementStatus> {
    let mut out = Vec::new();

    out.push(match d.defender_realtime_on() {
        Ok(true) => RequirementStatus {
            requirement: Requirement::DefenderToggled,
            met: Met::Fail,
            detail: Some("real-time protection is ON".into()),
        },
        Ok(false) => RequirementStatus {
            requirement: Requirement::DefenderToggled,
            met: Met::Pass,
            detail: None,
        },
        Err(e) => RequirementStatus {
            requirement: Requirement::DefenderToggled,
            met: Met::Fail,
            detail: Some(e),
        },
    });

    out.push(match d.third_party_av() {
        Ok(list) if list.is_empty() => RequirementStatus {
            requirement: Requirement::NoThirdPartyAv,
            met: Met::Pass,
            detail: None,
        },
        Ok(list) => RequirementStatus {
            requirement: Requirement::NoThirdPartyAv,
            met: Met::Fail,
            detail: Some(list.join(", ")),
        },
        Err(e) => RequirementStatus {
            requirement: Requirement::NoThirdPartyAv,
            met: Met::Fail,
            detail: Some(e),
        },
    });

    out.push(match d.on_ac_power() {
        Ok(None) => RequirementStatus {
            requirement: Requirement::PluggedIn,
            met: Met::NotApplicable,
            detail: Some("no battery (desktop)".into()),
        },
        Ok(Some(true)) => RequirementStatus {
            requirement: Requirement::PluggedIn,
            met: Met::Pass,
            detail: None,
        },
        Ok(Some(false)) => RequirementStatus {
            requirement: Requirement::PluggedIn,
            met: Met::Fail,
            detail: Some("running on battery".into()),
        },
        Err(e) => RequirementStatus {
            requirement: Requirement::PluggedIn,
            met: Met::Fail,
            detail: Some(e),
        },
    });

    out
}

/// True when nothing blocks applying (NotApplicable counts as satisfied).
pub fn all_satisfied(statuses: &[RequirementStatus]) -> bool {
    statuses.iter().all(|s| s.met != Met::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{Device, Disk, Gpu};

    struct Mock {
        rt: bool,
        av: Vec<String>,
        ac: Option<bool>,
    }
    impl Detector for Mock {
        fn profile(&self) -> Result<SystemProfile, String> {
            Ok(SystemProfile {
                build: 26100,
                edition: "pro".into(),
                device: Device::Laptop,
                gpu: Gpu::Amd,
                disk: Disk::Ssd,
            })
        }
        fn defender_realtime_on(&self) -> Result<bool, String> {
            Ok(self.rt)
        }
        fn third_party_av(&self) -> Result<Vec<String>, String> {
            Ok(self.av.clone())
        }
        fn on_ac_power(&self) -> Result<Option<bool>, String> {
            Ok(self.ac)
        }
    }

    #[test]
    fn blocks_when_realtime_on_or_on_battery() {
        let m = Mock { rt: true, av: vec![], ac: Some(false) };
        let s = check_requirements(&m);
        assert!(!all_satisfied(&s));
        assert_eq!(s[0].met, Met::Fail); // defender
        assert_eq!(s[2].met, Met::Fail); // battery
    }

    #[test]
    fn passes_when_clean_on_desktop() {
        let m = Mock { rt: false, av: vec![], ac: None };
        let s = check_requirements(&m);
        assert!(all_satisfied(&s));
        assert_eq!(s[2].met, Met::NotApplicable);
    }

    #[test]
    fn flags_third_party_av() {
        let m = Mock { rt: false, av: vec!["Kaspersky".into()], ac: Some(true) };
        let s = check_requirements(&m);
        assert_eq!(s[1].met, Met::Fail);
        assert!(!all_satisfied(&s));
    }
}
