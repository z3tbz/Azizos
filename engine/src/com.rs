//! Classic COM services, called in-process through the `windows` crate:
//!
//!   scheduled tasks   -> Task Scheduler 2.0 (`ITaskService`), what schtasks.exe
//!                        and the Task Scheduler console themselves use
//!   antivirus products-> Windows Security Center (`IWSCProductList`), the
//!                        official API behind the "Virus & threat protection"
//!                        provider list — no WMI, no PowerShell
//!
//! Each call initialises COM on its own thread (multithreaded apartment;
//! harmless if that thread already has one) and releases everything on drop.

use windows::core::{BSTR, HRESULT};
use windows::Win32::Foundation::{VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::SecurityCenter::{IWSCProductList, WSCProductList, WSC_SECURITY_PROVIDER_ANTIVIRUS};
use windows::Win32::System::TaskScheduler::{IRegisteredTask, ITaskFolder, ITaskService, TaskScheduler};
use windows::Win32::System::Variant::VARIANT;

/// HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND): no such task.
const E_FILE_NOT_FOUND: HRESULT = HRESULT(0x8007_0002_u32 as i32);
const E_PATH_NOT_FOUND: HRESULT = HRESULT(0x8007_0003_u32 as i32);

struct Apartment(bool);
impl Drop for Apartment {
    fn drop(&mut self) { if self.0 { unsafe { CoUninitialize() }; } }
}

fn init() -> Result<Apartment, String> {
    let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    if hr.is_ok() { Ok(Apartment(true)) }
    else if hr.0 == 0x80010106u32 as i32 { Ok(Apartment(false)) }
    else { Err(format!("CoInitializeEx: 0x{:08X}", hr.0 as u32)) }
}

fn err(what: &str, e: windows::core::Error) -> String {
    format!("{what}: {} (0x{:08X})", e.message().trim(), e.code().0 as u32)
}

// ----------------------------------------------------------- scheduled tasks --

fn root_folder() -> Result<ITaskFolder, String> {
    unsafe {
        let service: ITaskService =
            CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).map_err(|e| err("Task Scheduler", e))?;
        let none = VARIANT::default();
        service.Connect(&none, &none, &none, &none).map_err(|e| err("Task Scheduler connect", e))?;
        service.GetFolder(&BSTR::from("\\")).map_err(|e| err("Task Scheduler root", e))
    }
}

/// `\Microsoft\Windows\X\Task` — the form the Task Scheduler API wants.
fn task_path(path: &str) -> String {
    let p = path.replace('/', "\\");
    let p = p.trim_matches('\\');
    format!("\\{p}")
}

fn find(path: &str) -> Result<Option<(ITaskFolder, IRegisteredTask)>, String> {
    let root = root_folder()?;
    match unsafe { root.GetTask(&BSTR::from(task_path(path))) } {
        Ok(t) => Ok(Some((root, t))),
        Err(e) if e.code() == E_FILE_NOT_FOUND || e.code() == E_PATH_NOT_FOUND => Ok(None),
        Err(e) => Err(err(path, e)),
    }
}

/// `None` = no such task on this Windows, otherwise whether it is enabled.
pub fn task_enabled(path: &str) -> Result<Option<bool>, String> {
    let _apartment = init()?;
    let Some((_, task)) = find(path)? else { return Ok(None) };
    let on: VARIANT_BOOL = unsafe { task.Enabled() }.map_err(|e| err(path, e))?;
    Ok(Some(on.as_bool()))
}

pub fn task_set_enabled(path: &str, enabled: bool) -> Result<(), String> {
    let _apartment = init()?;
    let Some((_, task)) = find(path)? else { return Err(format!("{path}: task not found")) };
    unsafe { task.SetEnabled(if enabled { VARIANT_TRUE } else { VARIANT_FALSE }) }.map_err(|e| err(path, e))
}

pub fn task_delete(path: &str) -> Result<(), String> {
    let _apartment = init()?;
    let root = root_folder()?;
    unsafe { root.DeleteTask(&BSTR::from(task_path(path)), 0) }.map_err(|e| err(path, e))
}

// ------------------------------------------------------- antivirus products --

/// Display names of every antivirus product registered with Windows Security
/// Center (Microsoft Defender included — the caller filters).
pub fn antivirus_products() -> Result<Vec<String>, String> {
    let _apartment = init()?;
    unsafe {
        let list: IWSCProductList =
            CoCreateInstance(&WSCProductList, None, CLSCTX_INPROC_SERVER).map_err(|e| err("Security Center", e))?;
        list.Initialize(WSC_SECURITY_PROVIDER_ANTIVIRUS).map_err(|e| err("Security Center", e))?;
        let count = list.Count().map_err(|e| err("Security Center", e))?;
        let mut names = Vec::new();
        for i in 0..count.max(0) as u32 {
            names.push(list.get_Item(i).and_then(|p| p.ProductName()).map_err(|e| err("Security Center product", e))?.to_string());
        }
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_paths_are_rooted() {
        assert_eq!(task_path(r"Microsoft\Windows\Defrag\ScheduledDefrag"), r"\Microsoft\Windows\Defrag\ScheduledDefrag");
        assert_eq!(task_path(r"\Microsoft/Windows\X\"), r"\Microsoft\Windows\X");
    }

    // Read-only calls against this machine's real Task Scheduler / Security Center.

    #[test]
    fn missing_task_is_none_not_an_error() {
        assert_eq!(task_enabled(r"\Azizos\No Such Task 42").unwrap(), None);
    }

    #[test]
    fn a_real_task_reports_its_state() {
        // Present on every Windows 10/11 edition.
        let state = task_enabled(r"\Microsoft\Windows\Time Synchronization\SynchronizeTime").unwrap();
        assert!(state.is_some(), "expected the built-in time sync task to exist");
    }

    #[test]
    fn security_center_answers() {
        // Server editions have no Security Center; a client must answer.
        let registered = windows_registry::LOCAL_MACHINE.open(
            r"SOFTWARE\Classes\CLSID\{17072f7b-9abe-4a74-a261-1eb76b55107a}").is_ok();
        match antivirus_products() {
            Ok(products) => assert!(products.iter().all(|n| !n.trim().is_empty())),
            Err(e) if !registered => {
                // A stripped Windows must report unknown, never "no AV".
                assert!(e.contains("0x80040154"), "{e}");
                eprintln!("Security Center unavailable on this machine: {e}");
            }
            Err(e) => panic!("registered Security Center failed: {e}"),
        }
    }
}
