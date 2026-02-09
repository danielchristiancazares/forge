//! Shared subprocess management utilities.

/// RAII guard that kills a child process (and its process group on Unix) on drop.
///
/// Wrap a spawned `tokio::process::Child` immediately after `spawn()` to ensure
/// cleanup if the owning future is cancelled. Call `disarm()` after the process
/// exits normally to prevent the kill.
pub(crate) struct ChildGuard {
    child: Option<tokio::process::Child>,
}

impl ChildGuard {
    pub(crate) fn new(child: tokio::process::Child) -> Self {
        Self { child: Some(child) }
    }

    pub(crate) fn child_mut(&mut self) -> &mut tokio::process::Child {
        self.child.as_mut().expect("child present")
    }

    pub(crate) fn disarm(&mut self) {
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

/// Put the child process in its own session (Unix only) so the entire process
/// group can be killed via `killpg` in `ChildGuard::drop`.
#[cfg(unix)]
pub(crate) fn set_new_session(cmd: &mut tokio::process::Command) {
    unsafe {
        cmd.as_std_mut().pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            // Linux-only: ensure the child dies if Forge dies (kill -9 / crash / power loss).
            // This reduces the "orphaned runaway process" failure mode for long-lived tools.
            #[cfg(target_os = "linux")]
            unsafe {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            Ok(())
        });
    }
}
