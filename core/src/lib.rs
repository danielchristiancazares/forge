//! Core domain logic for Forge.
//!
//! This crate contains standalone modules extracted from the engine:
//! environment context, system notifications, error formatting,
//! display types, and utility functions.
//!
//! Future phases will move the full App state machine here.

mod display;
pub mod environment;
pub mod errors;
pub mod notifications;
mod security;
pub mod thinking;
mod util;

pub use display::{DisplayItem, DisplayLog, DisplayPop, DisplayTail};
pub use environment::{EnvironmentContext, assemble_prompt};
pub use notifications::{NotificationQueue, SystemNotification};
pub use security::sanitize_display_text;
pub use util::{parse_model_name_from_string, wrap_api_key};
