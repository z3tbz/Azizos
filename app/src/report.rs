//! Desktop report archive: `Desktop\Azizos-Report-<stamp>\`
//!   report.txt  — bilingual, human-readable (what failed, why, error codes)
//!   report.json — full machine-readable record, including the captured
//!                 previous values (`revert`) so changes can be undone later.

use crate::system::now_local;
use azizos_engine::catalog::{Catalog, SystemProfile};
use azizos_engine::exec::{Report, Status};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub fn write_archive(
    desktop: &Path,
    catalog: &Catalog,
    sys: &SystemProfile,
    selected: &[String],
    report: &Report,
) -> std::io::Result<PathBuf> {
    let (stamp, when) = now_local();
    let dir = desktop.join(format!("Azizos-Report-{stamp}"));
    std::fs::create_dir_all(&dir)?;

    let (applied, failed, skipped) = report.counts();
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
    let _ = writeln!(t, "Applied / طُبّق: {applied}    Failed / فشل: {failed}    Skipped / تُخطّي: {skipped}\n");

    let _ = writeln!(t, "== Failures / الأخطاء ==");
    if failed == 0 {
        let _ = writeln!(t, "none / لا يوجد");
    }
    for tw in &report.tweaks {
        for a in tw.actions.iter().filter(|a| a.status == Status::Failed) {
            let step = if a.index == usize::MAX { "verify".to_string() } else { format!("#{}", a.index + 1) };
            let _ = writeln!(t, "[{}] {step}: {}", name(&tw.id), a.detail.as_deref().unwrap_or(""));
        }
    }

    let _ = writeln!(t, "\n== Not applicable on this PC / غير منطبق على هذا الجهاز ==");
    for tw in &report.tweaks {
        let n = tw.actions.iter().filter(|a| a.status == Status::Skipped).count();
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
        "counts": { "applied": applied, "failed": failed, "skipped": skipped },
        "report": report,
    });
    std::fs::write(dir.join("report.json"), serde_json::to_vec_pretty(&json).unwrap_or_default())?;
    Ok(dir)
}
