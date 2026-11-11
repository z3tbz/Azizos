//! Azizos — Tauri 2 shell. The UI (../ui) calls these commands through
//! `window.__TAURI__.core.invoke`; all real work lives in `azizos-engine`.
//! Everything (catalog, assets) is embedded at build time — see build.rs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(windows))]
compile_error!("the Azizos app targets Windows 10/11 only (the engine itself builds anywhere)");

mod report;
mod diagnostics;
mod recovery;
mod system;

use azizos_engine::catalog::{Catalog, SystemProfile};
use azizos_engine::detect::{self, Detector, RequirementStatus};
use azizos_engine::exec::{self, Phase, Report};
use azizos_engine::schema::Action;
use azizos_engine::{Backend, WindowsBackend, WindowsDetector};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

struct AppState {
    catalog: Arc<Catalog>,
    /// Where the embedded assets were unpacked for this session (lazy).
    assets: Arc<Mutex<Option<PathBuf>>>,
    running: Arc<AtomicBool>,
}

struct RunGuard(Arc<AtomicBool>);
impl Drop for RunGuard { fn drop(&mut self) { self.0.store(false, Ordering::Release); } }

fn begin_run(state: &AppState) -> Result<RunGuard, String> {
    state.running.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| "another operation is already running")?;
    Ok(RunGuard(state.running.clone()))
}

impl AppState {
    fn assets_dir(&self) -> Result<PathBuf, String> {
        let mut slot = self.assets.lock().map_err(|_| "state poisoned")?;
        match system::ensure_assets(&mut slot, embedded::ASSETS) {
            Ok((path, problem)) => {
                if let Some(problem) = problem {
                    recovery::record("AZ-2002", "embedded_assets", "repaired", format!("{problem}; regenerated from embedded files at {}", path.display()));
                }
                Ok(path)
            }
            Err(e) => {
                recovery::record("AZ-2002", "embedded_assets", "failed", &e);
                Err(e)
            }
        }
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
    diagnostics: Vec<(String, usize, azizos_engine::diagnostic::Diagnostic)>,
    log: diagnostics::LogInfo,
}

fn check_requirements_impl() -> Vec<RequirementStatus> {
    detect::check_requirements(&WindowsDetector)
}

fn detect_system_impl() -> Result<SystemProfile, String> {
    WindowsDetector.profile()
}

/// Apply the user's selection. Runs on a worker thread so the window stays
/// responsive, and emits `apply-progress` after every tweak.
async fn run_plan_impl(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    selected: Vec<String>,
    dry_run: bool,
) -> Result<ReportDto, String> {
    let guard = begin_run(&state)?;
    let catalog = state.catalog.clone();
    let assets = state.assets_dir()?;
    let desktop = app.path().desktop_dir().ok();
    let emitter = app.clone();

    tauri::async_runtime::spawn_blocking(move || -> Result<ReportDto, String> {
        let _guard = guard;
        let sys = WindowsDetector.profile()?;
        let plan = catalog.plan(&selected, &sys).map_err(|e| e.to_string())?;
        let total = plan.total();
        let mut done = 0usize;
        let mut on_tweak = |t: &exec::TweakOutcome| {
            for action in &t.actions {
                let diagnostic = if action.status == exec::Status::Failed {
                    Some(azizos_engine::diagnostic::classify(if action.index == usize::MAX { "verify" } else { "apply" }, action.detail.as_deref().unwrap_or("error without detail")))
                } else { None };
                diagnostics::event(if diagnostic.is_some() { "error" } else { "info" }, "action_result",
                    serde_json::json!({"tweak_id":t.id,"action_index":action.index,"status":action.status,"diagnostic":diagnostic}));
            }
            done += 1;
            let _ = emitter.emit(
                "apply-progress",
                Progress { done, total, id: t.id.clone(), failed: t.failed(), state: tweak_state(t) },
            );
        };

        // Create durable storage before any modification; disk errors cannot
        // turn into an unreported run with lost original values.
        let (dir, problem) = report::create_archive_resilient(desktop.as_deref(), std::env::var_os("LOCALAPPDATA").map(PathBuf::from).as_deref())
            .map_err(|e| { recovery::record("AZ-2004", "report_archive", "failed", &e); e })?;
        if let Some(problem) = problem {
            recovery::record("AZ-2004", "report_archive", "repaired", format!("{problem}; report and original state will be saved at {}", dir.display()));
        }
        let mut be = WindowsBackend::new(assets);
        if !dry_run { be.set_journal(&dir.join("journal.jsonl"))?; }
        // Immediate changes first; reboot-dependent ones last (they take effect
        // on the restart that follows).
        let mut report: Report =
            exec::run_phase_observed(&catalog, &plan, Phase::Immediate, &mut be, dry_run, &mut on_tweak);
        let deferred =
            exec::run_phase_observed(&catalog, &plan, Phase::Deferred, &mut be, dry_run, &mut on_tweak);
        report.tweaks.extend(deferred.tweaks);

        report::write_into(&dir, &catalog, &sys, &selected, &report)
            .map_err(|e| format!("report write failed; original state journal remains at {}: {e}", dir.display()))?;
        let archive = Some(dir.to_string_lossy().to_string());
        Ok(ReportDto { counts: report.counts(), failures: report.failures(), diagnostics: report.diagnostics(), archive, log: diagnostics::info() })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Read-only real-machine preview, including presence/already-applied checks.
async fn preview_plan_impl(state: tauri::State<'_, AppState>, selected: Vec<String>) -> Result<PreviewDto, String> {
    let guard = begin_run(&state)?;
    let catalog = state.catalog.clone();
    let assets = state.assets_dir()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let profile = WindowsDetector.profile()?;
        let plan = catalog.plan(&selected, &profile).map_err(|e| e.to_string())?;
        let mut backend = WindowsBackend::probing(assets);
        let mut report = exec::run_phase(&catalog, &plan, Phase::Immediate, &mut backend, false);
        report.tweaks.extend(exec::run_phase(&catalog, &plan, Phase::Deferred, &mut backend, false).tweaks);
        let diagnostics = report.diagnostics();
        diagnostics::event("info", "preview_result", serde_json::json!({"counts":report.counts(),"diagnostics":diagnostics}));
        Ok(PreviewDto { counts: report.counts(), diagnostics })
    }).await.map_err(|e| e.to_string())?
}

/// "Fix automatically" for a pre-flight requirement: runs the bundled file the
/// playbook author mapped to it in data/preflight.json. The UI only calls this
/// after the user has explicitly agreed in a confirmation dialog.
async fn run_preflight_fix_impl(state: tauri::State<'_, AppState>, requirement: String) -> Result<(), String> {
    let guard = begin_run(&state)?;
    let fix = system::preflight_fix(embedded::PREFLIGHT, &requirement)
        .ok_or("no automatic fix is bundled for this requirement")?;
    let assets = state.assets_dir()?;
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        let mut be = WindowsBackend::new(assets);
        be.apply(&Action::Bundled { file: fix.file, args: fix.args, runas: "user_elevated".into() })
            .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// "I'll do it myself": open Windows Security on the relevant page.
fn open_windows_security_impl() -> Result<(), String> {
    system::open_uri("windowsdefender://threatsettings")
}

#[derive(Serialize)]
struct FinishDto {
    /// AZIZ Gaming Optimizer will open once after the next sign-in.
    optimizer_scheduled: bool,
    restarting: bool,
}

fn finish_impl(app: tauri::AppHandle, state: tauri::State<'_, AppState>, auto_restart: bool) -> Result<FinishDto, String> {
    let _guard = begin_run(&state)?;
    let optimizer_scheduled = match system::schedule_optimizer_after_reboot() {
        Ok(()) => true,
        Err(e) => { diagnostics::event("warning", "optimizer_handoff", serde_json::json!({"diagnostic":azizos_engine::diagnostic::classify("optimizer_handoff", &e)})); false }
    };
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

fn run_app() -> Result<(), String> {
    let catalog = Catalog::from_json_docs(embedded::TWEAKS.iter().map(|s| s.to_string()))
        .map_err(|e| format!("catalog: {e}"))?;

    let app = tauri::Builder::default()
        .manage(AppState { catalog: Arc::new(catalog), assets: Arc::new(Mutex::new(None)), running: Arc::new(AtomicBool::new(false)) })
        .invoke_handler(tauri::generate_handler![
            check_requirements,
            detect_system,
            run_plan,
            preview_plan,
            run_preflight_fix,
            open_windows_security,
            finish,
            diagnostic_info,
            open_diagnostic_logs,
            log_frontend_error,
            repair_app,
            log_frontend_repair,
            window_control,
            set_window_theme
        ])
        .setup(|_| { diagnostics::event("info", "window_ready", serde_json::json!({})); Ok(()) })
        .build(tauri::generate_context!())
        .map_err(|e| format!("Tauri startup/runtime: {e}"))?;
    app.run(|_, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            diagnostics::event("info", "shutdown", serde_json::json!({}));
        }
    });
    Ok(())
}

#[derive(Serialize)]
struct PreviewDto {
    #[serde(flatten)]
    counts: exec::Counts,
    diagnostics: Vec<(String, usize, azizos_engine::diagnostic::Diagnostic)>,
}

fn command_result<T>(stage: &str, operation: impl FnOnce() -> Result<T, String>) -> Result<T, diagnostics::AppError> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
        .unwrap_or_else(|_| Err("internal error: command panicked; see session log".into()))
        .map_err(|e| diagnostics::failure(stage, e))
}

#[tauri::command]
fn check_requirements() -> Vec<RequirementDto> {
    let statuses = check_requirements_impl();
    for status in &statuses {
        if status.met == detect::Met::Fail {
            diagnostics::event("warning", "preflight", serde_json::json!({"requirement":status.requirement,
                "diagnostic":azizos_engine::diagnostic::classify("precondition", status.detail.as_deref().unwrap_or("unmet prerequisite"))}));
        }
    }
    statuses.into_iter().map(|status| {
        let diagnostic = if status.met == detect::Met::Fail {
            Some(azizos_engine::diagnostic::classify("precondition", status.detail.as_deref().unwrap_or("unmet prerequisite")))
        } else { None };
        RequirementDto { status, diagnostic }
    }).collect()
}

#[derive(Serialize)]
struct RequirementDto {
    #[serde(flatten)]
    status: RequirementStatus,
    diagnostic: Option<azizos_engine::diagnostic::Diagnostic>,
}

#[tauri::command]
fn detect_system() -> Result<SystemProfile, diagnostics::AppError> {
    command_result("detection", detect_system_impl)
}

#[tauri::command]
async fn run_plan(app: tauri::AppHandle, state: tauri::State<'_, AppState>, selected: Vec<String>, dry_run: bool) -> Result<ReportDto, diagnostics::AppError> {
    diagnostics::event("info", "run_start", serde_json::json!({"selected":selected,"dry_run":dry_run}));
    let result = run_plan_impl(app, state, selected, dry_run).await.map_err(|e| diagnostics::failure("run_plan", e));
    if let Ok(report) = &result { diagnostics::event("info", "run_complete", serde_json::json!({"counts":report.counts,"archive":report.archive})); }
    result
}

#[tauri::command]
async fn preview_plan(state: tauri::State<'_, AppState>, selected: Vec<String>) -> Result<PreviewDto, diagnostics::AppError> {
    preview_plan_impl(state, selected).await.map_err(|e| diagnostics::failure("preview", e))
}

#[tauri::command]
async fn run_preflight_fix(state: tauri::State<'_, AppState>, requirement: String) -> Result<(), diagnostics::AppError> {
    run_preflight_fix_impl(state, requirement).await.map_err(|e| diagnostics::failure("preflight_fix", e))
}

#[tauri::command]
fn open_windows_security() -> Result<(), diagnostics::AppError> {
    command_result("windows_security", open_windows_security_impl)
}

#[tauri::command]
fn finish(app: tauri::AppHandle, state: tauri::State<'_, AppState>, auto_restart: bool) -> Result<FinishDto, diagnostics::AppError> {
    command_result("finish", || finish_impl(app, state, auto_restart))
}

#[tauri::command]
fn diagnostic_info() -> diagnostics::LogInfo { diagnostics::info() }

#[tauri::command]
fn open_diagnostic_logs() -> Result<(), diagnostics::AppError> {
    command_result("open_logs", || {
        let directory = diagnostics::info().directory.ok_or("diagnostic directory unavailable")?;
        system::open_uri(&directory)
    })
}

#[tauri::command]
fn log_frontend_error(stage: String, message: String) -> diagnostics::AppError {
    // Only text, never arbitrary frontend objects, form values or stack arguments.
    let stage: String = stage.chars().take(128).collect();
    diagnostics::failure(&format!("frontend:{stage}"), message)
}

#[tauri::command]
async fn repair_app(state: tauri::State<'_, AppState>) -> Result<diagnostics::LogInfo, diagnostics::AppError> {
    let guard = begin_run(&state).map_err(|e| diagnostics::failure("app_repair", e))?;
    let assets = state.assets.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<diagnostics::LogInfo, String> {
        // The worker owns the guard even if the UI drops its pending request.
        let _guard = guard;
        diagnostics::repair();
        let mut slot = assets.lock().map_err(|_| "internal error: asset state poisoned")?;
        let result = system::ensure_assets(&mut slot, embedded::ASSETS);
        match result {
            Ok((_, problem)) => {
                recovery::record("AZ-2002", "embedded_assets", if problem.is_some() { "repaired" } else { "healthy" },
                    problem.unwrap_or_else(|| "Embedded application files verified".into()));
            }
            Err(e) => { recovery::record("AZ-2002", "embedded_assets", "failed", e); }
        }
        Ok(diagnostics::info())
    }).await.map_err(|e| diagnostics::failure("app_repair", e))?
        .map_err(|e| diagnostics::failure("app_repair", e))
}

#[tauri::command]
fn log_frontend_repair(detail: String) {
    recovery::record("AZ-2003", "ui_preferences", "repaired", detail.chars().take(1024).collect::<String>());
}

#[tauri::command]
fn set_window_theme(window: tauri::Window, theme: String) -> Result<(), diagnostics::AppError> {
    command_result("window_theme", || {
        let theme = match theme.as_str() {
            "light" => tauri::Theme::Light,
            "dark" => tauri::Theme::Dark,
            _ => return Err("invalid window theme".into()),
        };
        window.set_theme(Some(theme)).map_err(|e| e.to_string())
    })
}

#[tauri::command]
fn window_control(window: tauri::Window, state: tauri::State<'_, AppState>, action: String) -> Result<(), diagnostics::AppError> {
    command_result("window_control", || {
        match action.as_str() {
            "minimize" => window.minimize(),
            "maximize" => if window.is_maximized().map_err(|e| e.to_string())? { window.unmaximize() } else { window.maximize() },
            "drag" => window.start_dragging(),
            "close" => {
                if state.running.load(Ordering::Acquire) { return Err("another operation is already running".into()); }
                window.close()
            }
            _ => return Err("invalid window action".into()),
        }.map_err(|e| e.to_string())
    })
}

fn main() {
    diagnostics::init();
    diagnostics::hook_panics();
    let result = std::panic::catch_unwind(run_app)
        .unwrap_or_else(|_| Err("internal error: startup panicked; see session log".into()));
    match result {
        Ok(()) => diagnostics::event("info", "shutdown", serde_json::json!({})),
        Err(e) => {
            let error = diagnostics::failure("startup", e);
            diagnostics::startup_dialog(&error);
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn embedded_assets_include_transitive_script_dependencies() {
        let required: Vec<String> = serde_json::from_str(include_str!("../../data/assets-required.json")).unwrap();
        for required in required {
            assert!(super::embedded::ASSETS.iter().any(|(name, bytes)| !bytes.is_empty()
                && (name.eq_ignore_ascii_case(&required) || name.starts_with(&format!("{required}/")))), "{required}");
        }
    }

    #[test]
    fn embedded_catalog_loads() {
        let c = azizos_engine::Catalog::from_json_docs(super::embedded::TWEAKS.iter().map(|s| s.to_string()))
            .expect("catalog");
        assert!(!c.all().is_empty());
    }
}
