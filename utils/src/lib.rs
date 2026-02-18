//! Shared infrastructure utilities for Forge.
//!
//! This crate provides cross-cutting utilities that multiple Forge crates need
//! but that don't belong in the domain-pure `forge-types` crate:
//!
//! - **`atomic_write`**: Crash-safe file persistence (temp + rename)
//! - **`security`**: Secret redaction and sanitization for display
//! - **`diff`**: Unified diff formatting and stats

pub mod atomic_write;
pub mod diff;
pub mod security;

pub use atomic_write::{
    AtomicWriteOptions, atomic_write, atomic_write_new_with_options, atomic_write_with_options,
};
pub use diff::{compute_diff_stats, format_unified_diff, format_unified_diff_width};
pub use security::{sanitize_display_text, sanitize_stream_error};
