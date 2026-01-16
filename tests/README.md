# Forge Integration Tests

This directory contains integration tests that verify the behavior of the Forge application as a whole, focusing on the interaction between multiple crates.

## Test Structure

| File | Description |
| ---- | ----------- |
| `integration_test.rs` | General integration tests for engine and TUI workflows |

## Running Tests

Integration tests can be run using Cargo:

```bash
cargo test --test integration_test
```

Or run all tests in the workspace:

```bash
cargo test
```

## Writing Integration Tests

Integration tests should focus on:

- Cross-crate communication
- End-to-end command execution
- Complex state transitions in the engine
- Persistence and recovery scenarios
