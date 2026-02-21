//! Windows-focused sandbox policy for the `Run` tool.
//!
//! This module is policy-first hardening (IFA mechanism/policy split):
//! - Mechanism: classify shell and command content
//! - Policy: decide allow/deny/fallback behavior

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::{DenialReason, DetectedShell, ToolError};
use unicode_normalization::UnicodeNormalization;

const ENV_RUN_ALLOW_UNSANDBOXED: &str = "FORGE_RUN_ALLOW_UNSANDBOXED";

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
    pub sandbox_mode: RunSandboxMode,
    pub shell_constraint: ShellConstraint,
    pub network_policy: NetworkPolicy,
    pub fallback_mode: RunSandboxFallbackMode,
}

impl Default for WindowsRunSandboxPolicy {
    fn default() -> Self {
        Self {
            sandbox_mode: RunSandboxMode::Enabled,
            shell_constraint: ShellConstraint::PowerShellOnly,
            network_policy: NetworkPolicy::Blocked,
            fallback_mode: RunSandboxFallbackMode::Prompt,
        }
    }
}

/// macOS-specific run sandbox policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacOsRunSandboxPolicy {
    pub sandbox_mode: RunSandboxMode,
    pub fallback_mode: RunSandboxFallbackMode,
}

impl Default for MacOsRunSandboxPolicy {
    fn default() -> Self {
        Self {
            sandbox_mode: RunSandboxMode::Enabled,
            fallback_mode: RunSandboxFallbackMode::Prompt,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunSandboxMode {
    Enabled,
    Disabled,
}

impl RunSandboxMode {
    #[must_use]
    pub const fn from_enabled(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellConstraint {
    PowerShellOnly,
    AnyShell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkPolicy {
    Blocked,
    Allowed,
}

/// Aggregate run sandbox policy (platform-specific sub-policies).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RunSandboxPolicy {
    pub windows: WindowsRunSandboxPolicy,
    pub macos: MacOsRunSandboxPolicy,
}

/// Prepared command after sandbox policy evaluation.
///
/// Encapsulates the program binary, argument list, and metadata needed to spawn
/// the process. On macOS the program may be `sandbox-exec` rather than the shell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRunCommand {
    program: PathBuf,
    args: Vec<OsString>,
    warning: Option<String>,
    requires_host_sandbox: bool,
}

impl PreparedRunCommand {
    pub(crate) fn new(
        program: PathBuf,
        args: Vec<OsString>,
        warning: Option<String>,
        requires_host_sandbox: bool,
    ) -> Self {
        Self {
            program,
            args,
            warning,
            requires_host_sandbox,
        }
    }

    pub(crate) fn passthrough(shell: &DetectedShell, command: &str) -> Self {
        let mut args: Vec<OsString> = shell.args.iter().map(OsString::from).collect();
        args.push(OsString::from(command));
        Self {
            program: shell.binary.clone(),
            args,
            warning: None,
            requires_host_sandbox: false,
        }
    }

    #[must_use]
    pub fn program(&self) -> &Path {
        &self.program
    }

    #[must_use]
    pub fn args(&self) -> &[OsString] {
        &self.args
    }

    #[must_use]
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    #[must_use]
    pub fn requires_host_sandbox(&self) -> bool {
        self.requires_host_sandbox
    }
}

const NETWORK_BLOCKLIST: &[&str] = &[
    "invoke-webrequest",
    "invoke-restmethod",
    "start-bitstransfer",
    "curl",
    "curl.exe",
    "wget",
    "wget.exe",
    "bitsadmin",
    "nslookup",
    "resolve-dnsname",
    "certutil",
    "ssh",
    "ssh.exe",
    "scp",
    "scp.exe",
    "sftp",
    "sftp.exe",
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
    "bash.exe",
    "bash -c",
    "python.exe",
    "python -c",
    "python3.exe",
    "python3 -c",
    "py.exe",
    "py -c",
    "node.exe",
    "node -e",
    "perl.exe",
    "perl -e",
    "ruby.exe",
    "ruby -e",
    "java.exe",
    "javaw.exe",
    "php.exe",
    "php -r",
    "rundll32",
    "mshta",
    "regsvr32",
    "cscript",
    "wscript",
];

const PROCESS_ESCAPE_COMMAND_NAMES: &[&str] = &[
    "powershell",
    "pwsh",
    "cmd",
    "wsl",
    "bash",
    "sh",
    "dash",
    "zsh",
    "fish",
    "ksh",
    "python",
    "python3",
    "py",
    "node",
    "perl",
    "ruby",
    "java",
    "javaw",
    "php",
    "rundll32",
    "mshta",
    "regsvr32",
    "cscript",
    "wscript",
];

/// Prepare a command for execution under run sandbox policy.
///
/// On Windows this enforces `WindowsRunSandboxPolicy`. On macOS it may wrap the shell in
/// `sandbox-exec` (Seatbelt). On Linux/BSD, execution is denied when sandboxing is enabled
/// because there is no supported OS-level sandbox backend yet.
pub(crate) fn prepare_run_command(
    command: RunCommandText<'_>,
    shell: &DetectedShell,
    policy: RunSandboxPolicy,
    working_dir: &Path,
) -> Result<PreparedRunCommand, ToolError> {
    if cfg!(windows) {
        let _ = working_dir;
        return prepare_windows_run_command(command, shell, policy.windows);
    }
    #[cfg(target_os = "macos")]
    {
        prepare_macos_run_command(command.raw(), shell, policy.macos, working_dir)
    }
    #[cfg(not(target_os = "macos"))]
    {
        if !matches!(policy.windows.sandbox_mode, RunSandboxMode::Enabled) {
            return Ok(PreparedRunCommand::passthrough(shell, command.raw()));
        }
        let _ = working_dir;
        Err(ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: "Run sandbox is unavailable on Linux/BSD; refusing unsandboxed execution."
                .to_string(),
        })
    }
}

pub(crate) fn prepare_windows_run_command(
    command: RunCommandText<'_>,
    shell: &DetectedShell,
    policy: WindowsRunSandboxPolicy,
) -> Result<PreparedRunCommand, ToolError> {
    prepare_windows_run_command_with_host_probe(
        command,
        shell,
        policy,
        cfg!(windows),
        default_windows_host_probe,
    )
}

fn prepare_windows_run_command_with_host_probe<F>(
    command: RunCommandText<'_>,
    shell: &DetectedShell,
    policy: WindowsRunSandboxPolicy,
    check_windows_host: bool,
    host_probe: F,
) -> Result<PreparedRunCommand, ToolError>
where
    F: FnOnce() -> Result<(), String>,
{
    if !matches!(policy.sandbox_mode, RunSandboxMode::Enabled) {
        return Ok(PreparedRunCommand::passthrough(shell, command.raw()));
    }

    let shell_is_powershell = is_powershell_shell(shell);
    if matches!(policy.shell_constraint, ShellConstraint::PowerShellOnly) && !shell_is_powershell {
        return handle_unsandboxed_fallback(
            PreparedRunCommand::passthrough(shell, command.raw()),
            policy.fallback_mode,
            format!(
                "configured shell '{}' is not PowerShell",
                shell.binary.display()
            ),
        );
    }

    if let Some(command_name) = blocked_process_escape_command_name(command.policy_text()) {
        return Err(ToolError::SandboxViolation(DenialReason::LimitsExceeded {
            message: format!(
                "Windows Run sandbox blocked potential process escape command '{command_name}'"
            ),
        }));
    }

    if let Some(token) = blocked_token(command.policy_text(), PROCESS_ESCAPE_BLOCKLIST) {
        return Err(ToolError::SandboxViolation(DenialReason::LimitsExceeded {
            message: format!(
                "Windows Run sandbox blocked potential process escape token '{token}'"
            ),
        }));
    }

    if matches!(policy.network_policy, NetworkPolicy::Blocked)
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

    let requires_host_sandbox = if check_windows_host {
        if let Err(reason) = host_probe() {
            let mut fallback_args: Vec<OsString> = shell.args.iter().map(OsString::from).collect();
            fallback_args.push(OsString::from(command_for_execution));
            return handle_unsandboxed_fallback(
                PreparedRunCommand::new(shell.binary.clone(), fallback_args, None, false),
                policy.fallback_mode,
                format!("host isolation unavailable ({reason})"),
            );
        }
        true
    } else {
        false
    };

    let mut args: Vec<OsString> = shell.args.iter().map(OsString::from).collect();
    args.push(OsString::from(command_for_execution));
    Ok(PreparedRunCommand::new(
        shell.binary.clone(),
        args,
        None,
        requires_host_sandbox,
    ))
}

fn blocked_process_escape_command_name(policy_text: &str) -> Option<&'static str> {
    let normalized = normalize_policy_text(policy_text);
    let command_name = normalized.split_whitespace().next()?;
    let command_leaf = command_name
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(command_name);
    let command_leaf = command_leaf.strip_suffix(".exe").unwrap_or(command_leaf);
    PROCESS_ESCAPE_COMMAND_NAMES
        .iter()
        .copied()
        .find(|candidate| *candidate == command_leaf)
}

fn blocked_token<'a>(command: &str, tokens: &'a [&str]) -> Option<&'a str> {
    let normalized = normalize_policy_text(command);
    tokens.iter().copied().find(|token| {
        let normalized_token = normalize_policy_text(token);
        if should_match_on_token_boundaries(&normalized_token) {
            contains_token_with_boundaries(&normalized, &normalized_token)
        } else {
            normalized.contains(&normalized_token)
        }
    })
}

fn normalize_policy_text(text: &str) -> String {
    text.nfkc().collect::<String>().to_ascii_lowercase()
}

fn should_match_on_token_boundaries(token: &str) -> bool {
    !token.is_empty()
        && token
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn is_policy_token_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')
}

fn contains_token_with_boundaries(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, _)| {
        let before = haystack[..start].chars().next_back();
        let end = start + needle.len();
        let after = haystack[end..].chars().next();
        !before.is_some_and(is_policy_token_char) && !after.is_some_and(is_policy_token_char)
    })
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
    passthrough: PreparedRunCommand,
    mode: RunSandboxFallbackMode,
    reason: String,
) -> Result<PreparedRunCommand, ToolError> {
    handle_unsandboxed_fallback_with_opt_in(
        passthrough,
        mode,
        reason,
        run_unsandboxed_opt_in_enabled(),
    )
}

fn handle_unsandboxed_fallback_with_opt_in(
    passthrough: PreparedRunCommand,
    mode: RunSandboxFallbackMode,
    reason: String,
    allow_unsandboxed: bool,
) -> Result<PreparedRunCommand, ToolError> {
    match mode {
        RunSandboxFallbackMode::Deny => Err(ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!("Sandbox unavailable: {reason}. Fallback mode is deny."),
        }),
        RunSandboxFallbackMode::Prompt => Err(ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!(
                "Sandbox unavailable: {reason}. Fallback mode is prompt but per-call unsandboxed override is disabled."
            ),
        }),
        RunSandboxFallbackMode::AllowWithWarning => {
            if !allow_unsandboxed {
                tracing::warn!(
                    "Run allow_with_warning requested but {} is not set; denying unsandboxed fallback",
                    ENV_RUN_ALLOW_UNSANDBOXED
                );
                return Err(ToolError::ExecutionFailed {
                    tool: "Run".to_string(),
                    message: format!(
                        "Sandbox unavailable: {reason}. Fallback mode is allow_with_warning but runtime opt-in is missing ({ENV_RUN_ALLOW_UNSANDBOXED}=1)."
                    ),
                });
            }
            Ok(PreparedRunCommand {
                warning: Some(format!(
                    "WARNING: sandbox unavailable ({reason}); running unsandboxed."
                )),
                requires_host_sandbox: false,
                ..passthrough
            })
        }
    }
}

fn run_unsandboxed_opt_in_enabled() -> bool {
    env::var(ENV_RUN_ALLOW_UNSANDBOXED)
        .ok()
        .is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

#[cfg(target_os = "macos")]
fn prepare_macos_run_command(
    command: &str,
    shell: &DetectedShell,
    policy: MacOsRunSandboxPolicy,
    working_dir: &Path,
) -> Result<PreparedRunCommand, ToolError> {
    use std::sync::OnceLock;

    static SANDBOX_EXEC: OnceLock<Option<PathBuf>> = OnceLock::new();

    fn probe() -> Option<PathBuf> {
        let p = PathBuf::from("/usr/bin/sandbox-exec");
        p.is_file().then_some(p)
    }

    if !matches!(policy.sandbox_mode, RunSandboxMode::Enabled) {
        return Ok(PreparedRunCommand::passthrough(shell, command));
    }

    let sandbox_exec = SANDBOX_EXEC.get_or_init(probe);
    let Some(sandbox_exec) = sandbox_exec.as_ref() else {
        return handle_unsandboxed_fallback(
            PreparedRunCommand::passthrough(shell, command),
            policy.fallback_mode,
            "sandbox-exec not found at /usr/bin/sandbox-exec".to_string(),
        );
    };

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let tmp = std::env::temp_dir();
    let cwd = working_dir
        .canonicalize()
        .unwrap_or_else(|_| working_dir.to_path_buf());
    let profile = generate_seatbelt_profile(&cwd, &tmp, &home);

    let mut args: Vec<OsString> = vec![
        OsString::from("-p"),
        OsString::from(&profile),
        OsString::from(&shell.binary),
    ];
    args.extend(shell.args.iter().map(OsString::from));
    args.push(OsString::from(command));

    Ok(PreparedRunCommand::new(
        sandbox_exec.clone(),
        args,
        None,
        false,
    ))
}

#[cfg(target_os = "macos")]
fn generate_seatbelt_profile(cwd: &Path, tmp: &Path, home: &Path) -> String {
    let cwd = escape_seatbelt_literal(&cwd.to_string_lossy());
    let tmp = escape_seatbelt_literal(&tmp.to_string_lossy());
    let home = escape_seatbelt_literal(&home.to_string_lossy());
    format!(
        r#"(version 1)
(deny default)
(allow process-exec)
(allow process-fork)
(allow file-read*)
(deny file-read* (subpath "{home}/.ssh"))
(deny file-read* (subpath "{home}/.gnupg"))
(deny file-read* (subpath "{home}/.aws"))
(deny file-read* (subpath "{home}/.azure"))
(deny file-read* (subpath "{home}/.config/gcloud"))
(deny file-read* (subpath "{home}/Library/Keychains"))
(allow file-write* (subpath "{cwd}"))
(allow file-write* (subpath "{tmp}"))
(deny network*)
(allow mach-lookup)
(allow sysctl-read)
(allow signal)"#
    )
}

#[cfg(any(test, target_os = "macos"))]
fn escape_seatbelt_literal(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;
    use std::sync::Once;

    use super::{
        DenialReason, DetectedShell, MacOsRunSandboxPolicy, NetworkPolicy, Path,
        PreparedRunCommand, RunCommandText, RunSandboxFallbackMode, RunSandboxMode,
        ShellConstraint, ToolError, WindowsRunSandboxPolicy, escape_seatbelt_literal,
        handle_unsandboxed_fallback_with_opt_in, is_powershell_shell, prepare_windows_run_command,
        prepare_windows_run_command_with_host_probe,
    };

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

    fn enable_unsandboxed_opt_in_for_tests() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            // SAFETY: test-only global opt-in set once and never mutated.
            unsafe {
                env::set_var("FORGE_RUN_ALLOW_UNSANDBOXED", "1");
            }
        });
    }

    #[test]
    fn escapes_seatbelt_literals() {
        let escaped = escape_seatbelt_literal("a\\b\"c\nd\re\tf");
        assert_eq!(escaped, "a\\\\b\\\"c\\nd\\re\\tf");
    }

    #[test]
    fn windows_policy_defaults_are_hardened() {
        let policy = WindowsRunSandboxPolicy::default();
        assert!(matches!(policy.sandbox_mode, RunSandboxMode::Enabled));
        assert!(matches!(
            policy.shell_constraint,
            ShellConstraint::PowerShellOnly
        ));
        assert!(matches!(policy.network_policy, NetworkPolicy::Blocked));
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
        )
        .unwrap_err();
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("process escape"));
            }
            _ => panic!("expected sandbox violation"),
        }
    }

    #[test]
    fn blocks_interpreter_escape_tokens() {
        let err = prepare_windows_run_command(
            cmd("python -c \"print('owned')\""),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
        )
        .unwrap_err();
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("process escape"));
            }
            _ => panic!("expected sandbox violation"),
        }
    }

    #[test]
    fn blocks_bare_process_escape_command_names() {
        let err = prepare_windows_run_command(
            cmd("powershell -NoProfile"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
        )
        .unwrap_err();
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("process escape command"));
            }
            _ => panic!("expected sandbox violation"),
        }
    }

    #[test]
    fn blocks_process_escape_command_paths() {
        let err = prepare_windows_run_command(
            cmd("C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe -NoProfile"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
        )
        .unwrap_err();
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("process escape command"));
            }
            _ => panic!("expected sandbox violation"),
        }
    }

    #[test]
    fn blocks_posix_shell_escape_command_names() {
        let cases = [
            ("sh -c whoami", "sh"),
            ("dash -c whoami", "dash"),
            ("zsh -c whoami", "zsh"),
            ("fish -c whoami", "fish"),
            ("ksh -c whoami", "ksh"),
            ("sh.exe -c whoami", "sh"),
            ("C:\\Git\\bin\\sh.exe -c whoami", "sh"),
        ];

        for (command, expected) in cases {
            let err = prepare_windows_run_command(
                cmd(command),
                &shell("pwsh"),
                WindowsRunSandboxPolicy::default(),
            )
            .unwrap_err();
            match err {
                ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                    assert!(
                        message.contains("process escape command") && message.contains(expected),
                        "cmd={command} msg={message}"
                    );
                }
                _ => panic!("expected sandbox violation for {command}"),
            }
        }
    }

    #[test]
    fn blocked_tokens_apply_nfkc_normalization() {
        let err = prepare_windows_run_command(
            cmd("ｐｙｔｈｏｎ -c \"print('owned')\""),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
        )
        .unwrap_err();
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("process escape command"));
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
    fn blocks_dns_tokens_when_enabled() {
        let err = prepare_windows_run_command(
            cmd("nslookup example.com"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
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
    fn blocks_bare_posix_network_tools_when_enabled() {
        let cases = [
            ("curl https://example.com", "curl"),
            ("wget https://example.com", "wget"),
            ("ssh user@example.com", "ssh"),
            ("scp file.txt user@example.com:/tmp/file.txt", "scp"),
            ("sftp user@example.com", "sftp"),
        ];

        for (network_cmd, expected_token) in cases {
            let err = prepare_windows_run_command(
                cmd(network_cmd),
                &shell("pwsh"),
                WindowsRunSandboxPolicy::default(),
            )
            .unwrap_err();
            match err {
                ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                    assert!(
                        message.contains("network token") && message.contains(expected_token),
                        "cmd={network_cmd} msg={message}"
                    );
                }
                _ => panic!("expected sandbox violation for {network_cmd}"),
            }
        }
    }

    #[test]
    fn prompt_fallback_denies_when_shell_is_not_powershell() {
        let err = prepare_windows_run_command(
            cmd("Get-ChildItem"),
            &shell("cmd.exe"),
            WindowsRunSandboxPolicy {
                sandbox_mode: RunSandboxMode::Enabled,
                shell_constraint: ShellConstraint::PowerShellOnly,
                network_policy: NetworkPolicy::Allowed,
                fallback_mode: RunSandboxFallbackMode::Prompt,
            },
        )
        .unwrap_err();
        match err {
            ToolError::ExecutionFailed { message, .. } => {
                assert!(message.contains("Fallback mode is prompt"));
            }
            _ => panic!("expected execution failure"),
        }
    }

    #[test]
    fn allow_with_warning_fallback_allows_when_shell_is_not_powershell() {
        enable_unsandboxed_opt_in_for_tests();
        let prepared = prepare_windows_run_command(
            cmd("Get-ChildItem"),
            &shell("cmd.exe"),
            WindowsRunSandboxPolicy {
                sandbox_mode: RunSandboxMode::Enabled,
                shell_constraint: ShellConstraint::PowerShellOnly,
                network_policy: NetworkPolicy::Allowed,
                fallback_mode: RunSandboxFallbackMode::AllowWithWarning,
            },
        )
        .expect("allow-with-warning fallback");
        assert!(prepared.warning().is_some());
        assert_eq!(prepared.args().last().unwrap(), "Get-ChildItem");
    }

    #[test]
    fn allow_with_warning_denied_without_env_var() {
        let err = handle_unsandboxed_fallback_with_opt_in(
            PreparedRunCommand::passthrough(&shell("pwsh"), "Get-ChildItem"),
            RunSandboxFallbackMode::AllowWithWarning,
            "host isolation unavailable".to_string(),
            false,
        )
        .unwrap_err();

        match err {
            ToolError::ExecutionFailed { message, .. } => {
                assert!(message.contains("allow_with_warning"));
                assert!(message.contains("FORGE_RUN_ALLOW_UNSANDBOXED"));
            }
            _ => panic!("expected execution failure"),
        }
    }

    #[test]
    fn wraps_command_for_constrained_language() {
        let prepared = prepare_windows_run_command_with_host_probe(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            true,
            || Ok(()),
        )
        .expect("wrapped command");
        let last_arg = prepared.args().last().unwrap().to_string_lossy();
        assert!(last_arg.contains("ConstrainedLanguage"));
        assert!(last_arg.contains("Set-StrictMode"));
    }

    #[test]
    fn host_probe_failure_in_prompt_mode_denies() {
        let err = prepare_windows_run_command_with_host_probe(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            true,
            || Err("job object API unavailable".to_string()),
        )
        .unwrap_err();
        match err {
            ToolError::ExecutionFailed { message, .. } => {
                assert!(message.contains("host isolation unavailable"));
                assert!(message.contains("Fallback mode is prompt"));
            }
            _ => panic!("expected execution failure"),
        }
    }

    #[test]
    fn host_probe_failure_allow_with_warning_keeps_constrained_language_wrapper() {
        enable_unsandboxed_opt_in_for_tests();
        let prepared = prepare_windows_run_command_with_host_probe(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy {
                sandbox_mode: RunSandboxMode::Enabled,
                shell_constraint: ShellConstraint::PowerShellOnly,
                network_policy: NetworkPolicy::Blocked,
                fallback_mode: RunSandboxFallbackMode::AllowWithWarning,
            },
            true,
            || Err("job object API unavailable".to_string()),
        )
        .expect("allow-with-warning fallback");
        assert!(prepared.warning().is_some());
        let last_arg = prepared.args().last().unwrap().to_string_lossy();
        assert!(last_arg.contains("ConstrainedLanguage"));
        assert!(!prepared.requires_host_sandbox());
    }

    #[test]
    fn host_probe_success_marks_host_sandbox_as_required() {
        let prepared = prepare_windows_run_command_with_host_probe(
            cmd("Get-ChildItem"),
            &shell("pwsh"),
            WindowsRunSandboxPolicy::default(),
            true,
            || Ok(()),
        )
        .expect("host sandbox required");
        assert!(prepared.requires_host_sandbox());
    }

    #[test]
    fn passthrough_uses_shell_binary_as_program() {
        let shell = shell("pwsh");
        let prepared = PreparedRunCommand::passthrough(&shell, "Get-ChildItem");
        assert_eq!(prepared.program(), Path::new("pwsh"));
        assert_eq!(prepared.args().last().unwrap(), "Get-ChildItem");
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_bsd_denies_when_run_sandbox_is_enabled() {
        let err = super::prepare_run_command(
            cmd("echo hi"),
            &shell("bash"),
            super::RunSandboxPolicy::default(),
            Path::new("."),
        )
        .unwrap_err();
        match err {
            ToolError::ExecutionFailed { message, .. } => {
                assert!(message.contains("unavailable on Linux/BSD"));
            }
            _ => panic!("expected execution failure"),
        }
    }

    #[cfg(not(any(windows, target_os = "macos")))]
    #[test]
    fn linux_bsd_allows_passthrough_when_run_sandbox_is_disabled() {
        let mut policy = super::RunSandboxPolicy::default();
        policy.windows.sandbox_mode = RunSandboxMode::Disabled;

        let prepared =
            super::prepare_run_command(cmd("echo hi"), &shell("bash"), policy, Path::new("."))
                .expect("passthrough when disabled");
        assert_eq!(prepared.program(), Path::new("bash"));
        assert!(!prepared.requires_host_sandbox());
    }

    #[test]
    fn macos_policy_defaults_are_hardened() {
        let policy = MacOsRunSandboxPolicy::default();
        assert!(matches!(policy.sandbox_mode, RunSandboxMode::Enabled));
        assert_eq!(policy.fallback_mode, RunSandboxFallbackMode::Prompt);
    }
}
