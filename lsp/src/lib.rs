//! LSP client for consuming language server diagnostics.

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
    DiagnosticSeverity, DiagnosticsSnapshot, ForgeDiagnostic, LspConfig, LspEvent, ServerConfig,
    ServerStopReason,
};
