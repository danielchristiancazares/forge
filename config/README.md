# Forge Config (`forge-config`)

`forge-config` owns Forge configuration loading, parsing, resolution, and persistence.

## Responsibilities

- Load `config.toml` from the user config path.
- Expand `${ENV_VAR}` placeholders in configured values.
- Deserialize strongly typed settings for providers, tools, UI, context, and LSP.
- Persist user edits for model, UI settings, context settings, and tool approval mode.
- Convert configured tool definitions into runtime `ToolDefinition` values.

## Key API

- `ForgeConfig::load()` returns parsed config (or `None` when missing).
- `ForgeConfig::path()` and `config_path()` resolve the canonical config file location.
- `ForgeConfig::persist_model()` updates `[app].model`.
- `ForgeConfig::persist_ui_settings()` updates UI defaults.
- `ForgeConfig::persist_context_settings()` updates context-memory settings.
- `ForgeConfig::persist_tool_approval_settings()` updates tool approval policy.

## Notes

- API key fields use a redacted `Debug` implementation to avoid leaking secrets.
- Persistence uses atomic write helpers from `forge-utils`.
- On supported platforms, config writes apply owner-only file/dir ACL or mode hardening.
