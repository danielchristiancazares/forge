# Forge Integration Tests

This directory contains integration tests that verify the behavior of the Forge application as a whole, focusing on the interaction between multiple crates.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-21 | Header, Intro, LLM-TOC, Table of Contents |
| 22-26 | Test Structure |
| 28-40 | Running Tests |
| 42-49 | Writing Integration Tests |

## Table of Contents

1. [Test Structure](#test-structure)
2. [Running Tests](#running-tests)
3. [Writing Integration Tests](#writing-integration-tests)

---

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
