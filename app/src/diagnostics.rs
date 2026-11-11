//! Per-user, per-launch bounded JSONL logs. Never uploads logs or original values.
use azizos_engine::diagnostic::{classify, Diagnostic};
use serde::Serialize;
use serde_json::{json, Value};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

const LIMIT: u64 = 1024 * 1024;
const KEEP: usize = 30;
static LOG: OnceLock<Mutex<LogState>> = OnceLock::new();
static SESSION_NONCE: AtomicU64 = AtomicU64::new(0);

struct LogState {
    log: Option<SessionLog>,
    error: Option<String>,
    automatic_retry_used: bool,
}

fn log_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(base).join("Azizos").join("Logs"));
    }
    let temporary = std::env::temp_dir().join("Azizos").join("Logs");
    if !candidates.contains(&temporary) { candidates.push(temporary); }
    candidates
}

impl LogState {
    fn open(candidates: &[PathBuf]) -> Self {
        let mut errors = Vec::new();
        for directory in candidates {
            match SessionLog::create(directory.clone()).and_then(|mut log| {
                log.append("info", "log_ready", json!({}))?;
                Ok(log)
            }) {
                Ok(log) => {
                    if !errors.is_empty() {
                        crate::recovery::remember(crate::recovery::Recovery { code:"AZ-2001", area:"logging", status:"repaired",
                            detail:format!("{}; using {}", errors.join("; "), directory.display()) });
                    }
                    return Self { log:Some(log), error:None, automatic_retry_used:false };
                }
                Err(e) => errors.push(format!("{}: {e}", directory.display())),
            }
        }
        Self { log:None, error:Some(errors.join("; ")), automatic_retry_used:false }
    }

    fn emit(&mut self, level: &str, stage: &str, data: Value, candidates: &[PathBuf]) {
        let result = match self.log.as_mut() {
            Some(log) => log.append(level, stage, data.clone()),
            None => return,
        };
        if let Err(error) = result {
            self.error = Some(error.to_string());
            if self.automatic_retry_used { return; }
            self.automatic_retry_used = true;
            let previous = self.log.as_ref().map(|l| l.path.display().to_string());
            let mut replacement = Self::open(candidates);
            if let Some(log) = replacement.log.as_mut() {
                let record = crate::recovery::Recovery { code:"AZ-2001", area:"logging", status:"repaired",
                    detail:format!("Reopened session log after {error}; previous log: {}", previous.as_deref().unwrap_or("unavailable")) };
                let retry = log.append("info", "app_recovery", json!(&record))
                    .and_then(|_| log.append(level, stage, data));
                match retry {
                    Ok(()) => { crate::recovery::remember(record); self.log = replacement.log; self.error = None; }
                    Err(e) => self.error = Some(format!("{error}; automatic recovery failed: {e}")),
                }
            } else { self.error = Some(format!("{error}; automatic recovery failed: {}", replacement.error.unwrap_or_default())); }
        }
    }
}

struct SessionLog {
    directory: PathBuf,
    path: PathBuf,
    session: String,
    file: File,
    bytes: u64,
    part: u64,
    sequence: u64,
}

fn timestamp() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis()
}

fn prune(directory: &Path, current: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else { return };
    let mut files: Vec<_> = entries.filter_map(Result::ok).filter(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        e.path() != current && name.starts_with("session-") && name.ends_with(".jsonl")
            && e.file_type().map(|t| t.is_file()).unwrap_or(false)
    }).collect();
    files.sort_by_key(|e| e.file_name());
    let remove = files.len().saturating_sub(KEEP - 1);
    for file in files.into_iter().take(remove) { let _ = std::fs::remove_file(file.path()); }
}

impl SessionLog {
    fn create(directory: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&directory)?;
        let session = format!("{}-{}-{:04}", timestamp(), std::process::id(), SESSION_NONCE.fetch_add(1, Ordering::Relaxed));
        let path = directory.join(format!("session-{session}-0000.jsonl"));
        let file = OpenOptions::new().create_new(true).write(true).open(&path)?;
        prune(&directory, &path);
        Ok(Self { directory, path, session, file, bytes: 0, part: 0, sequence: 0 })
    }

    fn append(&mut self, level: &str, stage: &str, data: Value) -> std::io::Result<()> {
        if !self.path.try_exists()? { return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Session log path disappeared")); }
        if self.bytes >= LIMIT {
            let part = self.part + 1;
            let path = self.directory.join(format!("session-{}-{part:04}.jsonl", self.session));
            let file = OpenOptions::new().create_new(true).write(true).open(&path)?;
            self.file = file; self.path = path; self.bytes = 0; self.part = part;
            prune(&self.directory, &self.path);
        }
        self.sequence += 1;
        let entry = json!({"schema":1,"time_unix_ms":timestamp(),"session_id":self.session,
            "sequence":self.sequence,"pid":std::process::id(),"level":level,"stage":stage,"data":data});
        let mut bytes = serde_json::to_vec(&entry)?;
        bytes.push(b'\n');
        self.file.write_all(&bytes)?;
        self.file.sync_data()?;
        self.bytes += bytes.len() as u64;
        Ok(())
    }
}

pub fn init() {
    LOG.get_or_init(|| Mutex::new(LogState::open(&log_candidates())));
    let version = serde_json::from_str::<Value>(include_str!("../tauri.conf.json")).ok()
        .and_then(|v| v.get("version").cloned());
    event("info", "startup", json!({"version":version,"architecture":std::env::consts::ARCH,"recoveries":crate::recovery::history()}));
}

pub fn event(level: &str, stage: &str, data: Value) {
    if let Some(store) = LOG.get() {
        let mut store = if stage == "panic" {
            match store.try_lock() { Ok(s) => s, Err(_) => return }
        } else { store.lock().unwrap_or_else(|e| e.into_inner()) };
        if stage == "panic" {
            if let Some(log) = store.log.as_mut() { let _ = log.append(level, stage, data); }
        } else { store.emit(level, stage, data, &log_candidates()); }
    }
}

/// Explicit repair is bounded and only reopens application-owned log files.
pub fn repair() -> crate::recovery::Recovery {
    let Some(store) = LOG.get() else { return crate::recovery::record("AZ-2001", "logging", "failed", "Logger not initialized"); };
    let mut state = store.lock().unwrap_or_else(|e| e.into_inner());
    let result = state.log.as_mut().ok_or_else(|| std::io::Error::other("log unavailable"))
        .and_then(|log| log.append("info", "log_health_check", json!({})));
    let (status, detail) = match result {
        Ok(()) if state.error.is_none() => ("healthy", "Session log is writable".to_string()),
        _ => {
            let original = state.error.clone();
            *state = LogState::open(&log_candidates());
            if state.log.is_some() { ("repaired", format!("Session log reopened; previous error: {}", original.unwrap_or_default())) }
            else { ("failed", state.error.clone().unwrap_or_else(|| "No writable log directory".into())) }
        }
    };
    drop(state);
    crate::recovery::record("AZ-2001", "logging", status, detail)
}

#[derive(Debug, Serialize)]
pub struct LogInfo {
    pub session_id: Option<String>,
    pub directory: Option<String>,
    pub path: Option<String>,
    pub error: Option<String>,
    pub recoveries: Vec<crate::recovery::Recovery>,
}

pub fn info() -> LogInfo {
    let recoveries = crate::recovery::history();
    match LOG.get() {
        Some(store) => {
            let store = store.lock().unwrap_or_else(|e| e.into_inner());
            LogInfo { session_id: store.log.as_ref().map(|l| l.session.clone()),
                directory:store.log.as_ref().map(|l| l.directory.display().to_string()),
                path:store.log.as_ref().map(|l| l.path.display().to_string()), error:store.error.clone(), recoveries }
        }
        None => LogInfo { session_id: None, directory: None, path: None, error: Some("logging has not initialized".into()), recoveries },
    }
}

#[derive(Debug, Serialize)]
pub struct AppError {
    #[serde(flatten)]
    pub diagnostic: Diagnostic,
    pub session_id: Option<String>,
    pub log_path: Option<String>,
    pub logging_error: Option<String>,
}

pub fn failure(stage: &str, message: impl ToString) -> AppError {
    let message: String = message.to_string().chars().take(32768).collect();
    let diagnostic = classify(stage, &message);
    event("error", stage, json!({"diagnostic":diagnostic}));
    let info = if stage == "panic" {
        // The hook must not reacquire a possibly held logger mutex.
        LogInfo { session_id: None, directory: None, path: None, error: None, recoveries:Vec::new() }
    } else { info() };
    AppError { diagnostic, session_id: info.session_id, log_path: info.path, logging_error: info.error }
}

pub fn hook_panics() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        // A panic may occur while an action is isolated by catch_unwind. Log it,
        // but never interrupt the entire plan with a modal from a worker thread.
        let _ = failure("panic", format!("{panic}\n{}", std::backtrace::Backtrace::force_capture()));
        previous(panic);
    }));
}

pub fn startup_dialog(error: &AppError) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let message = format!("{}\n{}\n\n{}\n\nLog: {}{}", error.diagnostic.code, error.diagnostic.message,
        error.diagnostic.solution_ar, error.log_path.as_deref().unwrap_or("unavailable"),
        error.logging_error.as_ref().map(|e| format!("\nLogging error: {e}")).unwrap_or_default());
    let text: Vec<u16> = message.encode_utf16().chain(Some(0)).collect();
    let title: Vec<u16> = "Azizos — خطأ بدء التشغيل".encode_utf16().chain(Some(0)).collect();
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR); }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> PathBuf {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("Azizos-{name}-{}-{nonce:x}",std::process::id()));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn log_falls_back_to_a_writable_directory_and_preserves_the_original_problem() {
        let root = fixture("log-fallback");
        let blocked = root.join("blocked");
        std::fs::write(&blocked,b"not a directory").unwrap();
        let fallback = root.join("fallback");
        let mut state = LogState::open(&[blocked.clone(),fallback.clone()]);
        assert!(state.error.is_none());
        assert_eq!(state.log.as_ref().unwrap().directory,fallback);
        state.emit("error","synthetic_failure",json!({"code":"AZ-1005"}),&[fallback.clone()]);
        let path = state.log.as_ref().unwrap().path.clone();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("synthetic_failure") && text.contains("AZ-1005"));
        assert!(crate::recovery::history().iter().any(|r| r.code=="AZ-2001" && r.detail.contains("blocked")));
        drop(state);
        std::fs::remove_file(path).unwrap(); std::fs::remove_dir(fallback).unwrap();
        std::fs::remove_file(blocked).unwrap(); std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn disappeared_log_path_is_recovered_once_and_the_failed_event_is_retried() {
        let root = fixture("log-reopen");
        let mut state = LogState::open(&[root.clone()]);
        let original = state.log.as_ref().unwrap().path.clone();
        let renamed = root.join("preserved-original.jsonl");
        std::fs::rename(&original,&renamed).unwrap();
        state.emit("error","retained_event",json!({"important":true}),&[root.clone()]);
        assert!(state.error.is_none()); assert!(state.automatic_retry_used);
        let recovered = state.log.as_ref().unwrap().path.clone();
        assert_ne!(recovered,original);
        assert!(std::fs::read_to_string(&recovered).unwrap().contains("retained_event"));
        assert!(renamed.exists());
        drop(state);
        std::fs::remove_file(renamed).unwrap(); std::fs::remove_file(recovered).unwrap(); std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn unwritable_log_candidates_are_reported_as_failure() {
        let root = fixture("log-unavailable");
        let blocked = root.join("blocked"); std::fs::write(&blocked,b"file").unwrap();
        let state = LogState::open(&[blocked.clone()]);
        assert!(state.log.is_none()); assert!(state.error.as_deref().unwrap().contains("blocked"));
        std::fs::remove_file(blocked).unwrap(); std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn real_files_are_jsonl_rotated_and_keep_session_sequence() {
        let directory = std::env::temp_dir().join(format!("Azizos-log-test-{}-{}", std::process::id(), timestamp()));
        let mut log = SessionLog::create(directory.clone()).unwrap();
        log.append("error", "startup", json!({"message":"العربية\nline","code":"AZ-1001"})).unwrap();
        let first = log.path.clone();
        log.bytes = LIMIT;
        log.append("info", "finish", json!({"ok":true})).unwrap();
        let last = log.path.clone();
        assert_ne!(first, last);
        let entry: Value = serde_json::from_str(std::fs::read_to_string(&first).unwrap().trim()).unwrap();
        assert_eq!(entry["sequence"], 1);
        assert_eq!(entry["data"]["message"], "العربية\nline");
        let entry: Value = serde_json::from_str(std::fs::read_to_string(&last).unwrap().trim()).unwrap();
        assert_eq!(entry["sequence"], 2);
        drop(log);
        // Delete only files this test created, in its unique temporary folder.
        std::fs::remove_file(first).unwrap(); std::fs::remove_file(last).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
