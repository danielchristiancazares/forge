//! Core, deterministic state machines and invariant-carrying domain types.
//!
//! Phase 1 scaffold (Invariant First Architecture): this module is introduced as an
//! internal authority boundary. Subsequent phases will move deterministic state
//! transitions and proof types under `crate::core` and restrict visibility with
//! `pub(in crate::core)`.

pub(crate) mod thinking;
