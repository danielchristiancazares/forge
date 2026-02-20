//! Shell detection for command execution.

use std::path::PathBuf;

/// Detected shell for command execution.
#[derive(Debug, Clone)]
pub struct DetectedShell {
    /// Path or name of the shell binary.
    pub binary: PathBuf,
    /// Arguments to pass before the command (e.g., `["-c"]` or `["/C"]`).
    pub args: Vec<String>,
    /// Human-readable name for logging.
    pub name: String,
}

impl std::fmt::Display for DetectedShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

/// Detect the best available shell via platform-specific probing.
#[must_use]
pub fn detect_shell() -> DetectedShell {
    detect_platform_shell()
}

#[cfg(windows)]
fn detect_platform_shell() -> DetectedShell {
    if let Ok(path) = which::which("pwsh") {
        return DetectedShell {
            binary: path,
            args: vec!["-NoProfile".to_string(), "-Command".to_string()],
            name: "pwsh".into(),
        };
    }

    if let Ok(path) = which::which("powershell") {
        return DetectedShell {
            binary: path,
            args: vec!["-NoProfile".to_string(), "-Command".to_string()],
            name: "powershell".into(),
        };
    }

    let comspec = std::env::var("ComSpec")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\System32\cmd.exe"));
    DetectedShell {
        binary: comspec,
        args: vec!["/C".to_string()],
        name: "cmd".into(),
    }
}

#[cfg(not(windows))]
fn detect_platform_shell() -> DetectedShell {
    if let Ok(shell) = std::env::var("SHELL") {
        let path = std::path::Path::new(&shell);
        if path.exists() {
            let name = path
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("user-shell")
                .to_string();
            return DetectedShell {
                binary: PathBuf::from(&shell),
                args: vec!["-c".to_string()],
                name,
            };
        }
    }

    if let Ok(path) = which::which("bash") {
        return DetectedShell {
            binary: path,
            args: vec!["-c".to_string()],
            name: "bash".into(),
        };
    }

    DetectedShell {
        binary: PathBuf::from("/bin/sh"),
        args: vec!["-c".to_string()],
        name: "sh".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::detect_shell;

    #[test]
    fn detect_shell_returns_valid_result() {
        let shell = detect_shell();
        assert!(!shell.binary.as_os_str().is_empty());
        assert!(!shell.args.is_empty());
        assert!(!shell.name.is_empty());
    }
}
