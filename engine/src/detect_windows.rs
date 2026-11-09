//! Windows backend for detection (`#[cfg(windows)]`) — strictly read-only.
//!
//! Cheapest source first:
//!   * build / edition / Defender state -> registry reads (microseconds,
//!     no child process),
//!   * AC / battery -> `GetSystemPowerStatus` via `windows-sys` (raw binding,
//!     zero wrapper cost),
//!   * GPU vendor -> PCI vendor id in the display-class registry key,
//!   * system-disk SSD/HDD -> DeviceIoControl on the system volume,
//!   * registered AV products -> one hidden PowerShell call (they live only
//!     in WMI SecurityCenter2, which has no lighter public interface).
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

/// PCI vendor of each display adapter, from the driver's `MatchingDeviceId`
/// (`pci\ven_10de&dev_…`) — locale- and branding-independent, unlike the
/// marketing name. A discrete GPU (NVIDIA / AMD) wins over integrated Intel.
fn detect_gpu() -> Gpu {
    let Ok(class) = LOCAL_MACHINE.open(DISPLAY_CLASS) else { return Gpu::Any };
    let Ok(keys) = class.keys() else { return Gpu::Any };
    let mut vendors = Vec::new();
    for sub in keys {
        let Ok(k) = class.open(&sub) else { continue };
        let id = k.get_string("MatchingDeviceId").unwrap_or_default();
        let desc = k.get_string("DriverDesc").unwrap_or_default();
        vendors.push(gpu_vendor(&id, &desc));
    }
    pick_gpu(&vendors)
}

pub(crate) fn gpu_vendor(matching_id: &str, desc: &str) -> Gpu {
    let id = matching_id.to_ascii_lowercase();
    if id.contains("ven_10de") {
        return Gpu::Nvidia;
    }
    if id.contains("ven_1002") || id.contains("ven_1022") {
        return Gpu::Amd;
    }
    if id.contains("ven_8086") {
        return Gpu::Intel;
    }
    // No PCI id (rare: virtual/remote adapters) -> fall back to the name.
    let d = desc.to_ascii_lowercase();
    if d.contains("nvidia") {
        Gpu::Nvidia
    } else if d.contains("radeon") || d.contains("amd") {
        Gpu::Amd
    } else if d.contains("intel") {
        Gpu::Intel
    } else {
        Gpu::Any
    }
}

pub(crate) fn pick_gpu(v: &[Gpu]) -> Gpu {
    for want in [Gpu::Nvidia, Gpu::Amd, Gpu::Intel] {
        if v.contains(&want) {
            return want;
        }
    }
    Gpu::Any
}

/// System-drive media type straight from the storage driver
/// (IOCTL_STORAGE_QUERY_PROPERTY / seek penalty) — no WMI, no PowerShell.
fn detect_disk() -> Disk {
    match crate::win32::system_disk_is_ssd() {
        Some(true) => Disk::Ssd,
        Some(false) => Disk::Hdd,
        None => Disk::Any,
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
    use super::{gpu_vendor, pick_gpu};
    use crate::schema::Gpu;

    #[test]
    fn gpu_by_pci_vendor() {
        assert_eq!(gpu_vendor(r"pci\ven_10de&dev_2684", ""), Gpu::Nvidia);
        assert_eq!(gpu_vendor(r"PCI\VEN_1002&DEV_744C", ""), Gpu::Amd);
        assert_eq!(gpu_vendor(r"pci\ven_8086&dev_a780", ""), Gpu::Intel);
        assert_eq!(gpu_vendor("", "Microsoft Basic Display Adapter"), Gpu::Any);
        // laptop: Intel iGPU + NVIDIA dGPU -> NVIDIA
        assert_eq!(pick_gpu(&[Gpu::Intel, Gpu::Nvidia]), Gpu::Nvidia);
        assert_eq!(pick_gpu(&[Gpu::Any]), Gpu::Any);
    }

    #[test]
    fn real_profile_reads() {
        use crate::detect::Detector;
        let p = super::WindowsDetector.profile().unwrap();
        assert!(p.build >= 10240);
        assert!(!p.edition.is_empty());
    }

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
