# forge

A custom TUI for interacting with GPT and Claude.

## Development

- Build: `cargo build`
- Run: `cargo run`

## Coverage (LCOV)

This project uses `cargo-llvm-cov` to generate an LCOV report.

One-time setup:

- `cargo install cargo-llvm-cov`
- `rustup component add llvm-tools-preview`

Generate `lcov.info`:

- `cargo cov`
- or `./scripts/coverage.ps1`
