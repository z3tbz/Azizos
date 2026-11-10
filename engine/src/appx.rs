//! Appx/MSIX removal via the WinRT `PackageManager` runtime class — the same
//! API Settings > Apps uses, replacing up to one PowerShell process *per app*
//! with none at all. Uses the `windows` crate's typed COM/WinRT bindings
//! (unlike `win32.rs`, which is raw `windows-sys` C bindings): reference
//! counting, HSTRING and the WinRT async pattern are exactly what that crate
//! exists to get right, so this is the lighter and safer choice here, not a
//! heavier one.

use windows::core::HSTRING;
use windows::Management::Deployment::{DeploymentProgress, DeploymentResult, PackageManager};
use windows::Win32::System::WinRT::{RoInitialize, RO_INIT_MULTITHREADED};
use windows_future::{AsyncStatus, IAsyncOperationWithProgress};

use std::time::{Duration, Instant};

/// `RoInitialize` is idempotent per thread; a second call on the same thread
/// returns `S_FALSE` ("already initialized"), which `windows::core::Result`
/// treats as success, so this is safe to call before every operation.
fn ensure_initialized() -> Result<(), String> {
    unsafe { RoInitialize(RO_INIT_MULTITHREADED) }.map_err(|e| format!("RoInitialize: {e}"))
}

/// One registered package: its full name (`Name_Version_Arch_Res_Publisher`,
/// what `remove_for_all_users` takes) and family name (`Name_Publisher`, what
/// `deprovision` takes) — read together so nothing has to be reconstructed by
/// string surgery later.
pub struct PackageRef {
    pub full_name: String,
    pub family_name: String,
}

/// Every package registered to any user — the WinRT equivalent of
/// `Get-AppxPackage -AllUsers`. An empty `userSecurityId` is how the API asks
/// for every user's packages, not just the caller's (needs admin, which
/// Azizos always runs as).
pub fn list_packages() -> Result<Vec<PackageRef>, String> {
    ensure_initialized()?;
    let pm = PackageManager::new().map_err(|e| format!("PackageManager: {e}"))?;
    let packages = pm
        .FindPackagesByUserSecurityId(&HSTRING::new())
        .map_err(|e| format!("FindPackagesByUserSecurityId: {e}"))?;
    Ok(packages
        .into_iter()
        .filter_map(|p| p.Id().ok())
        .filter_map(|id| Some(PackageRef {
            full_name: id.FullName().ok()?.to_string_lossy(),
            family_name: id.FamilyName().ok()?.to_string_lossy(),
        }))
        .collect())
}

fn wait_deployment(
    op: IAsyncOperationWithProgress<DeploymentResult, DeploymentProgress>,
    limit: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let status = op.Status().map_err(|e| e.to_string())?;
        if status != AsyncStatus::Started {
            break;
        }
        if start.elapsed() > limit {
            let _ = op.Cancel();
            return Err(format!("timed out after {}s", limit.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    match op.Status().map_err(|e| e.to_string())? {
        AsyncStatus::Completed => {
            let result = op.GetResults().map_err(|e| e.to_string())?;
            // A deployment can complete the async op yet still report its own failure.
            match result.ExtendedErrorCode() {
                Ok(code) if code.ok().is_ok() => Ok(()),
                Ok(code) => {
                    let text = result.ErrorText().map(|t| t.to_string_lossy()).unwrap_or_default();
                    Err(format!("0x{:08X} {text}", code.0 as u32).trim_end().to_string())
                }
                Err(_) => Ok(()),
            }
        }
        AsyncStatus::Error => {
            let code = op.ErrorCode().unwrap_or_default();
            Err(format!("0x{:08X}", code.0 as u32))
        }
        _ => Err("canceled".into()),
    }
}

const T_REMOVE: Duration = Duration::from_secs(2 * 60);

/// Remove a package for every user, by its exact full name (from `list_full_names`).
pub fn remove_for_all_users(full_name: &str) -> Result<(), String> {
    ensure_initialized()?;
    let pm = PackageManager::new().map_err(|e| format!("PackageManager: {e}"))?;
    let op = pm
        .RemovePackageAsync(&HSTRING::from(full_name))
        .map_err(|e| format!("RemovePackageAsync: {e}"))?;
    wait_deployment(op, T_REMOVE)
}

/// Stop offering a package to new users/resets (`DeprovisionPackageForAllUsersAsync`),
/// by family name (`Name_Publisher`, no version/arch — from `PackageId::FamilyName`).
pub fn deprovision(family_name: &str) -> Result<(), String> {
    ensure_initialized()?;
    let pm = PackageManager::new().map_err(|e| format!("PackageManager: {e}"))?;
    let op = pm
        .DeprovisionPackageForAllUsersAsync(&HSTRING::from(family_name))
        .map_err(|e| format!("DeprovisionPackageForAllUsersAsync: {e}"))?;
    wait_deployment(op, T_REMOVE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_real_packages_on_this_machine() {
        // Read-only: just enumerates. A fresh Windows install always has at
        // least the shell/store packages, so this should never come back empty.
        let pkgs = list_packages().expect("FindPackagesByUserSecurityId should succeed");
        assert!(!pkgs.is_empty(), "expected at least one registered package on this machine");
        // Every real full name has the Name_Version_Arch_Res_Publisher shape.
        assert!(pkgs.iter().all(|p| p.full_name.matches('_').count() >= 4 && !p.family_name.is_empty()));
    }

    // remove_for_all_users / deprovision are not exercised here: they would
    // actually uninstall a real app from this machine.
}
