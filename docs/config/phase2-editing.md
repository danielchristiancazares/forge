# Phase 2: Detail Views + Editing

**Goal**: Drill into categories, view full details, edit values inline, persist to config.toml.

## Features

- Detail view per category (Enter from list)
- Inline editing: toggle bools, input strings, select enums
- Immediate effect on running session
- Persist changes to `~/.forge/config.toml` via `toml_edit`

## Detail View Mockups

### Provider Detail

```
┌─ Settings › Provider ─────────────────────────────────────────────┐
│                                                                    │
│  ANTHROPIC                                          ● Connected    │
│    API Key                                    sk-ant-***wxyz       │
│                                                                    │
│  OPENAI                                             ○ Not Set      │
│    API Key                                      Press e to add     │
│                                                                    │
│  GOOGLE                                             ○ Not Set      │
│    API Key                                      Press e to add     │
│                                                                    │
│  e edit   t test connection   q back                               │
└────────────────────────────────────────────────────────────────────┘
```

### Thinking Detail

```
┌─ Settings › Thinking ─────────────────────────────────────────────┐
│                                                                    │
│  ANTHROPIC                                                         │
│    Mode            (•) adaptive  ( ) enabled  ( ) disabled         │
│    Effort          ( ) low  (•) medium  ( ) high  ( ) max          │
│    Budget Tokens                                          10000    │
│                                                                    │
│  GOOGLE                                                            │
│    Thinking        [ ] enabled                                     │
│                                                                    │
│  OPENAI                                                            │
│    Reasoning       (•) high  ( ) medium  ( ) low  ( ) none         │
│    Summary         ( ) detailed  (•) concise  ( ) auto  ( ) none   │
│                                                                    │
│  ↑↓ navigate   Enter/Space toggle   e edit value   q back          │
└────────────────────────────────────────────────────────────────────┘
```

## Config Persistence

Use `toml_edit` to preserve comments and formatting:

```rust
fn update_config_value(key_path: &[&str], value: &str) -> Result<()> {
    let path = config::config_path();
    let content = std::fs::read_to_string(&path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;
    
    // Navigate to key and update
    // ...
    
    std::fs::write(&path, doc.to_string())?;
    Ok(())
}
```

## Verification

1. Navigate to detail view, edit a bool toggle
2. Change persists to config.toml
3. Running session reflects the change immediately
4. Restart forge — change is still there
