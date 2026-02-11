//! Shared subprocess management utilities.

/// RAII guard that kills a child process (and its process group on Unix) on drop.
///
/// Wrap a spawned `tokio::process::Child` immediately after `spawn()` to ensure
/// cleanup if the owning future is cancelled. Call `disarm()` after the process
/// exits normally to prevent the kill.
pub struct ChildGuard {
    child: Option<tokio::process::Child>,
}

impl ChildGuard {
    #[must_use]
    pub fn new(child: tokio::process::Child) -> Self {
        Self { child: Some(child) }
    }

    pub fn child_mut(&mut self) -> &mut tokio::process::Child {
        self.child.as_mut().expect("child present")
    }

    pub fn disarm(&mut self) {
        self.child = None;
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let Some(child) = self.child.as_mut() else {
            return;
        };
        #[cfg(unix)]
        {
            if let Some(pid) = child.id() {
                unsafe {
                    if libc::killpg(pid as i32, libc::SIGKILL) == -1 {
                        let _ = child.start_kill();
                    }
                }
            }
            let _ = child.try_wait();
        }
        #[cfg(windows)]
        {
            let _ = child.start_kill();
            let _ = child.try_wait();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    NotRunning,
    Killed,
}

/// Failure modes for [`process_started_at_unix_ms`].
#[derive(Debug, thiserror::Error)]
pub enum ProcessStartTimeError {
    #[error("unsupported platform")]
    UnsupportedPlatform,
    #[error("OS error: {0}")]
    Os(#[from] std::io::Error),
    #[error("proc_pidinfo returned {got} bytes, expected {expected}")]
    PartialRead { expected: i32, got: i32 },
    #[error("sysconf(_SC_CLK_TCK) returned non-positive value")]
    InvalidClockTick,
    #[error("failed to parse {what}: {detail}")]
    ParseFailed { what: &'static str, detail: String },
    #[error("timestamp overflows i64")]
    TimestampOverflow,
}

/// Lookup a process start timestamp (Unix epoch milliseconds).
///
/// Used to reduce PID reuse risk when attempting to terminate orphaned subprocess tools
/// after a crash.
pub fn process_started_at_unix_ms(pid: u32) -> Result<i64, ProcessStartTimeError> {
    #[cfg(windows)]
    {
        windows_process_started_at_unix_ms(pid)
    }
    #[cfg(target_os = "linux")]
    {
        linux_process_started_at_unix_ms(pid)
    }
    #[cfg(target_os = "macos")]
    {
        macos_process_started_at_unix_ms(pid)
    }
    #[cfg(all(unix, not(target_os = "linux"), not(target_os = "macos")))]
    {
        let _ = pid;
        Err(ProcessStartTimeError::UnsupportedPlatform)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        Err(ProcessStartTimeError::UnsupportedPlatform)
    }
}

/// Terminate a process (and its process group on Unix) best-effort.
///
/// On Unix this targets the process group id matching `pid` (Forge creates a new session
/// for `Run`, making pid == process group id).
pub fn try_kill_process_group(pid: u32) -> std::io::Result<KillOutcome> {
    #[cfg(unix)]
    unsafe {
        if libc::killpg(pid as i32, libc::SIGKILL) == -1 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::ESRCH) {
                return Ok(KillOutcome::NotRunning);
            }
            return Err(err);
        }
        Ok(KillOutcome::Killed)
    }

    #[cfg(windows)]
    {
        windows_try_kill_process(pid)
    }
}

#[cfg(target_os = "linux")]
fn linux_process_started_at_unix_ms(pid: u32) -> Result<i64, ProcessStartTimeError> {
    use std::fs;

    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let start_ticks = parse_linux_proc_stat_starttime_ticks(&stat)?;

    let btime_secs = linux_boot_time_epoch_secs()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return Err(ProcessStartTimeError::InvalidClockTick);
    }
    let hz = hz as u64;

    let start_ms = (start_ticks.saturating_mul(1000)).saturating_div(hz);
    let unix_ms = btime_secs.saturating_mul(1000).saturating_add(start_ms);
    i64::try_from(unix_ms).map_err(|_| ProcessStartTimeError::TimestampOverflow)
}

#[cfg(target_os = "linux")]
fn linux_boot_time_epoch_secs() -> Result<u64, ProcessStartTimeError> {
    use std::fs;
    let stat = fs::read_to_string("/proc/stat")?;
    for line in stat.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("btime ") {
            return rest
                .trim()
                .parse::<u64>()
                .map_err(|e| ProcessStartTimeError::ParseFailed {
                    what: "/proc/stat btime",
                    detail: e.to_string(),
                });
        }
    }
    Err(ProcessStartTimeError::ParseFailed {
        what: "/proc/stat",
        detail: "no btime line found".to_string(),
    })
}

#[cfg(target_os = "linux")]
fn parse_linux_proc_stat_starttime_ticks(proc_stat: &str) -> Result<u64, ProcessStartTimeError> {
    // /proc/<pid>/stat format:
    // pid (comm) state ppid ... starttime ...
    // The comm field may contain spaces; find the last ')' to locate the end.
    let close_paren = proc_stat
        .rfind(')')
        .ok_or_else(|| ProcessStartTimeError::ParseFailed {
            what: "/proc/<pid>/stat",
            detail: "no closing paren in comm field".to_string(),
        })?;
    let after = proc_stat
        .get(close_paren + 1..)
        .ok_or_else(|| ProcessStartTimeError::ParseFailed {
            what: "/proc/<pid>/stat",
            detail: "unexpected end after comm field".to_string(),
        })?
        .trim();
    let fields: Vec<&str> = after.split_whitespace().collect();
    // starttime is field #22 overall => index 19 in the remainder (0-based).
    let raw = fields
        .get(19)
        .ok_or_else(|| ProcessStartTimeError::ParseFailed {
            what: "/proc/<pid>/stat starttime",
            detail: format!("only {n} fields after comm, need 20", n = fields.len()),
        })?;
    raw.parse::<u64>()
        .map_err(|e| ProcessStartTimeError::ParseFailed {
            what: "/proc/<pid>/stat starttime",
            detail: e.to_string(),
        })
}

#[cfg(target_os = "macos")]
fn macos_process_started_at_unix_ms(pid: u32) -> Result<i64, ProcessStartTimeError> {
    use std::mem::{MaybeUninit, size_of};

    unsafe extern "C" {
        unsafe fn proc_pidinfo(
            pid: libc::c_int,
            flavor: libc::c_int,
            arg: u64,
            buffer: *mut libc::c_void,
            buffersize: libc::c_int,
        ) -> libc::c_int;
    }

    const PROC_PIDTBSDINFO: libc::c_int = 3;

    #[repr(C)]
    struct ProcBsdInfo {
        pbi_flags: u32,
        pbi_status: u32,
        pbi_xstatus: u32,
        pbi_pid: u32,
        pbi_ppid: u32,
        pbi_uid: u32,
        pbi_gid: u32,
        pbi_ruid: u32,
        pbi_rgid: u32,
        pbi_svuid: u32,
        pbi_svgid: u32,
        _rfu_1: u32,
        pbi_comm: [u8; 16],
        pbi_name: [u8; 32],
        pbi_nfiles: u32,
        pbi_pgid: u32,
        pbi_pjobc: u32,
        e_tdev: u32,
        e_tpgid: u32,
        pbi_nice: i32,
        pbi_start_tvsec: u64,
        pbi_start_tvusec: u64,
    }

    let mut info = MaybeUninit::<ProcBsdInfo>::uninit();
    let expected_size = size_of::<ProcBsdInfo>() as libc::c_int;
    let ret = unsafe {
        proc_pidinfo(
            pid as libc::c_int,
            PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            expected_size,
        )
    };
    if ret != expected_size {
        return Err(ProcessStartTimeError::PartialRead {
            expected: expected_size,
            got: ret,
        });
    }

    let info = unsafe { info.assume_init() };
    let secs = info.pbi_start_tvsec as i64;
    let usecs = info.pbi_start_tvusec as i64;
    Ok(secs.saturating_mul(1000).saturating_add(usecs / 1000))
}

#[cfg(windows)]
fn windows_process_started_at_unix_ms(pid: u32) -> Result<i64, ProcessStartTimeError> {
    use std::mem::MaybeUninit;
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    // SAFETY: Win32 API call.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) } as HANDLE;
    if handle.is_null() {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut creation = MaybeUninit::<FILETIME>::uninit();
    let mut exit = MaybeUninit::<FILETIME>::uninit();
    let mut kernel = MaybeUninit::<FILETIME>::uninit();
    let mut user = MaybeUninit::<FILETIME>::uninit();
    // SAFETY: `handle` is valid; FILETIME pointers are valid.
    let ok = unsafe {
        GetProcessTimes(
            handle,
            creation.as_mut_ptr(),
            exit.as_mut_ptr(),
            kernel.as_mut_ptr(),
            user.as_mut_ptr(),
        )
    };
    let err = std::io::Error::last_os_error();
    // SAFETY: always close handle.
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        return Err(err.into());
    }
    // SAFETY: ok == 1 implies out params initialized.
    let creation = unsafe { creation.assume_init() };
    filetime_to_unix_ms(creation)
}

#[cfg(windows)]
fn windows_try_kill_process(pid: u32) -> std::io::Result<KillOutcome> {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    };

    // SAFETY: Win32 API call.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            0,
            pid,
        )
    } as HANDLE;
    if handle.is_null() {
        // Either the process exited or we lack permission. Treat as not running to avoid
        // false positives on recovery cleanup.
        return Ok(KillOutcome::NotRunning);
    }
    // SAFETY: handle is valid.
    let ok = unsafe { TerminateProcess(handle, 1) };
    let err = std::io::Error::last_os_error();
    unsafe {
        CloseHandle(handle);
    }
    if ok == 0 {
        return Err(err);
    }
    Ok(KillOutcome::Killed)
}

#[cfg(windows)]
fn filetime_to_unix_ms(
    filetime: windows_sys::Win32::Foundation::FILETIME,
) -> Result<i64, ProcessStartTimeError> {
    // FILETIME is 100-nanosecond intervals since 1601-01-01.
    const UNIX_EPOCH_AS_FILETIME_100NS: u64 = 116_444_736_000_000_000;
    let high = u64::from(filetime.dwHighDateTime);
    let low = u64::from(filetime.dwLowDateTime);
    let ft = (high << 32) | low;
    let unix_100ns = ft
        .checked_sub(UNIX_EPOCH_AS_FILETIME_100NS)
        .ok_or(ProcessStartTimeError::TimestampOverflow)?;
    let unix_ms = unix_100ns / 10_000;
    i64::try_from(unix_ms).map_err(|_| ProcessStartTimeError::TimestampOverflow)
}

/// Put the child process in its own session (Unix only) so the entire process
/// group can be killed via `killpg` in `ChildGuard::drop`.
#[cfg(unix)]
pub fn set_new_session(cmd: &mut tokio::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // Linux-only: ensure the child dies if Forge dies (kill -9 / crash / power loss).
            // This reduces the "orphaned runaway process" failure mode for long-lived tools.
            #[cfg(target_os = "linux")]
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_process_start_time_returns_ok() {
        let pid = std::process::id();
        let ms = process_started_at_unix_ms(pid).expect("should return start time for own process");
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        let diff = (now_ms - ms).abs();
        assert!(diff < 60_000, "start time should be within 60s of now");
    }
}
