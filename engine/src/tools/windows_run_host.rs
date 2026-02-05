//! Windows host isolation mechanism for the `Run` tool.
//!
//! This module is mechanism-only:
//! - It reports whether host isolation primitives are available.
//! - It attaches child processes to a constrained Job Object boundary.
//! - It does not decide policy; callers choose fallback behavior.

#[cfg(windows)]
use std::ffi::c_void;
#[cfg(windows)]
use std::io;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use tokio::process::Child;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_UILIMIT_DESKTOP,
    JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
};

/// Verify that Windows host isolation primitives are available.
///
/// On non-Windows hosts this is a no-op success.
pub(crate) fn sandbox_preflight() -> Result<(), String> {
    #[cfg(windows)]
    {
        let _ = create_sandbox_job()?;
    }
    Ok(())
}

#[cfg(windows)]
#[derive(Debug)]
pub(crate) struct WindowsHostSandboxGuard {
    _job: WinHandle,
}

#[cfg(windows)]
impl WindowsHostSandboxGuard {
    fn new(job: WinHandle) -> Self {
        Self { _job: job }
    }
}

#[cfg(windows)]
pub(crate) fn attach_process_to_sandbox(child: &Child) -> Result<WindowsHostSandboxGuard, String> {
    let process_handle = child
        .raw_handle()
        .ok_or_else(|| "child process handle unavailable".to_string())?
        as HANDLE;
    if process_handle.is_null() {
        return Err("child process handle was null".to_string());
    }

    let job = create_sandbox_job()?;
    // SAFETY: `job.handle` and `process_handle` are valid handles.
    let attached = unsafe { AssignProcessToJobObject(job.handle, process_handle) };
    if attached == 0 {
        return Err(last_os_error("AssignProcessToJobObject"));
    }

    Ok(WindowsHostSandboxGuard::new(job))
}

#[cfg(windows)]
const fn job_limit_flags() -> u32 {
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
}

#[cfg(windows)]
const fn job_ui_limit_flags() -> u32 {
    JOB_OBJECT_UILIMIT_HANDLES
        | JOB_OBJECT_UILIMIT_READCLIPBOARD
        | JOB_OBJECT_UILIMIT_WRITECLIPBOARD
        | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
        | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
        | JOB_OBJECT_UILIMIT_GLOBALATOMS
        | JOB_OBJECT_UILIMIT_DESKTOP
        | JOB_OBJECT_UILIMIT_EXITWINDOWS
}

#[cfg(windows)]
#[derive(Debug)]
struct WinHandle {
    handle: HANDLE,
}

// SAFETY: `HANDLE` is a thin kernel object identifier (similar to a file
// descriptor).  It carries no thread-affine state, so transferring
// ownership to another thread is safe.  The handle is closed on drop
// through `CloseHandle`, which is thread-safe.
#[cfg(windows)]
unsafe impl Send for WinHandle {}

#[cfg(windows)]
impl Drop for WinHandle {
    fn drop(&mut self) {
        if self.handle.is_null() {
            return;
        }
        // SAFETY: Handle was returned by Win32 APIs in this module.
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

#[cfg(windows)]
fn create_sandbox_job() -> Result<WinHandle, String> {
    // SAFETY: Passing null security attributes/name requests an unnamed job
    // with default security descriptor.
    let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if handle.is_null() {
        return Err(last_os_error("CreateJobObjectW"));
    }

    let job = WinHandle { handle };
    configure_job_limits(job.handle)?;
    configure_job_ui_limits(job.handle)?;
    Ok(job)
}

#[cfg(windows)]
fn configure_job_limits(job_handle: HANDLE) -> Result<(), String> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = job_limit_flags();
    limits.BasicLimitInformation.ActiveProcessLimit = 1;
    set_job_information(job_handle, JobObjectExtendedLimitInformation, &limits)
}

#[cfg(windows)]
fn configure_job_ui_limits(job_handle: HANDLE) -> Result<(), String> {
    let ui_restrictions = JOBOBJECT_BASIC_UI_RESTRICTIONS {
        UIRestrictionsClass: job_ui_limit_flags(),
    };
    set_job_information(job_handle, JobObjectBasicUIRestrictions, &ui_restrictions)
}

#[cfg(windows)]
fn set_job_information<T>(
    job_handle: HANDLE,
    info_class: JOBOBJECTINFOCLASS,
    value: &T,
) -> Result<(), String> {
    let info_len = u32::try_from(std::mem::size_of::<T>())
        .map_err(|_| "job information payload exceeded u32 size".to_string())?;
    // SAFETY: `value` points to a valid initialized payload of `info_len` bytes.
    let ok = unsafe {
        SetInformationJobObject(
            job_handle,
            info_class,
            std::ptr::from_ref(value).cast::<c_void>(),
            info_len,
        )
    };
    if ok == 0 {
        return Err(last_os_error("SetInformationJobObject"));
    }
    Ok(())
}

#[cfg(windows)]
fn last_os_error(operation: &str) -> String {
    format!("{operation} failed: {}", io::Error::last_os_error())
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn sandbox_preflight_succeeds() {
        sandbox_preflight().expect("preflight should create and configure a job");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn attach_process_to_sandbox_succeeds_for_running_child() {
        let mut child = tokio::process::Command::new("cmd")
            .arg("/C")
            .arg("ping -n 3 127.0.0.1 >NUL")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child");

        let _guard = attach_process_to_sandbox(&child).expect("attach child to job");
        let _status = child.wait().await.expect("wait child");
    }

    #[cfg(windows)]
    #[test]
    fn ui_restrictions_include_desktop_and_clipboard() {
        let flags = job_ui_limit_flags();
        assert_ne!(flags & JOB_OBJECT_UILIMIT_DESKTOP, 0);
        assert_ne!(flags & JOB_OBJECT_UILIMIT_READCLIPBOARD, 0);
        assert_ne!(flags & JOB_OBJECT_UILIMIT_WRITECLIPBOARD, 0);
    }
}
