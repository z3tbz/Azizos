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
