# Forge Scripts

This directory contains utility scripts for development, testing, and maintenance of the Forge project.

## LLM-TOC
<!-- Auto-generated section map for LLM context -->
| Lines | Section |
|-------|---------|
| 1-19 | Header, Intro, LLM-TOC, Table of Contents |
| 20-26 | Available Scripts |
| 27-57 | Usage |

## Table of Contents

1. [Available Scripts](#available-scripts)
2. [Usage](#usage)

---

## Available Scripts

| Script | Language | Description |
| ------ | -------- | ----------- |
| `coverage.ps1` | PowerShell | Generates a code coverage report for the entire workspace |
| `toc-gen` | Rust (CLI) | Updates LLM-TOC line ranges and section summaries |

## Usage

### Coverage Report

To generate a coverage report (requires `cargo-llvm-cov` and `llvm-tools-preview`):

```powershell
./scripts/coverage.ps1
```

The report will be generated in `lcov.info` and can be viewed using various coverage tools or IDE extensions.

### LLM-TOC Updates

Update a single README's LLM-TOC:

```powershell
cargo run --manifest-path scripts/toc-gen/Cargo.toml -- update README.md
```

Generate new section descriptions (uses the LLM-backed feature):

```powershell
cargo run --manifest-path scripts/toc-gen/Cargo.toml --features generate -- update README.md --generate
```

Check whether a README's TOC is current:

```powershell
cargo run --manifest-path scripts/toc-gen/Cargo.toml -- check README.md
```
