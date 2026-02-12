# Phase 3: Profiles

**Goal**: Named configuration bundles with quick-switcher.

## Features

- Profile CRUD (create, edit, duplicate, delete)
- Quick-switcher via `Ctrl+p`
- Profiles stored in `~/.forge/profiles/` as separate TOML files

## UI Mockup

### Profile List

```
┌─ Settings › Profiles ─────────────────────────────────────────────┐
│                                                                    │
│  ● deep-work                                           active      │
│    Chat: claude-opus-4-5  Context: 256k                            │
│    Tools: all enabled, sandboxed                                   │
│                                                                    │
│  ○ quick                                                           │
│    Chat: claude-haiku-4-5  Context: 32k                            │
│    Tools: read, write only                                         │
│                                                                    │
│  ○ research                                                        │
│    Chat: gpt-5.2-pro  Context: 128k                                │
│    Tools: fetch enabled, browser enabled                           │
│                                                                    │
│  ○ unsafe                                                  ⚠       │
│    Chat: claude-opus-4-5  Context: 256k                            │
│    Tools: all enabled, NO SANDBOX                                  │
│                                                                    │
│  Enter activate   e edit   d duplicate   n new   x delete  q back  │
└────────────────────────────────────────────────────────────────────┘
```

### Quick-Switcher (`Ctrl+p`)

```
┌─ Switch Profile ──────────────────────────────────────────────────┐
│                                                                    │
│  1.  ● deep-work       opus-4-5 · 256k · sandboxed                 │
│  2.  ○ quick           haiku · 32k · minimal                       │
│  3.  ○ research        gpt-5.2-pro · 128k · browser                │
│  4.  ○ unsafe          opus-4-5 · 256k · ⚠ no sandbox              │
│                                                                    │
│  1-9 quick pick   Enter confirm   Esc cancel                       │
└────────────────────────────────────────────────────────────────────┘
```

## Profile Schema

```toml
# ~/.forge/profiles/deep-work.toml
[profile]
name = "deep-work"
description = "Long-form coding sessions"

[app]
model = "claude-opus-4-5-20251101"

[context]
memory = true

[anthropic]
cache_enabled = true
thinking_mode = "adaptive"
thinking_effort = "high"

[tools]
sandbox = true
max_tool_calls_per_batch = 8
```

## Implementation Notes

- Profiles override base `config.toml` settings
- Active profile stored in `~/.forge/state.toml` or similar
- Profile switch = reload relevant App fields + persist active profile name
