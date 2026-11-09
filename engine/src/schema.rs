//! Azizos tweak schema — the declarative model every tweak is described with.
//! Tweaks live as JSON in `data/tweaks/` and are embedded into the binary at build time.
//! See `docs/SCHEMA.md` for the authoring reference.

use serde::{Deserialize, Serialize};

/// Accept `null` for optional string fields (older exports wrote `"args": null`).
fn null_as_empty<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(Option::<String>::deserialize(d)?.unwrap_or_default())
}

/// Bilingual display string.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Localized {
    pub ar: String,
    pub en: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Default for Risk {
    fn default() -> Self {
        Risk::Low
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Device {
    Any,
    Laptop,
    Desktop,
}
impl Default for Device {
    fn default() -> Self {
        Device::Any
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Gpu {
    Any,
    Nvidia,
    Amd,
    Intel,
}
impl Default for Gpu {
    fn default() -> Self {
        Gpu::Any
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Disk {
    Any,
    Ssd,
    Hdd,
}
impl Default for Disk {
    fn default() -> Self {
        Disk::Any
    }
}

/// Build-number window the tweak is valid for (inclusive).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildRange {
    #[serde(default = "BuildRange::default_min")]
    pub min: u32,
    #[serde(default = "BuildRange::default_max")]
    pub max: u32,
}
impl BuildRange {
    fn default_min() -> u32 {
        19044
    }
    fn default_max() -> u32 {
        26300
    }
    pub fn contains(&self, build: u32) -> bool {
        build >= self.min && build <= self.max
    }
}
impl Default for BuildRange {
    fn default() -> Self {
        BuildRange {
            min: Self::default_min(),
            max: Self::default_max(),
        }
    }
}

/// Compatibility gate: a tweak is skipped (not failed) when it does not match.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Compat {
    #[serde(default)]
    pub builds: BuildRange,
    /// `["any"]` or editions like `pro`, `enterprise`, `ltsc`, `iot`.
    #[serde(default = "Compat::any_editions")]
    pub editions: Vec<String>,
    #[serde(default)]
    pub device: Device,
    #[serde(default)]
    pub gpu: Gpu,
    #[serde(default)]
    pub disk: Disk,
}
impl Compat {
    fn any_editions() -> Vec<String> {
        vec!["any".to_string()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Hive {
    Hklm,
    Hkcu,
    Hkcr,
    Hku,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegKind {
    Dword,
    Qword,
    Sz,
    ExpandSz,
    MultiSz,
    Binary,
}

/// Registry value payload; typed so the executor writes the right REG_* kind.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RegData {
    Int(i64),
    Str(String),
    List(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceStartup {
    Boot,
    System,
    Auto,
    Demand,
    Disabled,
}

/// One executable step of a tweak. `#[serde(tag="type")]` → `{"type":"registry", ...}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    Registry {
        #[serde(default = "Action::reg_set")]
        op: String, // set | delete | delete_key | take_own
        hive: Hive,
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<RegKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<RegData>,
    },
    Service {
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        startup: Option<ServiceStartup>,
        #[serde(default = "Action::svc_change")]
        op: String, // change | delete | stop
    },
    ScheduledTask {
        path: String,
        op: String, // disable | enable | delete
    },
    Appx {
        name: String,
        #[serde(default = "Action::appx_family")]
        scope: String, // family | app
        #[serde(default = "Action::appx_remove")]
        op: String, // remove | unregister | clear_cache
    },
    File {
        op: String, // delete | copy | rename
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to: Option<String>,
    },
    Command {
        exe: String,
        #[serde(default, deserialize_with = "null_as_empty")]
        args: String,
    },
    Powershell {
        script: String,
        #[serde(default = "Action::ps_user")]
        runas: String, // user_elevated | trusted_installer
    },
    Device {
        op: String, // disable | enable
        name: String,
    },
    Bcd {
        args: String,
    },
    Powerplan {
        import: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        guid: Option<String>,
    },
    /// Run a script/executable shipped *inside* the app (embedded asset), not a
    /// system tool. The executor picks the interpreter from the extension
    /// (.ps1 → PowerShell, .bat/.cmd → cmd, .exe → direct).
    Bundled {
        file: String,
        #[serde(default, deserialize_with = "null_as_empty")]
        args: String,
        #[serde(default = "Action::ps_user")]
        runas: String,
    },
    /// Run a bundled installer silently (shipped offline — no download).
    Installer {
        file: String,
        #[serde(default, deserialize_with = "null_as_empty")]
        args: String,
    },
    Env {
        name: String,
        data: String,
        #[serde(default = "Action::env_machine")]
        scope: String, // machine | user
    },
}
impl Action {
    fn reg_set() -> String {
        "set".into()
    }
    fn svc_change() -> String {
        "change".into()
    }
    fn appx_family() -> String {
        "family".into()
    }
    fn appx_remove() -> String {
        "remove".into()
    }
    fn ps_user() -> String {
        "user_elevated".into()
    }
    fn env_machine() -> String {
        "machine".into()
    }

    /// Independent actions can run in parallel; these touch global state and must serialize.
    pub fn is_serial(&self) -> bool {
        matches!(
            self,
            Action::Bcd { .. }
                | Action::Powerplan { .. }
                | Action::Device { .. }
                | Action::Powershell { .. }
                | Action::Bundled { .. }
                | Action::Installer { .. }
                | Action::Command { .. }
        )
    }
}

/// Post-apply verification probe.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Check {
    Registry {
        hive: Hive,
        path: String,
        name: String,
        equals: RegData,
    },
    Service {
        name: String,
        startup: ServiceStartup,
    },
    FileAbsent {
        path: String,
    },
}

/// A full tweak definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tweak {
    pub id: String,
    pub name: Localized,
    #[serde(default)]
    pub description: Option<Localized>,
    pub category: String,
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub requires_reboot: bool,
    #[serde(default)]
    pub risk: Risk,
    #[serde(default)]
    pub compat: Compat,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub conflicts_with: Vec<String>,
    pub actions: Vec<Action>,
    #[serde(default)]
    pub verify: Vec<Check>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revert: Option<Vec<Action>>,
}

impl Tweak {
    pub fn display_name<'a>(&'a self, lang: &str) -> &'a str {
        if lang.starts_with("ar") {
            &self.name.ar
        } else {
            &self.name.en
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_registry_tweak() {
        let json = r#"{
            "id":"enable-dark-mode",
            "name":{"ar":"الوضع الداكن","en":"Dark Mode"},
            "category":"appearance",
            "actions":[
              {"type":"registry","hive":"HKCU",
               "path":"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
               "name":"AppsUseLightTheme","kind":"dword","data":0}
            ],
            "verify":[
              {"type":"registry","hive":"HKCU",
               "path":"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
               "name":"AppsUseLightTheme","equals":0}
            ]
        }"#;
        let t: Tweak = serde_json::from_str(json).unwrap();
        assert_eq!(t.id, "enable-dark-mode");
        assert_eq!(t.display_name("ar"), "الوضع الداكن");
        assert_eq!(t.actions.len(), 1);
        assert!(t.compat.builds.contains(26100));
    }

    #[test]
    fn null_args_are_tolerated() {
        let a: Action =
            serde_json::from_str(r#"{"type":"bundled","file":"x.bat","args":null}"#).unwrap();
        assert!(matches!(a, Action::Bundled { ref args, .. } if args.is_empty()));
    }

    #[test]
    fn parses_service_tweak() {
        let json = r#"{
            "id":"disable-diagtrack",
            "name":{"ar":"تعطيل التتبّع","en":"Disable Telemetry"},
            "category":"privacy",
            "actions":[{"type":"service","name":"DiagTrack","startup":"disabled"}]
        }"#;
        let t: Tweak = serde_json::from_str(json).unwrap();
        assert!(matches!(&t.actions[0], Action::Service { name, .. } if name == "DiagTrack"));
    }
}
