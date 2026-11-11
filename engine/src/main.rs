//! azizos-cli — load the tweak catalog from `data/tweaks/` and print a plan.
//! Lets us validate the schema + planner without the UI or any system calls.
//! On the real build, `detect` fills `SystemProfile` from the live machine.

use azizos_engine::catalog::{Catalog, SystemProfile};
use azizos_engine::schema::{Device, Disk, Gpu};
use std::fs;
use std::path::Path;

fn load_dir(dir: &Path) -> Result<Catalog, String> {
    let mut docs = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let p = entry.map_err(|e| e.to_string())?.path();
        if p.extension().and_then(|s| s.to_str()) == Some("json") {
            docs.push(fs::read_to_string(&p).map_err(|e| e.to_string())?);
        }
    }
    Catalog::from_json_docs(docs)
}

fn main() {
    #[cfg(all(windows, feature = "native"))]
    if std::env::args().any(|a| a == "--probe") {
        probe();
        return;
    }
    let dir = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "../data/tweaks".to_string());
    let cat = match load_dir(Path::new(&dir)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("load error: {e}");
            std::process::exit(1);
        }
    };

    println!("loaded {} tweaks from {dir}", cat.all().len());
    for t in cat.all() {
        println!("  - {:20} [{}] {}", t.id, t.category, t.display_name("ar"));
    }

    // Stand-in system profile until `detect` lands.
    let sys = SystemProfile {
        build: 26100,
        edition: "pro".into(),
        device: Device::Desktop,
        gpu: Gpu::Nvidia,
        disk: Disk::Ssd,
    };

    let selected: Vec<String> = cat.all().iter().map(|t| t.id.clone()).collect();
    match cat.plan(&selected, &sys) {
        Ok(plan) => {
            println!("\nplan for build {}:", sys.build);
            println!("  immediate ({}): {:?}", plan.immediate.len(), plan.immediate);
            println!("  deferred  ({}): {:?}", plan.deferred.len(), plan.deferred);
            println!("  skipped   ({}): {:?}", plan.skipped_incompatible.len(), plan.skipped_incompatible);
            let conflicts = cat.conflicts(&selected);
            if !conflicts.is_empty() {
                println!("  conflicts: {:?}", conflicts);
            }
        }
        Err(e) => eprintln!("plan error: {e}"),
    }
}

#[cfg(all(windows, feature = "native"))]
fn probe() {
    use azizos_engine::{Backend, Detector, WindowsBackend, WindowsDetector};
    let args: Vec<String> = std::env::args().collect();
    let value = |key: &str, default: &str| args.iter().position(|a| a == key)
        .and_then(|i| args.get(i + 1)).cloned().unwrap_or_else(|| default.into());
    let dir = value("--catalog", "data/tweaks");
    let assets = value("--assets", "assets");
    let catalog = load_dir(Path::new(&dir)).expect("load catalog");
    let profile = WindowsDetector.profile().expect("read system profile");
    let selected = catalog.all().iter().map(|t| t.id.clone()).collect::<Vec<_>>();
    let plan = catalog.plan(&selected, &profile).expect("plan catalog");
    let mut backend = WindowsBackend::probing(assets.into());
    let mut records = Vec::new();
    let (mut changes, mut already, mut absent, mut failed) = (0, 0, 0, 0);
    for id in plan.immediate.iter().chain(&plan.deferred) {
        for (index, action) in catalog.get(id).unwrap().actions.iter().enumerate() {
            let (state, detail) = match backend.apply(action) {
                Ok(azizos_engine::Applied::Done) => { changes += 1; ("would_change", String::new()) }
                Ok(azizos_engine::Applied::Already(s)) => { already += 1; ("already", s) }
                Ok(azizos_engine::Applied::NotApplicable(s)) => { absent += 1; ("not_applicable", s) }
                Err(e) => { failed += 1; ("failed", e) }
            };
            records.push(serde_json::json!({"tweak": id, "index": index, "state": state, "detail": detail}));
        }
    }
    let document = serde_json::json!({"mode": "read_only_probe", "system": profile,
        "requirements": azizos_engine::check_requirements(&WindowsDetector),
        "counts": {"would_change": changes, "already": already, "not_applicable": absent, "failed": failed},
        "actions": records});
    let output = value("--out", "probe-results.json");
    std::fs::write(&output, serde_json::to_vec_pretty(&document).unwrap()).expect("write diagnostic result");
    println!("Read-only probe: {changes} potential changes, {already} already, {absent} absent, {failed} failures. {output}");
    if failed != 0 { std::process::exit(2); }
}
