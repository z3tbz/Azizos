//! Repairs concern Azizos-owned state, never Windows settings or tweak replay.
use serde::Serialize;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize)]
pub struct Recovery {
    pub code: &'static str,
    pub area: &'static str,
    pub status: &'static str,
    pub detail: String,
}

static HISTORY: Mutex<Vec<Recovery>> = Mutex::new(Vec::new());

pub fn remember(record: Recovery) {
    let mut history = HISTORY.lock().unwrap_or_else(|e| e.into_inner());
    if history.len() < 64 { history.push(record); }
}

pub fn history() -> Vec<Recovery> {
    HISTORY.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn record(code: &'static str, area: &'static str, status: &'static str, detail: impl Into<String>) -> Recovery {
    let record = Recovery { code, area, status, detail: detail.into() };
    remember(record.clone());
    crate::diagnostics::event(if status == "failed" { "error" } else { "info" }, "app_recovery", serde_json::json!(&record));
    record
}
