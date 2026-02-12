# /config Settings TUI

Interactive settings panel for forge, inspired by Claude Code's `/config` and Gemini CLI's `/settings`.

## Phases

| Phase | Scope | Status |
|-------|-------|--------|
| [Phase 1](phase1-foundation.md) | Modal + read-only category list | Planned |
| [Phase 2](phase2-editing.md) | Detail views + inline editing | Planned |
| [Phase 3](phase3-profiles.md) | Named configuration bundles | Planned |
| [Phase 4](phase4-runtime.md) | Live session state panel | Planned |
| [Phase 5](phase5-full.md) | Complete architecture | Planned |

## Design Decisions

- **Modal overlay** — Centered overlay, keeps conversation context visible
- **Immediate effect** — Edits apply to running session + persist to `config.toml`
- **Vim-native navigation** — j/k, Enter, Esc, /search

## Architecture Reference

See [SETTINGS_ARCHITECTURE.md](SETTINGS_ARCHITECTURE.md) for the full vision document.
