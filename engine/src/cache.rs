//! Appx cache clearing, matching the author's TempState / LocalState *Cache*
//! contract. Keeps SettingsCache.txt and never follows directory reparse points.
use std::path::{Path, PathBuf};
use crate::{Applied, win32};

fn entries(path: &Path) -> Result<Vec<std::fs::DirEntry>, String> {
    match std::fs::read_dir(path) {
        Ok(rd) => rd.map(|e| e.map_err(|e| format!("{}: {e}", path.display()))).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn collect(path: &Path, targets: &mut Vec<PathBuf>) -> Result<(), String> {
    use std::os::windows::fs::MetadataExt;
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_attributes() & 0x400 != 0 => return Err(format!("cache reparse point refused: {}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
        _ => {},
    }
    for entry in entries(path)? {
        if entry.file_name().eq_ignore_ascii_case("SettingsCache.txt") { continue; }
        let p = entry.path();
        let meta = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        if meta.file_attributes() & 0x400 != 0 { return Err(format!("cache reparse point refused: {}", p.display())); }
        if meta.is_dir() { collect(&p, targets)?; }
        // Keep cache directory structure; delete only files, with original ACLs
        // and the directory's SettingsCache.txt left intact.
        else { targets.push(p); }
    }
    Ok(())
}

pub fn clear(pattern: &str, profiles: &[PathBuf], app_roots: &[PathBuf], probe: bool) -> Result<Applied, String> {
    let mut targets = Vec::new();
    let mut found = false;
    for profile in profiles {
        for package in entries(&profile.join(r"AppData\Local\Packages"))? {
            if !win32::glob(pattern, &package.file_name().to_string_lossy()) { continue; }
            use std::os::windows::fs::MetadataExt;
            if std::fs::symlink_metadata(package.path()).map_err(|e| e.to_string())?.file_attributes() & 0x400 != 0 {
                return Err("refusing Appx package reparse point".into());
            }
            found = true;
            let base = package.path();
            collect(&base.join("TempState"), &mut targets)?;
            for cache in entries(&base.join("LocalState"))? {
                if cache.file_type().map_err(|e| e.to_string())?.is_dir()
                    && win32::glob("*Cache*", &cache.file_name().to_string_lossy()) {
                    collect(&cache.path(), &mut targets)?;
                }
            }
        }
    }
    if targets.is_empty() {
        return Ok(if found { Applied::Already("Appx cache already empty".into()) }
            else { Applied::NotApplicable("no matching per-user Appx cache".into()) });
    }
    if probe { return Ok(Applied::Done); }
    // Stop only executables inside the matching package install directory.
    for root in app_roots {
        for package in entries(root)? {
            if !win32::glob(pattern, &package.file_name().to_string_lossy()) { continue; }
            for exe in entries(&package.path())? {
                if exe.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
                    let (_, _, error) = win32::kill_processes_filtered(&exe.file_name().to_string_lossy(),
                        Some(&package.path().to_string_lossy()), false);
                    if let Some(e) = error { return Err(e); }
                }
            }
        }
    }
    for file in targets { std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?; }
    Ok(Applied::Done)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cache_preview_is_read_only_and_clear_preserves_settings_and_user_data() {
        let base = std::env::temp_dir().join(format!("AzizosCacheTest-{}", std::process::id()));
        let package = base.join(r"AppData\Local\Packages\AzizosFixture_x\LocalState\TileCache");
        std::fs::create_dir_all(&package).unwrap();
        std::fs::write(package.join("SettingsCache.txt"), b"settings").unwrap();
        std::fs::write(package.join("temp.bin"), b"cache").unwrap();
        let user_data = package.parent().unwrap().join("user-document.txt");
        std::fs::write(&user_data, b"keep").unwrap();
        assert_eq!(clear("AzizosFixture*", &[base.clone()], &[], true).unwrap(), Applied::Done);
        assert!(package.join("temp.bin").exists());
        assert_eq!(clear("AzizosFixture*", &[base.clone()], &[], false).unwrap(), Applied::Done);
        assert!(package.join("SettingsCache.txt").exists());
        assert!(user_data.exists());
        assert!(!package.join("temp.bin").exists());
        assert!(matches!(clear("AzizosFixture*", &[base.clone()], &[], false).unwrap(), Applied::Already(_)));
        std::fs::remove_dir_all(base).unwrap();
    }
}
