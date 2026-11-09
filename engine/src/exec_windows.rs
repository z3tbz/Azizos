//! Windows backend for the executor (`#[cfg(windows)]`).
//!
//! Design choice: keep the FFI surface tiny and reliable.
//!   * registry  -> `windows-registry` (Microsoft's crate)
//!   * service   -> write the `Start` value in the registry (0..4), exactly what
//!                  the playbook's `startup: 4` means — no Service Control Manager
//!                  FFI, far fewer failure modes.
//!   * everything else (command/bcd/powershell/bundled/installer/file/env,
//!     appx/scheduled_task/device/powerplan) -> std::process / std::fs.
//!
//! Build on a Windows machine (`cargo build`). If a `windows-registry` signature
//! differs in your pinned version, adjust per the compiler message.

use crate::exec::Backend;
use crate::schema::{Action, Check, Hive, RegData, RegKind, ServiceStartup};
use std::path::PathBuf;
use std::process::Command;

pub struct WindowsBackend {
    /// Where bundled assets (scripts, installers, tools) were unpacked.
    pub asset_dir: PathBuf,
}

impl WindowsBackend {
    pub fn new(asset_dir: PathBuf) -> Self {
        Self { asset_dir }
    }
}

fn root(h: Hive) -> windows_registry::Key {
    use windows_registry::*;
    match h {
        Hive::Hklm => LOCAL_MACHINE,
        Hive::Hkcu => CURRENT_USER,
        Hive::Hkcr => CLASSES_ROOT,
        Hive::Hku => USERS,
    }
}

fn reg_data_u32(d: &Option<RegData>) -> u32 {
    match d {
        Some(RegData::Int(i)) => *i as u32,
        Some(RegData::Str(s)) => s.parse().unwrap_or(0),
        _ => 0,
    }
}
fn reg_data_str(d: &Option<RegData>) -> String {
    match d {
        Some(RegData::Str(s)) => s.clone(),
        Some(RegData::Int(i)) => i.to_string(),
        _ => String::new(),
    }
}

fn startup_val(s: ServiceStartup) -> u32 {
    match s {
        ServiceStartup::Boot => 0,
        ServiceStartup::System => 1,
        ServiceStartup::Auto => 2,
        ServiceStartup::Demand => 3,
        ServiceStartup::Disabled => 4,
    }
}

fn run(exe: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(exe)
        .args(args)
        .output()
        .map_err(|e| format!("spawn {exe}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{exe} exited {}: {}",
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// Split a cmd/bcd argument string into argv, respecting simple quoting.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in s.chars() {
        match c {
            '"' => q = !q,
            ' ' if !q => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl Backend for WindowsBackend {
    fn apply(&mut self, action: &Action) -> Result<(), String> {
        match action {
            Action::Registry { op, hive, path, name, kind, data } => {
                let k = root(*hive);
                match op.as_str() {
                    "set" => {
                        let key = k.create(path).map_err(|e| e.to_string())?;
                        let name = name.as_deref().unwrap_or("");
                        let kind = kind.unwrap_or(RegKind::Sz);
                        match kind {
                            RegKind::Dword => key.set_u32(name, reg_data_u32(data)),
                            RegKind::Qword => key.set_u64(name, reg_data_u32(data) as u64),
                            RegKind::Sz => key.set_string(name, reg_data_str(data)),
                            RegKind::ExpandSz => key.set_expand_string(name, reg_data_str(data)),
                            RegKind::MultiSz => key.set_multi_string(name, &[reg_data_str(data)]),
                            RegKind::Binary => {
                                let bytes = hex_bytes(&reg_data_str(data));
                                key.set_bytes(name, windows_registry::Type::Bytes, &bytes)
                            }
                        }
                        .map_err(|e| e.to_string())
                    }
                    "delete" => {
                        let key = k.open(path).map_err(|e| e.to_string())?;
                        key.remove_value(name.as_deref().unwrap_or(""))
                            .map_err(|e| e.to_string())
                    }
                    "delete_key" => k.remove_tree(path).map_err(|e| e.to_string()),
                    "take_own" => Ok(()), // handled via elevated helper; see docs/ENGINE.md
                    other => Err(format!("unknown registry op: {other}")),
                }
            }

            Action::Service { name, startup, op } => match op.as_str() {
                "change" => {
                    let s = startup.unwrap_or(ServiceStartup::Disabled);
                    let key = root(Hive::Hklm)
                        .create(format!(r"SYSTEM\CurrentControlSet\Services\{name}"))
                        .map_err(|e| e.to_string())?;
                    key.set_u32("Start", startup_val(s)).map_err(|e| e.to_string())
                }
                "stop" => run("net", &["stop", name]),
                "delete" => run("sc", &["delete", name]),
                other => Err(format!("unknown service op: {other}")),
            },

            Action::ScheduledTask { path, op } => match op.as_str() {
                "disable" => run("schtasks", &["/Change", "/TN", path, "/Disable"]),
                "enable" => run("schtasks", &["/Change", "/TN", path, "/Enable"]),
                "delete" => run("schtasks", &["/Delete", "/TN", path, "/F"]),
                other => Err(format!("unknown task op: {other}")),
            },

            Action::Appx { name, scope: _, op } => match op.as_str() {
                "remove" => run(
                    "powershell",
                    &[
                        "-NoProfile",
                        "-Command",
                        &format!("Get-AppxPackage '{name}' | Remove-AppxPackage -ErrorAction SilentlyContinue"),
                    ],
                ),
                "unregister" => run(
                    "powershell",
                    &[
                        "-NoProfile",
                        "-Command",
                        &format!("Get-AppxPackage '{name}' | Remove-AppxPackage -ErrorAction SilentlyContinue"),
                    ],
                ),
                "clear_cache" => Ok(()), // cache clears are best-effort/no-op here
                other => Err(format!("unknown appx op: {other}")),
            },

            Action::File { op, path, to } => {
                let p = expand(path);
                match op.as_str() {
                    "delete" => {
                        let pb = PathBuf::from(&p);
                        if pb.is_dir() {
                            std::fs::remove_dir_all(&pb).map_err(|e| e.to_string())
                        } else if pb.exists() {
                            std::fs::remove_file(&pb).map_err(|e| e.to_string())
                        } else {
                            Ok(()) // already absent — idempotent
                        }
                    }
                    "copy" => std::fs::copy(&p, expand(to.as_deref().unwrap_or("")))
                        .map(|_| ())
                        .map_err(|e| e.to_string()),
                    "rename" => std::fs::rename(&p, expand(to.as_deref().unwrap_or("")))
                        .map_err(|e| e.to_string()),
                    other => Err(format!("unknown file op: {other}")),
                }
            }

            Action::Command { exe, args } => {
                let a: Vec<String> = split_args(args);
                let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
                run(exe, &refs)
            }

            Action::Bcd { args } => {
                let a = split_args(args);
                let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
                run("bcdedit", &refs)
            }

            Action::Powershell { script, runas: _ } => {
                run("powershell", &["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", script])
            }

            Action::Bundled { file, args, runas: _ } => {
                let full = self.asset_dir.join(file);
                let fs = full.to_string_lossy().to_string();
                let extra = split_args(args);
                let ext = full
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("")
                    .to_lowercase();
                match ext.as_str() {
                    "ps1" => {
                        let mut a = vec![
                            "-NoProfile".into(),
                            "-ExecutionPolicy".into(),
                            "Bypass".into(),
                            "-File".into(),
                            fs,
                        ];
                        a.extend(extra);
                        let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
                        run("powershell", &refs)
                    }
                    "bat" | "cmd" => {
                        let mut a = vec!["/c".to_string(), fs];
                        a.extend(extra);
                        let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
                        run("cmd", &refs)
                    }
                    _ => {
                        let refs: Vec<&str> = extra.iter().map(|s| s.as_str()).collect();
                        run(&fs, &refs)
                    }
                }
            }

            Action::Installer { file, args } => {
                let full = self.asset_dir.join(file);
                let fs = full.to_string_lossy().to_string();
                let a = split_args(args);
                let refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
                run(&fs, &refs)
            }

            Action::Powerplan { import, guid } => {
                let full = self.asset_dir.join(import);
                let g = guid.clone().unwrap_or_default();
                run("powercfg", &["-import", &full.to_string_lossy(), &g])?;
                if !g.is_empty() {
                    run("powercfg", &["-setactive", &g])?;
                }
                Ok(())
            }

            Action::Device { op, name } => {
                // Uses the bundled DevManView the playbook already ships.
                let tool = self.asset_dir.join("DevManView.exe");
                let flag = if op == "disable" { "/disable" } else { "/enable" };
                run(&tool.to_string_lossy(), &[flag, name])
            }

            Action::Env { name, data, scope } => {
                if scope == "machine" {
                    let key = root(Hive::Hklm)
                        .create(r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
                        .map_err(|e| e.to_string())?;
                    key.set_string(name, data).map_err(|e| e.to_string())
                } else {
                    let key = root(Hive::Hkcu)
                        .create("Environment")
                        .map_err(|e| e.to_string())?;
                    key.set_string(name, data).map_err(|e| e.to_string())
                }
            }
        }
    }

    fn verify(&mut self, check: &Check) -> Result<bool, String> {
        match check {
            Check::Registry { hive, path, name, equals } => {
                let key = match root(*hive).open(path) {
                    Ok(k) => k,
                    Err(_) => return Ok(false),
                };
                match equals {
                    RegData::Int(want) => Ok(key.get_u32(name).map(|v| v as i64 == *want).unwrap_or(false)),
                    RegData::Str(want) => Ok(key.get_string(name).map(|v| &v == want).unwrap_or(false)),
                    RegData::List(_) => Ok(false),
                }
            }
            Check::Service { name, startup } => {
                let key = root(Hive::Hklm)
                    .open(format!(r"SYSTEM\CurrentControlSet\Services\{name}"))
                    .map_err(|e| e.to_string())?;
                Ok(key.get_u32("Start").map(|v| v == startup_val(*startup)).unwrap_or(false))
            }
            Check::FileAbsent { path } => Ok(!PathBuf::from(expand(path)).exists()),
        }
    }

    fn snapshot(&mut self, action: &Action) -> Option<Action> {
        // Capture the current registry value so a rollback can restore it.
        if let Action::Registry { op, hive, path, name, kind, .. } = action {
            if op == "set" {
                let name = name.as_deref().unwrap_or("");
                let key = root(*hive).open(path).ok()?;
                let kind = kind.unwrap_or(RegKind::Sz);
                let data = match kind {
                    RegKind::Dword | RegKind::Qword => RegData::Int(key.get_u32(name).ok()? as i64),
                    _ => RegData::Str(key.get_string(name).ok()?),
                };
                return Some(Action::Registry {
                    op: "set".into(),
                    hive: *hive,
                    path: path.clone(),
                    name: Some(name.to_string()),
                    kind: Some(kind),
                    data: Some(data),
                });
            }
        }
        None
    }
}

fn hex_bytes(s: &str) -> Vec<u8> {
    let clean: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    clean
        .as_bytes()
        .chunks(2)
        .filter_map(|c| std::str::from_utf8(c).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}

/// Expand %VAR% in paths using the process environment.
fn expand(p: &str) -> String {
    let mut out = String::new();
    let mut it = p.split('%');
    if let Some(first) = it.next() {
        out.push_str(first);
    }
    let mut toggle = true;
    for part in it {
        if toggle {
            if let Ok(v) = std::env::var(part) {
                out.push_str(&v);
            } else {
                out.push('%');
                out.push_str(part);
                out.push('%');
            }
        } else {
            out.push_str(part);
        }
        toggle = !toggle;
    }
    out
}
