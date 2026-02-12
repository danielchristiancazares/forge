//! Security utilities for sanitization and redaction.
//!
//! The engine intentionally re-uses the canonical implementation from `forge-tools`
//! to avoid duplicated invariant encoding and drift between UI surfaces.

pub use forge_tools::security::{sanitize_display_text, sanitize_stream_error};
