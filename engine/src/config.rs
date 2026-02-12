//! Compatibility shim for config types/functions now owned by `forge-config`.
//!
//! Keeping this module preserves existing `crate::config::*` call sites while
//! moving implementation ownership into a dedicated crate.

pub use forge_config::*;
