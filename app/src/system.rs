//! Small Windows helpers for the app shell: unpacking embedded assets, the
//! after-reboot hand-off to AZIZ Gaming Optimizer, restart, local time.

use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn env_path(var: &str, fallback: &str) -> PathBuf {
    PathBuf::from(std::env::var(var).unwrap_or_else(|_| fallback.into()))
}

/// Unpack the embedded playbook files into a fresh folder under
/// %SystemRoot%\Temp. That location is admin-owned, and the folder name is new
/// for every session, so nothing outside this elevated process can swap a
/// script between unpacking and running it.
pub fn unpack_assets(files: &[(&str, &[u8])]) -> Result<PathBuf, String> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let base = env_path("SystemRoot", r"C:\Windows")
        .join("Temp")
        .join(format!("Azizos-{}-{:x}", std::process::id(), nanos));
    std::fs::create_dir(&base).map_err(|e| format!("{}: {e}", base.display()))?;

    for (rel, bytes) in files {
        let rel = Path::new(rel);
        if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
            return Err(format!("refusing asset path {}", rel.display()));
        }
        let dst = base.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dst, bytes).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    Ok(base)
}

pub struct Fix {
    pub file: String,
    pub args: String,
}

/// Look up `fixes.<requirement>` in data/preflight.json.
pub fn preflight_fix(json: &str, requirement: &str) -> Option<Fix> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let f = v.get("fixes")?.get(requirement)?;
    Some(Fix {
        file: f.get("file")?.as_str()?.to_string(),
        args: f.get("args").and_then(|a| a.as_str()).unwrap_or("").to_string(),
    })
}

pub fn open_uri(uri: &str) -> Result<(), String> {
    Command::new(env_path("SystemRoot", r"C:\Windows").join("explorer.exe"))
        .arg(uri)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The optimizer's install location (same path the playbook's own
/// Install-AzizOptimizer.ps1 / Launch-AzizOptimizer.ps1 use).
pub fn optimizer_exe() -> PathBuf {
    env_path("ProgramFiles", r"C:\Program Files")
        .join("AZIZ Gaming Optimizer")
        .join("AZIZGamingOptimizer.exe")
}

/// Open AZIZ Gaming Optimizer once, at the next sign-in (RunOnce removes the
/// entry itself after it fires).
pub fn schedule_optimizer_after_reboot() -> Result<(), String> {
    let exe = optimizer_exe();
    if !exe.exists() {
        return Err("AZIZ Gaming Optimizer is not installed".into());
    }
    windows_registry::CURRENT_USER
        .create(r"Software\Microsoft\Windows\CurrentVersion\RunOnce")
        .and_then(|k| k.set_string("AzizosOptimizer", format!("\"{}\" --from-playbook", exe.display())))
        .map_err(|e| e.to_string())
}

pub fn restart_in(seconds: u32) -> Result<(), String> {
    Command::new(env_path("SystemRoot", r"C:\Windows").join(r"System32\shutdown.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        // /d p:2:4 = planned, operating system reconfiguration
        .args(["/r", "/t", &seconds.to_string(), "/d", "p:2:4", "/c", "Azizos"])
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Local time as (file-name stamp, display stamp).
pub fn now_local() -> (String, String) {
    use windows_sys::Win32::System::SystemInformation::GetLocalTime;
    // SAFETY: SYSTEMTIME is a plain C struct; GetLocalTime fills every field.
    let t = unsafe {
        let mut t: windows_sys::Win32::Foundation::SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    let (y, mo, d, h, mi, s) = (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    (
        format!("{y:04}{mo:02}{d:02}-{h:02}{mi:02}{s:02}"),
        format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preflight_lookup() {
        let j = r#"{"fixes":{"DefenderToggled":{"file":"a.bat"}}}"#;
        assert_eq!(preflight_fix(j, "DefenderToggled").unwrap().file, "a.bat");
        assert!(preflight_fix(j, "PluggedIn").is_none());
        assert!(preflight_fix("{}", "DefenderToggled").is_none());
    }
}
