# Phase 3: Detail Views + Editing

**Goal**: Enable setting edits with safe runtime semantics and persistent storage.

## Scope

- Editable detail views per settings category
- Inline editors for bool/enums/strings/numbers
- Persist edits to `~/.forge/config.toml` using `toml_edit`
- Apply edits to defaults immediately, but effect starts on the next turn
- Never mutate in-flight streaming/tool execution

## Runtime Semantics

- **In-flight turn**: frozen snapshot
- **After save**: defaults are updated and visible in UI/runtime metadata
- **Next turn**: new turn resolves with updated values

This matches model switching behavior while preserving deterministic execution for active work.

## Files to Modify

| File | Changes |
|------|---------|
| `engine/src/lib.rs` | Add pending/resolved setting application hooks for next-turn behavior |
| `engine/src/commands.rs` | Add save/apply notifications and guardrails during in-flight operations |
| `tui/src/lib.rs` | Add editable detail views and dirty-state indicator |
| `tui/src/input.rs` | Add edit mode key handling (`Enter`, `Space`, `e`, save/cancel) |
| `config/src/lib.rs` | Expose typed persistence helpers used by settings editor |

## Config Persistence

Use `toml_edit` to preserve comments and existing formatting:

```rust
fn update_config_value(key_path: &[&str], value: impl Into<toml_edit::Value>) -> Result<()> {
    let path = config::config_path().ok_or_else(|| anyhow!("missing home dir"))?;
    let content = std::fs::read_to_string(&path)?;
    let mut doc = content.parse::<toml_edit::DocumentMut>()?;
    // set value...
    std::fs::write(path, doc.to_string())?;
    Ok(())
}
```

## Verification

1. `just verify` passes
2. Edit and save a setting from `/settings`
3. Runtime surface reflects updated defaults
4. Active in-flight turn remains unchanged
5. New turn uses the updated setting
6. Restart Forge and confirm persisted value loads

