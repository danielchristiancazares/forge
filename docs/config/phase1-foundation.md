# Phase 1: Settings Shell (Read-only)

**Goal**: Ship a working `/settings` command (with `/config` alias) that opens a modal with read-only category navigation.

## Scope

- Add `/settings` command entry point
- Keep `/config` as alias for migration safety
- Render top-level category list with status summaries
- No editing in this phase

## Files to Modify

| File | Changes |
|------|---------|
| `engine/src/commands.rs` | Add `Settings` command + alias parsing for `/settings` and `/config` |
| `engine/src/ui/modal.rs` | Add `SettingsModalState` and category selection state |
| `engine/src/lib.rs` | Handle `Command::Settings` open/close actions |
| `tui/src/lib.rs` | Add `draw_settings_modal()` |
| `tui/src/input.rs` | Add modal navigation keys (`j/k`, arrows, `Enter`, `Esc`, `q`) |

## UI Mockup

```
┌─ Settings ────────────────────────────────────────────────────────┐
│                                                                   │
│  Type to filter...                                                │
│                                                                   │
│  ▸ Providers                                    2 verified        │
│    Models                                       4 usable          │
│    Model Overrides                              chat/code split   │
│    Context                                      128k default      │
│    Tools                                        12 enabled        │
│    Keybindings                                  vim               │
│    Profiles                                     4 saved           │
│    History                                      30 days           │
│    Appearance                                   tokyo-night       │
│                                                                   │
│  Enter select   / filter   q quit                                 │
└───────────────────────────────────────────────────────────────────┘
```

## Keyboard Handling

| Key | Action |
|-----|--------|
| `k` / `Up` | Move selection up |
| `j` / `Down` | Move selection down |
| `Enter` | Open read-only category detail |
| `/` | Focus filter input |
| `Esc` / `q` | Close modal |

## Verification

1. `just verify` passes
2. Launch Forge, run `/settings`
3. `/config` opens the same modal (alias)
4. Category navigation and filtering work
5. Modal closes via `Esc` or `q`

