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

## Action Pinning Policy

- All third-party GitHub Actions in `.github/workflows/*.yml` must be pinned to immutable commit SHAs.
- Keep the upstream tag as an inline comment (for example `# v4`) to preserve readability.
- Never switch back to floating tags (`@v*`) in workflow edits.

## Updating Pinned Actions

1. Resolve the tag to its commit SHA:

```sh
gh api repos/<owner>/<repo>/git/ref/tags/<tag>
```

2. If the ref points to an annotated tag (`"type":"tag"`), resolve one more hop:

```sh
gh api repos/<owner>/<repo>/git/tags/<tag-object-sha>
```

3. Update workflow `uses:` entries to the resolved commit SHA and run CI in a PR.
