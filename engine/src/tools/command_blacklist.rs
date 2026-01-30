//! Command blacklist for blocking catastrophically dangerous commands.
//!
//! This module provides a regex-based blacklist that **always denies** commands
//! with no legitimate AI assistant use case. This is distinct from the tool-level
//! denylist (which blocks entire tools like `run_command`).

use regex::RegexSet;

use super::{DenialReason, ToolError};

/// Default command blacklist patterns.
///
/// Each tuple: `(regex_pattern, human_readable_reason)`
///
/// Patterns are case-sensitive on Unix. Use `(?i)` prefix for case-insensitive
/// matching (e.g., Windows commands).
pub const DEFAULT_PATTERNS: &[(&str, &str)] = &[
    // Unix: rm -r / (root filesystem wipe)
    // Case-insensitive: weird casing (RM -RF /) suggests prompt injection
    (
        r"(?i)\brm\s+(?:(?:--recursive|-[^\s-]*[rR][^\s-]*)(?:\s+(?:--[\w-]+|-[^\s]+))*|(?:--[\w-]+|-[^\s]+)\s+(?:--recursive|-[^\s-]*[rR][^\s-]*)(?:\s+(?:--[\w-]+|-[^\s]+))*)\s+(?:--\s+)?(?:/+|/\*|/\.\*(?:/+)?|/(?:\.{1,2})(?:/\.{1,2})*(?:/+)?|/(?:\.{1,2})(?:/\.{1,2})*/+\*)(?:\s|$|[&|;])",
        "Attempting to delete root filesystem",
    ),
    // Unix: rm -r ~ or $HOME (home directory wipe)
    (
        r"(?i)\brm\s+(?:(?:--recursive|-[^\s-]*[rR][^\s-]*)(?:\s+(?:--[\w-]+|-[^\s]+))*|(?:--[\w-]+|-[^\s]+)\s+(?:--recursive|-[^\s-]*[rR][^\s-]*)(?:\s+(?:--[\w-]+|-[^\s]+))*)\s+(?:--\s+)?(?:~|\$HOME|\$\{HOME\})(?:\s|$|[&|;/])",
        "Attempting to delete home directory",
    ),
    // Fork bomb (bash)
    (r":\(\)\s*\{\s*:\|:&\s*\}\s*;:", "Fork bomb detected"),
    // dd overwriting disk devices
    (
        r"(?i)dd\s+.*of=/dev/(?:sd|hd|nvme|vd|xvd|loop)\w*",
        "Attempting to overwrite disk device",
    ),
    // mkfs on disk devices (formatting)
    (
        r"(?i)mkfs(?:\.\w+)?\s+/dev/(?:sd|hd|nvme|vd|xvd)\w*",
        "Attempting to format disk device",
    ),
    // chmod -R on root or system directories
    (
        r"(?i)chmod\s+-R\s+\d+\s+/(?:\s|$|[&|;])",
        "Recursive permission change on root filesystem",
    ),
    // Windows: Remove-Item with path first, then both -Recurse and -Force
    // e.g., "Remove-Item C:\ -Recurse -Force" or "Remove-Item ~ -Force -Recurse"
    (
        r"(?i)Remove-Item\s+(?:C:\\|~)\s+-(?:Recurse|Force)\s+-(?:Recurse|Force)",
        "Attempting to delete system drive or home directory",
    ),
    // Windows: Remove-Item with flags first, then C:\ or ~ (must be at end or followed by whitespace)
    // e.g., "Remove-Item -Recurse -Force C:\" or "Remove-Item -Force -Recurse ~"
    (
        r"(?i)Remove-Item\s+-(?:Recurse|Force)\s+-(?:Recurse|Force)\s+(?:C:\\|~)(?:\s|$)",
        "Attempting to delete system drive or home directory",
    ),
    // Windows: rd (rmdir) with /s /q on drive root
    // e.g., "rd /s /q C:\" or "rd /q /s D:\"
    (
        r"(?i)rd\s+/[sq]\s+/[sq]\s+[A-Z]:\\(?:\s|$)",
        "Attempting to recursively delete drive via rd",
    ),
    // Windows: PowerShell ri alias (alias for Remove-Item) with dangerous flags
    (
        r"(?i)\bri\s+(?:C:\\|~)\s+-(?:Recurse|Force)\s+-(?:Recurse|Force)",
        "Attempting to delete system drive or home directory via ri alias",
    ),
    (
        r"(?i)\bri\s+-(?:Recurse|Force)\s+-(?:Recurse|Force)\s+(?:C:\\|~)(?:\s|$)",
        "Attempting to delete system drive or home directory via ri alias",
    ),
];

/// Command blacklist validator.
///
/// Uses a `RegexSet` for efficient multi-pattern matching in a single pass.
#[derive(Debug, Clone)]
pub struct CommandBlacklist {
    regex_set: RegexSet,
    /// Human-readable reasons for each pattern (parallel to regex_set patterns).
    reasons: Vec<String>,
}

impl CommandBlacklist {
    /// Create a new blacklist from pattern-reason pairs.
    pub fn new(patterns: &[(&str, &str)]) -> Result<Self, ToolError> {
        let mut reasons = Vec::with_capacity(patterns.len());
        let mut pattern_strs = Vec::with_capacity(patterns.len());

        for (pattern, reason) in patterns {
            pattern_strs.push(*pattern);
            reasons.push((*reason).to_string());
        }

        let regex_set = RegexSet::new(&pattern_strs).map_err(|e| ToolError::BadArgs {
            message: format!("Failed to compile blacklist patterns: {e}"),
        })?;

        Ok(Self { regex_set, reasons })
    }

    /// Create a blacklist with default patterns.
    pub fn with_defaults() -> Result<Self, ToolError> {
        Self::new(DEFAULT_PATTERNS)
    }

    /// Validate a command against the blacklist.
    ///
    /// Returns `Ok(())` if allowed, or `Err(ToolError::SandboxViolation)` if blocked.
    pub fn validate(&self, command: &str) -> Result<(), ToolError> {
        let matches: Vec<usize> = self.regex_set.matches(command).iter().collect();
        if let Some(&idx) = matches.first() {
            return Err(ToolError::SandboxViolation(
                DenialReason::CommandBlacklisted {
                    command: truncate_command(command, 100),
                    reason: self.reasons[idx].clone(),
                },
            ));
        }
        Ok(())
    }
}

/// Truncate command for error messages (avoid giant output).
fn truncate_command(cmd: &str, max_len: usize) -> String {
    if cmd.len() <= max_len {
        cmd.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !cmd.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}...", &cmd[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_blacklist() -> CommandBlacklist {
        CommandBlacklist::with_defaults().unwrap()
    }

    #[test]
    fn blocks_rm_rf_root() {
        let bl = default_blacklist();
        assert!(bl.validate("rm -rf /").is_err());
        assert!(bl.validate("rm -r -f /").is_err());
        assert!(bl.validate("rm -fr /").is_err());
        assert!(bl.validate("rm --recursive --force /").is_err());
        assert!(bl.validate("rm --force --recursive /").is_err());
        assert!(bl.validate("rm -rf -- /").is_err());
        assert!(bl.validate("sudo rm -rf /").is_err());
        assert!(bl.validate("rm -rf /*").is_err());
        assert!(bl.validate("rm -r /.*").is_err());
        assert!(bl.validate("rm -r /.").is_err());
        assert!(bl.validate("rm -r /..").is_err());
        assert!(bl.validate("rm -r /./").is_err());
        assert!(bl.validate("rm -r /../").is_err());
        assert!(bl.validate("rm -r /./*").is_err());
        assert!(bl.validate("rm -rf / && echo done").is_err());
        assert!(bl.validate("rm -rf / | tee log").is_err());
    }

    #[test]
    fn blocks_rm_rf_home() {
        let bl = default_blacklist();
        assert!(bl.validate("rm -rf ~").is_err());
        assert!(bl.validate("rm -rf ~/").is_err());
        assert!(bl.validate("rm -rf $HOME").is_err());
        assert!(bl.validate("rm -rf ${HOME}").is_err());
        assert!(bl.validate("rm --recursive --force ~").is_err());
        assert!(bl.validate("rm -r -- $HOME").is_err());
    }

    #[test]
    fn blocks_fork_bomb() {
        let bl = default_blacklist();
        assert!(bl.validate(":(){ :|:& };:").is_err());
    }

    #[test]
    fn blocks_dd_device_overwrite() {
        let bl = default_blacklist();
        assert!(bl.validate("dd if=/dev/zero of=/dev/sda").is_err());
        assert!(bl.validate("dd if=/dev/zero of=/dev/nvme0n1").is_err());
    }

    #[test]
    fn blocks_mkfs() {
        let bl = default_blacklist();
        assert!(bl.validate("mkfs.ext4 /dev/sda1").is_err());
        assert!(bl.validate("mkfs /dev/sda").is_err());
    }

    #[test]
    fn allows_safe_commands() {
        let bl = default_blacklist();
        assert!(bl.validate("ls -la").is_ok());
        assert!(bl.validate("rm -rf ./build").is_ok());
        assert!(bl.validate("rm -rf /tmp/test").is_ok());
        assert!(bl.validate("echo hello").is_ok());
        assert!(bl.validate("cargo build").is_ok());
    }

    #[test]
    fn allows_rm_in_subdirectories() {
        let bl = default_blacklist();
        // These are dangerous but not catastrophic - approval flow handles them
        assert!(bl.validate("rm -rf /var/log/old").is_ok());
        assert!(bl.validate("rm -rf ./node_modules").is_ok());
    }

    #[test]
    fn blocks_windows_remove_item() {
        let bl = default_blacklist();
        // Various parameter orderings
        assert!(bl.validate("Remove-Item -Recurse -Force C:\\").is_err());
        assert!(bl.validate("Remove-Item C:\\ -Recurse -Force").is_err());
        assert!(bl.validate("Remove-Item -Force -Recurse C:\\").is_err());
        assert!(bl.validate("remove-item -recurse -force ~").is_err()); // case insensitive
    }

    #[test]
    fn allows_windows_safe_commands() {
        let bl = default_blacklist();
        assert!(bl.validate("Remove-Item ./temp -Recurse").is_ok()); // no -Force
        assert!(bl.validate("Remove-Item C:\\temp -Force").is_ok()); // no -Recurse
        assert!(bl.validate("Get-ChildItem C:\\").is_ok()); // read-only
    }

    #[test]
    fn blocks_windows_rd_command() {
        let bl = default_blacklist();
        assert!(bl.validate("rd /s /q C:\\").is_err());
        assert!(bl.validate("rd /q /s D:\\").is_err());
        assert!(bl.validate("RD /S /Q C:\\").is_err()); // case insensitive
    }

    #[test]
    fn blocks_windows_ri_alias() {
        let bl = default_blacklist();
        assert!(bl.validate("ri C:\\ -Recurse -Force").is_err());
        assert!(bl.validate("ri -Recurse -Force C:\\").is_err());
        assert!(bl.validate("ri ~ -Force -Recurse").is_err());
        assert!(bl.validate("ri -Force -Recurse ~").is_err());
    }

    #[test]
    fn allows_safe_rd_and_ri() {
        let bl = default_blacklist();
        assert!(bl.validate("rd /s /q C:\\temp").is_ok()); // subdirectory, not root
        assert!(bl.validate("ri ./temp -Recurse -Force").is_ok()); // relative path
    }

    #[test]
    fn blocks_chmod_with_chain() {
        let bl = default_blacklist();
        assert!(bl.validate("chmod -R 777 / && echo done").is_err());
        assert!(bl.validate("chmod -R 000 /; ls").is_err());
        assert!(bl.validate("chmod -R 755 / | tee log").is_err());
    }

    #[test]
    fn truncates_long_commands_in_error() {
        let long_cmd = "x".repeat(200);
        let result = truncate_command(&long_cmd, 100);
        assert_eq!(result.len(), 103); // 100 chars + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn short_commands_not_truncated() {
        let short_cmd = "rm -rf /";
        let result = truncate_command(short_cmd, 100);
        assert_eq!(result, short_cmd);
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // "用户" is 6 bytes (3 bytes per char), truncating at byte 5 would panic without fix
        let cmd = "rm -rf /home/用户/data";
        let result = truncate_command(cmd, 15); // cuts into multi-byte char
        assert!(result.ends_with("..."));
        assert!(result.len() <= 18); // 15 max + "..."
        // Ensure it's valid UTF-8 (would panic on construction if not)
        let _ = result.chars().count();
    }

    #[test]
    fn truncate_at_exact_char_boundary() {
        let cmd = "rm -rf /home/用户";
        // "rm -rf /home/" is 13 bytes, "用" is 3 bytes = 16 bytes total
        let result = truncate_command(cmd, 16);
        assert_eq!(result, "rm -rf /home/用...");
    }

    #[test]
    fn empty_command_allowed() {
        let bl = default_blacklist();
        assert!(bl.validate("").is_ok());
    }

    #[test]
    fn blocks_case_variations_prompt_injection() {
        let bl = default_blacklist();
        // Weird casing suggests prompt injection attempting to bypass filters
        assert!(bl.validate("RM -RF /").is_err());
        assert!(bl.validate("Rm -Rf /").is_err());
        assert!(bl.validate("DD if=/dev/zero OF=/dev/sda").is_err());
        assert!(bl.validate("MKFS.EXT4 /dev/sda1").is_err());
        assert!(bl.validate("CHMOD -R 777 /").is_err());
        assert!(bl.validate("SUDO RM -RF /").is_err());
    }

    #[test]
    fn truncate_empty_command() {
        let result = truncate_command("", 100);
        assert_eq!(result, "");
    }

    #[test]
    fn truncate_at_exact_limit() {
        let cmd = "x".repeat(100);
        let result = truncate_command(&cmd, 100);
        assert_eq!(result, cmd); // No truncation needed
    }

    #[test]
    fn truncate_one_over_limit() {
        let cmd = "x".repeat(101);
        let result = truncate_command(&cmd, 100);
        assert_eq!(result.len(), 103); // 100 + "..."
        assert!(result.ends_with("..."));
    }

    #[test]
    fn truncate_zero_limit() {
        let result = truncate_command("rm -rf /", 0);
        assert_eq!(result, "...");
    }
}
