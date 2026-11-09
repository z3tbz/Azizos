//! Direct Win32 calls (windows-sys raw bindings), each wrapped in a small safe
//! function with RAII handle cleanup. This is the lowest layer Azizos uses —
//! the same entry points `sc.exe`, `taskkill.exe` and Windows' own SSD check
//! call, without starting those programs.
//!
//!   services  -> advapi32: OpenSCManagerW / OpenServiceW / ControlService / DeleteService
//!   processes -> kernel32: CreateToolhelp32Snapshot / Process32*W / OpenProcess / TerminateProcess
//!   disk type -> kernel32: CreateFileW + DeviceIoControl (seek-penalty query)
//!
//! Signatures are windows-sys 0.61.2 (checked against its source).

use std::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Closes a kernel handle on drop.
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(self.0) };
        }
    }
}

// ------------------------------------------------------------------ services --

use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, DeleteService, OpenSCManagerW, OpenServiceW, SC_HANDLE, SERVICE_STATUS,
};

const SC_MANAGER_CONNECT: u32 = 0x0001;
const SERVICE_STOP: u32 = 0x0020;
const DELETE: u32 = 0x0001_0000;
const SERVICE_CONTROL_STOP: u32 = 0x0000_0001;
const ERROR_SERVICE_DOES_NOT_EXIST: u32 = 1060;
const ERROR_SERVICE_NOT_ACTIVE: u32 = 1062;
const ERROR_SERVICE_MARKED_FOR_DELETE: u32 = 1072;

struct Sc(SC_HANDLE);
impl Drop for Sc {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CloseServiceHandle(self.0) };
        }
    }
}

/// Outcome of a service call, mapped from the Win32 error code.
#[derive(Debug, PartialEq, Eq)]
pub enum SvcResult {
    Done,
    /// Already in the wanted state (already stopped / already deleted).
    Already,
    /// No such service on this Windows.
    Missing,
}

fn open_service(name: &str, access: u32) -> Result<Sc, u32> {
    unsafe {
        let scm = Sc(OpenSCManagerW(std::ptr::null(), std::ptr::null(), SC_MANAGER_CONNECT));
        if scm.0.is_null() {
            return Err(GetLastError());
        }
        let n = wide(name);
        let svc = Sc(OpenServiceW(scm.0, n.as_ptr(), access));
        if svc.0.is_null() {
            return Err(GetLastError());
        }
        Ok(svc)
    }
}

fn win32_msg(code: u32) -> String {
    format!("{} (0x{:08X})", std::io::Error::from_raw_os_error(code as i32), 0x8007_0000 | (code & 0xFFFF))
}

pub fn service_stop(name: &str) -> Result<SvcResult, String> {
    let svc = match open_service(name, SERVICE_STOP) {
        Ok(s) => s,
        Err(ERROR_SERVICE_DOES_NOT_EXIST) => return Ok(SvcResult::Missing),
        Err(e) => return Err(format!("{name}: {}", win32_msg(e))),
    };
    let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
    if unsafe { ControlService(svc.0, SERVICE_CONTROL_STOP, &mut status) } != 0 {
        return Ok(SvcResult::Done);
    }
    match unsafe { GetLastError() } {
        ERROR_SERVICE_NOT_ACTIVE => Ok(SvcResult::Already),
        e => Err(format!("{name}: {}", win32_msg(e))),
    }
}

pub fn service_delete(name: &str) -> Result<SvcResult, String> {
    let svc = match open_service(name, DELETE) {
        Ok(s) => s,
        Err(ERROR_SERVICE_DOES_NOT_EXIST) => return Ok(SvcResult::Already),
        Err(e) => return Err(format!("{name}: {}", win32_msg(e))),
    };
    if unsafe { DeleteService(svc.0) } != 0 {
        return Ok(SvcResult::Done);
    }
    match unsafe { GetLastError() } {
        ERROR_SERVICE_MARKED_FOR_DELETE => Ok(SvcResult::Already),
        e => Err(format!("{name}: {}", win32_msg(e))),
    }
}

// ----------------------------------------------------------------- processes --

use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
};
use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess};

const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
const PROCESS_TERMINATE: u32 = 0x0001;

/// Case-insensitive glob with `*`; `.exe` is optional on the pattern.
/// Case-insensitive glob where `*` matches any run of characters.
pub fn glob(pattern: &str, text: &str) -> bool {
    fn g(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => g(&p[1..], s) || (!s.is_empty() && g(p, &s[1..])),
            (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => g(&p[1..], &s[1..]),
            _ => false,
        }
    }
    g(pattern.as_bytes(), text.as_bytes())
}

pub fn image_matches(pattern: &str, image: &str) -> bool {
    let img = image.to_ascii_lowercase();
    let pat = pattern.to_ascii_lowercase();
    let stem = img.strip_suffix(".exe").unwrap_or(&img);
    glob(&pat, &img) || glob(&pat, stem)
}

/// Terminate every process whose image name matches `pattern`.
/// Returns (matched, terminated, first error).
pub fn kill_processes(pattern: &str) -> (usize, usize, Option<String>) {
    let me = std::process::id();
    let (mut matched, mut killed, mut err) = (0, 0, None);
    unsafe {
        let snap = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0));
        if snap.0 == INVALID_HANDLE_VALUE {
            return (0, 0, Some(format!("process snapshot: {}", win32_msg(GetLastError()))));
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap.0, &mut e) != 0;
        while ok {
            let len = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let image = String::from_utf16_lossy(&e.szExeFile[..len]);
            if e.th32ProcessID != me && e.th32ProcessID != 0 && image_matches(pattern, &image) {
                matched += 1;
                let h = Handle(OpenProcess(PROCESS_TERMINATE, 0, e.th32ProcessID));
                if !h.0.is_null() && TerminateProcess(h.0, 1) != 0 {
                    killed += 1;
                } else if err.is_none() {
                    err = Some(format!("{image}: {}", win32_msg(GetLastError())));
                }
            }
            ok = Process32NextW(snap.0, &mut e) != 0;
        }
    }
    (matched, killed, err)
}

// ---------------------------------------------------------------------- disk --

use windows_sys::Win32::Storage::FileSystem::CreateFileW;
use windows_sys::Win32::System::IO::DeviceIoControl;

const FILE_SHARE_READ_WRITE: u32 = 0x1 | 0x2;
const OPEN_EXISTING: u32 = 3;
const IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS: u32 = 0x0056_0000;
const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const STORAGE_DEVICE_SEEK_PENALTY_PROPERTY: u32 = 7;
const PROPERTY_STANDARD_QUERY: u32 = 0;

#[repr(C)]
struct DiskExtent {
    disk_number: u32,
    starting_offset: i64,
    extent_length: i64,
}
#[repr(C)]
struct VolumeDiskExtents {
    count: u32,
    extents: [DiskExtent; 1],
}
#[repr(C)]
struct StoragePropertyQuery {
    property_id: u32,
    query_type: u32,
    additional: [u8; 1],
}
#[repr(C)]
struct SeekPenaltyDescriptor {
    version: u32,
    size: u32,
    incurs_seek_penalty: u8,
}

fn open_device(path: &str) -> Option<Handle> {
    let p = wide(path);
    // access 0 = query-only: no read/write rights needed on the device
    let h = unsafe {
        CreateFileW(p.as_ptr(), 0, FILE_SHARE_READ_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut())
    };
    (h != INVALID_HANDLE_VALUE).then_some(Handle(h))
}

fn ioctl<I, O>(h: &Handle, code: u32, input: Option<&I>, out: &mut O) -> bool {
    let mut ret = 0u32;
    let (ip, il) = match input {
        Some(i) => (i as *const I as *const c_void, std::mem::size_of::<I>() as u32),
        None => (std::ptr::null(), 0),
    };
    unsafe {
        DeviceIoControl(
            h.0,
            code,
            ip,
            il,
            out as *mut O as *mut c_void,
            std::mem::size_of::<O>() as u32,
            &mut ret,
            std::ptr::null_mut(),
        ) != 0
    }
}

/// `Some(true)` = the Windows drive is an SSD (no seek penalty), `Some(false)` =
/// rotating disk, `None` = unknown. The same query Windows' optimizer uses.
pub fn system_disk_is_ssd() -> Option<bool> {
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    let vol = open_device(&format!(r"\\.\{}", drive.trim_end_matches('\\')))?;
    let mut ext: VolumeDiskExtents = unsafe { std::mem::zeroed() };
    if !ioctl::<(), _>(&vol, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None, &mut ext) || ext.count == 0 {
        return None;
    }
    let disk = open_device(&format!(r"\\.\PhysicalDrive{}", ext.extents[0].disk_number))?;
    let q = StoragePropertyQuery {
        property_id: STORAGE_DEVICE_SEEK_PENALTY_PROPERTY,
        query_type: PROPERTY_STANDARD_QUERY,
        additional: [0],
    };
    let mut d: SeekPenaltyDescriptor = unsafe { std::mem::zeroed() };
    if !ioctl(&disk, IOCTL_STORAGE_QUERY_PROPERTY, Some(&q), &mut d) || d.size == 0 {
        return None;
    }
    let _ = (d.version, ext.extents[0].starting_offset, ext.extents[0].extent_length);
    Some(d.incurs_seek_penalty == 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_glob() {
        assert!(image_matches("msedge", "msedge.exe"));
        assert!(image_matches("OneDrive*", "OneDrive.Sync.Service.exe"));
        assert!(image_matches("*teams*", "ms-teams.exe"));
        assert!(image_matches("MicrosoftEdge*", "MicrosoftEdgeUpdate.exe"));
        assert!(!image_matches("setup", "setupact.exe"));
        assert!(!image_matches("msedge", "msedgewebview2.exe"));
    }

    #[test]
    fn struct_layouts_match_win32() {
        assert_eq!(std::mem::size_of::<DiskExtent>(), 24);
        assert_eq!(std::mem::size_of::<VolumeDiskExtents>(), 32);
        assert_eq!(std::mem::size_of::<StoragePropertyQuery>(), 12);
        assert_eq!(std::mem::size_of::<SeekPenaltyDescriptor>(), 12);
    }

    #[test]
    fn missing_service_is_reported_as_missing() {
        assert_eq!(service_stop("Azizos_No_Such_Service_42").unwrap(), SvcResult::Missing);
        assert_eq!(service_delete("Azizos_No_Such_Service_42").unwrap(), SvcResult::Already);
    }

    #[test]
    fn kill_with_no_match_touches_nothing() {
        assert_eq!(kill_processes("azizos_no_such_process_*").0, 0);
    }

    #[test]
    fn ssd_query_answers_on_this_machine() {
        // just must not crash; value depends on the hardware
        let _ = system_disk_is_ssd();
    }
}
