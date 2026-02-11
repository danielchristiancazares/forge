//! LSP client for consuming language server diagnostics.

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
