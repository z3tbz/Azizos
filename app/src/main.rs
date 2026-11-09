//! Azizos — Tauri 2 shell. The UI (../ui) calls these commands through
//! `window.__TAURI__.core.invoke`; all real work lives in `azizos-engine`.
//! Everything (catalog, assets) is embedded at build time — see build.rs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("the Azizos app targets Windows 10/11 only (the engine itself builds anywhere)");

mod report;
mod system;

use azizos_engine::catalog::{Catalog, SystemProfile};
use azizos_engine::detect::{self, Detector, RequirementStatus};
use azizos_engine::exec::{self, Phase, Report};
use azizos_engine::schema::Action;
use azizos_engine::{Backend, WindowsBackend, WindowsDetector};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{Emitter, Manager};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

struct AppState {
    catalog: Arc<Catalog>,
    /// Where the embedded assets were unpacked for this session (lazy).
    assets: Mutex<Option<PathBuf>>,
}

impl AppState {
    fn assets_dir(&self) -> Result<PathBuf, String> {
        let mut slot = self.assets.lock().map_err(|_| "state poisoned")?;
        if let Some(p) = slot.as_ref() {
            return Ok(p.clone());
        }
        let p = system::unpack_assets(embedded::ASSETS)?;
        *slot = Some(p.clone());
        Ok(p)
    }
}

#[derive(Serialize, Clone)]
struct Progress {
    done: usize,
    total: usize,
    id: String,
    failed: bool,
    /// One word for the live log: "applied" | "already" | "na" | "failed".
    state: &'static str,
}

/// Summarise one tweak for the live log: any real error wins; otherwise it
/// counts as applied if anything changed, else already/not-applicable.
fn tweak_state(t: &exec::TweakOutcome) -> &'static str {
    use azizos_engine::Status as S;
    let has = |s: S| t.actions.iter().any(|a| a.status == s);
    if t.failed() {
        "failed"
    } else if has(S::Applied) || has(S::Verified) || has(S::Skipped) {
        "applied"
    } else if has(S::AlreadyApplied) {
        "already"
    } else {
        "na"
    }
}

#[derive(Serialize)]
struct ReportDto {
    /// applied / already / not_applicable / failed / skipped (action totals)
    counts: exec::Counts,
    /// Only REAL errors: (tweak id, action index, reason incl. error code).
    /// "Already applied" and "not on this Windows" are never listed here.
    failures: Vec<(String, usize, String)>,
    /// Folder on the Desktop holding report.txt + report.json (None if it could not be written).
    archive: Option<String>,
}

#[tauri::command]
fn check_requirements() -> Vec<RequirementStatus> {
    detect::check_requirements(&WindowsDetector)
}

#[tauri::command]
fn detect_system() -> Result<SystemProfile, String> {
    WindowsDetector.profile()
}

/// Apply the user's selection. Runs on a worker thread so the window stays
/// responsive, and emits `apply-progress` after every tweak.
#[tauri::command]
async fn run_plan(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    selected: Vec<String>,
    dry_run: bool,
) -> Result<ReportDto, String> {
    let catalog = state.catalog.clone();
    let assets = state.assets_dir()?;
    let desktop = app.path().desktop_dir().ok();
    let emitter = app.clone();

    tauri::async_runtime::spawn_blocking(move || -> Result<ReportDto, String> {
        let sys = WindowsDetector.profile()?;
        let plan = catalog.plan(&selected, &sys).map_err(|e| e.to_string())?;
        let total = plan.total();
        let mut done = 0usize;
        let mut on_tweak = |t: &exec::TweakOutcome| {
            done += 1;
            let _ = emitter.emit(
                "apply-progress",
                Progress { done, total, id: t.id.clone(), failed: t.failed(), state: tweak_state(t) },
            );
        };

        let mut be = WindowsBackend::new(assets);
        // Immediate changes first; reboot-dependent ones last (they take effect
        // on the restart that follows).
        let mut report: Report =
            exec::run_phase_observed(&catalog, &plan, Phase::Immediate, &mut be, dry_run, &mut on_tweak);
        let deferred =
            exec::run_phase_observed(&catalog, &plan, Phase::Deferred, &mut be, dry_run, &mut on_tweak);
        report.tweaks.extend(deferred.tweaks);

        let archive = desktop
            .and_then(|d| report::write_archive(&d, &catalog, &sys, &selected, &report).ok())
            .map(|p| p.to_string_lossy().to_string());
        Ok(ReportDto { counts: report.counts(), failures: report.failures(), archive })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "Fix automatically" for a pre-flight requirement: runs the bundled file the
/// playbook author mapped to it in data/preflight.json. The UI only calls this
/// after the user has explicitly agreed in a confirmation dialog.
#[tauri::command]
async fn run_preflight_fix(state: tauri::State<'_, AppState>, requirement: String) -> Result<(), String> {
    let fix = system::preflight_fix(embedded::PREFLIGHT, &requirement)
        .ok_or("no automatic fix is bundled for this requirement")?;
    let assets = state.assets_dir()?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut be = WindowsBackend::new(assets);
        be.apply(&Action::Bundled { file: fix.file, args: fix.args, runas: "user_elevated".into() })
            .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "I'll do it myself": open Windows Security on the relevant page.
#[tauri::command]
fn open_windows_security() -> Result<(), String> {
    system::open_uri("windowsdefender://threatsettings")
}

#[derive(Serialize)]
struct FinishDto {
    /// AZIZ Gaming Optimizer will open once after the next sign-in.
    optimizer_scheduled: bool,
    restarting: bool,
}

#[tauri::command]
fn finish(app: tauri::AppHandle, state: tauri::State<'_, AppState>, auto_restart: bool) -> Result<FinishDto, String> {
    let optimizer_scheduled = system::schedule_optimizer_after_reboot().is_ok();
    if let Ok(mut slot) = state.assets.lock() {
        if let Some(dir) = slot.take() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
    if auto_restart {
        system::restart_in(5)?;
        app.exit(0);
    }
    Ok(FinishDto { optimizer_scheduled, restarting: auto_restart })
}

fn main() {
    let catalog = Catalog::from_json_docs(embedded::TWEAKS.iter().map(|s| s.to_string()))
        .expect("embedded catalog is validated at build time by `cargo test`");

    tauri::Builder::default()
        .manage(AppState { catalog: Arc::new(catalog), assets: Mutex::new(None) })
        .invoke_handler(tauri::generate_handler![
            check_requirements,
            detect_system,
            run_plan,
            run_preflight_fix,
            open_windows_security,
            finish
        ])
        .run(tauri::generate_context!())
        .expect("error while running Azizos");
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_catalog_loads() {
        let c = azizos_engine::Catalog::from_json_docs(super::embedded::TWEAKS.iter().map(|s| s.to_string()))
            .expect("catalog");
        assert!(!c.all().is_empty());
    }
}
