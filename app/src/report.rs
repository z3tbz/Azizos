//! Desktop report archive: `Desktop\Azizos-Report-<stamp>\`
//!   report.txt  — bilingual, human-readable (what failed, why, error codes)
//!   report.json — full machine-readable record, including the captured
//!                 previous values (`revert`) so changes can be undone later.

use crate::system::now_local;
use azizos_engine::catalog::{Catalog, SystemProfile};
use azizos_engine::exec::{Report, Status};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub fn create_archive(desktop: &Path) -> std::io::Result<PathBuf> {
    let (stamp, _) = now_local();
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let dir = desktop.join(format!("Azizos-Report-{stamp}-{nonce:x}"));
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

/// Fall back only before any tweak has run. The journal lives in the same
/// durable archive; never rerun changes to recover a failed final report write.
pub fn create_archive_resilient(desktop: Option<&Path>, local_app_data: Option<&Path>) -> Result<(PathBuf, Option<String>), String> {
    let problem = match desktop {
        Some(desktop) => match create_archive(desktop) {
            Ok(path) => return Ok((path, None)),
            Err(e) => format!("Desktop archive unavailable: {e}"),
        },
        None => "Desktop path unavailable".into(),
    };
    let fallback = local_app_data.ok_or_else(|| format!("{problem}; LOCALAPPDATA unavailable; cannot safely save original values"))?
        .join("Azizos").join("Reports");
    std::fs::create_dir_all(&fallback).map_err(|e| format!("{problem}; report fallback {}: {e}", fallback.display()))?;
    let path = create_archive(&fallback).map_err(|e| format!("{problem}; report fallback {}: {e}", fallback.display()))?;
    Ok((path, Some(problem)))
}

pub fn write_into(dir: &Path, catalog: &Catalog, sys: &SystemProfile, selected: &[String], report: &Report) -> std::io::Result<()> {
    let (_, when) = now_local();

    let c = report.counts();
    let name = |id: &str| {
        catalog
            .get(id)
            .map(|t| format!("{} / {}", t.name.en, t.name.ar))
            .unwrap_or_else(|| id.to_string())
    };

    let mut t = String::new();
    // UTF-8 BOM so Notepad on older Windows 10 builds shows Arabic correctly.
    t.push('\u{feff}');
    let _ = writeln!(t, "Azizos — {when}");
    let _ = writeln!(t, "Windows build {} · {} · {:?}\n", sys.build, sys.edition, sys.device);
    let _ = writeln!(t, "Applied / طُبّق: {}", c.applied);
    let _ = writeln!(t, "Already applied / مطبّق مسبقاً: {}", c.already);
    let _ = writeln!(t, "Not applicable / غير منطبق: {}", c.not_applicable);
    let _ = writeln!(t, "Errors / أخطاء: {}", c.failed);
    if c.skipped > 0 {
        let _ = writeln!(t, "Skipped / تُخطّي: {}", c.skipped);
    }
    let _ = writeln!(t);
    let unsupported: usize = report.tweaks.iter().map(|t| t.without_automatic_restore.len()).sum();
    let _ = writeln!(t, "Actions without automatic restore: {unsupported}");
    let _ = writeln!(t, "journal.jsonl contains original state flushed BEFORE each action. Intent records alone do not prove execution succeeded.\n");
    let _ = writeln!(t, "== Application recovery / إصلاحات التطبيق ==");
    for repair in crate::recovery::history() {
        let _ = writeln!(t, "{} [{}] {}: {}", repair.code, repair.status, repair.area, repair.detail);
    }

    let _ = writeln!(t, "== Errors (real failures only) / الأخطاء الفعلية فقط ==");
    if c.failed == 0 {
        let _ = writeln!(t, "none / لا يوجد");
    }
    for tw in &report.tweaks {
        for a in tw.actions.iter().filter(|a| a.status == Status::Failed) {
            let step = if a.index == usize::MAX { "verify".to_string() } else { format!("#{}", a.index + 1) };
            let _ = writeln!(t, "[{}] {step}: {}", name(&tw.id), a.detail.as_deref().unwrap_or(""));
            let diagnostic = azizos_engine::diagnostic::classify(if a.index == usize::MAX { "verify" } else { "apply" }, a.detail.as_deref().unwrap_or(""));
            let _ = writeln!(t, "  {} · {} · Windows/exit: {}\n  {}\n  {}", diagnostic.code, diagnostic.category, diagnostic.native_code.as_deref().unwrap_or("not supplied"), diagnostic.solution_ar, diagnostic.solution_en);
        }
    }

    let _ = writeln!(t, "\n== Not applicable on this PC / غير منطبق على هذا الجهاز ==");
    for tw in &report.tweaks {
        let n = tw.actions.iter().filter(|a| a.status == Status::NotApplicable).count();
        if n > 0 {
            let _ = writeln!(t, "[{}] {n}", name(&tw.id));
        }
    }
    for id in &report.skipped_incompatible {
        let _ = writeln!(t, "[{}] incompatible with this Windows / غير متوافق", name(id));
    }

    let _ = writeln!(t, "\n== Selected / المختار ==");
    for id in selected {
        let _ = writeln!(t, "- {}", name(id));
    }

    std::fs::write(dir.join("report.txt"), t)?;

    let json = serde_json::json!({
        "generated": when,
        "system": sys,
        "selected": selected,
        "counts": c,
        "report": report,
        "diagnostics": report.diagnostics(),
        "session_log": crate::diagnostics::info(),
        "recoveries": crate::recovery::history(),
    });
    std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&json)?)?;
    let mut errors = std::fs::File::create(dir.join("errors.jsonl"))?;
    for (id, index, diagnostic) in report.diagnostics() {
        use std::io::Write;
        let record = serde_json::json!({"tweak_id":id,"action_index":index,"diagnostic":diagnostic});
        serde_json::to_writer(&mut errors, &record)?;
        errors.write_all(b"\n")?;
    }
    errors.sync_data()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    fn fixture() -> std::path::PathBuf {
        // The clock alone can repeat between two tests running in parallel.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nonce=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let path=std::env::temp_dir().join(format!("Azizos-report-recovery-{}-{nonce:x}-{n}",std::process::id()));
        std::fs::create_dir(&path).unwrap();path
    }
    #[test]
    fn report_archive_falls_back_before_any_changes_and_retains_the_reason() {
        let root=fixture(); let desktop=root.join("desktop");std::fs::write(&desktop,b"blocked").unwrap();
        let (archive,problem)=super::create_archive_resilient(Some(&desktop),Some(&root)).unwrap();
        assert!(problem.unwrap().contains("Desktop"));assert!(archive.starts_with(root.join("Azizos").join("Reports")));
        std::fs::write(archive.join("journal.jsonl"),b"test original state").unwrap();
        assert!(archive.join("journal.jsonl").is_file());
        std::fs::remove_file(archive.join("journal.jsonl")).unwrap();std::fs::remove_dir(archive).unwrap();
        std::fs::remove_dir(root.join("Azizos").join("Reports")).unwrap();std::fs::remove_dir(root.join("Azizos")).unwrap();
        std::fs::remove_file(desktop).unwrap();std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn unavailable_primary_and_fallback_report_storage_is_a_real_failure() {
        let root=fixture();let blocked=root.join("blocked");std::fs::write(&blocked,b"file").unwrap();
        assert!(super::create_archive_resilient(Some(&blocked),Some(&blocked)).is_err());
        assert!(!blocked.join("Azizos").exists());
        std::fs::remove_file(blocked).unwrap();std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn real_report_files_include_codes_solutions_and_action_reference() {
        use azizos_engine::exec::{ActionOutcome, TweakOutcome};
        use super::*;
        let catalog = Catalog::from_json_docs(crate::embedded::TWEAKS.iter().map(|s| s.to_string())).unwrap();
        let profile = SystemProfile { build: 26100, edition: "pro".into(), device: azizos_engine::Device::Desktop,
            gpu: azizos_engine::Gpu::Any, disk: azizos_engine::Disk::Any };
        let report = Report { tweaks: vec![TweakOutcome { id:"core-registry".into(),
            actions:vec![ActionOutcome { index:7, status:Status::Failed, detail:Some("Access is denied. (os error 5)".into()) }],
            revert:vec![],without_automatic_restore:vec![] }], skipped_incompatible:vec![] };
        let dir = create_archive(&std::env::temp_dir()).unwrap();
        write_into(&dir, &catalog, &profile, &[], &report).unwrap();
        let text = std::fs::read_to_string(dir.join("report.txt")).unwrap();
        assert!(text.contains("AZ-1001") && text.contains("#8"));
        let errors: serde_json::Value = serde_json::from_str(std::fs::read_to_string(dir.join("errors.jsonl")).unwrap().trim()).unwrap();
        assert_eq!(errors["action_index"], 7);
        assert_eq!(errors["diagnostic"]["native_code"], "5");
        assert!(!errors["diagnostic"]["solution_ar"].as_str().unwrap().is_empty());
        for name in ["report.txt","report.json","errors.jsonl"] { std::fs::remove_file(dir.join(name)).unwrap(); }
        std::fs::remove_dir(dir).unwrap();
    }
}
