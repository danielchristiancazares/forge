//! Boundary runtime: concurrency, IO, timers, and external side effects.
//!
//! Phase 1 scaffold (Invariant First Architecture): this module is introduced as an
//! internal authority boundary. Subsequent phases will move async orchestration,
//! retries, and IO drivers under `crate::runtime`.
