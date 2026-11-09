//! Windows backend for detection (`#[cfg(windows)]`) — read-only.
//!
//! Registry for build/edition/Defender state; `GetSystemPowerStatus` for AC/
//! battery; PowerShell probes for GPU, disk media type and third-party AV
//! (root\SecurityCenter2). Nothing here changes the machine.

use crate::catalog::SystemProfile;
use crate::detect::Detector;
use crate::schema::{Device, Disk, Gpu};
use std::process::Command;

pub struct WindowsDetector;

fn reg_string(path: &str, name: &str) -> Option<String> {
    windows_registry::LOCAL_MACHINE.open(path).ok()?.get_string(name).ok()
}
fn reg_u32(path: &str, name: &str) -> Option<u32> {
    windows_registry::LOCAL_MACHINE.open(path).ok()?.get_u32(name).ok()
}

fn ps(command: &str) -> String {
    Command::new("powershell")
        .args(["-NoProfile", "-Command", command])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

const CURVER: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

impl Detector for WindowsDetector {
    fn profile(&self) -> Result<SystemProfile, String> {
        let build = reg_string(CURVER, "CurrentBuildNumber")
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);

        let edition = reg_string(CURVER, "EditionID")
            .unwrap_or_default()
            .to_lowercase();
        // Normalise a few common editions into our compat tags.
        let edition = if edition.contains("iot") {
            "iot".to_string()
        } else if edition.contains("enterprises") || edition.contains("ltsc") {
            "ltsc".to_string()
        } else if edition.contains("enterprise") {
            "enterprise".to_string()
        } else if edition.contains("pro") {
            "pro".to_string()
        } else if edition.contains("core") || edition.contains("home") {
            "home".to_string()
        } else {
            edition
        };

        let device = match self.on_ac_power_raw() {
            Some(_has_battery @ true) => Device::Laptop,
            _ => Device::Desktop,
        };

        let gpu_s = ps("(Get-CimInstance Win32_VideoController | Select-Object -First 1 -ExpandProperty Name)").to_lowercase();
        let gpu = if gpu_s.contains("nvidia") {
            Gpu::Nvidia
        } else if gpu_s.contains("amd") || gpu_s.contains("radeon") {
            Gpu::Amd
        } else if gpu_s.contains("intel") {
            Gpu::Intel
        } else {
            Gpu::Any
        };

        let media = ps("(Get-PhysicalDisk | Select-Object -First 1 -ExpandProperty MediaType)").to_lowercase();
        let disk = if media.contains("ssd") {
            Disk::Ssd
        } else if media.contains("hdd") {
            Disk::Hdd
        } else {
            Disk::Any
        };

        Ok(SystemProfile { build, edition, device, gpu, disk })
    }

    fn defender_realtime_on(&self) -> Result<bool, String> {
        // DisableRealtimeMonitoring == 1 means real-time protection is OFF.
        let a = reg_u32(
            r"SOFTWARE\Microsoft\Windows Defender\Real-Time Protection",
            "DisableRealtimeMonitoring",
        );
        let b = reg_u32(
            r"SOFTWARE\Policies\Microsoft\Windows Defender\Real-Time Protection",
            "DisableRealtimeMonitoring",
        );
        let off = a == Some(1) || b == Some(1);
        Ok(!off) // on unless explicitly disabled
    }

    fn third_party_av(&self) -> Result<Vec<String>, String> {
        // SecurityCenter2 lists all registered AV products; exclude Defender.
        let out = ps(
            "Get-CimInstance -Namespace root/SecurityCenter2 -ClassName AntiVirusProduct \
             | Select-Object -ExpandProperty displayName",
        );
        let list: Vec<String> = out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .filter(|l| {
                let lc = l.to_lowercase();
                !(lc.contains("windows defender") || lc.contains("microsoft defender"))
            })
            .collect();
        Ok(list)
    }

    fn on_ac_power(&self) -> Result<Option<bool>, String> {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut sps = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut sps).map_err(|e| e.to_string())? };
        // BatteryFlag 128 => no system battery (desktop) => N/A.
        if sps.BatteryFlag == 128 {
            return Ok(None);
        }
        // ACLineStatus: 0 offline, 1 online, 255 unknown.
        match sps.ACLineStatus {
            1 => Ok(Some(true)),
            0 => Ok(Some(false)),
            _ => Ok(Some(true)), // unknown -> don't block
        }
    }
}

impl WindowsDetector {
    /// true when a system battery is present (used for laptop/desktop heuristic).
    fn on_ac_power_raw(&self) -> Option<bool> {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut sps = SYSTEM_POWER_STATUS::default();
        unsafe { GetSystemPowerStatus(&mut sps).ok()? };
        Some(sps.BatteryFlag != 128)
    }
}
