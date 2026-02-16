# Security Policy

## Reporting a Vulnerability

Email **<forge@danielcazares.com>** with:

- Affected component(s) and version or commit hash
- Impact assessment
- Reproduction steps or minimal PoC
- Environment (OS, shell, terminal)

Do not include credentials or secrets in reports. Do not open public issues containing exploit details.

## Response Timeline

- Acknowledge: within 72 hours
- Triage: within 7 days
- Fix or mitigation plan: within 30 days (best effort)
- Coordinated disclosure: after fix is available, typically within 90 days

## Supported Versions

- **`main` branch**: supported
- **Latest tagged release**: supported
- Older releases: best-effort only

## Handling Secrets

- Provide API keys via environment variables, not hardcoded values.
- Prefer `${ENV_VAR}` expansion in `~/.forge/config.toml` over literal secrets.
- If you suspect a key was exposed, rotate it immediately at the provider.

## Security Architecture

Forge's defense-in-depth sanitization infrastructure (terminal escape stripping, steganographic character removal, API key redaction, tool sandboxing, SSRF mitigations) is documented in [`docs/SECURITY_SANITIZATION.md`](docs/SECURITY_SANITIZATION.md).

## Credit

If you want credit, include the name or handle you'd like used in release notes.
