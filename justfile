# Forge development commands

# Default recipe: list available commands
default:
    @just --list

# Fast type-check (use during development)
check:
    cargo check

# Debug build
build:
    cargo build

# Release build
release:
    cargo build --release

# Run tests
test:
    cargo test

# Run clippy lints
lint:
    cargo clippy -- -D warnings

# Format code
fmt:
    cargo fmt

# Check formatting without modifying
fmt-check:
    cargo fmt -- --check

# Coverage report (requires cargo-llvm-cov)
cov:
    cargo cov

# Run all checks before committing
pre-commit: fmt-check lint test

# Create source zip for bug analysis (excludes build artifacts)
[windows]
zip:
    pwsh -NoProfile -Command "Get-ChildItem -Path . -Exclude 'target','.git','*.zip','lcov.info','coverage','.env*','sha256.txt' | Compress-Archive -DestinationPath forge-source.zip -Force"

[unix]
zip:
    zip -r forge-source.zip . -x 'target/*' -x '.git/*' -x '*.zip' -x 'lcov.info' -x 'coverage/*' -x '.env*' -x 'sha256.txt'

# Clean build artifacts
clean:
    cargo clean
