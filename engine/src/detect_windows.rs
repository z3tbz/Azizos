//! Windows backend for detection (`#[cfg(windows)]`) — strictly read-only.
//!
//! Cheapest source first:
//!   * build / edition / GPU / Defender state -> registry reads (microseconds,
//!     no child process),
//!   * AC / battery -> `GetSystemPowerStatus` via `windows-sys` (raw binding,
//!     zero wrapper cost),
//!   * system-disk media type and registered AV products -> one hidden
//!     PowerShell call each (they live only in WMI/Storage).
//! Nothing here changes the machine.

use crate::catalog::SystemProfile;
use crate::detect::Detector;
use crate::exec_windows::{encode_ps, CREATE_NO_WINDOW};
use crate::schema::{Device, Disk, Gpu};
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use windows_registry::LOCAL_MACHINE;

pub struct WindowsDetector;

const CURVER: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
/// Device-setup class for display adapters.
const DISPLAY_CLASS: &str = r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";

fn reg_string(path: &str, name: &str) -> Option<String> {
    LOCAL_MACHINE.open(path).ok()?.get_string(name).ok()
}
fn reg_u32(path: &str, name: &str) -> Option<u32> {
    LOCAL_MACHINE.open(path).ok()?.get_u32(name).ok()
}

/// Run a read-only PowerShell query without a console window; stdout trimmed.
fn ps_query(script: &str) -> String {
    let exe = std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    Command::new(exe)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .args(["-NoLogo", "-NoProfile", "-NonInteractive", "-EncodedCommand"])
        .arg(encode_ps(script))
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Map the registry `EditionID` to our compat tags.
pub(crate) fn normalize_edition(id: &str) -> String {
    let e = id.to_ascii_lowercase();
    let tag = if e.contains("iot") {
        "iot"
    } else if e.ends_with("enterprises") || e.contains("ltsc") {
        "ltsc"
    } else if e.contains("enterprise") {
        "enterprise"
    } else if e.contains("education") {
        "education"
    } else if e.contains("pro") {
        "pro"
    } else if e.starts_with("core") || e.contains("home") {
        "home"
    } else {
        return e;
    };
    tag.to_string()
}

fn detect_gpu() -> Gpu {
    let Ok(class) = LOCAL_MACHINE.open(DISPLAY_CLASS) else { return Gpu::Any };
    let Ok(keys) = class.keys() else { return Gpu::Any };
    let mut found = Gpu::Any;
    for sub in keys {
        let Some(desc) = class.open(&sub).ok().and_then(|k| k.get_string("DriverDesc").ok()) else { continue };
        let d = desc.to_ascii_lowercase();
        // A discrete GPU wins over an integrated one.
        if d.contains("nvidia") {
            return Gpu::Nvidia;
        } else if d.contains("amd") || d.contains("radeon") {
            found = Gpu::Amd;
        } else if d.contains("intel") && found == Gpu::Any {
            found = Gpu::Intel;
        }
    }
    found
}

fn detect_disk() -> Disk {
    let media = ps_query(
        "$n=(Get-Partition -DriveLetter $env:SystemDrive[0] -ErrorAction SilentlyContinue).DiskNumber;\
         (Get-PhysicalDisk | Where-Object DeviceId -eq \"$n\" | Select-Object -First 1).MediaType",
    )
    .to_ascii_lowercase();
    if media.contains("ssd") {
        Disk::Ssd
    } else if media.contains("hdd") {
        Disk::Hdd
    } else {
        Disk::Any
    }
}

fn power_status() -> Option<(u8, u8)> {
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
    // SAFETY: plain C struct, zero is a valid bit pattern; the API fills it.
    let mut s: SYSTEM_POWER_STATUS = unsafe { std::mem::zeroed() };
    let ok = unsafe { GetSystemPowerStatus(&mut s) };
    (ok != 0).then_some((s.ACLineStatus, s.BatteryFlag))
}

impl Detector for WindowsDetector {
    fn profile(&self) -> Result<SystemProfile, String> {
        let build = reg_string(CURVER, "CurrentBuildNumber")
            .and_then(|s| s.parse::<u32>().ok())
            .ok_or("cannot read Windows build number")?;
        let edition = normalize_edition(&reg_string(CURVER, "EditionID").unwrap_or_default());
        // BatteryFlag 128 = "no system battery".
        let device = match power_status() {
            Some((_, flag)) if flag != 128 => Device::Laptop,
            _ => Device::Desktop,
        };
        Ok(SystemProfile { build, edition, device, gpu: detect_gpu(), disk: detect_disk() })
    }

    fn defender_realtime_on(&self) -> Result<bool, String> {
        // DisableRealtimeMonitoring == 1 (setting or policy) means real-time is OFF.
        let a = reg_u32(r"SOFTWARE\Microsoft\Windows Defender\Real-Time Protection", "DisableRealtimeMonitoring");
        let b = reg_u32(
            r"SOFTWARE\Policies\Microsoft\Windows Defender\Real-Time Protection",
            "DisableRealtimeMonitoring",
        );
        Ok(!(a == Some(1) || b == Some(1)))
    }

    fn third_party_av(&self) -> Result<Vec<String>, String> {
        let out = ps_query(
            "Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntiVirusProduct -ErrorAction SilentlyContinue \
             | Select-Object -ExpandProperty displayName",
        );
        Ok(out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .filter(|l| {
                let lc = l.to_ascii_lowercase();
                !(lc.contains("windows defender") || lc.contains("microsoft defender"))
            })
            .map(str::to_string)
            .collect())
    }

    fn on_ac_power(&self) -> Result<Option<bool>, String> {
        let (line, flag) = power_status().ok_or("GetSystemPowerStatus failed")?;
        if flag == 128 {
            return Ok(None); // desktop: not applicable
        }
        // ACLineStatus: 0 offline, 1 online, 255 unknown (don't block on unknown).
        Ok(Some(line != 0))
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_edition as n;

    #[test]
    fn editions_map_to_compat_tags() {
        assert_eq!(n("Core"), "home");
        assert_eq!(n("CoreSingleLanguage"), "home");
        assert_eq!(n("Professional"), "pro");
        assert_eq!(n("ProfessionalWorkstation"), "pro");
        assert_eq!(n("Education"), "education");
        assert_eq!(n("Enterprise"), "enterprise");
        assert_eq!(n("EnterpriseS"), "ltsc");
        assert_eq!(n("IoTEnterpriseS"), "iot");
    }
}
