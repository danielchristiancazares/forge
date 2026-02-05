# CI Runbook

## Failure Classification

| Signature | Category | Action |
|-----------|----------|--------|
| Setup/network timeout (e.g., downloading action from `api.github.com:443`) | Infra flake | Manual rerun once |
| Compiler, lint, or test failure | Code failure | Fix required |

## Manual Rerun

Rerun only the failed jobs:

```sh
gh run rerun <run-id> --failed
```

## Job Topology

- **lint** (ubuntu): format + clippy
- **msrv** (ubuntu): `cargo check` at minimum supported Rust version
- **test** (ubuntu, macos): `cargo test --workspace`
- **test-windows** (windows): `cargo test --workspace` (single dedicated job to reduce setup flake surface)
- **coverage** (ubuntu): lcov + Codecov upload
