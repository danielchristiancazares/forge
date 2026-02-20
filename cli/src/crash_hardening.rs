#[cfg(windows)]
use anyhow::bail;
use anyhow::{Context, Result};
use std::env;

const FORGE_ALLOW_COREDUMPS: &str = "FORGE_ALLOW_COREDUMPS";

pub fn apply() -> Result<()> {
    if coredumps_allowed_by_override() {
        tracing::warn!(
            env_var = FORGE_ALLOW_COREDUMPS,
            "Crash dump hardening disabled by environment override"
        );
        return Ok(());
    }

    apply_platform_hardening().context("failed to apply crash dump hardening")?;
    tracing::info!("Crash dump hardening enabled");
    Ok(())
}

fn coredumps_allowed_by_override() -> bool {
    match env::var(FORGE_ALLOW_COREDUMPS) {
        Ok(raw) => is_truthy(raw.as_str()),
        Err(_) => false,
    }
}

fn is_truthy(raw: &str) -> bool {
    matches!(
        raw.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes"
    )
}

#[cfg(unix)]
fn apply_platform_hardening() -> Result<()> {
    set_rlimit_core_zero().context("setrlimit(RLIMIT_CORE=0) failed")?;

    #[cfg(target_os = "linux")]
    {
        set_linux_dumpable_zero().context("prctl(PR_SET_DUMPABLE=0) failed")?;
    }

    Ok(())
}

#[cfg(unix)]
fn set_rlimit_core_zero() -> io::Result<()> {
    let limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_CORE, &raw const limit) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_os = "linux")]
fn set_linux_dumpable_zero() -> io::Result<()> {
    let rc = unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
fn apply_platform_hardening() -> Result<()> {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };
    use windows_sys::Win32::System::ErrorReporting::{WER_FAULT_REPORTING_NO_UI, WerSetFlags};

    unsafe {
        let _ = SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX);
        let hr = WerSetFlags(WER_FAULT_REPORTING_NO_UI);
        if hr < 0 {
            bail!("WerSetFlags failed with HRESULT 0x{:08X}", hr as u32);
        }
    }

    Ok(())
}

#[cfg(not(any(unix, windows)))]
fn apply_platform_hardening() -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::is_truthy;

    #[test]
    fn truthy_override_values_are_accepted() {
        assert!(is_truthy("1"));
        assert!(is_truthy("true"));
        assert!(is_truthy("TRUE"));
        assert!(is_truthy("yes"));
        assert!(is_truthy(" YeS "));
    }

    #[test]
    fn non_truthy_override_values_are_rejected() {
        assert!(!is_truthy(""));
        assert!(!is_truthy("0"));
        assert!(!is_truthy("false"));
        assert!(!is_truthy("no"));
        assert!(!is_truthy("on"));
    }
}
