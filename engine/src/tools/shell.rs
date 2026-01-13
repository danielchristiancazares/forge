//! Shell detection and configuration for command execution.

use std::path::PathBuf;

use crate::config::ShellConfig;

/// Detected shell for command execution.
#[derive(Debug, Clone)]
pub struct DetectedShell {
    /// Path or name of the shell binary.
    pub binary: PathBuf,
    /// Arguments to pass before the command (e.g., `["-c"]` or `["/C"]`).
    pub args: Vec<String>,
    /// Human-readable name for logging.
    pub name: &'static str,
}

impl std::fmt::Display for DetectedShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Detect the best available shell based on config and platform.
///
/// Priority:
/// - Config override (if set)
/// - Platform-specific detection
pub fn detect_shell(config: Option<&ShellConfig>) -> DetectedShell {
    // 1. Check config override
    if let Some(cfg) = config
        && let Some(binary) = &cfg.binary
    {
        let args = cfg
            .args
            .clone()
            .unwrap_or_else(|| default_args_for(binary));
        return DetectedShell {
            binary: PathBuf::from(binary),
            args,
            name: "configured",
        };
    }

    // 2. Platform-specific detection
    detect_platform_shell()
}

/// Infer default args for a shell binary name.
fn default_args_for(binary: &str) -> Vec<String> {
    let name = std::path::Path::new(binary)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(binary)
        .to_lowercase();

    match name.as_str() {
        "cmd" | "cmd.exe" => vec!["/C".to_string()],
        "pwsh" | "pwsh.exe" | "powershell" | "powershell.exe" => {
            vec!["-NoProfile".to_string(), "-Command".to_string()]
        }
        // Most Unix shells use -c
        _ => vec!["-c".to_string()],
    }
}

#[cfg(windows)]
fn detect_platform_shell() -> DetectedShell {
    // Try pwsh.exe first (PowerShell 7+)
    if which::which("pwsh").is_ok() {
        return DetectedShell {
            binary: "pwsh".into(),
            args: vec!["-NoProfile".to_string(), "-Command".to_string()],
            name: "pwsh",
        };
    }

    // Fall back to powershell.exe (5.1, always available on modern Windows)
    if which::which("powershell").is_ok() {
        return DetectedShell {
            binary: "powershell".into(),
            args: vec!["-NoProfile".to_string(), "-Command".to_string()],
            name: "powershell",
        };
    }

    // Last resort: cmd.exe
    DetectedShell {
        binary: "cmd.exe".into(),
        args: vec!["/C".to_string()],
        name: "cmd",
    }
}

#[cfg(not(windows))]
fn detect_platform_shell() -> DetectedShell {
    // Try $SHELL first (user's preferred shell)
    if let Ok(shell) = std::env::var("SHELL") {
        let path = std::path::Path::new(&shell);
        if path.exists() {
            // Extract shell name for logging
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("user-shell");
            // Leak the string to get a &'static str (acceptable for startup-only code)
            let name: &'static str = Box::leak(name.to_string().into_boxed_str());
            return DetectedShell {
                binary: PathBuf::from(&shell),
                args: vec!["-c".to_string()],
                name,
            };
        }
    }

    // Try bash
    if which::which("bash").is_ok() {
        return DetectedShell {
            binary: "bash".into(),
            args: vec!["-c".to_string()],
            name: "bash",
        };
    }

    // Fallback to sh (POSIX)
    DetectedShell {
        binary: "sh".into(),
        args: vec!["-c".to_string()],
        name: "sh",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_args_for_cmd() {
        assert_eq!(default_args_for("cmd"), vec!["/C"]);
        assert_eq!(default_args_for("cmd.exe"), vec!["/C"]);
        assert_eq!(default_args_for("C:\\Windows\\System32\\cmd.exe"), vec!["/C"]);
    }

    #[test]
    fn test_default_args_for_powershell() {
        assert_eq!(
            default_args_for("pwsh"),
            vec!["-NoProfile", "-Command"]
        );
        assert_eq!(
            default_args_for("powershell"),
            vec!["-NoProfile", "-Command"]
        );
        assert_eq!(
            default_args_for("powershell.exe"),
            vec!["-NoProfile", "-Command"]
        );
    }

    #[test]
    fn test_default_args_for_unix_shells() {
        assert_eq!(default_args_for("sh"), vec!["-c"]);
        assert_eq!(default_args_for("bash"), vec!["-c"]);
        assert_eq!(default_args_for("zsh"), vec!["-c"]);
        assert_eq!(default_args_for("/bin/bash"), vec!["-c"]);
        assert_eq!(default_args_for("/usr/local/bin/fish"), vec!["-c"]);
    }

    #[test]
    fn test_config_override() {
        let config = ShellConfig {
            binary: Some("fish".to_string()),
            args: Some(vec!["-c".to_string()]),
        };
        let shell = detect_shell(Some(&config));
        assert_eq!(shell.binary, PathBuf::from("fish"));
        assert_eq!(shell.args, vec!["-c"]);
        assert_eq!(shell.name, "configured");
    }

    #[test]
    fn test_config_override_infers_args() {
        let config = ShellConfig {
            binary: Some("pwsh".to_string()),
            args: None,
        };
        let shell = detect_shell(Some(&config));
        assert_eq!(shell.binary, PathBuf::from("pwsh"));
        assert_eq!(shell.args, vec!["-NoProfile", "-Command"]);
    }

    #[test]
    fn test_detect_shell_no_config() {
        // Should return some shell (platform-dependent)
        let shell = detect_shell(None);
        assert!(!shell.binary.as_os_str().is_empty());
        assert!(!shell.args.is_empty());
        assert!(!shell.name.is_empty());
    }
}
