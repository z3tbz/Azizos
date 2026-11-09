//! Azizos app — Tauri shell bridging the UI to the engine.
//!
//! The UI (../ui) calls these commands via `window.__TAURI__.invoke`. All the
//! real work lives in `azizos-engine`; this file only wires commands to it and
//! loads the embedded tweak catalog. Build on Windows (see docs/BUILD.md).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use azizos_engine::catalog::{Catalog, SystemProfile};
use azizos_engine::detect::{check_requirements, RequirementStatus};
use azizos_engine::exec::{self, Phase};
use azizos_engine::schema::Tweak;
use serde::Serialize;
use std::sync::Mutex;

/// Tweak catalog embedded at compile time from `../data/tweaks/*.json`.
/// `include_dir`-style embedding keeps everything inside the single binary.
const CATALOG_JSON: &[(&str, &str)] = &[]; // populated by build.rs / include! in real build

struct AppState {
    catalog: Catalog,
}

fn load_catalog() -> Catalog {
    // In the shipped build the JSON docs are embedded; here we read the folder
    // next to the app during development.
    let mut docs = Vec::new();
    if CATALOG_JSON.is_empty() {
        if let Ok(rd) = std::fs::read_dir("../data/tweaks") {
            for e in rd.flatten() {
                if e.path().extension().and_then(|s| s.to_str()) == Some("json") {
                    if let Ok(s) = std::fs::read_to_string(e.path()) {
                        docs.push(s);
                    }
                }
            }
        }
    } else {
        docs = CATALOG_JSON.iter().map(|(_, c)| c.to_string()).collect();
    }
    Catalog::from_json_docs(docs).expect("catalog must load")
}

#[derive(Serialize)]
struct ReportDto {
    applied: usize,
    failed: usize,
    skipped: usize,
    failures: Vec<(String, usize, String)>,
}

#[tauri::command]
fn get_catalog(state: tauri::State<Mutex<AppState>>) -> Vec<Tweak> {
    state.lock().unwrap().catalog.all().to_vec()
}

#[tauri::command]
fn check_requirements_cmd() -> Vec<RequirementStatus> {
    #[cfg(windows)]
    {
        return check_requirements(&azizos_engine::detect_windows::WindowsDetector);
    }
    #[allow(unreachable_code)]
    Vec::new()
}

#[tauri::command]
fn detect_system() -> Result<SystemProfile, String> {
    #[cfg(windows)]
    {
        use azizos_engine::detect::Detector;
        return azizos_engine::detect_windows::WindowsDetector.profile();
    }
    #[allow(unreachable_code)]
    Err("not windows".into())
}

#[tauri::command]
fn run_plan(
    state: tauri::State<Mutex<AppState>>,
    selected: Vec<String>,
    dry_run: bool,
) -> Result<ReportDto, String> {
    let st = state.lock().unwrap();
    let sys = detect_system().unwrap_or(SystemProfile {
        build: 0,
        edition: "any".into(),
        device: azizos_engine::schema::Device::Any,
        gpu: azizos_engine::schema::Gpu::Any,
        disk: azizos_engine::schema::Disk::Any,
    });
    let plan = st.catalog.plan(&selected, &sys).map_err(|e| e.to_string())?;

    #[cfg(windows)]
    {
        let mut be = azizos_engine::exec_windows::WindowsBackend::new(assets_dir());
        let mut report = exec::run_phase(&st.catalog, &plan, Phase::Immediate, &mut be, dry_run);
        let deferred = exec::run_phase(&st.catalog, &plan, Phase::Deferred, &mut be, dry_run);
        report.tweaks.extend(deferred.tweaks);
        let (applied, failed, skipped) = report.counts();
        return Ok(ReportDto { applied, failed, skipped, failures: report.failures() });
    }
    #[allow(unreachable_code)]
    Err("not windows".into())
}

#[cfg(windows)]
fn assets_dir() -> std::path::PathBuf {
    // Bundled assets are unpacked next to the running exe at startup.
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("assets")))
        .unwrap_or_else(|| std::path::PathBuf::from("assets"))
}

#[tauri::command]
fn finish(auto_restart: bool) {
    // Register the optimizer to open after reboot, then restart.
    #[cfg(windows)]
    if auto_restart {
        let _ = std::process::Command::new("shutdown").args(["/r", "/t", "5"]).spawn();
    }
    let _ = auto_restart;
}

fn main() {
    tauri::Builder::default()
        .manage(Mutex::new(AppState { catalog: load_catalog() }))
        .invoke_handler(tauri::generate_handler![
            get_catalog,
            check_requirements_cmd,
            detect_system,
            run_plan,
            finish
        ])
        .run(tauri::generate_context!())
        .expect("error while running Azizos");
}
