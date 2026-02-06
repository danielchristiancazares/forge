//! Windows-focused sandbox policy for the `Run` tool.
//!
//! This module is policy-first hardening (IFA mechanism/policy split):
//! - Mechanism: classify shell and command content
//! - Policy: decide allow/deny/fallback behavior

use std::path::Path;

use super::{DenialReason, DetectedShell, ToolError};

/// Command text views for Windows `Run` sandbox evaluation.
///
/// - `raw`: what will be executed (after wrapping).
/// - `policy_text`: normalized view used for token/pattern checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RunCommandText<'a> {
    raw: &'a str,
    policy_text: &'a str,
}

impl<'a> RunCommandText<'a> {
    #[must_use]
    pub(crate) fn new(raw: &'a str, policy_text: &'a str) -> Self {
        Self { raw, policy_text }
    }

    #[must_use]
    pub(crate) fn raw(&self) -> &'a str {
        self.raw
    }

    #[must_use]
    pub(crate) fn policy_text(&self) -> &'a str {
        self.policy_text
    }
}

/// Behavior when Windows sandbox prerequisites are unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSandboxFallbackMode {
    /// Require explicit opt-in per call before allowing unsandboxed execution.
    Prompt,
    /// Never allow unsandboxed execution.
    Deny,
    /// Automatically allow unsandboxed execution with a warning.
    AllowWithWarning,
}

/// Windows-specific run sandbox policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowsRunSandboxPolicy {
    pub enabled: bool,
    pub enforce_powershell_only: bool,
    pub block_network: bool,
    pub fallback_mode: RunSandboxFallbackMode,
}

impl Default for WindowsRunSandboxPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            enforce_powershell_only: true,
            block_network: true,
            fallback_mode: RunSandboxFallbackMode::Prompt,
        }
    }
}

/// Aggregate run sandbox policy (platform-specific sub-policies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSandboxPolicy {
    pub windows: WindowsRunSandboxPolicy,
}

/// Prepared command after sandbox policy evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRunCommand {
    command: String,
    warning: Option<String>,
    requires_windows_host_sandbox: bool,
}

impl PreparedRunCommand {
    fn new(command: String, warning: Option<String>, requires_windows_host_sandbox: bool) -> Self {
        Self {
            command,
            warning,
            requires_windows_host_sandbox,
        }
    }

    #[must_use]
    pub fn command(&self) -> &str {
        &self.command
    }

    #[must_use]
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    #[must_use]
    pub fn requires_windows_host_sandbox(&self) -> bool {
        self.requires_windows_host_sandbox
    }
}

const NETWORK_BLOCKLIST: &[&str] = &[
    "invoke-webrequest",
    "invoke-restmethod",
    "start-bitstransfer",
    "curl.exe",
    "wget.exe",
    "bitsadmin",
    "net.webclient",
    "http://",
    "https://",
];

const PROCESS_ESCAPE_BLOCKLIST: &[&str] = &[
    "start-process",
    "powershell.exe",
    "pwsh.exe",
    "cmd /c",
    "cmd.exe",
    "wsl.exe",
    "rundll32",
    "mshta",
    "regsvr32",
    "cscript",
    "wscript",
];

/// Prepare a command for execution under run sandbox policy.
///
/// On non-Windows hosts, this is a no-op passthrough.
pub(crate) fn prepare_run_command(
    command: RunCommandText<'_>,
    shell: &DetectedShell,
    policy: RunSandboxPolicy,
    unsafe_allow_unsandboxed: bool,
) -> Result<PreparedRunCommand, ToolError> {
    if cfg!(windows) {
        return prepare_windows_run_command(
            command,
            shell,
            policy.windows,
            unsafe_allow_unsandboxed,
        );
    }
    let _ = (shell, policy, unsafe_allow_unsandboxed);
    Ok(PreparedRunCommand::new(
        command.raw().to_string(),
        None,
        false,
    ))
}

pub(crate) fn prepare_windows_run_command(
    command: RunCommandText<'_>,
    shell: &DetectedShell,
    policy: WindowsRunSandboxPolicy,
    unsafe_allow_unsandboxed: bool,
) -> Result<PreparedRunCommand, ToolError> {
    prepare_windows_run_command_with_host_probe(
        command,
        shell,
        policy,
        unsafe_allow_unsandboxed,
        cfg!(windows),
        default_windows_host_probe,
    )
}

fn prepare_windows_run_command_with_host_probe<F>(
    command: RunCommandText<'_>,
    shell: &DetectedShell,
    policy: WindowsRunSandboxPolicy,
    unsafe_allow_unsandboxed: bool,
    check_windows_host: bool,
    host_probe: F,
) -> Result<PreparedRunCommand, ToolError>
where
    F: FnOnce() -> Result<(), String>,
{
    if !policy.enabled {
        return Ok(PreparedRunCommand::new(
            command.raw().to_string(),
            None,
            false,
        ));
    }

    let shell_is_powershell = is_powershell_shell(shell);
    if policy.enforce_powershell_only && !shell_is_powershell {
        return handle_unsandboxed_fallback(
            command.raw().to_string(),
            policy.fallback_mode,
            unsafe_allow_unsandboxed,
            format!(
                "configured shell '{}' is not PowerShell",
                shell.binary.display()
            ),
        );
    }

    if let Some(token) = blocked_token(command.policy_text(), PROCESS_ESCAPE_BLOCKLIST) {
        return Err(ToolError::SandboxViolation(DenialReason::LimitsExceeded {
            message: format!(
                "Windows Run sandbox blocked potential process escape token '{token}'"
            ),
        }));
    }

    if policy.block_network
        && let Some(token) = blocked_token(command.policy_text(), NETWORK_BLOCKLIST)
    {
        return Err(ToolError::SandboxViolation(DenialReason::LimitsExceeded {
            message: format!("Windows Run sandbox blocked network token '{token}'"),
        }));
    }

    let command_for_execution = if shell_is_powershell {
        wrap_constrained_powershell(command.raw())
    } else {
        command.raw().to_string()
    };

    let requires_windows_host_sandbox = if check_windows_host {
        if let Err(reason) = host_probe() {
            return handle_unsandboxed_fallback(
                command_for_execution,
                policy.fallback_mode,
                unsafe_allow_unsandboxed,
                format!("host isolation unavailable ({reason})"),
            );
        }
        true
    } else {
        false
    };

    Ok(PreparedRunCommand::new(
        command_for_execution,
        None,
        requires_windows_host_sandbox,
    ))
}

fn blocked_token<'a>(command: &str, tokens: &'a [&str]) -> Option<&'a str> {
    let normalized = command.to_ascii_lowercase();
    tokens
        .iter()
        .copied()
        .find(|token| normalized.contains(token))
}

pub(crate) fn is_powershell_shell(shell: &DetectedShell) -> bool {
    let stem = Path::new(&shell.binary)
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(stem.as_str(), "pwsh" | "powershell")
}

fn wrap_constrained_powershell(command: &str) -> String {
    format!(
        "$ErrorActionPreference='Stop';$ProgressPreference='SilentlyContinue';\
$ExecutionContext.SessionState.LanguageMode='ConstrainedLanguage';\
Set-StrictMode -Version Latest;{command}"
    )
}

fn default_windows_host_probe() -> Result<(), String> {
    super::windows_run_host::sandbox_preflight()
}

fn handle_unsandboxed_fallback(
    command: String,
    mode: RunSandboxFallbackMode,
    unsafe_allow_unsandboxed: bool,
    reason: String,
) -> Result<PreparedRunCommand, ToolError> {
    match mode {
        RunSandboxFallbackMode::Deny => Err(ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!("Windows sandbox unavailable: {reason}. Fallback mode is deny."),
        }),
        RunSandboxFallbackMode::Prompt => {
            if !unsafe_allow_unsandboxed {
                return Err(ToolError::ExecutionFailed {
                    tool: "Run".to_string(),
                    message: format!(
                        "Windows sandbox unavailable: {reason}. \
To run unsandboxed once, set unsafe_allow_unsandboxed=true."
                    ),
                });
            }
            Ok(PreparedRunCommand::new(
                command,
                Some(format!(
                    "WARNING: Windows sandbox unavailable ({reason}); running unsandboxed due to explicit override."
                )),
                false,
            ))
        }
        RunSandboxFallbackMode::AllowWithWarning => Ok(PreparedRunCommand::new(
            command,
            Some(format!(
                "WARNING: Windows sandbox unavailable ({reason}); running unsandboxed."
            )),
            false,
        )),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn shell(binary: &str) -> DetectedShell {
        DetectedShell {
            binary: PathBuf::from(binary),
            args: vec!["-NoProfile".to_string(), "-Command".to_string()],
            name: "test".to_string(),
        }
    }

    fn cmd(command: &str) -> RunCommandText<'_> {
        RunCommandText::new(command, command)
    }

    #[test]
    fn windows_policy_defaults_are_hardened() {
        let policy = WindowsRunSandboxPolicy::default();
        assert!(policy.enabled);
        assert!(policy.enforce_powershell_only);
        assert!(policy.block_network);
        assert_eq!(policy.fallback_mode, RunSandboxFallbackMode::Prompt);
    }

    #[test]
    fn powershell_shell_detection_matches_known_variants() {
        assert!(is_powershell_shell(&shell("pwsh")));
        assert!(is_powershell_shell(&shell("powershell.exe")));
        assert!(!is_powershell_shell(&shell("cmd.exe")));
    }

    #[test]
    fn blocks_process_escape_tokens() {
        let err = prepare_windows_run_command(
            cmd("Start-Process cmd.exe /c whoami"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            false,
        )
        .unwrap_err();
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("process escape token"));
            }
            _ => panic!("expected sandbox violation"),
        }
    }

    #[test]
    fn blocks_network_tokens_when_enabled() {
        let err = prepare_windows_run_command(
            cmd("Invoke-WebRequest https://example.com"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            false,
        )
        .unwrap_err();
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("network token"));
            }
            _ => panic!("expected sandbox violation"),
        }
    }

    #[test]
    fn prompt_fallback_requires_explicit_override() {
        let err = prepare_windows_run_command(
            cmd("Get-ChildItem"),
            &shell("cmd.exe"),
            WindowsRunSandboxPolicy {
                enabled: true,
                enforce_powershell_only: true,
                block_network: false,
                fallback_mode: RunSandboxFallbackMode::Prompt,
            },
            false,
        )
        .unwrap_err();
        match err {
            ToolError::ExecutionFailed { message, .. } => {
                assert!(message.contains("unsafe_allow_unsandboxed=true"));
            }
            _ => panic!("expected execution failure"),
        }
    }

    #[test]
    fn prompt_fallback_allows_when_override_set() {
        let prepared = prepare_windows_run_command(
            cmd("Get-ChildItem"),
            &shell("cmd.exe"),
            WindowsRunSandboxPolicy {
                enabled: true,
                enforce_powershell_only: true,
                block_network: false,
                fallback_mode: RunSandboxFallbackMode::Prompt,
            },
            true,
        )
        .expect("fallback allowed");
        assert!(prepared.warning().is_some());
        assert_eq!(prepared.command(), "Get-ChildItem");
    }

    #[test]
    fn wraps_command_for_constrained_language() {
        let prepared = prepare_windows_run_command(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            false,
        )
        .expect("wrapped command");
        assert!(prepared.command().contains("ConstrainedLanguage"));
        assert!(prepared.command().contains("Set-StrictMode"));
    }

    #[test]
    fn host_probe_failure_requires_override_in_prompt_mode() {
        let err = prepare_windows_run_command_with_host_probe(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            false,
            true,
            || Err("job object API unavailable".to_string()),
        )
        .unwrap_err();
        match err {
            ToolError::ExecutionFailed { message, .. } => {
                assert!(message.contains("host isolation unavailable"));
                assert!(message.contains("unsafe_allow_unsandboxed=true"));
            }
            _ => panic!("expected execution failure"),
        }
    }

    #[test]
    fn host_probe_failure_with_override_keeps_constrained_language_wrapper() {
        let prepared = prepare_windows_run_command_with_host_probe(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            true,
            true,
            || Err("job object API unavailable".to_string()),
        )
        .expect("fallback allowed");
        assert!(prepared.warning().is_some());
        assert!(prepared.command().contains("ConstrainedLanguage"));
        assert!(!prepared.requires_windows_host_sandbox());
    }

    #[test]
    fn host_probe_success_marks_host_sandbox_as_required() {
        let prepared = prepare_windows_run_command_with_host_probe(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            false,
            true,
            || Ok(()),
        )
        .expect("host sandbox required");
        assert!(prepared.requires_windows_host_sandbox());
    }
}
