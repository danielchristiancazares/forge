# Phase 1: Foundation

**Goal**: Working `/config` command that opens a modal with category navigation and read-only display of current settings.

## Files to Modify

| File | Changes |
|------|---------|
| `engine/src/commands.rs` | Add `Config` to `CommandKind`, `Command`, `COMMAND_SPECS`, `COMMAND_ALIASES` |
| `engine/src/ui/modal.rs` | Add `SettingsModal` state, `SettingsCategory` enum |
| `engine/src/lib.rs` | Handle `Command::Config` → open settings modal |
| `tui/src/lib.rs` | Add `draw_settings_modal()` renderer |
| `tui/src/input.rs` | Handle keyboard nav in settings modal (↑↓, Enter, Esc, q) |

## UI Mockup

```
┌─ Settings ────────────────────────────────────────────────────────┐
│  ▸ Provider          Claude (connected)                           │
│    Model             claude-opus-4-5-20251101                      │
│    Context           Memory: on │ Cache: on                        │
│    Thinking          adaptive / medium                             │
│    Tools             8 max/batch │ sandbox: on                     │
│    Appearance        high_contrast: off │ ascii: off               │
│                                                                    │
│  Config: ~/.forge/config.toml                                      │
│                                                                    │
│  ↑↓ navigate   Enter view details   q close                        │
└────────────────────────────────────────────────────────────────────┘
```

## Data Sources

| Category | App Fields |
|----------|------------|
| Provider | `self.provider`, `self.api_keys` (key present/missing) |
| Model | `self.model` |
| Context | `self.memory_enabled`, `self.cache_enabled` |
| Thinking | `self.anthropic_thinking_mode`, `self.anthropic_thinking_effort`, `self.gemini_thinking_enabled` |
| Tools | `self.tool_settings` (max_calls, sandbox, timeouts) |
| Appearance | `self.view.ui_options` (ascii_only, high_contrast, reduced_motion, show_thinking) |

## New Types

```rust
// engine/src/ui/modal.rs

pub enum SettingsCategory {
    Provider,
    Model,
    Context,
    Thinking,
    Tools,
    Appearance,
}

pub struct SettingsModalState {
    pub selected: usize,
    pub categories: Vec<SettingsCategory>,
    pub detail_view: Option<SettingsCategory>,
}
```

## Keyboard Handling

| Key | Action |
|-----|--------|
| `↑/k` | Move selection up |
| `↓/j` | Move selection down |
| `Enter` | Enter detail view (Phase 2) |
| `Esc/q` | Close modal |

## Patterns to Reuse

- `tui/src/lib.rs` — `draw_approval_modal()` for modal rendering
- `engine/src/ui/modal.rs` — `ModalEffectKind` for animations
- `engine/src/commands.rs` — existing command infrastructure

## Verification

1. `just verify` passes
2. Launch forge, type `/config`
3. Modal opens with category list
4. ↑↓/jk navigation works
5. `q` or `Esc` closes modal
6. Values shown match actual config
