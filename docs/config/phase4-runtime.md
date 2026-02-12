# Phase 4: Runtime Panel

**Goal**: Separate panel showing live session state, distinct from config.

## Rationale

Config = what's configured. Runtime = what's happening now.

This separation aids debugging and makes state observable.

## UI Mockup

```
┌─ Runtime ─────────────────────────────────────────────────────────┐
│                                                                    │
│  Session                                                           │
│  ─────────────────────────────────────────────────────────────     │
│  Active Model             claude-opus-4-5-20251101                 │
│  Provider                 Anthropic                                │
│  Context Used             38% (28k / 200k tokens)                  │
│  Streaming                Enabled                                  │
│  Tool Mode                Approval Required                        │
│                                                                    │
│  Health                                                            │
│  ─────────────────────────────────────────────────────────────     │
│  Last Error               None                                     │
│  Rate Limit State         Healthy                                  │
│  API Latency (p50)        320ms                                    │
│                                                                    │
│  Session Overrides                                                 │
│  ─────────────────────────────────────────────────────────────     │
│  Temperature              0.0 (code mode)                          │
│  Model                    gpt-5.3-codex (code mode)                │
│                                                                    │
│  Config Hash              8f3a21c                                  │
│                                                                    │
│  r refresh   Esc close                                             │
└────────────────────────────────────────────────────────────────────┘
```

## Access

Either:
- Separate `/runtime` command
- Tab within `/config` modal (Status | Config)

## Data Sources

| Field | Source |
|-------|--------|
| Active Model | `self.model` |
| Provider | `self.provider` |
| Context Used | `self.context_manager.usage()` |
| Rate Limit | Track 429 responses, cooldown state |
| Config Hash | SHA256 of effective config (for reproducibility) |

## Config Hash

Enables reproducible bug reports:

```
"Config hash: 8f3a21c, context at 38%, model opus-4-5"
```

If two users have the same hash, they have identical configurations.
