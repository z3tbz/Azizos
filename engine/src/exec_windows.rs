//! Windows backend for the executor (`#[cfg(windows)]`).
//!
//! Layering, lightest first:
//!   * registry  -> `windows-registry` (Microsoft's thin safe wrapper over the
//!                  Reg* Win32 API: no COM, no WMI, no child process).
//!   * service   -> the service's `Start` value in the registry (0..4). This is
//!                  exactly what the playbook's `startup: N` means, applies on the
//!                  next boot (we always reboot), and needs no SCM handle.
//!   * everything the playbook expresses as a program (bcdedit, fsutil, its own
//!     .bat/.ps1/.exe) -> one child process each, with:
//!       - no console window flashing (CREATE_NO_WINDOW),
//!       - system tools resolved from %SystemRoot%\System32 (no PATH lookups),
//!       - the asset folder as working directory (= AME's `exeDir: true`),
//!       - the argument string passed verbatim (`raw_arg`) — Windows command
//!         lines are strings, re-quoting them is what breaks `cmd /c` lines,
//!       - inline PowerShell sent as -EncodedCommand (no quoting at all),
//!       - a timeout, so a stuck installer can never hang the whole run.
//!
//! Pinned against windows-registry 0.6.1 (see engine/Cargo.toml).

use crate::exec::{Applied, Backend};
use crate::schema::{Action, Check, Hive, RegData, RegKind, ServiceStartup};
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
}

impl WindowsBackend {
    pub fn new(asset_dir: PathBuf) -> Self {
        Self { asset_dir }
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
        let mut c = Command::new(exe);
        c.creation_flags(CREATE_NO_WINDOW)
            .current_dir(&self.asset_dir)
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

/// Read a value back in schema form (used for verify + rollback snapshots).
fn read_value(key: &Key, name: &str) -> Option<(RegKind, RegData)> {
    Some(match key.get_type(name).ok()? {
        Type::U32 => (RegKind::Dword, RegData::Int(key.get_u32(name).ok()? as i64)),
        Type::U64 => (RegKind::Qword, RegData::Int(key.get_u64(name).ok()? as i64)),
        Type::String => (RegKind::Sz, RegData::Str(key.get_string(name).ok()?)),
        Type::ExpandString => (RegKind::ExpandSz, RegData::Str(key.get_string(name).ok()?)),
        Type::MultiString => (RegKind::MultiSz, RegData::List(key.get_multi_string(name).ok()?)),
        Type::Bytes => (RegKind::Binary, RegData::Str(to_hex(&key.get_bytes(name).ok()?))),
        _ => return None,
    })
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

// ----------------------------------------------------------------- process --

fn system_root() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into()))
}

/// Resolve a bare system tool name (`cmd`, `bcdedit`, `DISM.exe`…) to its full
/// System32 path. Anything else is returned unchanged.
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
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
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

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
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
}

// ----------------------------------------------------------------- backend --

impl Backend for WindowsBackend {
    fn apply(&mut self, action: &Action) -> Result<Applied, String> {
        match action {
            Action::Registry { op, hive, path, name, kind, data } => {
                let name = name.as_deref().unwrap_or("");
                match op.as_str() {
                    "set" => {
                        let key = root(*hive).create(path).map_err(|e| format!("{path}: {e}"))?;
                        set_value(&key, name, kind.unwrap_or(RegKind::Sz), data)
                            .map(|_| Applied::Done)
                            .map_err(|e| format!("{path}\\{name}: {e}"))
                    }
                    "delete" => {
                        if root(*hive).open(path).is_err() {
                            return Ok(Applied::NotApplicable("key absent".into()));
                        }
                        let key = root(*hive).create(path).map_err(|e| format!("{path}: {e}"))?;
                        let r = key.remove_value(name);
                        if not_found(&r) {
                            return Ok(Applied::NotApplicable("value absent".into()));
                        }
                        r.map(|_| Applied::Done).map_err(|e| format!("{path}\\{name}: {e}"))
                    }
                    "delete_key" => {
                        let r = root(*hive).remove_tree(path);
                        if not_found(&r) {
                            return Ok(Applied::NotApplicable("key absent".into()));
                        }
                        r.map(|_| Applied::Done).map_err(|e| format!("{path}: {e}"))
                    }
                    other => Err(format!("unsupported registry op '{other}'")),
                }
            }

            Action::Service { name, startup, op } => match op.as_str() {
                "change" => {
                    // Never create a key for a service this edition doesn't have.
                    if root(Hive::Hklm).open(svc_path(name)).is_err() {
                        return Ok(Applied::NotApplicable(format!("service {name} not present")));
                    }
                    let key = root(Hive::Hklm).create(svc_path(name)).map_err(|e| format!("{name}: {e}"))?;
                    key.set_u32("Start", startup_val(startup.unwrap_or(ServiceStartup::Disabled)))
                        .map(|_| Applied::Done)
                        .map_err(|e| format!("{name}: {e}"))
                }
                "stop" => self.program(&sys_tool("sc"), &format!("stop \"{name}\""), name, T_SHORT),
                "delete" => {
                    if root(Hive::Hklm).open(svc_path(name)).is_err() {
                        return Ok(Applied::NotApplicable(format!("service {name} not present")));
                    }
                    self.program(&sys_tool("sc"), &format!("delete \"{name}\""), name, T_SHORT)
                }
                other => Err(format!("unsupported service op '{other}'")),
            },

            Action::ScheduledTask { path, op } => {
                let a = match op.as_str() {
                    "disable" => format!("/Change /TN \"{path}\" /Disable"),
                    "enable" => format!("/Change /TN \"{path}\" /Enable"),
                    "delete" => format!("/Delete /TN \"{path}\" /F"),
                    other => return Err(format!("unsupported task op '{other}'")),
                };
                self.program(&sys_tool("schtasks"), &a, path, T_SHORT)
            }

            Action::Appx { name, scope: _, op } => match op.as_str() {
                "remove" | "unregister" => {
                    let n = ps_quote(name);
                    let script = format!(
                        "$ErrorActionPreference='SilentlyContinue';\
                         Get-AppxPackage -AllUsers | Where-Object {{ $_.Name -like {n} -or $_.PackageFamilyName -like {n} }} | Remove-AppxPackage -AllUsers;\
                         Get-AppxProvisionedPackage -Online | Where-Object {{ $_.DisplayName -like {n} }} | Remove-AppxProvisionedPackage -Online | Out-Null"
                    );
                    self.powershell(&script, name, T_SHORT)
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
                            return Ok(Applied::NotApplicable("already absent".into()));
                        };
                        r.map(|_| Applied::Done).map_err(|e| format!("{}: {e}", p.display()))
                    }
                    "copy" => std::fs::copy(&p, expand(to.as_deref().unwrap_or("")))
                        .map(|_| Applied::Done)
                        .map_err(|e| format!("{}: {e}", p.display())),
                    "rename" => std::fs::rename(&p, expand(to.as_deref().unwrap_or("")))
                        .map(|_| Applied::Done)
                        .map_err(|e| format!("{}: {e}", p.display())),
                    other => Err(format!("unsupported file op '{other}'")),
                }
            }

            Action::Command { exe, args } => {
                let tool = sys_tool(exe);
                let is_cmd = tool.file_stem().map(|s| s.eq_ignore_ascii_case("cmd")).unwrap_or(false);
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
                let g = guid.clone().unwrap_or_default();
                self.program(
                    &sys_tool("powercfg"),
                    &format!("-import \"{}\" {g}", full.display()),
                    "powercfg",
                    T_SHORT,
                )?;
                if !g.is_empty() {
                    self.program(&sys_tool("powercfg"), &format!("-setactive {g}"), "powercfg", T_SHORT)?;
                }
                Ok(Applied::Done)
            }

            Action::Device { op, name } => {
                // Built-in PnpDevice module — no third-party tool needed.
                let verb = if op == "disable" { "Disable-PnpDevice" } else { "Enable-PnpDevice" };
                let script = format!(
                    "Get-PnpDevice -FriendlyName {} -ErrorAction Stop | {verb} -Confirm:$false -ErrorAction Stop",
                    ps_quote(name)
                );
                self.powershell(&script, name, T_SHORT)
            }

            Action::Env { name, data, scope } => {
                let (hive, path) = if scope == "machine" {
                    (Hive::Hklm, r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment")
                } else {
                    (Hive::Hkcu, "Environment")
                };
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

    /// The inverse of an action, captured before it runs, for rollback.
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
}
