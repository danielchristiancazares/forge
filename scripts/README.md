# Forge Scripts

This directory contains utility scripts for development, testing, and maintenance of the Forge project.

## Available Scripts

| Script | Language | Description |
| ------ | -------- | ----------- |
| `coverage.ps1` | PowerShell | Generates a code coverage report for the entire workspace |

## Usage

### Coverage Report

To generate a coverage report (requires `cargo-llvm-cov` and `llvm-tools-preview`):

```powershell
./scripts/coverage.ps1
```

The report will be generated in `lcov.info` and can be viewed using various coverage tools or IDE extensions.
