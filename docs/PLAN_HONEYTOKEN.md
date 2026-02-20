# Honeytoken Canary Plan

**Status:** Draft v1
**Author:** Codex
**Date:** 2026-02-18

---

## Problem Statement

Agentic workflows can be prompt-injected by untrusted content (README files, pasted logs, web pages, etc.). A malicious instruction may try to coerce the agent into reading secrets or exfiltrating data through tools. Full content scanning is expensive and brittle. We need a low-latency, low-overhead tripwire that detects suspicious behavior at execution time.

## Goals

1. Detect likely prompt-injection-driven secret access attempts with minimal runtime overhead.
2. Abort dangerous execution quickly when the agent touches a protected canary path.
3. Preserve forensic evidence (who, when, what command/tool, which source context).
4. Keep normal UX unchanged unless an alert fires.
5. Minimize false positives while still catching clear compromise patterns.

## Non-Goals

1. Replace existing policy/sandbox controls.
2. Perfectly classify attacker intent.
3. Block all possible exfiltration channels in the first iteration.
4. Scan every token of every prompt for malicious text.

## Threat Model Snapshot

- **Attacker capability:** Injects text into the model context that instructs secret discovery/exfiltration.
- **Target behavior:** Agent attempts to access fake sensitive paths (for example `.env.production`, `id_rsa_backup`).
- **Detection principle:** If the agent asks tools to read or transmit synthetic secret artifacts that do not exist, treat as high-risk compromise signal.

## Design Overview

### 1) Canary Artifact Generation

- Generate canary names per session (randomized, not static).
- Keep a small set of realistic secret-like paths:
  - project-local: `.env.production`, `.env.secrets`
  - user-level: `.ssh/id_rsa_backup`, `credentials-export.csv`
- Store only metadata in memory/state; do not create these files on disk.

### 2) Context Injection

- Inject canary references into agent-visible context as "sensitive paths" metadata.
- Injection must be invisible to user-facing TUI output by default.
- Rotate names each session to prevent hardcoded bypasses.

### 3) Tripwire Detection

Trigger when tools attempt any canary interaction:

- **Read attempts:** `cat`, `type`, `Get-Content`, file-read tool calls.
- **Copy/package attempts:** archive, copy, move, or encode actions on canary paths.
- **Exfil hints:** `curl`, `Invoke-WebRequest`, upload tools, clipboard export, stdout dumps.

Rule severity:

- **Critical:** direct canary read + network/upload action in same step/batch.
- **High:** direct canary read/copy attempt.
- **Medium:** command string includes canary token but no execution reached.

### 4) Response Policy

- **Medium:** block action, warn, continue with strict watch.
- **High:** block action, terminate current agent run, display security alert.
- **Critical:** immediate termination, lock tool execution for session, require explicit user unlock.

Default for v1: terminate on High or Critical.

### 5) Forensics and Audit

Capture structured event before termination:

- session id, agent id, timestamp
- canary id/path alias
- triggering tool name and arguments
- recent prompt window hash/reference
- classification (Medium/High/Critical)
- action taken (blocked/terminated/locked)

Persist to security audit log and show an incident summary in UI.

## UX Plan

Normal behavior: no visible canary indicators.

On trigger, show modal/banner:

> Security Alert: Agent attempted to access a protected honeytoken path. Execution terminated.

Optional details drawer:

- Triggered tool
- Time
- Risk level
- Incident id

## Implementation Plan

### Phase 1: Policy + Data Model

1. Add honeytoken policy types (enabled, strictness, kill-threshold).
2. Add runtime canary session state (generated names, ids, timestamps).
3. Add config flags for opt-in/opt-out and severity behavior.

Likely touch points:

- `config/` crate for settings schema
- `engine/src/state.rs` for runtime session state
- `types/` if shared event types are needed

### Phase 2: Injection Pipeline

1. Generate session canaries at conversation start.
2. Inject canary metadata into model/system context in a non-user-visible channel.
3. Ensure distillation/compaction preserves detection coherence without leaking raw canary map.

Likely touch points:

- `engine/src/streaming.rs`
- `context/` prompt assembly code

### Phase 3: Tool Interception + Detection

1. Add pre-execution inspection in tool orchestration.
2. Normalize path variants (slash style, quoting, env-expansion where possible).
3. Match canary tokens against command/tool args.
4. Classify severity and emit incident event.

Likely touch points:

- `engine/src/tool_loop.rs`
- `tools/` executor boundary where arguments are available

### Phase 4: Enforcement + UI

1. Implement terminate/lock behavior on configured severity.
2. Surface alert modal/banner in TUI.
3. Add incident summary to tool/event display.

Likely touch points:

- `engine/src/ui/modal.rs`
- `tui/src/lib.rs`
- `tui/src/tool_display.rs`

### Phase 5: Telemetry + Hardening

1. Add structured audit log sink.
2. Add redaction pass for sensitive payloads in incident logs.
3. Add rate-limits/debounce so repeated triggers do not flood UI/logging.

## Testing Strategy

1. Unit tests: canary generation uniqueness and stable matching behavior.
2. Unit tests: severity classifier (Medium/High/Critical) for command/tool patterns.
3. Integration tests: injected malicious prompt causes canary access attempt and forced termination.
4. Integration tests: benign workflows do not trigger tripwires.
5. Regression tests: cross-platform path handling (Windows and POSIX path spellings).

## Rollout Plan

1. Ship behind feature flag (`security.honeytoken_enabled`).
2. Enable in internal builds first.
3. Monitor incident rates and false positives.
4. Promote to default-on after stability threshold is met.

## Success Metrics

1. Detection latency from tool submit to termination under 100 ms (target).
2. False positive rate under agreed threshold (for example <1% of sessions).
3. Zero silent canary access attempts (all must emit audited incidents).

## Open Questions

1. Should first trigger always terminate, or should there be a warning-only mode for local dev?
2. Should canary names be model-visible plain text, or obfuscated labels resolved by runtime?
3. Should session unlock require explicit command, full restart, or user approval modal?
4. What incident retention policy and redaction defaults are required?
