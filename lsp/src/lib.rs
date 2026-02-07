//! LSP client for consuming language server diagnostics.
//!
//! This crate implements a minimal LSP client focused on `publishDiagnostics`.
//! It spawns language servers (e.g. rust-analyzer) as child processes, communicates
//! via JSON-RPC over stdio, and surfaces diagnostics to the engine.
//!
//! # Architecture
//!
//! - [`LspManager`] is the public facade consumed by the engine
//! - [`ServerHandle`](server::ServerHandle) owns one child process per language server
//! - [`DiagnosticsStore`](diagnostics::DiagnosticsStore) accumulates per-file diagnostics
//! - [`codec`] handles JSON-RPC framing (`Content-Length: N\r\n\r\n{json}`)
//! - [`protocol`] defines internal LSP message serde types
//!
//! # Usage
//!
//! The engine:
//! 1. Constructs `LspManager::new(config)` at init
//! 2. Calls `ensure_started(workspace_root)` to spawn servers
//! 3. Calls `on_file_changed(path, text)` after tool batches modify files
//! 4. Calls `poll_events(budget)` each tick to drain diagnostics
//! 5. Reads `snapshot()` for UI display and agent feedback

// Pedantic lint configuration
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

pub mod codec;
pub mod types;

pub(crate) mod diagnostics;
pub(crate) mod protocol;
pub(crate) mod server;

mod manager;

pub use manager::LspManager;
pub use types::{
    DiagnosticSeverity, DiagnosticsSnapshot, ForgeDiagnostic, LspConfig, LspEvent, LspState,
    ServerConfig,
};
