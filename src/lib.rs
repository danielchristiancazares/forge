//! Forge - A TUI for interacting with GPT and Claude
//!
//! This library exposes core types for testing.
//! The binary entry point is in main.rs.

pub mod markdown;
pub mod message;
pub mod provider;

pub use context_infinity::{ModelLimits, ModelRegistry, TokenCounter};

// Internal modules (not exposed for testing)
mod app;
mod config;
mod context_infinity;
mod input;
mod theme;
mod ui;
mod ui_inline;
