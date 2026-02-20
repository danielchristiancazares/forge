//! PowerShell AST parsing helpers for Windows `Run` sandbox policy.
//!
//! This module is mechanism-only (IFA): it extracts normalized facts about a
//! PowerShell command string without executing it.

use std::path::Path;

use serde::Deserialize;
use tokio::process::Command;

use super::process::ChildGuard;
use super::{DenialReason, EnvSanitizer, ToolError};

/// Result of parsing a PowerShell command into a policy-facing normalized form.
///
/// Invariant: `policy_text` contains a whitespace-joined token list where the
/// first token is the **alias-resolved** command name (when resolvable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PowerShellPolicyText {
    policy_text: String,
}

impl PowerShellPolicyText {
    #[must_use]
    pub(crate) fn as_str(&self) -> &str {
        &self.policy_text
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProbeOutput {
    ok: bool,
    violation: Option<String>,
    #[serde(default)]
    errors: Vec<String>,
    policy_text: Option<String>,
}

const AST_PROBE_SCRIPT: &str = r"& {
  $ErrorActionPreference = 'Stop'
  $ProgressPreference = 'SilentlyContinue'

  $Raw = $env:FORGE_POWERSHELL_AST_RAW
  if (-not $Raw) {
    $out = [ordered]@{
      ok = $false
      violation = 'missing_raw_command'
      errors = @()
      policy_text = $null
    }
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $out = [ordered]@{
    ok = $true
    violation = $null
    errors = @()
    policy_text = $null
  }

  $tokens = $null
  $errors = $null

  try {
    $ast = [System.Management.Automation.Language.Parser]::ParseInput($Raw, [ref]$tokens, [ref]$errors)
  } catch {
    $out.ok = $false
    $out.violation = 'parse_exception'
    $out.errors = @($_.Exception.Message)
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  if ($errors -and $errors.Count -gt 0) {
    $out.ok = $false
    $out.violation = 'parse_error'
    $out.errors = @($errors | ForEach-Object { $_.Message })
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $stmts = $ast.EndBlock.Statements
  if ($stmts.Count -ne 1) {
    $out.ok = $false
    $out.violation = 'multiple_statements'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $stmt = $stmts[0]
  if ($stmt -isnot [System.Management.Automation.Language.PipelineAst]) {
    $out.ok = $false
    $out.violation = 'non_pipeline_statement'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $pipe = [System.Management.Automation.Language.PipelineAst]$stmt
  if ($pipe.Background) {
    $out.ok = $false
    $out.violation = 'background_pipeline'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  if ($pipe.PipelineElements.Count -ne 1) {
    $out.ok = $false
    $out.violation = 'pipeline_not_supported'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $elem = $pipe.PipelineElements[0]
  if ($elem -isnot [System.Management.Automation.Language.CommandAst]) {
    $out.ok = $false
    $out.violation = 'non_command_pipeline_element'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $cmdAst = [System.Management.Automation.Language.CommandAst]$elem
  if ($cmdAst.InvocationOperator -ne [System.Management.Automation.Language.TokenKind]::Unknown) {
    $out.ok = $false
    $out.violation = 'invocation_operator_not_supported'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  if ($cmdAst.Redirections.Count -gt 0) {
    $out.ok = $false
    $out.violation = 'redirection_not_supported'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $cmdName = $cmdAst.GetCommandName()
  if (-not $cmdName) {
    $out.ok = $false
    $out.violation = 'dynamic_command_name'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $resolvedName = $cmdName
  try {
    $ci = Get-Command $cmdName -ErrorAction Stop
    if ($ci.CommandType -eq 'Alias') {
      $resolvedName = $ci.Definition
    } elseif ($ci.Name) {
      $resolvedName = $ci.Name
    }
  } catch {
    # Keep cmdName when resolution fails.
  }

  $norm = New-Object System.Collections.Generic.List[string]
  [void]$norm.Add([string]$resolvedName)

  $elements = $cmdAst.CommandElements
  for ($i = 1; $i -lt $elements.Count; $i++) {
    $e = $elements[$i]
    if ($e -is [System.Management.Automation.Language.StringConstantExpressionAst]) {
      [void]$norm.Add([string]$e.Value)
      continue
    }
    if ($e -is [System.Management.Automation.Language.CommandParameterAst]) {
      [void]$norm.Add('-' + [string]$e.ParameterName)
      continue
    }
    $out.ok = $false
    $out.violation = 'non_literal_element:' + $e.GetType().Name
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  if ($norm.Contains('--%')) {
    $out.ok = $false
    $out.violation = 'verbatim_arguments_not_supported'
    $out | ConvertTo-Json -Compress -Depth 6
    exit 0
  }

  $out.policy_text = ($norm -join ' ')
  $out | ConvertTo-Json -Compress -Depth 6
}";

const PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const MAX_PROBE_OUTPUT: usize = 64 * 1024;

pub(crate) async fn policy_text_for_command(
    powershell_binary: &Path,
    raw_command: &str,
    env_sanitizer: &EnvSanitizer,
) -> Result<PowerShellPolicyText, ToolError> {
    let mut cmd = Command::new(powershell_binary);

    super::process::apply_sanitized_env(&mut cmd, env_sanitizer);
    cmd.env("FORGE_POWERSHELL_AST_RAW", raw_command);

    cmd.arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(AST_PROBE_SCRIPT)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    super::process::set_new_session(&mut cmd);

    let child = cmd.spawn().map_err(|e| ToolError::ExecutionFailed {
        tool: "Run".to_string(),
        message: format!("failed to invoke PowerShell AST probe: {e}"),
    })?;
    let mut guard = ChildGuard::new(child);

    let stdout_pipe =
        guard
            .child_mut()
            .stdout
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed {
                tool: "Run".to_string(),
                message: "failed to capture PowerShell AST probe stdout".to_string(),
            })?;
    let stderr_pipe =
        guard
            .child_mut()
            .stderr
            .take()
            .ok_or_else(|| ToolError::ExecutionFailed {
                tool: "Run".to_string(),
                message: "failed to capture PowerShell AST probe stderr".to_string(),
            })?;

    let io_future = async {
        use tokio::io::AsyncReadExt;
        let mut stdout_buf = Vec::with_capacity(4096);
        let mut stderr_buf = Vec::with_capacity(1024);
        let mut stdout_bounded = stdout_pipe.take(MAX_PROBE_OUTPUT as u64);
        let mut stderr_bounded = stderr_pipe.take(MAX_PROBE_OUTPUT as u64);
        let (r1, r2) = futures_util::future::join(
            stdout_bounded.read_to_end(&mut stdout_buf),
            stderr_bounded.read_to_end(&mut stderr_buf),
        )
        .await;
        r1.map_err(|e| ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!("failed to read PowerShell AST probe stdout: {e}"),
        })?;
        r2.map_err(|e| ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!("failed to read PowerShell AST probe stderr: {e}"),
        })?;
        Ok::<_, ToolError>((stdout_buf, stderr_buf))
    };

    let (stdout_buf, stderr_buf) = tokio::time::timeout(PROBE_TIMEOUT, io_future)
        .await
        .map_err(|_| ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: "PowerShell AST probe timed out (10s)".to_string(),
        })??;

    let status = guard
        .child_mut()
        .wait()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!("failed to wait on PowerShell AST probe: {e}"),
        })?;
    guard.disarm();

    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&stderr_buf);
        let stdout = String::from_utf8_lossy(&stdout_buf);
        return Err(ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!(
                "PowerShell AST probe failed (exit code {code}). stderr='{}' stdout='{}'",
                stderr.trim(),
                stdout.trim()
            ),
        });
    }

    let stdout = String::from_utf8_lossy(&stdout_buf);
    let stderr = String::from_utf8_lossy(&stderr_buf);
    let probe: ProbeOutput =
        serde_json::from_str(stdout.trim()).map_err(|e| ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!(
                "PowerShell AST probe returned invalid JSON: {e} (stdout='{}', stderr='{}')",
                stdout.trim(),
                stderr.trim()
            ),
        })?;

    if !probe.ok {
        let violation = probe
            .violation
            .unwrap_or_else(|| "unknown_violation".to_string());
        if violation == "parse_error" {
            let message = if probe.errors.is_empty() {
                "PowerShell parse error".to_string()
            } else {
                format!("PowerShell parse error: {}", probe.errors.join("; "))
            };
            return Err(ToolError::ExecutionFailed {
                tool: "Run".to_string(),
                message,
            });
        }

        return Err(ToolError::SandboxViolation(DenialReason::LimitsExceeded {
            message: format!(
                "Windows Run sandbox rejected PowerShell syntax it cannot safely analyze ({violation}). \
Only a single, literal command invocation is supported (no pipelines, semicolons, redirection, \
dynamic invocation, or string interpolation)."
            ),
        }));
    }

    let policy_text = probe
        .policy_text
        .ok_or_else(|| ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: "PowerShell AST probe succeeded but did not return policy_text".to_string(),
        })?;

    Ok(PowerShellPolicyText { policy_text })
}

#[cfg(all(test, windows))]
mod tests {
    use std::path::PathBuf;

    use super::{DenialReason, EnvSanitizer, ToolError, policy_text_for_command};

    fn powershell_binary() -> PathBuf {
        which::which("pwsh")
            .or_else(|_| which::which("powershell"))
            .expect("PowerShell binary")
    }

    fn test_sanitizer() -> EnvSanitizer {
        EnvSanitizer::new(&[]).expect("empty denylist")
    }

    #[tokio::test]
    async fn policy_text_normalizes_backtick_escapes_in_command_name() {
        let bin = powershell_binary();
        let san = test_sanitizer();
        let out = policy_text_for_command(&bin, "Start-Pr`ocess notepad", &san)
            .await
            .expect("policy text");
        assert!(out.as_str().starts_with("Start-Process "));
    }

    #[tokio::test]
    async fn policy_text_rejects_multiple_statements() {
        let bin = powershell_binary();
        let san = test_sanitizer();
        let err = policy_text_for_command(&bin, "cd ~; ls", &san)
            .await
            .expect_err("expected rejection");
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("multiple_statements"));
            }
            other => panic!("expected sandbox violation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn policy_text_rejects_expandable_strings() {
        let bin = powershell_binary();
        let san = test_sanitizer();
        let err = policy_text_for_command(&bin, "echo \"foo $bar\"", &san)
            .await
            .expect_err("expected rejection");
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("ExpandableStringExpressionAst"));
            }
            other => panic!("expected sandbox violation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn policy_text_rejects_verbatim_arguments() {
        let bin = powershell_binary();
        let san = test_sanitizer();
        let err = policy_text_for_command(&bin, "cmd --% /c whoami", &san)
            .await
            .expect_err("expected rejection");
        match err {
            ToolError::SandboxViolation(DenialReason::LimitsExceeded { message }) => {
                assert!(message.contains("verbatim_arguments_not_supported"));
            }
            other => panic!("expected sandbox violation, got {other:?}"),
        }
    }
}
