//! PowerShell AST parsing helpers for Windows `Run` sandbox policy.
//!
//! This module is mechanism-only (IFA): it extracts normalized facts about a
//! PowerShell command string without executing it.

use std::path::Path;

use serde::Deserialize;
use tokio::process::Command;

use super::{DenialReason, ToolError};

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

pub(crate) async fn policy_text_for_command(
    powershell_binary: &Path,
    raw_command: &str,
) -> Result<PowerShellPolicyText, ToolError> {
    let output = Command::new(powershell_binary)
        .env("FORGE_POWERSHELL_AST_RAW", raw_command)
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(AST_PROBE_SCRIPT)
        .output()
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!("failed to invoke PowerShell AST probe: {e}"),
        })?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Err(ToolError::ExecutionFailed {
            tool: "Run".to_string(),
            message: format!(
                "PowerShell AST probe failed (exit code {code}). stderr='{}' stdout='{}'",
                stderr.trim(),
                stdout.trim()
            ),
        });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
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

    use super::*;

    fn powershell_binary() -> PathBuf {
        which::which("pwsh")
            .or_else(|_| which::which("powershell"))
            .expect("PowerShell binary")
    }

    #[tokio::test]
    async fn policy_text_normalizes_backtick_escapes_in_command_name() {
        let bin = powershell_binary();
        let out = policy_text_for_command(&bin, "Start-Pr`ocess notepad")
            .await
            .expect("policy text");
        assert!(out.as_str().starts_with("Start-Process "));
    }

    #[tokio::test]
    async fn policy_text_rejects_multiple_statements() {
        let bin = powershell_binary();
        let err = policy_text_for_command(&bin, "cd ~; ls")
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
        let err = policy_text_for_command(&bin, "echo \"foo $bar\"")
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
        let err = policy_text_for_command(&bin, "cmd --% /c whoami")
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
