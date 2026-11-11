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
    /// Windows 10/11 build inside the supported window (AME `SupportedBuilds`).
    SupportedBuild,
    /// Defender real-time protection is off / manageable.
    DefenderToggled,
    /// No third-party antivirus is registered with Security Center.
    NoThirdPartyAv,
    /// On AC power (laptops) so the run is not interrupted.
    PluggedIn,
}

impl Requirement {
    pub const ALL: [Requirement; 4] = [
        Requirement::SupportedBuild,
        Requirement::DefenderToggled,
        Requirement::NoThirdPartyAv,
        Requirement::PluggedIn,
    ];

    pub fn title(&self, lang: &str) -> &'static str {
        let ar = lang.starts_with("ar");
        match self {
            Requirement::SupportedBuild => if ar { "إصدار ويندوز مدعوم" } else { "Supported Windows build" },
            Requirement::DefenderToggled => if ar { "حماية Defender اللحظية" } else { "Defender real-time protection" },
            Requirement::NoThirdPartyAv => if ar { "لا يوجد مضاد فيروسات آخر" } else { "No third-party antivirus" },
            Requirement::PluggedIn => if ar { "موصول بالشاحن" } else { "On AC power" },
        }
    }

    /// How the UI should offer to fix a failing requirement.
    pub fn remedy(&self) -> Remedy {
        match self {
            Requirement::SupportedBuild => Remedy::UserAction,
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

    let range = crate::schema::BuildRange::default();
    out.push(match d.profile() {
        Ok(p) if range.contains(p.build) => RequirementStatus {
            requirement: Requirement::SupportedBuild,
            met: Met::Pass,
            detail: Some(format!("build {}", p.build)),
        },
        Ok(p) => RequirementStatus {
            requirement: Requirement::SupportedBuild,
            met: Met::Fail,
            detail: Some(format!("build {} (supported {}–{})", p.build, range.min, range.max)),
        },
        Err(e) => RequirementStatus { requirement: Requirement::SupportedBuild, met: Met::Fail, detail: Some(e) },
    });

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
        // REGDB_E_CLASSNOTREG: this Windows has no Security Center at all (an
        // already-debloated system — e.g. a second run of this very playbook).
        // The product list cannot be read, which is not the same as "an
        // antivirus is installed": say exactly that and do not block the run.
        Err(e) if e.contains("0x80040154") => RequirementStatus {
            requirement: Requirement::NoThirdPartyAv,
            met: Met::NotApplicable,
            detail: Some("Security Center is not present on this Windows, so antivirus products cannot be listed. If one is installed, uninstall it first.".into()),
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
        assert_eq!(s[0].met, Met::Pass); // build 26100 supported
        assert_eq!(s[1].met, Met::Fail); // defender
        assert_eq!(s[3].met, Met::Fail); // battery
    }

    #[test]
    fn passes_when_clean_on_desktop() {
        let m = Mock { rt: false, av: vec![], ac: None };
        let s = check_requirements(&m);
        assert!(all_satisfied(&s));
        assert_eq!(s[3].met, Met::NotApplicable);
    }

    #[test]
    fn flags_third_party_av() {
        let m = Mock { rt: false, av: vec!["Kaspersky".into()], ac: Some(true) };
        let s = check_requirements(&m);
        assert_eq!(s[2].met, Met::Fail);
        assert!(!all_satisfied(&s));
    }

    #[test]
    fn missing_security_center_is_unknown_not_a_block() {
        struct NoWsc;
        impl Detector for NoWsc {
            fn profile(&self) -> Result<SystemProfile, String> {
                Ok(SystemProfile { build: 26100, edition: "pro".into(), device: Device::Desktop, gpu: Gpu::Any, disk: Disk::Any })
            }
            fn defender_realtime_on(&self) -> Result<bool, String> { Ok(false) }
            fn third_party_av(&self) -> Result<Vec<String>, String> {
                Err("Security Center: Class not registered (0x80040154)".into())
            }
            fn on_ac_power(&self) -> Result<Option<bool>, String> { Ok(None) }
        }
        let s = check_requirements(&NoWsc);
        assert_eq!(s[2].met, Met::NotApplicable);
        assert!(s[2].detail.as_ref().unwrap().contains("cannot be listed"));
        assert!(all_satisfied(&s));
        // any other read failure still blocks: unknown for an unknown reason
        struct Broken;
        impl Detector for Broken {
            fn profile(&self) -> Result<SystemProfile, String> { NoWsc.profile() }
            fn defender_realtime_on(&self) -> Result<bool, String> { Ok(false) }
            fn third_party_av(&self) -> Result<Vec<String>, String> { Err("Security Center: Access is denied (0x80070005)".into()) }
            fn on_ac_power(&self) -> Result<Option<bool>, String> { Ok(None) }
        }
        assert_eq!(check_requirements(&Broken)[2].met, Met::Fail);
    }
}
