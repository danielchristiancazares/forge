//! Configuration types used by tool executors.
//!
//! These were extracted from `forge-engine::config` to break the
//! circular dependency between tools and the engine.

use serde::Deserialize;

/// Serde helper for fields that default to `true`.
#[must_use]
pub const fn default_true() -> bool {
    true
}

/// Shell configuration for command execution.
///
/// ```toml
/// [tools.shell]
/// binary = "pwsh"
/// args = ["-NoProfile", "-Command"]
/// ```
#[derive(Debug, Default, Deserialize)]
pub struct ShellConfig {
    /// Override shell binary (e.g., "pwsh", "bash", "/usr/local/bin/fish").
    pub binary: Option<String>,
    /// Override shell args (e.g., `["-c"]` or `["/C"]`).
    pub args: Option<Vec<String>>,
}
