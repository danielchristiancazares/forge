# Phase 5: Full Architecture

**Goal**: Complete implementation of the settings architecture after Phases 1-4 land.

## Remaining Categories

### Models Panel

```
┌─ Settings › Models ───────────────────────────────────────────────┐
│                                                                    │
│  ANTHROPIC                                                         │
│  ─────────────────────────────────────────────────────────────     │
│  ▸ claude-opus-4-5-20251101      ★ default  200k       $15/$75     │
│    claude-opus-4-6                          1M         $15/$75     │
│    claude-sonnet-4-5-20250929               200k       $3/$15      │
│                                                                    │
│  OPENAI                                                            │
│  ─────────────────────────────────────────────────────────────     │
│    gpt-5.2                                  128k       $2/$10      │
│                                                                    │
│  Enter set default   i model info   t test   q back                │
└────────────────────────────────────────────────────────────────────┘
```

### Keybindings Panel

```
┌─ Settings › Keybindings ──────────────────────────────────────────┐
│                                                                    │
│  Preset                                         ▸ vim              │
│                                                   emacs            │
│                                                   minimal          │
│                                                                    │
│  Navigation                                                        │
│  ─────────────────────────────────────────────────────────────     │
│  Insert mode                                    i                  │
│  Normal mode                                    Esc                │
│  Command palette                                /                  │
│                                                                    │
│  Enter edit binding   r reset to preset   q back                   │
└────────────────────────────────────────────────────────────────────┘
```

### History Panel

```
┌─ Settings › History ──────────────────────────────────────────────┐
│                                                                    │
│  Retention                                                         │
│  ─────────────────────────────────────────────────────────────     │
│  Keep conversations for                         30 days            │
│  Keep journal entries for                       7 days             │
│  Max storage                                    500 MB             │
│                                                                    │
│  Storage: ~/.forge/history    Using: 127 MB of 500 MB              │
│                                                                    │
│  c clear all history   x export history   q back                   │
└────────────────────────────────────────────────────────────────────┘
```

### Context Panel (Enhanced)

```
┌─ Settings › Context ──────────────────────────────────────────────┐
│                                                                    │
│  Context Management                                                │
│  ─────────────────────────────────────────────────────────────     │
│  Default Limit                              200k tokens            │
│  Distill Threshold                          80% capacity           │
│  Distill Strategy                           summarize              │
│                                                                    │
│  Auto-Attach                                                       │
│  ─────────────────────────────────────────────────────────────     │
│  CLAUDE.md                                  ✓ always               │
│  AGENTS.md                                  ✓ always               │
│                                                                    │
│  + Add auto-attach file...                                         │
│                                                                    │
│  ↑↓ navigate   Enter toggle   a add   q back                       │
└────────────────────────────────────────────────────────────────────┘
```

## Additional Features

- **Search/filter** — Type to filter settings across all categories
- **Model pricing display** — Show input/output costs
- **Connection testing** — `t` to test API connectivity
- **Effective resolution** — Show which layer (profile/config/default) a value came from
- **Validation inline** — Show errors immediately when editing

## Design Principles

1. **Fail Fast** — No silent fallbacks. Errors are visible.
2. **Explicit Provider Binding** — Model always belongs to a provider.
3. **Validation First-Class** — Config panels show validation status inline.
4. **Effective Resolution Visible** — Show what resolved and why.
5. **Turn Snapshot Immutability** — Edits apply to next turn, never mid-turn.
6. **Observable** — Config hash, runtime panel, clear state display.
