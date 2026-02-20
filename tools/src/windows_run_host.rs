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
use std::mem;
#[cfg(windows)]
use std::ptr;

#[cfg(windows)]
use tokio::process::Child;
#[cfg(windows)]
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
    JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES, JOB_OBJECT_UILIMIT_READCLIPBOARD,
    JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS, JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
    JOBOBJECT_BASIC_UI_RESTRICTIONS, JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECTINFOCLASS,
    JobObjectBasicUIRestrictions, JobObjectExtendedLimitInformation, SetInformationJobObject,
};

/// Verify that Windows host isolation primitives are available.
///
/// On Windows this probes that Forge can create/configure a Job Object and assign a newly-spawned
/// child process to it. On non-Windows hosts this is a no-op success.
///
/// The result is cached per-process to avoid repeating the probe.
pub(crate) fn sandbox_preflight() -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::sync::OnceLock;

        static PREFLIGHT: OnceLock<Result<(), String>> = OnceLock::new();
        PREFLIGHT.get_or_init(sandbox_preflight_inner).clone()
    }

    #[cfg(not(windows))]
    Ok(())
}

#[cfg(windows)]
fn sandbox_preflight_inner() -> Result<(), String> {
    let job = create_sandbox_job()?;
    probe_assign_process_to_job(job.handle)
}

#[cfg(windows)]
fn probe_assign_process_to_job(job_handle: HANDLE) -> Result<(), String> {
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut child = Command::new("cmd")
        .arg("/C")
        .arg("exit 0")
        .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("preflight failed to spawn child process: {e}"))?;

    let process_handle = child.as_raw_handle() as HANDLE;
    if process_handle.is_null() {
        let _ = child.kill();
        let _ = child.wait();
        return Err("preflight child process handle was null".to_string());
    }

    // SAFETY: `job_handle` and `process_handle` are valid handles.
    let attached = unsafe { AssignProcessToJobObject(job_handle, process_handle) };

    // Ensure the probe child is terminated regardless of attach outcome.
    let _ = child.kill();
    let _ = child.wait();

    if attached == 0 {
        return Err(last_os_error("AssignProcessToJobObject"));
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

/// Attach a child process to a kill-on-close Job Object without UI restrictions.
///
/// This is best-effort host cleanup: if Forge crashes, the job handle is closed and the child
/// is terminated. Unlike [`attach_process_to_sandbox`], this does not impose UI limits and is
/// suitable for the common `Run` tool case where host sandboxing is not required by policy.
#[cfg(windows)]
pub(crate) fn attach_process_to_kill_on_close(
    child: &Child,
) -> Result<WindowsHostSandboxGuard, String> {
    let process_handle = child
        .raw_handle()
        .ok_or_else(|| "child process handle unavailable".to_string())?
        as HANDLE;
    if process_handle.is_null() {
        return Err("child process handle was null".to_string());
    }

    let job = create_kill_on_close_job()?;
    // SAFETY: `job.handle` and `process_handle` are valid handles.
    let attached = unsafe { AssignProcessToJobObject(job.handle, process_handle) };
    if attached == 0 {
        return Err(last_os_error("AssignProcessToJobObject"));
    }

    Ok(WindowsHostSandboxGuard::new(job))
}

#[cfg(windows)]
const fn job_limit_flags() -> u32 {
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
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
fn create_kill_on_close_job() -> Result<WinHandle, String> {
    // SAFETY: Passing null security attributes/name requests an unnamed job
    // with default security descriptor.
    let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
    if handle.is_null() {
        return Err(last_os_error("CreateJobObjectW"));
    }

    let job = WinHandle { handle };
    configure_job_limits(job.handle)?;
    Ok(job)
}

#[cfg(windows)]
fn configure_job_limits(job_handle: HANDLE) -> Result<(), String> {
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = job_limit_flags();
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
    let info_len = u32::try_from(mem::size_of::<T>())
        .map_err(|_| "job information payload exceeded u32 size".to_string())?;
    // SAFETY: `value` points to a valid initialized payload of `info_len` bytes.
    let ok = unsafe {
        SetInformationJobObject(
            job_handle,
            info_class,
            ptr::from_ref(value).cast::<c_void>(),
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
    use std::process::Stdio;

    #[cfg(windows)]
    use tokio::process::Command;

    #[cfg(windows)]
    use super::{
        JOB_OBJECT_UILIMIT_DESKTOP, JOB_OBJECT_UILIMIT_READCLIPBOARD,
        JOB_OBJECT_UILIMIT_WRITECLIPBOARD, attach_process_to_sandbox, job_ui_limit_flags,
        sandbox_preflight,
    };

    #[cfg(windows)]
    #[test]
    fn sandbox_preflight_reports_availability_or_reason() {
        match sandbox_preflight() {
            Ok(()) => {}
            Err(reason) => assert!(!reason.trim().is_empty()),
        }
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn attach_process_to_sandbox_succeeds_for_running_child() {
        if let Err(reason) = sandbox_preflight() {
            eprintln!("Windows host sandbox preflight unavailable: {reason}");
            return;
        }

        let mut child = Command::new("cmd")
            .arg("/C")
            .arg("ping -n 3 127.0.0.1 >NUL")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
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
