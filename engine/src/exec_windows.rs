//! Windows backend for the executor (feature `native`).
//!
//! Lightest layer for each kind of change:
//!   registry       `windows-registry` (thin safe wrapper over RegCreateKeyExW /
//!                  RegSetValueExW / RegQueryValueExW) — in-process, no reg.exe
//!   service start  the service's `Start` value (0..4) via the same registry API
//!                  — exactly what `startup: N` means; effective on the reboot
//!   service stop / delete   SCM API (ControlService / DeleteService) — no sc.exe
//!   kill process   Toolhelp API (CreateToolhelp32Snapshot + TerminateProcess)
//!   files / env    std::fs / registry — in-process
//!   bcdedit, DISM, label, rundll32, the playbook's own .bat/.ps1/.exe
//!                  child process (these have no lighter public interface):
//!                  hidden window, System32 path, assets as working dir,
//!                  System32 first on PATH, verbatim arguments, timeout.
//!
//! Every action first reads the current state (lightly, in-process) so the
//! result is honest: Done / Already (nothing written) / NotApplicable (target
//! absent on this Windows) — only a real refusal or error is `Err`.

use crate::exec::{Applied, Backend};
use crate::schema::{Action, Check, Hive, RegData, RegKind, ServiceStartup};
use crate::win32;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows_registry::{Key, Type};

pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND): key or value does not exist.
const E_NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

const T_SHORT: Duration = Duration::from_secs(5 * 60);
const T_SCRIPT: Duration = Duration::from_secs(20 * 60);
const T_INSTALL: Duration = Duration::from_secs(30 * 60);

pub struct WindowsBackend {
    /// Where bundled assets (the playbook's scripts, tools, installers) live.
    pub asset_dir: PathBuf,
    /// Installed + provisioned Appx package full names, read once per run.
    appx_cache: Option<Vec<crate::appx::PackageRef>>,
}

impl WindowsBackend {
    pub fn new(asset_dir: PathBuf) -> Self {
        Self { asset_dir, appx_cache: None }
    }

    fn asset(&self, file: &str) -> Result<PathBuf, String> {
        let p = self.asset_dir.join(file);
        if p.exists() {
            Ok(p)
        } else {
            Err(format!("missing asset '{file}' (ship it in assets/)"))
        }
    }

    fn proc(&self, exe: &Path) -> Command {
        let sys32 = system_root().join("System32");
        let path = format!(
            "{};{};{};{}",
            sys32.display(),
            system_root().display(),
            sys32.join(r"WindowsPowerShell\v1.0").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let mut c = Command::new(exe);
        c.creation_flags(CREATE_NO_WINDOW)
            .current_dir(&self.asset_dir)
            .env("PATH", path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        c
    }
}

// ---------------------------------------------------------------- registry --

fn root(h: Hive) -> &'static Key {
    match h {
        Hive::Hklm => windows_registry::LOCAL_MACHINE,
        Hive::Hkcu => windows_registry::CURRENT_USER,
        Hive::Hkcr => windows_registry::CLASSES_ROOT,
        Hive::Hku => windows_registry::USERS,
    }
}

fn not_found(e: &windows_registry::Result<()>) -> bool {
    matches!(e, Err(err) if err.code().0 == E_NOT_FOUND)
}

fn data_i64(d: &Option<RegData>) -> i64 {
    match d {
        Some(RegData::Int(i)) => *i,
        Some(RegData::Str(s)) => {
            let s = s.trim();
            match s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
                Some(h) => i64::from_str_radix(h, 16).unwrap_or(0),
                None => s.parse().unwrap_or(0),
            }
        }
        _ => 0,
    }
}
fn data_str(d: &Option<RegData>) -> String {
    match d {
        Some(RegData::Str(s)) => s.clone(),
        Some(RegData::Int(i)) => i.to_string(),
        Some(RegData::List(v)) => v.join("\n"),
        None => String::new(),
    }
}
fn data_list(d: &Option<RegData>) -> Vec<String> {
    match d {
        Some(RegData::List(v)) => v.clone(),
        Some(RegData::Str(s)) => s.split("\\0").map(str::to_string).collect(),
        Some(RegData::Int(i)) => vec![i.to_string()],
        None => Vec::new(),
    }
}

fn hex_bytes(s: &str) -> Vec<u8> {
    let clean: Vec<u8> = s.bytes().filter(u8::is_ascii_hexdigit).collect();
    clean
        .chunks(2)
        .filter_map(|c| std::str::from_utf8(c).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
        .collect()
}
fn to_hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn set_value(key: &Key, name: &str, kind: RegKind, data: &Option<RegData>) -> windows_registry::Result<()> {
    match kind {
        // `as u32` keeps the two's-complement bits, so -1 -> 0xFFFFFFFF as regedit does.
        RegKind::Dword => key.set_u32(name, data_i64(data) as u32),
        RegKind::Qword => key.set_u64(name, data_i64(data) as u64),
        RegKind::Sz => key.set_string(name, data_str(data)),
        RegKind::ExpandSz => key.set_expand_string(name, data_str(data)),
        RegKind::MultiSz => key.set_multi_string(name.to_string(), &data_list(data)),
        RegKind::Binary => key.set_bytes(name, Type::Bytes, &hex_bytes(&data_str(data))),
    }
}

/// Read a value in schema form — one RegQueryValueExW for the type, one for the data.
fn read_value(key: &Key, name: &str) -> Option<(RegKind, RegData)> {
    Some(match key.get_type(name).ok()? {
        Type::U32 => (RegKind::Dword, RegData::Int(key.get_u32(name).ok()? as i64)),
        Type::U64 => (RegKind::Qword, RegData::Int(key.get_u64(name).ok()? as i64)),
        Type::String => (RegKind::Sz, RegData::Str(key.get_string(name).ok()?)),
        Type::ExpandString => (RegKind::ExpandSz, RegData::Str(key.get_string(name).ok()?)),
        Type::MultiString => (RegKind::MultiSz, RegData::List(key.get_multi_string(name).ok()?)),
        Type::Bytes => (RegKind::Binary, RegData::Str(to_hex(&key.get_value(name).ok()?))),
        _ => return None,
    })
}

/// True when the current value already equals what `set` would write.
fn same_value(cur: &(RegKind, RegData), kind: RegKind, data: &Option<RegData>) -> bool {
    match (kind, &cur.1) {
        (RegKind::Dword, RegData::Int(v)) => cur.0 == RegKind::Dword && *v as u32 == data_i64(data) as u32,
        (RegKind::Qword, RegData::Int(v)) => cur.0 == RegKind::Qword && *v as u64 == data_i64(data) as u64,
        (RegKind::Sz | RegKind::ExpandSz, RegData::Str(v)) => cur.0 == kind && *v == data_str(data),
        (RegKind::MultiSz, RegData::List(v)) => *v == data_list(data),
        (RegKind::Binary, RegData::Str(v)) => *v == to_hex(&hex_bytes(&data_str(data))),
        _ => false,
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
fn startup_from(v: u32) -> Option<ServiceStartup> {
    Some(match v {
        0 => ServiceStartup::Boot,
        1 => ServiceStartup::System,
        2 => ServiceStartup::Auto,
        3 => ServiceStartup::Demand,
        4 => ServiceStartup::Disabled,
        _ => return None,
    })
}
fn svc_path(name: &str) -> String {
    format!(r"SYSTEM\CurrentControlSet\Services\{name}")
}

// ------------------------------------------------------- other light reads --

/// Does a package name/pattern (e.g. `*Disney*`, `Microsoft.BingNews`) match a
/// package full name (`Name_Version_Arch_Res_Publisher`)?
fn appx_match(pattern: &str, full: &str) -> bool {
    let name = full.split('_').next().unwrap_or(full);
    win32::glob(pattern, full) || win32::glob(pattern, name)
}

/// Scheduled task state read from its definition file (System32\Tasks\…):
/// None = no such task, Some(true) = enabled, Some(false) = disabled.
fn task_enabled(task: &str) -> Option<bool> {
    let rel = task.trim_start_matches('\\');
    let file = system_root().join(r"System32\Tasks").join(rel);
    let bytes = std::fs::read(file).ok()?;
    // Task XML is UTF-16LE with a BOM.
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    let xml = String::from_utf16_lossy(&units);
    let settings = xml.split("<Settings>").nth(1).and_then(|s| s.split("</Settings>").next()).unwrap_or("");
    Some(!settings.contains("<Enabled>false</Enabled>"))
}

// ----------------------------------------------------------------- process --

fn system_root() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
}

/// Resolve a bare system tool name (`cmd`, `bcdedit`, `DISM.exe`…) to its full
/// System32 path — never a PATH search. Anything else is returned unchanged.
fn sys_tool(exe: &str) -> PathBuf {
    let lower = exe.to_ascii_lowercase();
    if lower == "powershell" || lower == "powershell.exe" {
        return system_root().join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
    }
    if exe.contains('\\') || exe.contains('/') {
        return PathBuf::from(exe);
    }
    let file = if lower.ends_with(".exe") { exe.to_string() } else { format!("{exe}.exe") };
    let p = system_root().join("System32").join(&file);
    if p.exists() {
        p
    } else {
        PathBuf::from(exe)
    }
}

/// Spawn, wait with a timeout, and turn a non-zero exit into a readable error.
fn wait(mut cmd: Command, what: &str, limit: Duration) -> Result<Applied, String> {
    let mut child = cmd.spawn().map_err(|e| format!("{what}: cannot start ({e})"))?;
    // Drain pipes on threads so a chatty child can never block on a full pipe.
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let t_out = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(p) = so.as_mut() {
            let _ = p.read_to_end(&mut s);
        }
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = Vec::new();
        if let Some(p) = se.as_mut() {
            let _ = p.read_to_end(&mut s);
        }
        s
    });

    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st,
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{what}: timed out after {}s", limit.as_secs()));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err(format!("{what}: {e}")),
        }
    };
    let out = t_out.join().unwrap_or_default();
    let err = t_err.join().unwrap_or_default();
    if status.success() {
        return Ok(Applied::Done);
    }
    let msg = if !err.is_empty() { err } else { out };
    let tail = String::from_utf8_lossy(&msg);
    let tail = tail.trim().lines().last().unwrap_or("").trim();
    let code = status.code().unwrap_or(-1);
    Err(format!("{what}: exit code {code} (0x{:08X}) {tail}", code as u32))
}

/// UTF-16LE + base64, the format `powershell -EncodedCommand` expects.
pub(crate) fn encode_ps(script: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

impl WindowsBackend {
    fn powershell(&self, script: &str, what: &str, limit: Duration) -> Result<Applied, String> {
        let mut c = self.proc(&sys_tool("powershell"));
        c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"])
            .arg(encode_ps(script));
        wait(c, what, limit)
    }

    /// Run a program with its argument string untouched.
    fn program(&self, exe: &Path, args: &str, what: &str, limit: Duration) -> Result<Applied, String> {
        let mut c = self.proc(exe);
        if !args.trim().is_empty() {
            c.raw_arg(args);
        }
        wait(c, what, limit)
    }

    /// Run a file shipped with the app, picking the interpreter by extension.
    fn run_asset(&self, file: &str, args: &str, limit: Duration) -> Result<Applied, String> {
        let full = self.asset(file)?;
        let ext = full.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let path = full.to_string_lossy().to_string();
        match ext.as_str() {
            "ps1" => {
                let mut c = self.proc(&sys_tool("powershell"));
                c.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-File"])
                    .arg(&path);
                if !args.trim().is_empty() {
                    c.raw_arg(args);
                }
                wait(c, file, limit)
            }
            // cmd's own quoting rule: /c ""<path>" <args>"
            "bat" | "cmd" => self.program(&sys_tool("cmd"), &format!("/d /c \"\"{path}\" {args}\""), file, limit),
            "msi" => self.program(&sys_tool("msiexec"), &format!("/i \"{path}\" {args}"), file, limit),
            _ => self.program(&full, args, file, limit),
        }
    }

    /// `cmd /c X.exe …` where X is meant to be a shipped file: a missing file
    /// is reported clearly instead of cmd's localized "not recognized".
    fn check_cmd_asset(&self, args: &str) -> Result<(), String> {
        let line = args.trim_start().strip_prefix("/c").or_else(|| args.trim_start().strip_prefix("/C"));
        let Some(first) = line.and_then(|l| l.split_whitespace().next()) else { return Ok(()) };
        let first = first.trim_matches('"');
        let lower = first.to_ascii_lowercase();
        let is_file = (lower.ends_with(".exe") || lower.ends_with(".bat") || lower.ends_with(".cmd"))
            && !first.contains('\\')
            && !first.contains('%');
        if is_file && !sys_tool(first).is_absolute() && !self.asset_dir.join(first).exists() {
            return Err(format!("missing asset '{first}' (ship it in assets/)"));
        }
        Ok(())
    }
}

// ----------------------------------------------------------------- backend --

impl Backend for WindowsBackend {
    fn apply(&mut self, action: &Action) -> Result<Applied, String> {
        match action {
            Action::Registry { op, hive, path, name, kind, data } => {
                let name = name.as_deref().unwrap_or("");
                match op.as_str() {
                    "set" => {
                        let kind = kind.unwrap_or(RegKind::Sz);
                        if let Some(cur) = root(*hive).open(path).ok().and_then(|k| read_value(&k, name)) {
                            if same_value(&cur, kind, data) {
                                return Ok(Applied::Already("value already set".into()));
                            }
                        }
                        let key = root(*hive).create(path).map_err(|e| format!("{path}: {e}"))?;
                        set_value(&key, name, kind, data)
                            .map(|_| Applied::Done)
                            .map_err(|e| format!("{path}\\{name}: {e}"))
                    }
                    "delete" => {
                        let Ok(ro) = root(*hive).open(path) else {
                            return Ok(Applied::Already("key already absent".into()));
                        };
                        if ro.get_type(name).is_err() {
                            return Ok(Applied::Already("value already absent".into()));
                        }
                        let key = root(*hive).create(path).map_err(|e| format!("{path}: {e}"))?;
                        let r = key.remove_value(name);
                        if not_found(&r) {
                            return Ok(Applied::Already("value already absent".into()));
                        }
                        r.map(|_| Applied::Done).map_err(|e| format!("{path}\\{name}: {e}"))
                    }
                    "delete_key" => {
                        if root(*hive).open(path).is_err() {
                            return Ok(Applied::Already("key already absent".into()));
                        }
                        let r = root(*hive).remove_tree(path);
                        if not_found(&r) {
                            return Ok(Applied::Already("key already absent".into()));
                        }
                        r.map(|_| Applied::Done).map_err(|e| format!("{path}: {e}"))
                    }
                    other => Err(format!("unsupported registry op '{other}'")),
                }
            }

            Action::Service { name, startup, op } => match op.as_str() {
                "change" => {
                    let want = startup_val(startup.unwrap_or(ServiceStartup::Disabled));
                    // Never create a key for a service this edition doesn't have.
                    let Ok(ro) = root(Hive::Hklm).open(svc_path(name)) else {
                        return Ok(Applied::NotApplicable(format!("service {name} not on this Windows")));
                    };
                    if ro.get_u32("Start").ok() == Some(want) {
                        return Ok(Applied::Already("startup already set".into()));
                    }
                    let key = root(Hive::Hklm).create(svc_path(name)).map_err(|e| format!("{name}: {e}"))?;
                    key.set_u32("Start", want).map(|_| Applied::Done).map_err(|e| format!("{name}: {e}"))
                }
                "stop" => Ok(match win32::service_stop(name)? {
                    win32::SvcResult::Done => Applied::Done,
                    win32::SvcResult::Already => Applied::Already("service not running".into()),
                    win32::SvcResult::Missing => Applied::NotApplicable(format!("service {name} not on this Windows")),
                }),
                "delete" => Ok(match win32::service_delete(name)? {
                    win32::SvcResult::Done => Applied::Done,
                    _ => Applied::Already("service already removed".into()),
                }),
                other => Err(format!("unsupported service op '{other}'")),
            },

            Action::ScheduledTask { path, op } => {
                let state = task_enabled(path);
                let a = match (op.as_str(), state) {
                    ("delete", None) => return Ok(Applied::Already("task already absent".into())),
                    (_, None) => return Ok(Applied::NotApplicable("task not on this Windows".into())),
                    ("disable", Some(false)) => return Ok(Applied::Already("task already disabled".into())),
                    ("enable", Some(true)) => return Ok(Applied::Already("task already enabled".into())),
                    ("disable", _) => format!("/Change /TN \"{path}\" /Disable"),
                    ("enable", _) => format!("/Change /TN \"{path}\" /Enable"),
                    ("delete", _) => format!("/Delete /TN \"{path}\" /F"),
                    (other, _) => return Err(format!("unsupported task op '{other}'")),
                };
                self.program(&sys_tool("schtasks"), &a, path, T_SHORT)
            }

            Action::Appx { name, scope: _, op } => match op.as_str() {
                "remove" | "unregister" => {
                    let known = match &self.appx_cache {
                        Some(k) => k,
                        None => {
                            let k = crate::appx::list_packages().map_err(|e| format!("{name}: {e}"))?;
                            self.appx_cache.insert(k)
                        }
                    };
                    let hits: Vec<(String, String)> = known
                        .iter()
                        .filter(|p| appx_match(name, &p.full_name))
                        .map(|p| (p.full_name.clone(), p.family_name.clone()))
                        .collect();
                    if hits.is_empty() {
                        return Ok(Applied::Already("app not installed".into()));
                    }
                    // Every matched package: remove for all users, then
                    // deprovision its family so it doesn't come back for a new
                    // user — the WinRT PackageManager itself, zero PowerShell.
                    let mut first_err = None;
                    for (full, family) in &hits {
                        if let Err(e) = crate::appx::remove_for_all_users(full) {
                            if first_err.is_none() {
                                first_err = Some(format!("{full}: {e}"));
                            }
                            continue;
                        }
                        let _ = crate::appx::deprovision(family);
                    }
                    self.appx_cache = None; // re-read after a change
                    match first_err {
                        Some(e) => Err(e),
                        None => Ok(Applied::Done),
                    }
                }
                "clear_cache" => Ok(Applied::NotApplicable("cache clear not needed".into())),
                other => Err(format!("unsupported appx op '{other}'")),
            },

            Action::File { op, path, to } => {
                let p = PathBuf::from(expand(path));
                match op.as_str() {
                    "delete" => {
                        let r = if p.is_dir() {
                            std::fs::remove_dir_all(&p)
                        } else if p.exists() {
                            std::fs::remove_file(&p)
                        } else {
                            return Ok(Applied::Already("already absent".into()));
                        };
                        r.map(|_| Applied::Done).map_err(|e| format!("{}: {e}", p.display()))
                    }
                    "rename" | "copy" => {
                        let dst = PathBuf::from(expand(to.as_deref().unwrap_or("")));
                        if !p.exists() {
                            return Ok(if op == "rename" && dst.exists() {
                                Applied::Already("already renamed".into())
                            } else {
                                Applied::NotApplicable("file not on this Windows".into())
                            });
                        }
                        let r = if op == "rename" { std::fs::rename(&p, &dst) } else { std::fs::copy(&p, &dst).map(|_| ()) };
                        r.map(|_| Applied::Done).map_err(|e| format!("{}: {e}", p.display()))
                    }
                    other => Err(format!("unsupported file op '{other}'")),
                }
            }

            Action::Kill { name } => match win32::kill_processes(name) {
                (0, _, _) => Ok(Applied::NotApplicable("not running".into())),
                (_, killed, _) if killed > 0 => Ok(Applied::Done),
                (_, _, err) => Err(err.unwrap_or_else(|| format!("{name}: could not terminate"))),
            },

            Action::Command { exe, args } => {
                let tool = sys_tool(exe);
                let is_cmd = tool.file_stem().map(|s| s.eq_ignore_ascii_case("cmd")).unwrap_or(false);
                if is_cmd {
                    self.check_cmd_asset(args)?;
                }
                // cmd expands %VARS% itself (and owns `%a` loop vars); other tools don't.
                let args = if is_cmd { args.clone() } else { expand(args) };
                self.program(&tool, &args, exe, T_SCRIPT)
            }

            Action::Bcd { args } => self.program(&sys_tool("bcdedit"), args, "bcdedit", T_SHORT),

            Action::Powershell { script, runas: _ } => self.powershell(script, "powershell", T_SCRIPT),

            Action::Bundled { file, args, runas: _ } => self.run_asset(file, args, T_SCRIPT),

            Action::Installer { file, args } => self.run_asset(file, args, T_INSTALL),

            Action::Powerplan { import, guid } => {
                let full = self.asset(import)?;
                let path = full.to_string_lossy().to_string();
                let activate = guid.as_deref().is_some_and(|g| !g.is_empty());
                // powrprof.dll directly — the DLL powercfg.exe itself calls.
                // The activated GUID is always the one Windows just assigned,
                // never the schema's `guid` hint (which only says "activate").
                win32::import_power_scheme(&path, activate).map(|_| Applied::Done)
            }

            Action::Device { op, name } => {
                // SetupAPI directly — the same DIF_PROPERTYCHANGE call Device
                // Manager's own Disable/Enable makes. No PowerShell, no process.
                match win32::set_device_enabled(name, op == "enable") {
                    (0, _, _) => Ok(Applied::NotApplicable(format!("device '{name}' not present"))),
                    (_, changed, _) if changed > 0 => Ok(Applied::Done),
                    (_, _, err) => Err(err.unwrap_or_else(|| format!("{name}: could not change device state"))),
                }
            }

            Action::Env { name, data, scope } => {
                let (hive, path) = if scope == "machine" {
                    (Hive::Hklm, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
                } else {
                    (Hive::Hkcu, "Environment")
                };
                if root(hive).open(path).and_then(|k| k.get_string(name)).ok().as_deref() == Some(data.as_str()) {
                    return Ok(Applied::Already("variable already set".into()));
                }
                root(hive)
                    .create(path)
                    .and_then(|k| k.set_string(name, data))
                    .map(|_| Applied::Done)
                    .map_err(|e| format!("env {name}: {e}"))
            }
        }
    }

    fn verify(&mut self, check: &Check) -> Result<bool, String> {
        match check {
            Check::Registry { hive, path, name, equals } => {
                let Ok(key) = root(*hive).open(path) else { return Ok(false) };
                Ok(match (read_value(&key, name), equals) {
                    (Some((_, RegData::Int(have))), RegData::Int(want)) => {
                        have == *want || (have as u32) == (*want as u32)
                    }
                    (Some((_, RegData::Str(have))), RegData::Str(want)) => have.eq_ignore_ascii_case(want),
                    (Some((_, RegData::List(have))), RegData::List(want)) => &have == want,
                    _ => false,
                })
            }
            Check::Service { name, startup } => {
                let Ok(key) = root(Hive::Hklm).open(svc_path(name)) else { return Ok(false) };
                Ok(key.get_u32("Start").map(|v| v == startup_val(*startup)).unwrap_or(false))
            }
            Check::FileAbsent { path } => Ok(!PathBuf::from(expand(path)).exists()),
        }
    }

    /// The inverse of an action, read before it runs (exact type + data).
    fn snapshot(&mut self, action: &Action) -> Option<Action> {
        match action {
            Action::Registry { op, hive, path, name, .. } if op == "set" || op == "delete" => {
                let n = name.clone().unwrap_or_default();
                let prev = root(*hive).open(path).ok().and_then(|k| read_value(&k, &n));
                Some(match prev {
                    // value existed: restore it exactly (same type, same data)
                    Some((kind, data)) => Action::Registry {
                        op: "set".into(),
                        hive: *hive,
                        path: path.clone(),
                        name: Some(n),
                        kind: Some(kind),
                        data: Some(data),
                    },
                    // value did not exist: undoing a set means deleting it
                    None if op == "set" => Action::Registry {
                        op: "delete".into(),
                        hive: *hive,
                        path: path.clone(),
                        name: Some(n),
                        kind: None,
                        data: None,
                    },
                    None => return None,
                })
            }
            Action::Service { name, op, .. } if op == "change" => {
                let v = root(Hive::Hklm).open(svc_path(name)).ok()?.get_u32("Start").ok()?;
                Some(Action::Service { name: name.clone(), startup: startup_from(v), op: "change".into() })
            }
            Action::Env { name, scope, .. } => {
                let (hive, path) = if scope == "machine" {
                    (Hive::Hklm, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
                } else {
                    (Hive::Hkcu, "Environment")
                };
                let prev = root(hive).open(path).ok().and_then(|k| read_value(&k, name));
                Some(match prev {
                    Some((kind, data)) => Action::Registry {
                        op: "set".into(),
                        hive,
                        path: path.into(),
                        name: Some(name.clone()),
                        kind: Some(kind),
                        data: Some(data),
                    },
                    None => Action::Registry {
                        op: "delete".into(),
                        hive,
                        path: path.into(),
                        name: Some(name.clone()),
                        kind: None,
                        data: None,
                    },
                })
            }
            Action::File { op, path, to: Some(to) } if op == "rename" => {
                Some(Action::File { op: "rename".into(), path: to.clone(), to: Some(path.clone()) })
            }
            _ => None,
        }
    }
}

/// Expand %VAR% using the process environment and tidy doubled backslashes
/// left over from YAML escaping (`C:\\Windows` -> `C:\Windows`).
pub(crate) fn expand(p: &str) -> String {
    let mut out = String::with_capacity(p.len());
    let mut rest = p;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let var = &after[..end];
                match std::env::var(var) {
                    Ok(v) if !var.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(var);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let unc = out.starts_with(r"\\");
    let mut tidy = String::with_capacity(out.len());
    let mut prev_bs = false;
    for (i, c) in out.chars().enumerate() {
        if c == '\\' && prev_bs && !(unc && i == 1) {
            continue;
        }
        prev_bs = c == '\\';
        tidy.push(c);
    }
    tidy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_powershell_like_dotnet() {
        // [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes('dir'))
        assert_eq!(encode_ps("dir"), "ZABpAHIA");
    }

    #[test]
    fn expands_and_tidies_paths() {
        std::env::set_var("AZ_T", r"C:\X");
        assert_eq!(expand(r"%AZ_T%\\a\\b"), r"C:\X\a\b");
        assert_eq!(expand("%NOPE_AZ%\\x"), "%NOPE_AZ%\\x");
        assert_eq!(expand(r"\\server\share"), r"\\server\share");
    }

    #[test]
    fn hex_roundtrip() {
        assert_eq!(to_hex(&hex_bytes("a8 03 00 FF")), "a80300ff");
    }

    #[test]
    fn appx_patterns() {
        let full = "Microsoft.BingNews_4.55.62231.0_x64__8wekyb3d8bbwe";
        assert!(appx_match("Microsoft.BingNews", full));
        assert!(appx_match("*BingNews*", full));
        assert!(!appx_match("*Disney*", full));
    }

    #[test]
    fn same_value_rules() {
        let cur = (RegKind::Dword, RegData::Int(0x8000_0001));
        assert!(same_value(&cur, RegKind::Dword, &Some(RegData::Int(2147483649))));
        assert!(!same_value(&cur, RegKind::Dword, &Some(RegData::Int(1))));
        // same number but different type = must be rewritten
        assert!(!same_value(&(RegKind::Sz, RegData::Str("1".into())), RegKind::Dword, &Some(RegData::Int(1))));
    }

    // ---- real-machine checks: read-only or on a throwaway key under HKCU ----

    const TEST_KEY: &str = r"Software\AzizosEngineTest";

    #[test]
    fn registry_set_is_honest_on_this_machine() {
        let mut be = WindowsBackend::new(std::env::temp_dir());
        let set = Action::Registry {
            op: "set".into(),
            hive: Hive::Hkcu,
            path: TEST_KEY.into(),
            name: Some("V".into()),
            kind: Some(RegKind::Dword),
            data: Some(RegData::Int(7)),
        };
        let _ = windows_registry::CURRENT_USER.remove_tree(TEST_KEY);
        // first: snapshot says "delete to undo", apply writes
        assert!(matches!(be.snapshot(&set), Some(Action::Registry { ref op, .. }) if op == "delete"));
        assert_eq!(be.apply(&set).unwrap(), Applied::Done);
        // second: already applied, nothing written
        assert!(matches!(be.apply(&set).unwrap(), Applied::Already(_)));
        // snapshot now captures the exact current value for rollback
        assert!(matches!(be.snapshot(&set), Some(Action::Registry { data: Some(RegData::Int(7)), .. })));
        let del_key = Action::Registry {
            op: "delete_key".into(),
            hive: Hive::Hkcu,
            path: TEST_KEY.into(),
            name: None,
            kind: None,
            data: None,
        };
        assert_eq!(be.apply(&del_key).unwrap(), Applied::Done);
        assert!(matches!(be.apply(&del_key).unwrap(), Applied::Already(_)));
    }

    #[test]
    fn absent_targets_are_not_errors() {
        let mut be = WindowsBackend::new(std::env::temp_dir());
        let svc = Action::Service { name: "Azizos_No_Such_Svc".into(), startup: Some(ServiceStartup::Disabled), op: "change".into() };
        assert!(matches!(be.apply(&svc).unwrap(), Applied::NotApplicable(_)));
        let kill = Action::Kill { name: "azizos_no_such_process*".into() };
        assert!(matches!(be.apply(&kill).unwrap(), Applied::NotApplicable(_)));
        let file = Action::File { op: "delete".into(), path: r"%TEMP%\azizos_no_such_file.tmp".into(), to: None };
        assert!(matches!(be.apply(&file).unwrap(), Applied::Already(_)));
        let task = Action::ScheduledTask { path: r"\Azizos\NoSuchTask".into(), op: "disable".into() };
        assert!(matches!(be.apply(&task).unwrap(), Applied::NotApplicable(_)));
        let appx = Action::Appx { name: "*AzizosNoSuchApp*".into(), scope: "family".into(), op: "remove".into() };
        assert!(matches!(be.apply(&appx).unwrap(), Applied::Already(_)));
    }

    #[test]
    fn existing_service_reads_its_start_type() {
        // Read-only: the snapshot of a real service returns its current startup.
        let mut be = WindowsBackend::new(std::env::temp_dir());
        let a = Action::Service { name: "Schedule".into(), startup: Some(ServiceStartup::Auto), op: "change".into() };
        assert!(matches!(be.snapshot(&a), Some(Action::Service { startup: Some(_), .. })));
    }

    #[test]
    fn missing_cmd_asset_is_reported_clearly() {
        let be = WindowsBackend::new(std::env::temp_dir().join("azizos_empty_assets"));
        let e = be.check_cmd_asset("/c vcpp.exe /ai").unwrap_err();
        assert!(e.contains("missing asset 'vcpp.exe'"));
        assert!(be.check_cmd_asset("/c xcopy \"Windows\" \"%windir%\" /E").is_ok());
    }
}
